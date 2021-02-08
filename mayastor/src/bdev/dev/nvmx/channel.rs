/* I/O channel for NVMe controller, one per core. */

use crate::{
    bdev::dev::nvmx::{
        nvme_bdev_running_config,
        NvmeControllerState,
        NVME_CONTROLLERS,
    },
    core::{poller, CoreError},
};
use std::{cmp::max, mem::size_of, os::raw::c_void, ptr::NonNull};

use spdk_sys::{
    nvme_qpair_abort_reqs,
    spdk_io_channel,
    spdk_nvme_ctrlr,
    spdk_nvme_ctrlr_alloc_io_qpair,
    spdk_nvme_ctrlr_connect_io_qpair,
    spdk_nvme_ctrlr_disconnect_io_qpair,
    spdk_nvme_ctrlr_free_io_qpair,
    spdk_nvme_ctrlr_get_default_io_qpair_opts,
    spdk_nvme_ctrlr_reconnect_io_qpair,
    spdk_nvme_io_qpair_opts,
    spdk_nvme_poll_group,
    spdk_nvme_poll_group_add,
    spdk_nvme_poll_group_create,
    spdk_nvme_poll_group_destroy,
    spdk_nvme_poll_group_process_completions,
    spdk_nvme_poll_group_remove,
    spdk_nvme_qpair,
    spdk_put_io_channel,
};

#[repr(C)]
pub struct NvmeIoChannel<'a> {
    inner: *mut NvmeIoChannelInner<'a>,
}

impl<'a> NvmeIoChannel<'a> {
    #[inline]
    fn from_raw(p: *mut c_void) -> &'a mut NvmeIoChannel<'a> {
        unsafe { &mut *(p as *mut NvmeIoChannel) }
    }

    #[inline]
    fn inner_mut(&mut self) -> &'a mut NvmeIoChannelInner<'a> {
        unsafe { &mut *self.inner }
    }

    #[inline]
    pub fn inner_from_channel(
        io_channel: *mut spdk_io_channel,
    ) -> &'a mut NvmeIoChannelInner<'a> {
        NvmeIoChannel::from_raw(Self::io_channel_ctx(io_channel)).inner_mut()
    }

    #[inline]
    fn io_channel_ctx(ch: *mut spdk_io_channel) -> *mut c_void {
        unsafe {
            (ch as *mut u8).add(size_of::<spdk_io_channel>()) as *mut c_void
        }
    }
}
pub struct IoQpair {
    qpair: NonNull<spdk_nvme_qpair>,
    ctrlr_handle: NonNull<spdk_nvme_ctrlr>,
}

impl IoQpair {
    fn get_default_options(
        ctrlr_handle: *mut spdk_nvme_ctrlr,
    ) -> spdk_nvme_io_qpair_opts {
        let mut opts = spdk_nvme_io_qpair_opts::default();
        let default_opts = nvme_bdev_running_config();

        unsafe {
            spdk_nvme_ctrlr_get_default_io_qpair_opts(
                ctrlr_handle,
                &mut opts,
                size_of::<spdk_nvme_io_qpair_opts>() as u64,
            )
        };

        opts.io_queue_requests =
            max(opts.io_queue_requests, default_opts.io_queue_requests);
        opts.create_only = true;

        opts
    }

    /// Create a qpair with default options for target NVMe controller.
    fn create(
        ctrlr_handle: *mut spdk_nvme_ctrlr,
        ctrlr_name: &str,
    ) -> Result<Self, CoreError> {
        let qpair_opts = IoQpair::get_default_options(ctrlr_handle);

        let qpair: *mut spdk_nvme_qpair = unsafe {
            spdk_nvme_ctrlr_alloc_io_qpair(
                ctrlr_handle,
                &qpair_opts,
                size_of::<spdk_nvme_io_qpair_opts>() as u64,
            )
        };

        if qpair.is_null() {
            error!(
                "Failed to allocate I/O qpair for controller {}",
                ctrlr_name
            );
            Err(CoreError::GetIoChannel {
                name: ctrlr_name.to_string(),
            })
        } else {
            debug!("qpair {:p} created for controller {}", qpair, ctrlr_name);
            Ok(Self {
                qpair: NonNull::new(qpair).unwrap(),
                ctrlr_handle: NonNull::new(ctrlr_handle).unwrap(),
            })
        }
    }

