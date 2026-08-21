//! Registry directory layout: fixed names, ignore file, and blob naming.

use std::fs;
use std::path::Path;

use serde_json::json;

use super::errors::{corrupt, path_text, registry_io};
use super::manifest::valid_fingerprint;
use super::persist::atomic_write_bytes;
use crate::error::ApiError;

pub(crate) const REGISTRY_DIR: &str = ".prof-mcp";
pub(crate) const MANIFEST: &str = "manifest.json";
pub(crate) const PROFILES_DIR: &str = "profiles";
pub(crate) const LOCK: &str = ".register.lock";
pub(crate) const IGNORE: &str = ".gitignore";
pub(crate) const IGNORE_CONTENT: &str = "\
# prof-mcp data files are local to this workspace.
# Keep this file visible so the registry directory can be intentionally ignored.
*
!.gitignore
";

pub(crate) fn profile_file(fingerprint: &str) -> String {
    format!("{PROFILES_DIR}/{fingerprint}.folded")
}

pub(crate) fn ensure_registry_layout(state: &Path) -> Result<(), ApiError> {
    ensure_real_directory(state, true, "Workspace .prof-mcp path")?;
    ensure_profiles_dir(state)?;
    ensure_ignore_file(state)
}

pub(crate) fn ensure_profiles_dir(state: &Path) -> Result<(), ApiError> {
    ensure_real_directory(&state.join(PROFILES_DIR), true, "Registry profiles path")
}

pub(crate) fn ensure_ignore_file(state: &Path) -> Result<(), ApiError> {
    let path = state.join(IGNORE);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            atomic_write_bytes(&path, IGNORE_CONTENT.as_bytes())
        }
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            Err(corrupt(
                "Registry .gitignore is not a regular file",
                json!({"path":path_text(&path)}),
            ))
        }
        Ok(_) => Ok(()),
        Err(error) => Err(registry_io(&path, error)),
    }
}

pub(crate) fn ensure_real_directory(
    path: &Path,
    create_if_missing: bool,
    label: &str,
) -> Result<(), ApiError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create_if_missing => {
            if let Err(error) = fs::create_dir(path)
                && error.kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(registry_io(path, error));
            }
            ensure_real_directory(path, false, label)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(corrupt(
            &format!("{label} is missing"),
            json!({"path":path_text(path)}),
        )),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() => {
            Err(corrupt(
                &format!("{label} is not a real directory"),
                json!({"path":path_text(path)}),
            ))
        }
        Ok(_) => Ok(()),
        Err(error) => Err(registry_io(path, error)),
    }
}

pub(crate) fn is_profile_blob_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_suffix(".folded"))
        .is_some_and(valid_fingerprint)
}
