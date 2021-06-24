//! Functions for CSI stage, unstage, publish and unpublish filesystem volumes.

use std::{fs, io::ErrorKind, path::PathBuf};

use tonic::{Code, Status};

macro_rules! failure {
    (Code::$code:ident, $msg:literal) => {{ error!($msg); Status::new(Code::$code, $msg) }};
    (Code::$code:ident, $fmt:literal $(,$args:expr)+) => {{ let message = format!($fmt $(,$args)+); error!("{}", message); Status::new(Code::$code, message) }};
}

use crate::{
    csi::volume_capability::MountVolume,
    format::prepare_device,
    mount,
};

pub async fn publish_fs_volume(
    volume_id: &str,
    target_path: &str,
    device_path: String,
    mnt: &MountVolume,
    filesystems: &[String],
) -> Result<(), Status> {
    // One final check for fs volumes, ignore for block volumes.
    if let Err(err) = fs::create_dir_all(PathBuf::from(target_path)) {
        if err.kind() != ErrorKind::AlreadyExists {
            return Err(Status::new(
                Code::Internal,
                format!(
                    "Failed to create mountpoint {} for volume {}: {}",
                    target_path, volume_id, err
                ),
            ));
        }
    }

    debug!("Staging volume {} to {}", volume_id, target_path);

    let fstype = if mnt.fs_type.is_empty() {
        String::from(&filesystems[0])
    } else {
        match filesystems.iter().find(|&entry| entry == &mnt.fs_type) {
            Some(fstype) => String::from(fstype),
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

    if mount::find_mount(Some(&device_path), Some(target_path)).is_some() {
        debug!(
            "Device {} is already mounted onto {}",
            device_path, target_path
        );
        info!("Volume {} is already staged to {}", volume_id, target_path);
        return Ok(());
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
    if mount::find_mount(None, Some(target_path)).is_some() {
        return Err(failure!(
                    Code::AlreadyExists,
                    "Failed to stage volume {}: another device is already mounted onto {}",
                    volume_id,
                    target_path
                ));
    }

    if let Err(error) = prepare_device(&device_path, &fstype).await {
        return Err(failure!(
            Code::Internal,
            "Failed to stage volume {}: error preparing device {}: {}",
            volume_id,
            device_path,
            error
        ));
    }

    debug!("Mounting device {} onto {}", device_path, target_path);

    if let Err(error) = mount::filesystem_mount(
        &device_path,
        target_path,
        &fstype,
        &mnt.mount_flags,
    ) {
        return Err(failure!(
            Code::Internal,
            "Failed to stage volume {}: failed to mount device {} onto {}: {}",
            volume_id,
            device_path,
            target_path,
            error
        ));
    }

    /* Need to mount readonly above if readonly is set

    if msg.readonly && !mount.options.readonly() {
        let mut options = mnt.mount_flags.clone();
        options.push(String::from("ro"));

        debug!("Remounting {} as readonly", target_path);

        if let Err(error) = mount::bind_remount(target_path, &options) {
            let message = format!(
                    "Failed to publish volume {}: failed to mount {} to {} as readonly: {}",
                    volume_id,
                    staging_target_path,
                    target_path,
                    error
                );

            error!("Failed to remount {}: {}", target_path, error);

            debug!("Unmounting {}", target_path);

            if let Err(error) = mount::bind_unmount(target_path) {
                error!("Failed to unmount {}: {}", target_path, error);
            }

            return Err(Status::new(Code::Internal, message));
        }
    }
    */

    info!("Volume {} published to {}", volume_id, target_path);

    Ok(())
}

pub fn unpublish_fs_volume(
    volume_id: &str,
    target_path: &str,
) -> Result<(), Status> {
    if mount::find_mount(None, Some(target_path)).is_none() {
        // No mount found for target_path.
        // The idempotency requirement means this is not an error.
        // Just clean up as best we can and claim success.

        if let Err(error) = fs::remove_dir(PathBuf::from(target_path)) {
            if error.kind() != ErrorKind::NotFound {
                error!("Failed to remove directory {}: {}", target_path, error);
            }
        }

        info!(
            "Volume {} is already unpublished from {}",
            volume_id, target_path
        );

        return Ok(());
    }

    debug!("Unmounting {}", target_path);

    if let Err(error) = mount::filesystem_unmount(target_path) {
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
            error!("Failed to remove directory {}: {}", target_path, error);
        }
    }

    info!("Volume {} unpublished from {}", volume_id, target_path);
    Ok(())
}
