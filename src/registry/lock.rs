//! Advisory registration lock with replaced-file identity detection.

use std::fs::{self, OpenOptions};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde_json::json;

use super::errors::{corrupt, path_text, registry_io};
use super::layout::LOCK;
use crate::error::ApiError;

pub(crate) struct RegistrationLock(fs::File);
impl RegistrationLock {
    pub(crate) fn acquire(state: &Path) -> Result<Self, ApiError> {
        let path = state.join(LOCK);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(metadata)
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() =>
            {
                return Err(corrupt(
                    "Registry lock path is not a regular file",
                    json!({"path":path_text(&path)}),
                ));
            }
            Ok(_) => {}
            Err(error) => return Err(registry_io(&path, error)),
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| registry_io(&path, error))?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| registry_io(&path, error))?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(corrupt(
                "Registry lock path is not a regular file",
                json!({"path":path_text(&path)}),
            ));
        }
        if !lock_file_matches_path(&file, &path)? {
            return Err(corrupt(
                "Registry lock path changed while it was being opened",
                json!({"path":path_text(&path)}),
            ));
        }
        file.try_lock_exclusive().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                ApiError::new(
                    "registry_busy",
                    "Another registry operation is already updating this workspace",
                    json!({}),
                    "Retry after the other prof-mcp registry operation exits.",
                )
            } else {
                registry_io(&path, error)
            }
        })?;
        if !lock_file_matches_path(&file, &path)? {
            return Err(corrupt(
                "Registry lock path changed while it was being acquired",
                json!({"path":path_text(&path)}),
            ));
        }
        Ok(Self(file))
    }
}
impl Drop for RegistrationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

#[cfg(unix)]
pub(crate) fn lock_file_matches_path(file: &fs::File, path: &Path) -> Result<bool, ApiError> {
    let opened = file.metadata().map_err(|error| registry_io(path, error))?;
    let current = fs::symlink_metadata(path).map_err(|error| registry_io(path, error))?;
    Ok(!current.file_type().is_symlink()
        && current.file_type().is_file()
        && opened.dev() == current.dev()
        && opened.ino() == current.ino())
}

#[cfg(not(unix))]
pub(crate) fn lock_file_matches_path(_file: &fs::File, path: &Path) -> Result<bool, ApiError> {
    Err(ApiError::new(
        "internal_error",
        "Safe registry advisory locking is unavailable on this platform",
        json!({"path":path_text(path)}),
        "Run prof-mcp on a platform with stable lock-file identity checks.",
    ))
}

pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, fs::OpenOptions};

    use super::lock_file_matches_path;
    use tempfile::tempdir;

    #[test]
    fn lock_identity_rejects_a_replaced_regular_file() {
        let root = tempdir().unwrap();
        let lock = root.path().join(".register.lock");
        let replacement = root.path().join("replacement.lock");
        fs::write(&lock, "old").unwrap();
        fs::write(&replacement, "new").unwrap();
        let opened = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock)
            .unwrap();
        fs::rename(&replacement, &lock).unwrap();
        assert!(!lock_file_matches_path(&opened, &lock).unwrap());
    }
}