    /// Get SPDK qpair object.
    pub fn as_ptr(&self) -> *mut spdk_nvme_qpair {
        self.qpair.as_ptr()
    }

    /// Connect qpair.
    fn connect(&mut self) -> i32 {
        unsafe {
            spdk_nvme_ctrlr_connect_io_qpair(
                self.ctrlr_handle.as_ptr(),
                self.qpair.as_ptr(),
            )
        }
    }
}

struct PollGroup(NonNull<spdk_nvme_poll_group>);

impl PollGroup {
    /// Create a poll group.
    fn create(ctx: *mut c_void, ctrlr_name: &str) -> Result<Self, CoreError> {
        let poll_group: *mut spdk_nvme_poll_group =
            unsafe { spdk_nvme_poll_group_create(ctx) };

        if poll_group.is_null() {
            Err(CoreError::GetIoChannel {
                name: ctrlr_name.to_string(),
            })
        } else {
            Ok(Self(NonNull::new(poll_group).unwrap()))
        }
    }

    /// Add I/O qpair to poll group.
    fn add_qpair(&mut self, qpair: &IoQpair) -> i32 {
        unsafe { spdk_nvme_poll_group_add(self.0.as_ptr(), qpair.as_ptr()) }
    }

    /// Remove I/O qpair to poll group.
    fn remove_qpair(&mut self, qpair: &IoQpair) -> i32 {
        unsafe { spdk_nvme_poll_group_remove(self.0.as_ptr(), qpair.as_ptr()) }
    }

    /// Get SPDK handle for poll group.
    fn as_ptr(&self) -> *mut spdk_nvme_poll_group {
        self.0.as_ptr()
    }
}

impl Drop for PollGroup {
    fn drop(&mut self) {
        debug!("dropping poll group {:p}", self.0.as_ptr());
        unsafe { spdk_nvme_poll_group_destroy(self.0.as_ptr()) };
        debug!("poll group {:p} successfully dropped", self.0.as_ptr());
    }
}

impl Drop for IoQpair {
    fn drop(&mut self) {
        let qpair = self.qpair.as_ptr();

        debug!("dropping qpair {:p}", qpair);
        unsafe {
            nvme_qpair_abort_reqs(qpair, 1);
            debug!("I/O requests successfully aborted for qpair {:p}", qpair);
            spdk_nvme_ctrlr_disconnect_io_qpair(qpair);
            debug!("qpair {:p} successfully disconnected", qpair);
            spdk_nvme_ctrlr_free_io_qpair(qpair);
        }
        debug!("qpair {:p} successfully dropped", qpair);
    }
}

pub struct NvmeIoChannelInner<'a> {
    poll_group: PollGroup,
    poller: poller::Poller<'a>,
    pub qpair: Option<IoQpair>,
    // Flag to indicate the shutdown state of the channel.
    // We need such a flag to differentiate between channel reset and shutdown.
    // Channel reset is a reversible operation, which is followed by
    // reinitialize(), which 'resurrects' channel (i.e. recreates all its
    // I/O resources): such behaviour is observed during controller reset.
    // Shutdown, in contrary, means 'one-way' ticket for the channel, which
    // doesn't assume any further resurrections: such behaviour is seen
    // upon controller shutdown. Being able to differentiate between these
    // 2 states allows controller reset to behave properly in parallel with
    // shutdown (if case reset is initiated before shutdown), and
    // not to reinitialize channels already processed by shutdown logic.
    is_shutdown: bool,
}

