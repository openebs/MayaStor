use nix::errno::Errno;
use serde::{export::Formatter, Serialize};
use snafu::{ResultExt, Snafu};
use spdk_sys::{
    spdk_bdev_get_io_channel,
    spdk_bdev_module_claim_bdev,
    spdk_bdev_module_release_bdev,
    spdk_io_channel,
};
use std::fmt::Display;

use crate::{
    bdev::{
        nexus::{
            nexus_label::{GPTHeader, GptEntry, NexusLabel},
            nexus_module::NEXUS_MODULE,
        },
        Bdev,
    },
    descriptor::{DescError, Descriptor},
    dma::{DmaBuf, DmaError},
    executor::errno_result_from_i32,
    nexus_uri::{bdev_destroy, BdevError},
};
use std::rc::Rc;

#[derive(Debug, Snafu)]
pub enum ChildError {
    #[snafu(display("Child is not closed"))]
    ChildNotClosed {},
    #[snafu(display(
        "Child is smaller than parent {} vs {}",
        child_size,
        parent_size
    ))]
    ChildTooSmall { child_size: u64, parent_size: u64 },
    #[snafu(display("Open child"))]
    OpenChild { source: DescError },
    #[snafu(display("Claim child"))]
    ClaimChild { source: Errno },
    #[snafu(display("Child is read-only"))]
    ChildReadOnly {},
    #[snafu(display("Invalid state of child"))]
    ChildInvalid {},
    #[snafu(display("Failed to allocate buffer for label"))]
    LabelAlloc { source: DmaError },
    #[snafu(display("Failed to read label from child"))]
    LabelRead { source: ChildIoError },
    #[snafu(display("Primary and backup labels are invalid"))]
    LabelInvalid {},
    #[snafu(display("Failed to allocate buffer for partition table"))]
    PartitionTableAlloc { source: DmaError },
    #[snafu(display("Failed to read partition table from child"))]
    PartitionTableRead { source: ChildIoError },
    #[snafu(display("Invalid partition table"))]
    InvalidPartitionTable {},
    #[snafu(display("Invalid partition table checksum"))]
    PartitionTableChecksum {},
    #[snafu(display("Opening child bdev without bdev pointer"))]
    OpenWithoutBdev {},
}

