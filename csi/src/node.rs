use std::{
    boxed::Box,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::Duration,
    vec::Vec,
};

use tonic::{Code, Request, Response, Status};

macro_rules! failure {
    (Code::$code:ident, $msg:literal) => {{ error!($msg); Status::new(Code::$code, $msg) }};
    (Code::$code:ident, $fmt:literal $(,$args:expr)+) => {{ let message = format!($fmt $(,$args)+); error!("{}", message); Status::new(Code::$code, message) }};
}

use glob::glob;
use uuid::Uuid;

use crate::{
    csi::{
        volume_capability::{access_mode::Mode, AccessType},
        *,
    },
    dev::Device,
    format::prepare_device,
    mount::{self, subset, ReadOnly},
};

#[derive(Clone, Debug)]
pub struct Node {
    pub node_name: String,
    pub filesystems: Vec<String>,
}

const FS_MOUNT: &str = "fs_mnt";
// For block volumes we do not stage a mount.
// For filesystem volumes we stage the mount at a subdirectory
// At unstage we differentiate between a filesystemvolume,
// a block volume by checking for the presence of the sub directory

/// Construct the filesystem volume staging path.
fn make_fs_staging_path(staging_path: &str) -> Result<String, String> {
    let mut fs_path = PathBuf::from(staging_path);
    fs_path.push(FS_MOUNT);
    match fs_path.into_os_string().into_string() {
        Ok(path) => Ok(path),
        Err(_) => Err(format!(
            "Failed to construct path {}/{}",
            staging_path, FS_MOUNT
        )),
    }
}

// Determine if given access mode in conjunction with ro mount flag makes
// sense or not. If access mode is not supported or the combination does
// not make sense, return error string.
//
// NOTE: Following is based on our limited understanding of access mode
// meaning. Access mode does not control if the mount is rw/ro (that is
// rather part of the mount flags). Access mode serves as advisory info
// for CO when attaching volumes to pods. It is out of scope of storage
// plugin running on particular node to check that access mode for particular
// publish or stage request makes sense.

/// Check that the access_mode from VolumeCapability is consistent with
/// the readonly status
fn check_access_mode(
    volume_capability: &Option<VolumeCapability>,
    readonly: bool,
) -> Result<(), String> {
    match volume_capability {
        Some(capability) => match &capability.access_mode {
            Some(access) => match Mode::from_i32(access.mode) {
                Some(mode) => match mode {
                    Mode::SingleNodeWriter | Mode::MultiNodeSingleWriter => {
                        Ok(())
                    }
                    Mode::SingleNodeReaderOnly | Mode::MultiNodeReaderOnly => {
                        if readonly {
                            return Ok(());
                        }
                        Err(format!("volume capability: invalid combination of access mode ({:?}) and mount flag (rw)", mode))
                    }
                    Mode::Unknown => Err(String::from(
                        "volume capability: unknown access mode",
                    )),
                    _ => Err(format!(
                        "volume capability: unsupported access mode: {:?}",
                        mode
                    )),
                },
                None => Err(format!(
                    "volume capability: invalid access mode: {}",
                    access.mode
                )),
            },
            None => Err(String::from("volume capability: missing access mode")),
        },
        None => Err(String::from("missing volume capability")),
    }
}

/// Retrieve the AccessType from VolumeCapability
fn get_access_type(
    volume_capability: &Option<VolumeCapability>,
) -> Result<&AccessType, String> {
    match volume_capability {
        Some(capability) => match &capability.access_type {
            Some(access) => Ok(access),
            None => Err(String::from("volume capability: missing access type")),
        },
        None => Err(String::from("missing volume capability")),
    }
}