impl NvmeIoChannelInner<'_> {
    /// Reset channel, making it unusable till reinitialize() is called.
    pub fn reset(&mut self) -> i32 {
        if self.qpair.is_some() {
            // Remove qpair and trigger its deallocation via drop().
            self.qpair.take();
        }
        0
    }

    /// Checks whether the I/O channel is shutdown.
    pub fn is_shutdown(&self) -> bool {
        self.is_shutdown
    }

    /// Shutdown I/O channel and make it completely unusable for I/O.
    pub fn shutdown(&mut self) -> i32 {
        if self.is_shutdown {
            return 0;
        }

        let rc = self.reset();
        if rc == 0 {
            self.is_shutdown = true;
        }
        rc
    }

    /// Reinitializes channel after reset unless the channel is shutdown.
    pub fn reinitialize(
        &mut self,
        ctrlr_name: &str,
        ctrlr_handle: *mut spdk_nvme_ctrlr,
    ) -> i32 {
        if self.is_shutdown {
            error!(
                "{} I/O channel is shutdown, channel reinitialization not possible",
                ctrlr_name
            );
            return -libc::ENODEV;
        }

        // We assume that channel is reinitialized after being reset, so we
        // expect to see no I/O qpair.
        if self.qpair.is_some() {
            warn!(
                "{} I/O channel has active I/O qpair while being reinitialized, clearing qpair",
                ctrlr_name
            );
            self.qpair.take().unwrap();
        }

        // Create qpair for target controller.
        let mut qpair = match IoQpair::create(ctrlr_handle, ctrlr_name) {
            Ok(qpair) => qpair,
            Err(e) => {
                error!("{} Failed to allocate qpair: {:?}", ctrlr_name, e);
                return -libc::ENOMEM;
            }
        };

        // Add qpair to the poll group.
        let mut rc = self.poll_group.add_qpair(&qpair);
        if rc != 0 {
            error!("{} failed to add qpair to poll group", ctrlr_name);
            return rc;
        }

        // Connect qpair.
        rc = qpair.connect();
        if rc != 0 {
            error!("{} failed to connect qpair (errno={})", ctrlr_name, rc);
            self.poll_group.remove_qpair(&qpair);
            return rc;
        }

        debug!("{} I/O channel successfully reinitialized", ctrlr_name);
        self.qpair = Some(qpair);
        0
    }
}

pub struct NvmeControllerIoChannel(NonNull<spdk_io_channel>);

extern "C" fn disconnected_qpair_cb(
    qpair: *mut spdk_nvme_qpair,
    _ctx: *mut c_void,
) {
    warn!("NVMe qpair disconnected, qpair={:p}", qpair);
    /*
     * Currently, just try to reconnect indefinitely. If we are doing a
     * reset, the reset will reconnect a qpair and we will stop getting a
     * callback for this one.
     */
    unsafe {
        spdk_nvme_ctrlr_reconnect_io_qpair(qpair);
    }
}

extern "C" fn nvme_poll(ctx: *mut c_void) -> i32 {
    let inner = NvmeIoChannel::from_raw(ctx).inner_mut();

    let num_completions = unsafe {
        spdk_nvme_poll_group_process_completions(
            inner.poll_group.as_ptr(),
            0,
            Some(disconnected_qpair_cb),
        )
    };

    if num_completions > 0 {
        1
    } else {
        0
    }
}

