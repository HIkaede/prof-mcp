//! Manifest schema, validation, and safe path resolution.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::errors::{corrupt, path_text, registry_io};
use super::layout::{MANIFEST, profile_file};
use super::persist::atomic_replace;
use super::valid_alias;
use crate::error::ApiError;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub active: String,
    pub profiles: BTreeMap<String, ManifestProfile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestProfile {
    pub fingerprint: String,
    pub file: String,
    pub source_name: String,
    pub byte_len: u64,
    pub registered_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_period_us: Option<u64>,
}

pub(crate) fn read_manifest_optional(state: &Path) -> Result<Option<Manifest>, ApiError> {
    let path = state.join(MANIFEST);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            return Err(corrupt(
                "Registry manifest is not a regular file",
                json!({"path":path_text(&path)}),
            ));
        }
        Ok(_) => {}
        Err(error) => return Err(registry_io(&path, error)),
    }
    read_manifest_required(state).map(Some)
}

pub(crate) fn read_manifest_required(state: &Path) -> Result<Manifest, ApiError> {
    let path = state.join(MANIFEST);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(corrupt(
                "Registry manifest is not a regular file",
                json!({"path":path_text(&path)}),
            ));
        }
        Err(error) => return Err(registry_io(&path, error)),
    }
    let bytes = fs::read(&path).map_err(|error| registry_io(&path, error))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        corrupt(
            "Registry manifest is not valid schema_version 1 JSON",
            json!({"error":error.to_string()}),
        )
    })
}

pub(crate) fn validate_manifest(manifest: &Manifest) -> Result<(), ApiError> {
    if manifest.schema_version != 1 || !valid_alias(&manifest.active) {
        return Err(corrupt(
            "Registry schema version or active alias is invalid",
            json!({}),
        ));
    }
    if manifest.profiles.is_empty() || !manifest.profiles.contains_key(&manifest.active) {
        return Err(corrupt(
            "Registry active alias is not registered",
            json!({"active":manifest.active}),
        ));
    }
    for (alias, entry) in &manifest.profiles {
        if !valid_alias(alias)
            || !valid_fingerprint(&entry.fingerprint)
            || entry.file != profile_file(&entry.fingerprint)
            || !valid_source_name(&entry.source_name)
        {
            return Err(corrupt(
                "Registry contains an invalid profile entry",
                json!({"alias":alias}),
            ));
        }
    }
    Ok(())
}

pub(crate) fn valid_source_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

pub(crate) fn safe_profile_path(
    state: &Path,
    entry: &ManifestProfile,
) -> Result<PathBuf, ApiError> {
    let relative = Path::new(&entry.file);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(corrupt(
            "Registry profile path escapes .prof-mcp",
            json!({"file":entry.file}),
        ));
    }
    Ok(state.join(relative))
}

pub(crate) fn valid_fingerprint(fingerprint: &str) -> bool {
    fingerprint.len() == 64
        && fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub(crate) fn atomic_write_json(destination: &Path, manifest: &Manifest) -> Result<(), ApiError> {
    let mut bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|_| ApiError::internal("Could not serialize registry manifest"))?;
    bytes.push(b'\n');
    atomic_replace(destination, &bytes)
}