impl Node {}
#[tonic::async_trait]
impl node_server::Node for Node {
    async fn node_get_info(
        &self,
        _request: Request<NodeGetInfoRequest>,
    ) -> Result<Response<NodeGetInfoResponse>, Status> {
        let node_id = format!("mayastor://{}", &self.node_name);
        let max_volumes_per_node =
            glob("/dev/nbd*").expect("Invalid glob pattern").count() as i64;

        debug!(
            "NodeGetInfo request: ID={}, max volumes={}",
            node_id, max_volumes_per_node,
        );

        Ok(Response::new(NodeGetInfoResponse {
            node_id,
            max_volumes_per_node,
            accessible_topology: None,
        }))
    }

    async fn node_get_capabilities(
        &self,
        _request: Request<NodeGetCapabilitiesRequest>,
    ) -> Result<Response<NodeGetCapabilitiesResponse>, Status> {
        let caps = vec![node_service_capability::rpc::Type::StageUnstageVolume];

        debug!("NodeGetCapabilities request: {:?}", caps);

        // We don't support stage/unstage and expand volume rpcs
        Ok(Response::new(NodeGetCapabilitiesResponse {
            capabilities: caps
                .into_iter()
                .map(|c| NodeServiceCapability {
                    r#type: Some(node_service_capability::Type::Rpc(
                        node_service_capability::Rpc {
                            r#type: c as i32,
                        },
                    )),
                })
                .collect(),
        }))
    }

    /// This RPC is called by the CO when a workload that wants to use the
    /// specified volume is placed (scheduled) on a node. The Plugin SHALL
    /// assume that this RPC will be executed on the node where the volume will
    /// be used. If the corresponding Controller Plugin has
    /// PUBLISH_UNPUBLISH_VOLUME controller capability, the CO MUST guarantee
    /// that this RPC is called after ControllerPublishVolume is called for the
    /// given volume on the given node and returns a success. This operation
    /// MUST be idempotent. If the volume corresponding to the volume_id has
    /// already been published at the specified target_path, and is compatible
    /// with the specified volume_capability and readonly flag, the Plugin MUST
    /// reply 0 OK. If this RPC failed, or the CO does not know if it failed or
    /// not, it MAY choose to call NodePublishVolume again, or choose to call
    /// NodeUnpublishVolume. This RPC MAY be called by the CO multiple times on
    /// the same node for the same volume with possibly different target_path
    /// and/or other arguments if the volume has MULTI_NODE capability (i.e.,
    /// access_mode is either MULTI_NODE_READER_ONLY, MULTI_NODE_SINGLE_WRITER
    /// or MULTI_NODE_MULTI_WRITER).
    async fn node_publish_volume(
        &self,
        request: Request<NodePublishVolumeRequest>,
    ) -> Result<Response<NodePublishVolumeResponse>, Status> {
        let msg = request.into_inner();

        trace!("node_publish_volume {:?}", msg);

        let target_path = &msg.target_path;
        let volume_id = &msg.volume_id;

        if let Err(error) =
            check_access_mode(&msg.volume_capability, msg.readonly)
        {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to publish volume {}: {}",
                volume_id,
                error
            ));
        }

        // The CO must ensure that the parent of target path exists.
        let target_parent = Path::new(target_path).parent().unwrap();
        if let Err(err) = fs::create_dir_all(PathBuf::from(target_parent)) {
            return Err(Status::new(
                Code::Internal,
                format!(
                    "Failed to find parent dir for mountpoint {}, volume {}: {}",
                    target_path, volume_id, err
                ),
            ));
        }

        if volume_id.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to publish volume: missing volume id"
            ));
        }

        if target_path.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to publish volume {}: missing target path",
                volume_id
            ));
        }

        // Note that the staging path is NOT optional,
        // as we advertise StageUnstageVolume.
        if msg.staging_target_path.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to publish volume {}: missing staging path",
                volume_id
            ));
        }

        let uri = msg.publish_context.get("uri").ok_or_else(|| {
            failure!(
                Code::InvalidArgument,
                "Failed to stage volume {}: URI attribute missing from publish context",
                volume_id
            )
        })?;

        match get_access_type(&msg.volume_capability).map_err(|error| {
            failure!(
                Code::InvalidArgument,
                "Failed to publish volume {}: {}",
                volume_id,
                error
            )
        })? {
            AccessType::Mount(mnt) => {
                // Filesystem mount
                let fs_staging_path =
                    match make_fs_staging_path(&msg.staging_target_path) {
                        Ok(path) => path,
                        Err(error) => {
                            return Err(failure!(
                                Code::Internal,
                                "{}: {}",
                                error,
                                volume_id
                            ))
                        }
                    };

                debug!(
                    "Publishing volume {} from {} to {}",
                    volume_id, fs_staging_path, target_path
                );

                let staged = mount::find_mount(None, Some(&fs_staging_path))
                    .ok_or_else(|| {
                        failure!(
                    Code::InvalidArgument,
                    "Failed to publish volume {}: no mount for staging path {}",
                    volume_id,
                    fs_staging_path
                )
                    })?;

                // TODO: Should also check that the staged "device"
                // corresponds to the the volume uuid

                if mnt.fs_type != "" && mnt.fs_type != staged.fstype {
                    return Err(failure!(
                Code::InvalidArgument,
                "Failed to publish volume {}: filesystem type ({}) does not match staged volume ({})",
                volume_id,
                mnt.fs_type,
                staged.fstype
            ));
                }

                if self
                    .filesystems
                    .iter()
                    .find(|&entry| entry == &staged.fstype)
                    .is_none()
                {
                    return Err(failure!(
                Code::InvalidArgument,
                "Failed to publish volume {}: unsupported filesystem type: {}",
                volume_id,
                staged.fstype
            ));
                }

                let readonly = staged.options.readonly();

                if readonly && !msg.readonly {
                    return Err(failure!(
                Code::InvalidArgument,
                "Failed to publish volume {}: volume is staged as \"ro\" but publish requires \"rw\"",
                volume_id
            ));
                }

                if let Some(mount) = mount::find_mount(None, Some(target_path))
                {
                    if mount.source != staged.source {
                        return Err(failure!(
                    Code::AlreadyExists,
                    "Failed to publish volume {}: directory {} is already in use",
                    volume_id,
                    target_path
                ));
                    }

                    if !subset(&mnt.mount_flags, &mount.options)
                        || msg.readonly != mount.options.readonly()
                    {
                        return Err(failure!(
                    Code::AlreadyExists,
                    "Failed to publish volume {}: directory {} is already mounted but with incompatible flags",
                    volume_id,
                    target_path
                ));
                    }

                    info!(
                        "Volume {} is already published to {}",
                        volume_id, target_path
                    );

                    return Ok(Response::new(NodePublishVolumeResponse {}));
                }

                debug!("Creating directory {}", target_path);

                if let Err(error) =
                    fs::create_dir_all(PathBuf::from(target_path))
                {
                    if error.kind() != ErrorKind::AlreadyExists {
                        return Err(failure!(
                    Code::Internal,
                    "Failed to publish volume {}: failed to create directory {}: {}",
                    volume_id,
                    target_path,
                    error
                ));
                    }
                }

                debug!("Mounting {} to {}", fs_staging_path, target_path);

                if let Err(error) =
                    mount::bind_mount(&fs_staging_path, &target_path, false)
                {
                    return Err(failure!(
                    Code::Internal,
                    "Failed to publish volume {}: failed to mount {} to {}: {}",
                    volume_id,
                    fs_staging_path,
                    target_path,
                    error
                ));
                }

                if msg.readonly && !readonly {
                    let mut options = mnt.mount_flags.clone();
                    options.push(String::from("ro"));

                    debug!("Remounting {} as readonly", target_path);

                    if let Err(error) =
                        mount::bind_remount(&target_path, &options)
                    {
                        let message = format!(
                    "Failed to publish volume {}: failed to mount {} to {} as readonly: {}",
                    volume_id,
                    fs_staging_path,
                    target_path,
                    error
                );

                        error!("Failed to remount {}: {}", target_path, error);

                        debug!("Unmounting {}", target_path);

                        if let Err(error) = mount::bind_unmount(&target_path) {
                            error!(
                                "Failed to unmount {}: {}",
                                target_path, error
                            );
                        }

                        return Err(Status::new(Code::Internal, message));
                    }
                }

                info!("Volume {} published to {}", volume_id, target_path);
                Ok(Response::new(NodePublishVolumeResponse {}))
            }
            AccessType::Block(_) => {
                // Block volumes are not staged, instead
                // bind mount to the device path,
                // this can be done for mutliple target paths.
                let device = Device::parse(&uri).map_err(|error| {
                    failure!(
                        Code::Internal,
                        "Failed to publish volume {}: error parsing URI {}: {}",
                        volume_id,
                        uri,
                        error
                    )
                })?;

                if let Some(device_path) =
                    device.find().await.map_err(|error| {
                        failure!(
            Code::Internal,
            "Failed to publish volume {}: error locating device for URI {}: {}",
            volume_id,
            uri,
            error
        )
                    })?
                {
                    let devt = unsafe { libc::makedev(259, 254) };

                    let cstr_dst =
                        std::ffi::CString::new(target_path.as_str()).unwrap();
                    let res = unsafe {
                        libc::mknod(cstr_dst.as_ptr(), libc::S_IFBLK, devt)
                    };

                    if res != 0 {
                        return Err(failure!(
                            Code::Internal,
                            "Failed to publish volume {}: mknod at {} failed",
                            volume_id,
                            target_path
                        ));
                    }

                    if let Err(error) = mount::blockdevice_mount(
                        &device_path,
                        target_path.as_str(),
                        msg.readonly,
                    ) {
                        return Err(failure!(
                            Code::Internal,
                            "Failed to publish volume {}: {}",
                            volume_id,
                            error
                        ));
                    }
                    Ok(Response::new(NodePublishVolumeResponse {}))
                } else {
                    Err(failure!(
                        Code::Internal,
                        "Failed to publish volume {}: unable to retrieve device path",
                        volume_id
                    ))
                }
            }
        }
    }

    /// This RPC is called by the CO when a workload using the specified
    /// volume is removed (unscheduled) from a node.
    /// If the corresponding Controller Plugin has PUBLISH_UNPUBLISH_VOLUME
    /// controller capability, the CO MUST guarantee that this RPC is called
    /// after ControllerPublishVolume is called for the given volume on the
    /// given node and returns a success.
    ///
    /// This operation MUST be idempotent.
    async fn node_unpublish_volume(
        &self,
        request: Request<NodeUnpublishVolumeRequest>,
    ) -> Result<Response<NodeUnpublishVolumeResponse>, Status> {
        let msg = request.into_inner();

        trace!("node_unpublish_volume {:?}", msg);

        if msg.volume_id.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to unpublish volume: missing volume id"
            ));
        }

        if msg.target_path.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to unpublish volume {}: missing target path",
                msg.volume_id
            ));
        }

        // target path will have been created previously in node_publish_volume
        // and is one of
        //  1. a directory for filesystem volumes ,
        //  2. a block special file for block volumes.
        //
        // If it does not exist, then a previously unpublish request has
        // succeeded.
        let path_target_path = Path::new(&msg.target_path);
        if path_target_path.exists() {
            let target_path = &msg.target_path;
            let volume_id = &msg.volume_id;

            if path_target_path.is_dir() {
                // filesystem mount
                if mount::find_mount(None, Some(target_path)).is_none() {
                    // No mount found for target_path.
                    // The idempotency requirement means this is not an error.
                    // Just clean up as best we can and claim success.

                    if let Err(error) =
                        fs::remove_dir(PathBuf::from(target_path))
                    {
                        if error.kind() != ErrorKind::NotFound {
                            error!(
                                "Failed to remove directory {}: {}",
                                target_path, error
                            );
                        }
                    }

                    info!(
                        "Volume {} is already unpublished from {}",
                        volume_id, target_path
                    );

                    return Ok(Response::new(NodeUnpublishVolumeResponse {}));
                }

                debug!("Unmounting {}", target_path);

                if let Err(error) = mount::bind_unmount(target_path) {
                    return Err(failure!(
                    Code::Internal,
                    "Failed to unpublish volume {}: failed to unmount {}: {}",
                    volume_id,
                    target_path,
                    error
                ));
                }

                debug!("Removing directory {}", target_path);

                if let Err(error) = fs::remove_dir(PathBuf::from(target_path)) {
                    if error.kind() != ErrorKind::NotFound {
                        error!(
                            "Failed to remove directory {}: {}",
                            target_path, error
                        );
                    }
                }
            } else {
                if path_target_path.is_file() {
                    return Err(Status::new(
                        Code::Unknown,
                        format!(
                            "Failed to unpublish volume {}: {} is a file.",
                            volume_id, target_path
                        ),
                    ));
                }
                // block volumes are mounted on block special file, which is not
                // a regular file.
                if mount::find_mount(None, Some(target_path)).is_some() {
                    match mount::blockdevice_unmount(&target_path) {
                        Ok(_) => {}
                        Err(err) => {
                            return Err(Status::new(
                                Code::Internal,
                                format!(
                                    "Failed to unpublish volume {}: {}",
                                    volume_id, err
                                ),
                            ));
                        }
                    }
                }

                debug!("Removing block special file {}", target_path);

                if let Err(error) = std::fs::remove_file(target_path) {
                    warn!(
                        "Failed to remove block file {}: {}",
                        target_path, error
                    );
                }
            }

            info!("Volume {} unpublished from {}", volume_id, target_path);
        }
        Ok(Response::new(NodeUnpublishVolumeResponse {}))
    }

    /// Get volume stats method is currently not implemented,
    /// although it's simple to do.
    ///
    /// TODO: Just read the data about capacity/used space
    /// inodes/bytes from the system using the mountpoint.
    async fn node_get_volume_stats(
        &self,
        request: Request<NodeGetVolumeStatsRequest>,
    ) -> Result<Response<NodeGetVolumeStatsResponse>, Status> {
        let msg = request.into_inner();
        trace!("node_get_volume_stats {:?}", msg);

        /*
        Ok(Response::new(NodeGetVolumeStatsResponse {
            usage: vec![VolumeUsage {
                total: 0 as i64,
                unit: volume_usage::Unit::Bytes as i32,
                available: 0,
                used: 0,
            }],
        }))
        */
        error!("Unimplemented {:?}", msg);
        Err(Status::new(Code::Unimplemented, "Method not implemented"))
    }

    async fn node_expand_volume(
        &self,
        request: Request<NodeExpandVolumeRequest>,
    ) -> Result<Response<NodeExpandVolumeResponse>, Status> {
        let msg = request.into_inner();
        error!("Unimplemented {:?}", msg);
        Err(Status::new(Code::Unimplemented, "Method not implemented"))
    }

    async fn node_stage_volume(
        &self,
        request: Request<NodeStageVolumeRequest>,
    ) -> Result<Response<NodeStageVolumeResponse>, Status> {
        let msg = request.into_inner();
        let volume_id = &msg.volume_id;
        let publish_context = &msg.publish_context;

        trace!("node_stage_volume {:?}", msg);

        if volume_id.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to stage volume: missing volume id"
            ));
        }

        if msg.staging_target_path.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to stage volume {}: missing staging path",
                volume_id
            ));
        }

        if let Err(error) = check_access_mode(
            &msg.volume_capability,
            // relax the check a bit by pretending all stage mounts are ro
            true,
        ) {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to stage volume {}: {}",
                volume_id,
                error
            ));
        };

        let uri = publish_context.get("uri").ok_or_else(|| {
            failure!(
                Code::InvalidArgument,
                "Failed to stage volume {}: URI attribute missing from publish context",
                volume_id
            )
        })?;

        debug!("Volume {} has URI {}", volume_id, uri);

        let device = Device::parse(&uri).map_err(|error| {
            failure!(
                Code::Internal,
                "Failed to stage volume {}: error parsing URI {}: {}",
                volume_id,
                uri,
                error
            )
        })?;

        // 10 retries at 100ms intervals
        const TIMEOUT: Duration = Duration::from_millis(100);
        const RETRIES: u32 = 100;

        match get_access_type(&msg.volume_capability).map_err(|error| {
            failure!(
                Code::InvalidArgument,
                "Failed to publish volume {}: {}",
                volume_id,
                error
            )
        })? {
            AccessType::Mount(mnt) => {
                let fs_staging_path =
                    match make_fs_staging_path(&msg.staging_target_path) {
                        Ok(path) => path,
                        Err(error) => {
                            return Err(failure!(
                                Code::Internal,
                                "Failed to stage volume{}: {}",
                                volume_id,
                                error
                            ))
                        }
                    };

                if let Err(error) =
                    fs::create_dir_all(PathBuf::from(&fs_staging_path))
                {
                    return Err(failure!(
                        Code::Internal,
                        "Failed to stage volumes{}: {}",
                        volume_id,
                        error
                    ));
                }

                debug!("Staging volume {} to {}", volume_id, fs_staging_path);

                let fstype = if mnt.fs_type.is_empty() {
                    &self.filesystems[0]
                } else {
                    match self
                        .filesystems
                        .iter()
                        .find(|&entry| entry == &mnt.fs_type)
                    {
                        Some(fstype) => fstype,
                        None => {
                            return Err(failure!(
                        Code::InvalidArgument,
                        "Failed to stage volume {}: unsupported filesystem type: {}",
                        volume_id,
                        mnt.fs_type
                    ));
                        }
                    }
                };

                debug!("Volume {} has URI {}", volume_id, uri);

                let device = Device::parse(&uri).map_err(|error| {
                    failure!(
                        Code::Internal,
                        "Failed to stage volume {}: error parsing URI {}: {}",
                        volume_id,
                        uri,
                        error
                    )
                })?;

                if let Some(device_path) =
                    device.find().await.map_err(|error| {
                        failure!(
            Code::Internal,
            "Failed to stage volume {}: error locating device for URI {}: {}",
            volume_id,
            uri,
            error
        )
                    })?
                {
                    debug!("Found device {} for URI {}", device_path, uri);

                    if mount::find_mount(
                        Some(&device_path),
                        Some(&fs_staging_path),
                    )
                    .is_some()
                    {
                        debug!(
                            "Device {} is already mounted onto {}",
                            device_path, fs_staging_path
                        );
                        info!(
                            "Volume {} is already staged to {}",
                            volume_id, fs_staging_path
                        );
                        return Ok(Response::new(NodeStageVolumeResponse {}));
                    }

                    // abort if device is mounted somewhere else
                    if mount::find_mount(Some(&device_path), None).is_some() {
                        return Err(failure!(
                    Code::AlreadyExists,
                    "Failed to stage volume {}: device {} is already mounted elsewhere",
                    volume_id,
                    device_path
                ));
                    }

                    // abort if some another device is mounted on staging_path
                    if mount::find_mount(None, Some(&fs_staging_path)).is_some()
                    {
                        return Err(failure!(
                    Code::AlreadyExists,
                    "Failed to stage volume {}: another device is already mounted onto {}",
                    volume_id,
                    fs_staging_path
                ));
                    }

                    if let Err(error) =
                        prepare_device(&device_path, &fstype).await
                    {
                        return Err(failure!(
                    Code::Internal,
                    "Failed to stage volume {}: error preparing device {}: {}",
                    volume_id,
                    device_path,
                    error
                ));
                    }

                    debug!(
                        "Mounting device {} onto {}",
                        device_path, fs_staging_path
                    );

                    if let Err(error) = mount::filesystem_mount(
                        &device_path,
                        &fs_staging_path,
                        &fstype,
                        &mnt.mount_flags,
                    ) {
                        return Err(failure!(
                    Code::Internal,
                    "Failed to stage volume {}: failed to mount device {} onto {}: {}",
                    volume_id,
                    device_path,
                    fs_staging_path,
                    error
                ));
                    }

                    info!("Volume {} staged to {}", volume_id, fs_staging_path);
                    return Ok(Response::new(NodeStageVolumeResponse {}));
                }

                // device is not attached

                // abort if some another device is mounted on staging_path
                if mount::find_mount(None, Some(&fs_staging_path)).is_some() {
                    return Err(failure!(
                Code::AlreadyExists,
                "Failed to stage volume {}: another device is already mounted onto {}",
                volume_id,
                fs_staging_path
            ));
                }

                debug!("Attaching volume");
                if let Err(error) = device.attach().await {
                    return Err(failure!(
                        Code::Internal,
                        "Failed to stage volume {}: attach failed: {}",
                        volume_id,
                        error
                    ));
                }

                let device_path =
                    Device::wait_for_device(device, TIMEOUT, RETRIES)
                        .await
                        .map_err(|error| {
                            failure!(
                                Code::Unavailable,
                                "Failed to stage volume {}: {}",
                                volume_id,
                                error
                            )
                        })?;

                debug!("Found new device {} for URI {}", device_path, uri);

                if let Err(error) = prepare_device(&device_path, &fstype).await
                {
                    return Err(failure!(
                    Code::Internal,
                    "Failed to stage volume {}: error preparing device {}: {}",
                    volume_id,
                    device_path,
                    error
                ));
                }

                debug!(
                    "Mounting device {} onto {}",
                    device_path, fs_staging_path
                );

                if let Err(error) = mount::filesystem_mount(
                    &device_path,
                    &fs_staging_path,
                    &fstype,
                    &mnt.mount_flags,
                ) {
                    return Err(failure!(
                Code::Internal,
                "Failed to stage volume {}: failed to mount device {} onto {}: {}",
                volume_id,
                device_path,
                fs_staging_path,
                error
            ));
                }

                info!("Volume {} staged to {}", volume_id, fs_staging_path);
                Ok(Response::new(NodeStageVolumeResponse {}))
            }
            AccessType::Block(_) => {
                let device_path = device.find().await.map_err(|error| {
                    failure!(
            Code::Internal,
            "Failed to stage volume {}: error locating device for URI {}: {}",
            volume_id,
            uri,
            error
        )
                })?;

                if device_path.is_some() {
                    // Device is already attached
                    return Ok(Response::new(NodeStageVolumeResponse {}));
                }

                debug!("Attaching volume");
                // device.attach is idempotent, so does not restart the attach
                // process
                if let Err(error) = device.attach().await {
                    return Err(failure!(
                        Code::Internal,
                        "Failed to stage volume {}: attach failed: {}",
                        volume_id,
                        error
                    ));
                }

                let _ = Device::wait_for_device(device, TIMEOUT, RETRIES)
                    .await
                    .map_err(|error| {
                        failure!(
                            Code::Unavailable,
                            "Failed to stage volume {}: {}",
                            volume_id,
                            error
                        )
                    })?;
                Ok(Response::new(NodeStageVolumeResponse {}))
            }
        }
    }

    async fn node_unstage_volume(
        &self,
        request: Request<NodeUnstageVolumeRequest>,
    ) -> Result<Response<NodeUnstageVolumeResponse>, Status> {
        let msg = request.into_inner();

        let volume_id = msg.volume_id.clone();

        if volume_id.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to unstage volume: missing volume id"
            ));
        }

        if msg.staging_target_path.is_empty() {
            return Err(failure!(
                Code::InvalidArgument,
                "Failed to unstage volume {}: missing staging path",
                volume_id
            ));
        }

        // Get the filesystem staging mount point
        // If it does not exist, that means the volume
        // is a block mounted volume.
        let fs_staging_path =
            match make_fs_staging_path(&msg.staging_target_path) {
                Ok(path) => path,
                Err(error) => {
                    return Err(failure!(
                        Code::Internal,
                        "{}: {}",
                        error,
                        volume_id
                    ))
                }
            };

        debug!("Unstaging volume {} from {}", volume_id, fs_staging_path);

        let uuid = Uuid::parse_str(&volume_id).map_err(|error| {
            failure!(
                Code::Internal,
                "Failed to unstage volume {}: not a valid UUID: {}",
                volume_id,
                error
            )
        })?;

        if let Some(device) = Device::lookup(&uuid).await.map_err(|error| {
            failure!(
                Code::Internal,
                "Failed to unstage volume {}: error locating device: {}",
                volume_id,
                error
            )
        })? {
            let device_path = device.devname();
            debug!("Device path is {}", device_path);

            if Path::new(&fs_staging_path).exists() {
                // Filesystem staging.
                if mount::find_mount(Some(&device_path), Some(&fs_staging_path))
                    .is_some()
                {
                    debug!(
                        "Unmounting device {} from {}",
                        device_path, fs_staging_path
                    );

                    if let Err(error) =
                        mount::filesystem_unmount(&fs_staging_path)
                    {
                        return Err(failure!(
                        Code::Internal,
                        "Failed to unstage volume {}: failed to unmount device {} from {}: {}",
                        volume_id,
                        device_path,
                        fs_staging_path,
                        error
                    ));
                    }

                    debug!("Detaching device {}", device_path);
                    if let Err(error) = device.detach().await {
                        return Err(failure!(
                        Code::Internal,
                        "Failed to unstage volume {}: failed to detach device {}: {}",
                        volume_id,
                        device_path,
                        error
                    ));
                    }

                    info!(
                        "Volume {} unstaged from {}",
                        volume_id, fs_staging_path
                    );
                    return Ok(Response::new(NodeUnstageVolumeResponse {}));
                }

                // abort if device is mounted somewhere else
                if mount::find_mount(Some(&device_path), None).is_some() {
                    return Err(failure!(
                    Code::AlreadyExists,
                    "Failed to unstage volume {}: device {} is mounted elsewhere",
                    volume_id,
                    device_path
                ));
                }

                // abort if some other device is mounted on staging_path
                if mount::find_mount(None, Some(&fs_staging_path)).is_some() {
                    return Err(failure!(
                    Code::AlreadyExists,
                    "Failed to stage volume {}: another device is mounted onto {}",
                    volume_id,
                    fs_staging_path
                ));
                }
            } else {
                // Block volumes are not staged.
            }

            // Detach is required for filesystem and block volume mounts.
            debug!("Detaching device {}", device_path);
            if let Err(error) = device.detach().await {
                return Err(failure!(
                    Code::Internal,
                    "Failed to unstage volume {}: failed to detach device {}: {}",
                    volume_id,
                    device_path,
                    error
                ));
            }

            info!("Volume {} unstaged ", volume_id);
            if let Err(e) = std::fs::remove_dir(fs_staging_path) {
                warn!("{}", e);
            }
            return Ok(Response::new(NodeUnstageVolumeResponse {}));
        }

        // We did not find a device in udev.
        // This need not be an error however, as some device types (eg. nbd)
        // don't show up there. In the case where a mount is present,
        // just assume that the device in question does not need
        // to be detached once it has been unmounted.

        if Path::new(&fs_staging_path).exists() {
            if let Some(mount) = mount::find_mount(None, Some(&fs_staging_path))
            {
                debug!(
                    "Unmounting device {} from {}",
                    mount.source, fs_staging_path
                );
                if let Err(error) = mount::filesystem_unmount(&fs_staging_path)
                {
                    return Err(failure!(
                    Code::Internal,
                    "Failed to unstage volume {}: failed to unmount device {} from {}: {}",
                    volume_id,
                    mount.source,
                    fs_staging_path,
                    error
                ));
                }
            }
            if let Err(e) = std::fs::remove_dir(fs_staging_path) {
                warn!("{}", e);
            }
        } else {
            // Nothing to do for Block volumes.
        }

        info!("Volume {} unstaged", volume_id);
        Ok(Response::new(NodeUnstageVolumeResponse {}))
    }
}