impl NvmeControllerIoChannel {
    pub extern "C" fn create(device: *mut c_void, ctx: *mut c_void) -> i32 {
        let id = device as u64;

        debug!("Creating IO channel for controller ID 0x{:X}", id);

        let carc = match NVME_CONTROLLERS.lookup_by_name(id.to_string()) {
            None => {
                error!("No NVMe controller found for ID 0x{:X}", id);
                return 1;
            }
            Some(c) => c,
        };

        let cname;
        let spdk_handle;

        {
            let controller = carc.lock().expect("lock error");
            // Make sure controller is available.
            if controller.get_state() != NvmeControllerState::Running {
                error!(
                    "{} controller is in {:?} state, I/O channel creation not possible",
                    controller.get_name(),
                    controller.get_state()
                );
                return 1;
            }
            cname = controller.get_name();
            spdk_handle = controller.ctrlr_as_ptr();
            // Release controller's lock before proceeding to avoid deadlocks,
            // as qpair-related operations might hang in case of
            // network connection failures. Note that we still hold
            // the reference to the controller instance (carc) which
            // guarantees that the controller exists during I/O channel
            // creation.
        }

        let nvme_channel = NvmeIoChannel::from_raw(ctx);

        // Allocate qpair.
        let mut qpair = match IoQpair::create(spdk_handle, &cname) {
            Ok(qpair) => qpair,
            Err(e) => {
                error!("{} Failed to allocate qpair: {:?}", cname, e);
                return 1;
            }
        };
        debug!("{} I/O qpair successfully created", cname);

        // Create poll group.
        let mut poll_group = match PollGroup::create(ctx, &cname) {
            Ok(poll_group) => poll_group,
            Err(e) => {
                error!("{} Failed to create a poll group: {:?}", cname, e);
                return 1;
            }
        };

        // Add qpair to poll group.
        let mut rc = poll_group.add_qpair(&qpair);
        if rc != 0 {
            error!("{} failed to add qpair to poll group, rc = {}", cname, rc);
            return 1;
        }

        // Create poller.
        let poller = poller::Builder::new()
            .with_interval(nvme_bdev_running_config().nvme_ioq_poll_period_us)
            .with_poll_fn(move || nvme_poll(ctx))
            .build();

        // Connect qpair.
        rc = qpair.connect();
        if rc != 0 {
            error!("{} failed to connect qpair, rc = {}", cname, rc);
            poll_group.remove_qpair(&qpair);
            return 1;
        }

        let inner = Box::new(NvmeIoChannelInner {
            qpair: Some(qpair),
            poll_group,
            poller,
            is_shutdown: false,
        });

        nvme_channel.inner = Box::into_raw(inner);
        info!("{} I/O channel {:?} successfully initialized", cname, ctx);
        0
    }

    /// Callback function to be invoked by SPDK to deinitialize I/O channel for
    /// NVMe controller.
    pub extern "C" fn destroy(device: *mut c_void, ctx: *mut c_void) {
        debug!(
            "Destroying IO channel for controller ID 0x{:X}",
            device as u64
        );

        {
            let ch = NvmeIoChannel::from_raw(ctx);
            let mut inner = unsafe { Box::from_raw(ch.inner) };

            // Stop the poller and do extra handling for I/O qpair, as it needs
            // to be detached from the poller prior poller
            // destruction.
            inner.poller.stop();

            if let Some(qpair) = inner.qpair.take() {
                inner.poll_group.remove_qpair(&qpair);
            }
        }

        debug!(
            "IO channel for controller ID 0x{:X} successfully destroyed",
            device as u64
        );
    }
}

/// Wrapper around SPDK I/O channel.
impl NvmeControllerIoChannel {
    pub fn from_null_checked(
        ch: *mut spdk_io_channel,
    ) -> Option<NvmeControllerIoChannel> {
        if ch.is_null() {
            None
        } else {
            Some(NvmeControllerIoChannel(NonNull::new(ch).unwrap()))
        }
    }

    pub fn as_ptr(&self) -> *mut spdk_io_channel {
        self.0.as_ptr()
    }
}

impl Drop for NvmeControllerIoChannel {
    fn drop(&mut self) {
        debug!("I/O channel {:p} dropped", self.0.as_ptr());
        unsafe { spdk_put_io_channel(self.0.as_ptr()) }
    }
}
