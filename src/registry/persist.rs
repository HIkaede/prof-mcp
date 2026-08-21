//! Crash-safe byte persistence via temp file plus atomic rename.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use super::errors::registry_io;
use crate::error::ApiError;

pub(crate) fn atomic_write_bytes(destination: &Path, bytes: &[u8]) -> Result<(), ApiError> {
    let parent = destination.parent().expect("profile path has parent");
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        destination
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("profile"),
        std::process::id()
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|error| registry_io(destination, error))
}

pub(crate) fn atomic_replace(destination: &Path, bytes: &[u8]) -> Result<(), ApiError> {
    let parent = destination.parent().expect("manifest has parent");
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        destination
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("manifest"),
        std::process::id()
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|error| registry_io(destination, error))
}
