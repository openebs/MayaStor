use crate::{
    bdev::nexus::{
        instances,
        nexus_bdev::{nexus_create, Nexus},
        Error,
    },
    jsonrpc::{jsonrpc_register, Code, JsonRpcError},
};
use futures::{future, FutureExt};
use rpc::mayastor::{
    Child,
    ChildNexusRequest,
    CreateNexusRequest,
    DestroyNexusRequest,
    ListNexusReply,
    Nexus as RpcNexus,
    PublishNexusReply,
    PublishNexusRequest,
    UnpublishNexusRequest,
};
use uuid::Uuid;

/// Convert UUID to a nexus name of form "nexus-{uuid}".
/// Return error if the UUID is not valid.
fn uuid_to_name(uuid: &str) -> Result<String, JsonRpcError> {
    match Uuid::parse_str(uuid) {
        Ok(uuid) => Ok(format!("nexus-{}", uuid.to_hyphenated().to_string())),
        Err(_) => Err(JsonRpcError::new(
            Code::InvalidParams,
            "Invalid UUID".to_owned(),
        )),
    }
}

/// Lookup a nexus by its uuid. Return error if uuid is invalid or nexus
/// not found.
fn nexus_lookup(uuid: &str) -> Result<&mut Nexus, JsonRpcError> {
    let name = uuid_to_name(uuid)?;

    if let Some(nexus) = instances().iter_mut().find(|n| n.name() == name) {
        Ok(nexus)
    } else {
        Err(JsonRpcError::new(
            Code::NotFound,
            format!("Nexus {} not found", uuid),
        ))
    }
}

/// Convert nexus name to uuid.
///
/// This function never fails which means that if there is a nexus with
/// unconventional name which likely means it was not created using nexus
/// jsonrpc api, we return the whole name without modifications as it is.
fn name_to_uuid(name: &str) -> &str {
    if name.starts_with("nexus-") {
        &name[6 ..]
    } else {
        name
    }
}

pub(crate) fn register_rpc_methods() {
    // JSON rpc method to list the nexus and their states
    jsonrpc_register::<(), _, _>("list_nexus", |_| {
        future::ok(ListNexusReply {
            nexus_list: instances()
                .iter()
                .map(|nexus| RpcNexus {
                    uuid: name_to_uuid(nexus.name()).to_string(),
                    size: nexus.size(),
                    state: nexus.state.to_string(),
                    children: nexus
                        .children
                        .iter()
                        .map(|child| Child {
                            uri: child.name.clone(),
                            state: child.state.to_string(),
                        })
                        .collect::<Vec<_>>(),
                    device_path: nexus.get_share_path().unwrap_or_default(),
                })
                .collect::<Vec<_>>(),
        })
        .boxed_local()
    });

    // rpc method to construct a new Nexus
    jsonrpc_register("create_nexus", |args: CreateNexusRequest| {
        let fut = async move {
            let name = match uuid_to_name(&args.uuid) {
                Ok(name) => name,
                Err(err) => return Err(err),
            };
            match nexus_create(
                &name,
                4096,
                args.size / 4096,
                Some(&args.uuid),
                &args.children,
            )
            .await
            {
                Ok(_) => Ok(()),
                Err(Error::Exists) => Ok(()),
                Err(Error::ChildExists) => Err(JsonRpcError::new(
                    Code::InternalError,
                    "child bdev already exists",
                )),
                Err(_) => Err(JsonRpcError::new(
                    Code::InternalError,
                    "failed to create nexus",
                )),
            }
        };
        fut.boxed_local()
    });

    jsonrpc_register("destroy_nexus", |args: DestroyNexusRequest| {
        let fut = async move {
            let nexus = nexus_lookup(&args.uuid)?;
            nexus.destroy().await;
            Ok(())
        };
        fut.boxed_local()
    });

    jsonrpc_register("publish_nexus", |args: PublishNexusRequest| {
        let fut = async move {
            let nexus = nexus_lookup(&args.uuid)?;
            match nexus.share().await {
                Ok(device_path) => Ok(PublishNexusReply {
                    device_path,
                }),
                Err(err) => Err(JsonRpcError::new(
                    Code::InternalError,
                    format!("Internal error {:?}", err),
                )),
            }
        };
        fut.boxed_local()
    });

    jsonrpc_register("unpublish_nexus", |args: UnpublishNexusRequest| {
        let fut = async move {
            let nexus = nexus_lookup(&args.uuid)?;
            match nexus.unshare().await {
                Ok(_) => Ok(()),
                Err(err) => Err(JsonRpcError::new(
                    Code::InternalError,
                    format!("Internal error {:?}", err),
                )),
            }
        };
        fut.boxed_local()
    });

    jsonrpc_register("offline_child", |args: ChildNexusRequest| {
        let fut = async move {
            let nexus = nexus_lookup(&args.uuid)?;
            match nexus.offline_child(&args.uri).await {
                Ok(_) => Ok(()),
                Err(Error::NotFound) => Ok(()),
                Err(e) => Err(JsonRpcError::new(
                    Code::InternalError,
                    format!("Internal error {:?}", e),
                )),
            }
        };
        fut.boxed_local()
    });

    jsonrpc_register("online_child", |args: ChildNexusRequest| {
        let fut = async move {
            let nexus = nexus_lookup(&args.uuid)?;
            match nexus.online_child(&args.uri).await {
                Ok(_) => Ok(()),
                Err(Error::NotFound) => Ok(()),
                Err(e) => Err(JsonRpcError::new(
                    Code::InternalError,
                    format!("Internal error {:?}", e),
                )),
            }
        };
        fut.boxed_local()
    });
}