#[derive(Debug, Snafu)]
pub enum ChildIoError {
    #[snafu(display("Error writing to {}", name))]
    WriteError { source: DescError, name: String },
    #[snafu(display("Error reading from {}", name))]
    ReadError { source: DescError, name: String },
    #[snafu(display("Invalid descriptor for child bdev {}", name))]
    InvalidDescriptor { name: String },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub(crate) enum ChildState {
    /// child has not been opened, but we are in the process of opening it
    Init,
    /// cannot add this bdev to the parent as its incompatible property wise
    ConfigInvalid,
    /// the child is open for RW
    Open,
    /// The child has been closed by its parent
    Closed,
    /// a non-fatal have occurred on this child
    Faulted,
}

impl ToString for ChildState {
    fn to_string(&self) -> String {
        match *self {
            ChildState::Init => "init",
            ChildState::ConfigInvalid => "configInvalid",
            ChildState::Open => "open",
            ChildState::Faulted => "faulted",
            ChildState::Closed => "closed",
        }
        .parse()
        .unwrap()
    }
}

#[derive(Debug, Serialize)]
pub struct NexusChild {
    /// name of the parent this child belongs too
    pub(crate) parent: String,
    /// Name of the child is the URI used to create it.
    /// Note that bdev name can differ from it!
    pub(crate) name: String,
    #[serde(skip_serializing)]
    /// the bdev wrapped in Bdev
    pub(crate) bdev: Option<Bdev>,
    #[serde(skip_serializing)]
    /// channel on which we submit the IO
    pub(crate) ch: *mut spdk_io_channel,
    /// current state of the child
    pub(crate) state: ChildState,
    pub(crate) repairing: bool,
    /// descriptor obtained after opening a device
    #[serde(skip_serializing)]
    pub(crate) descriptor: Option<Rc<Descriptor>>,
}

impl Display for NexusChild {
    fn fmt(&self, f: &mut Formatter) -> Result<(), std::fmt::Error> {
        if self.bdev.is_some() {
            let bdev = self.bdev.as_ref().unwrap();
            writeln!(
                f,
                "{}: {:?}, blk_cnt: {}, blk_size: {}",
                self.name,
                self.state,
                bdev.num_blocks(),
                bdev.block_len(),
            )
        } else {
            writeln!(f, "{}: state {:?}", self.name, self.state)
        }
    }
}

impl NexusChild {
    /// Open the child in RW mode and claim the device to be ours. If the child
    /// is already opened by someone else (i.e one of the targets) it will
    /// error out.
    ///
    /// only devices in the closed or Init state can be opened.
    pub(crate) fn open(
        &mut self,
        parent_size: u64,
    ) -> Result<String, ChildError> {
        trace!("{}: Opening child device {}", self.parent, self.name);

        if self.state != ChildState::Closed && self.state != ChildState::Init {
            return Err(ChildError::ChildNotClosed {});
        }

        // TODO: I think this should be an assert (= unwrap)
        if let Some(bdev) = self.bdev.as_ref() {
            let child_size = bdev.size_in_bytes();
            if parent_size > child_size {
                error!(
                    "{}: child to small parent size: {} child size: {}",
                    self.name, parent_size, child_size
                );
                self.state = ChildState::ConfigInvalid;
                return Err(ChildError::ChildTooSmall {
                    parent_size,
                    child_size,
                });
            }

            // used for internal IOs like updating labels
            let desc = match Descriptor::open(bdev, true) {
                Ok(desc) => desc,
                Err(err) => {
                    self.state = ChildState::Faulted;
                    return Err(err).context(OpenChild {});
                }
            };

            // TODO: This should be a method in bdev module
            let errno = unsafe {
                spdk_bdev_module_claim_bdev(
                    bdev.inner,
                    desc.as_ptr(),
                    &NEXUS_MODULE.as_ptr() as *const _ as *mut _,
                )
            };
            if let Err(err) = errno_result_from_i32((), errno) {
                self.state = ChildState::Faulted;
                return Err(err).context(ClaimChild {});
            }

            self.descriptor = Some(Rc::new(desc));
            self.state = ChildState::Open;

            debug!("{}: child {} opened successfully", self.parent, self.name);

            Ok(self.name.clone())
        } else {
            Err(ChildError::OpenWithoutBdev {})
        }
    }

    /// close the bdev -- we have no means of determining if this succeeds
    pub(crate) fn close(&mut self) -> ChildState {
        trace!("{}: Closing child {}", self.parent, self.name);

        debug!(
            "{} has {} references to the descriptor",
            self.parent,
            Rc::strong_count(self.descriptor.as_ref().unwrap())
        );

        if let Some(bdev) = self.bdev.as_ref() {
            unsafe {
                spdk_bdev_module_release_bdev(bdev.inner);
            }
        }

        // just to be explicit
        let descriptor = self.descriptor.take();
        drop(descriptor);

        // we leave the child structure around for when we want reopen it
        self.state = ChildState::Closed;
        self.state
    }

    /// Called to get IO channel to this child.
    /// Returns None of the child has not been opened.
    pub(crate) fn get_io_channel(&self) -> Option<*mut spdk_io_channel> {
        match &self.descriptor {
            Some(desc) => unsafe {
                Some(spdk_bdev_get_io_channel(desc.as_ptr()))
            },
            None => None,
        }
    }

    /// create a new nexus child
    pub fn new(name: String, parent: String, bdev: Option<Bdev>) -> Self {
        NexusChild {
            name,
            bdev,
            parent,
            ch: std::ptr::null_mut(),
            state: ChildState::Init,
            descriptor: None,
            repairing: false,
        }
    }

    /// destroy the child bdev
    pub(crate) async fn destroy(&mut self) -> Result<(), BdevError> {
        assert_eq!(self.state, ChildState::Closed);
        if let Some(bdev) = &self.bdev {
            bdev_destroy(&self.name, &bdev.name()).await
        } else {
            warn!("Destroy child without bdev");
            Ok(())
        }
    }

