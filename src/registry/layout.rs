//! Registry directory layout: fixed names, ignore file, and blob naming.

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use serde_json::json;

use super::errors::{corrupt, path_text, registry_io};
use super::manifest::valid_fingerprint;
use super::persist::{atomic_write_bytes, sync_directory};
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

pub(crate) fn ensure_registry_layout(state: &Path, create: bool) -> Result<(), ApiError> {
    ensure_real_directory(state, create, "Workspace .prof-mcp path")?;
    ensure_profiles_dir(state, create)?;
    ensure_ignore_file(state, create)
}

pub(crate) fn ensure_profiles_dir(state: &Path, create: bool) -> Result<(), ApiError> {
    ensure_real_directory(&state.join(PROFILES_DIR), create, "Registry profiles path")
}

pub(crate) fn ensure_ignore_file(state: &Path, repair: bool) -> Result<(), ApiError> {
    // Only registration touches the ignore file; queries and gc stay read-only.
    if !repair {
        return Ok(());
    }
    let path = state.join(IGNORE);
    let current = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            return Err(corrupt(
                "Registry .gitignore is not a regular file",
                json!({"path":path_text(&path)}),
            ));
        }
        Ok(_) => Some(fs::read(&path).map_err(|error| registry_io(&path, error))?),
        Err(error) => return Err(registry_io(&path, error)),
    };
    if current.as_deref() == Some(IGNORE_CONTENT.as_bytes()) {
        return Ok(());
    }
    atomic_write_bytes(&path, IGNORE_CONTENT.as_bytes())
}

pub(crate) fn ensure_real_directory(
    path: &Path,
    create_if_missing: bool,
    label: &str,
) -> Result<(), ApiError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create_if_missing => {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            if let Err(error) = builder.create(path)
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
    }?;
    if create_if_missing {
        let parent = path.parent().expect("registry directory has parent");
        sync_directory(parent).map_err(|error| registry_io(parent, error))?;
    }
    Ok(())
}

pub(crate) fn is_profile_blob_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_suffix(".folded"))
        .is_some_and(valid_fingerprint)
}
