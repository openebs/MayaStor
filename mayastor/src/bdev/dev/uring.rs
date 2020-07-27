use std::{collections::HashMap, convert::TryFrom, ffi::CString};

use async_trait::async_trait;
use futures::channel::oneshot;
use snafu::ResultExt;
use url::Url;

use spdk_sys::{create_uring_bdev, delete_uring_bdev};

use crate::{
    bdev::{util::uri, CreateDestroy, GetName},
    core::Bdev,
    ffihelper::{cb_arg, done_errno_cb, ErrnoResult},
    nexus_uri::{self, NexusBdevError},
};

#[derive(Debug)]
pub(super) struct Uring {
    name: String,
    alias: String,
    blk_size: u32,
    uuid: Option<uuid::Uuid>,
}

/// Convert a URI to an Uring "object"
impl TryFrom<&Url> for Uring {
    type Error = NexusBdevError;

    fn try_from(url: &Url) -> Result<Self, Self::Error> {
        let segments = uri::segments(url);

        if segments.is_empty() {
            return Err(NexusBdevError::UriInvalid {
                uri: url.to_string(),
                message: String::from("no path segments"),
            });
        }

        let mut parameters: HashMap<String, String> =
            url.query_pairs().into_owned().collect();

        let blk_size: u32 = match parameters.remove("blk_size") {
            Some(value) => {
                value.parse().context(nexus_uri::IntParamParseError {
                    uri: url.to_string(),
                    parameter: String::from("blk_size"),
                })?
            }
            None => 512,
        };

        let uuid = uri::uuid(parameters.remove("uuid")).context(
            nexus_uri::UuidParamParseError {
                uri: url.to_string(),
            },
        )?;

        if let Some(keys) = uri::keys(parameters) {
            warn!("ignored parameters: {}", keys);
        }

        Ok(Uring {
            name: url.path().into(),
            alias: url.to_string(),
            blk_size,
            uuid,
        })
    }
}

impl GetName for Uring {
    fn get_name(&self) -> String {
        self.name.clone()
    }
}

#[async_trait(?Send)]
impl CreateDestroy for Uring {
    type Error = NexusBdevError;

    /// Create a uring bdev
    async fn create(&self) -> Result<String, Self::Error> {
        if Bdev::lookup_by_name(&self.name).is_some() {
            return Err(NexusBdevError::BdevExists {
                name: self.get_name(),
            });
        }

        let cname = CString::new(self.get_name()).unwrap();

        let name = Bdev::from_ptr(unsafe {
            create_uring_bdev(cname.as_ptr(), cname.as_ptr(), self.blk_size)
        })
        .map(|mut bdev| {
            if let Some(u) = self.uuid {
                bdev.set_uuid(Some(u.to_string()))
            }
            if !bdev.add_alias(&self.alias) {
                error!(
                    "Failed to add alias {} to device {}",
                    self.alias,
                    self.get_name()
                );
            }
            bdev.name()
        });

        async {
            name.ok_or_else(|| NexusBdevError::BdevNotFound {
                name: self.get_name(),
            })
        }
        .await
    }

    /// Destroy the given uring bdev
    async fn destroy(self: Box<Self>) -> Result<(), Self::Error> {
        match Bdev::lookup_by_name(&self.name) {
            Some(bdev) => {
                let (sender, receiver) = oneshot::channel::<ErrnoResult<()>>();
                unsafe {
                    delete_uring_bdev(
                        bdev.as_ptr(),
                        Some(done_errno_cb),
                        cb_arg(sender),
                    );
                }
                receiver
                    .await
                    .context(nexus_uri::CancelBdev {
                        name: self.get_name(),
                    })?
                    .context(nexus_uri::DestroyBdev {
                        name: self.get_name(),
                    })
            }
            None => Err(NexusBdevError::BdevNotFound {
                name: self.get_name(),
            }),
        }
    }
}