    /// returns if a child can be written too
    pub fn can_rw(&self) -> bool {
        self.state == ChildState::Open || self.state == ChildState::Faulted
    }

    pub async fn probe_label(&mut self) -> Result<NexusLabel, ChildError> {
        if !self.can_rw() {
            info!(
                "{}: Trying to read from closed child: {}",
                self.parent, self.name
            );
            return Err(ChildError::ChildReadOnly {});
        }

        let bdev = self.bdev.as_ref();
        let desc = self.descriptor.as_ref();

        if bdev.is_none() || desc.is_none() {
            return Err(ChildError::ChildInvalid {});
        }

        let bdev = bdev.unwrap();
        let desc = desc.unwrap();

        let block_size = bdev.block_len();

        let primary = u64::from(block_size);
        let secondary = bdev.num_blocks() - 1;

        let mut buf = desc
            .dma_malloc(block_size as usize)
            .context(LabelAlloc {})?;

        self.read_at(primary, &mut buf)
            .await
            .context(LabelRead {})?;

        let mut label = GPTHeader::from_slice(buf.as_slice());
        if label.is_err() {
            warn!(
                "{}: {}: The primary label is invalid!",
                self.parent, self.name
            );
            self.read_at(secondary, &mut buf)
                .await
                .context(LabelRead {})?;
            label = GPTHeader::from_slice(buf.as_slice());
        }

        let label = match label {
            Ok(label) => label,
            Err(_) => return Err(ChildError::LabelInvalid {}),
        };

        // determine number of blocks we need to read from the partition table
        let num_blocks =
            ((label.entry_size * label.num_entries) / block_size) + 1;

        let mut buf = desc
            .dma_malloc((num_blocks * block_size) as usize)
            .context(PartitionTableAlloc {})?;

        self.read_at(label.lba_table * u64::from(block_size), &mut buf)
            .await
            .context(PartitionTableRead {})?;

        let mut partitions =
            match GptEntry::from_slice(&buf.as_slice(), label.num_entries) {
                Ok(parts) => parts,
                Err(_) => return Err(ChildError::InvalidPartitionTable {}),
            };

        if GptEntry::checksum(&partitions) != label.table_crc {
            return Err(ChildError::PartitionTableChecksum {});
        }

        // some tools write 128 partition entries, even though only two are
        // created, in any case we are only ever interested in the first two
        // partitions, so we drain the others.
        let parts = partitions.drain(.. 2).collect::<Vec<_>>();

        let nl = NexusLabel {
            primary: label,
            partitions: parts,
        };

        Ok(nl)
    }

    /// write the contents of the buffer to this child
    pub async fn write_at(
        &self,
        offset: u64,
        buf: &DmaBuf,
    ) -> Result<usize, ChildIoError> {
        if let Some(desc) = self.descriptor.as_ref() {
            Ok(desc.write_at(offset, buf).await.context(WriteError {
                name: self.name.clone(),
            })?)
        } else {
            Err(ChildIoError::InvalidDescriptor {
                name: self.name.clone(),
            })
        }
    }

    /// read from this child device into the given buffer
    pub async fn read_at(
        &self,
        offset: u64,
        buf: &mut DmaBuf,
    ) -> Result<usize, ChildIoError> {
        if let Some(desc) = self.descriptor.as_ref() {
            Ok(desc.read_at(offset, buf).await.context(ReadError {
                name: self.name.clone(),
            })?)
        } else {
            Err(ChildIoError::InvalidDescriptor {
                name: self.name.clone(),
            })
        }
    }

    /// get a dma buffer that is aligned to this child
    pub fn get_buf(&self, size: usize) -> Option<DmaBuf> {
        match self.descriptor.as_ref() {
            Some(descriptor) => match descriptor.dma_malloc(size) {
                Ok(buf) => Some(buf),
                Err(..) => None,
            },
            None => None,
        }
    }
}
