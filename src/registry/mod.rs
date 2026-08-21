//! Workspace profile registry: registration, alias resolution, and GC.
//!
//! Submodules: [`layout`] owns directory structure, [`manifest`] the manifest
//! schema and validation, [`persist`] atomic byte writes, [`errors`] shared
//! error constructors, and [`lock`] the advisory registration lock.

mod errors;
mod layout;
mod lock;
mod manifest;
mod persist;

pub use manifest::{Manifest, ManifestProfile};

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;

use crate::error::ApiError;
use crate::profile::{BuildLimits, ProfileBuilder};
use std::io::Cursor;

use errors::{
    corrupt, invalid_alias, path_text, profile_too_large, registry_io, serialize_path, source_error,
};
use layout::{
    MANIFEST, PROFILES_DIR, REGISTRY_DIR, ensure_profiles_dir, ensure_registry_layout,
    is_profile_blob_name, profile_file,
};
use lock::{RegistrationLock, unix_ms};
use manifest::{
    atomic_write_json, read_manifest_optional, read_manifest_required, safe_profile_path,
    validate_manifest,
};
use persist::atomic_write_bytes;

#[derive(Clone, Debug)]
pub struct Registration {
    pub alias: String,
    pub fingerprint: String,
    pub byte_len: u64,
    pub sample_period_us: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct ResolvedProfile {
    pub alias: String,
    pub fingerprint: String,
    pub path: PathBuf,
    pub byte_len: u64,
    pub sample_period_us: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegistryStatus {
    #[serde(serialize_with = "serialize_path")]
    pub registry_root: PathBuf,
    pub active: String,
    pub profiles: Vec<RegistryProfile>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegistryProfile {
    pub alias: String,
    pub fingerprint: String,
    pub source_name: String,
    pub byte_len: u64,
    pub registered_unix_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_period_us: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct GcReport {
    pub registry_root: PathBuf,
    pub dry_run: bool,
    pub removed: Vec<String>,
    pub skipped: Vec<String>,
}

pub fn valid_alias(alias: &str) -> bool {
    let bytes = alias.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
}

pub fn default_alias(source: &Path) -> String {
    source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| valid_alias(stem))
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "default".into())
}

pub fn register(
    workspace: &Path,
    source: &Path,
    explicit_alias: Option<&str>,
    max_file_size: u64,
    sample_period_us: Option<u64>,
) -> Result<Registration, ApiError> {
    if sample_period_us == Some(0) {
        return Err(ApiError::new(
            "invalid_sample_period",
            "sample period must be a positive integer of microseconds",
            json!({"sample_period_us":0}),
            "Pass --sample-period-us with a positive integer.",
        ));
    }
    let alias = explicit_alias
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| default_alias(source));
    if !valid_alias(&alias) {
        return Err(invalid_alias(&alias));
    }
    let metadata = fs::metadata(source).map_err(|error| source_error(source, error))?;
    if !metadata.file_type().is_file() {
        return Err(ApiError::new(
            "not_a_regular_file",
            format!("Profile is not a regular file: {}", source.display()),
            json!({"profile":path_text(source)}),
            "Select a regular folded stack file.",
        ));
    }
    if metadata.len() > max_file_size {
        return Err(profile_too_large(metadata.len(), max_file_size));
    }
    let bytes = fs::read(source).map_err(|error| source_error(source, error))?;
    if bytes.len() as u64 > max_file_size {
        return Err(profile_too_large(bytes.len() as u64, max_file_size));
    }
    let canonical_source = fs::canonicalize(source).map_err(|error| source_error(source, error))?;
    let parsed = ProfileBuilder::new(BuildLimits {
        max_file_bytes: max_file_size,
        ..BuildLimits::default()
    })
    .from_reader(
        Cursor::new(&bytes),
        canonical_source,
        bytes.len() as u64,
        None,
    )
    .map_err(ApiError::from)?;
    let fingerprint = parsed.source.fingerprint;
    let source_name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            ApiError::new(
                "not_a_regular_file",
                "Profile path does not have a file name",
                json!({"profile":path_text(source)}),
                "Select a folded profile file rather than a directory.",
            )
        })?
        .to_owned();

    let state = workspace.join(REGISTRY_DIR);
    ensure_registry_layout(&state)?;
    let _lock = RegistrationLock::acquire(&state)?;
    let file = profile_file(&fingerprint);
    let mut manifest = match read_manifest_optional(&state)? {
        Some(manifest) => {
            validate_manifest(&manifest)?;
            manifest
        }
        None => Manifest {
            schema_version: 1,
            active: alias.clone(),
            profiles: BTreeMap::new(),
        },
    };
    manifest.schema_version = 1;
    manifest.active = alias.clone();
    manifest.profiles.insert(
        alias.clone(),
        ManifestProfile {
            fingerprint: fingerprint.clone(),
            file: file.clone(),
            source_name,
            byte_len: bytes.len() as u64,
            registered_unix_ms: unix_ms(),
            sample_period_us,
        },
    );
    validate_manifest(&manifest)?;
    let destination = state.join(&file);
    let created_blob = match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            atomic_write_bytes(&destination, &bytes)?;
            true
        }
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            return Err(corrupt(
                "Registered profile path is not a regular file",
                json!({"file":file}),
            ));
        }
        Ok(_) => {
            if fs::read(&destination).map_err(|error| registry_io(&destination, error))? != bytes {
                return Err(corrupt(
                    "Existing deduplicated profile bytes do not match their fingerprint",
                    json!({"file":file}),
                ));
            }
            false
        }
        Err(error) => return Err(registry_io(&destination, error)),
    };
    if let Err(error) = atomic_write_json(&state.join(MANIFEST), &manifest) {
        if created_blob {
            let _ = fs::remove_file(&destination);
        }
        return Err(error);
    }
    Ok(Registration {
        alias,
        fingerprint,
        sample_period_us,
        byte_len: bytes.len() as u64,
    })
}

pub fn discover(workspace: &Path) -> Result<Option<PathBuf>, ApiError> {
    let mut here = workspace.to_path_buf();
    loop {
        let state = here.join(REGISTRY_DIR);
        match fs::symlink_metadata(&state) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() => {
                return Err(corrupt(
                    "Workspace .prof-mcp path is not a real directory",
                    json!({"path":path_text(&state)}),
                ));
            }
            Ok(_) => match fs::symlink_metadata(state.join(MANIFEST)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(metadata)
                    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() =>
                {
                    return Err(corrupt(
                        "Registry manifest is not a regular file",
                        json!({"path":path_text(&state.join(MANIFEST))}),
                    ));
                }
                Ok(_) => return Ok(Some(state)),
                Err(error) => return Err(registry_io(&state.join(MANIFEST), error)),
            },
            Err(error) => return Err(registry_io(&state, error)),
        }
        if !here.pop() {
            return Ok(None);
        }
    }
}

pub fn resolve(workspace: &Path, requested: Option<&str>) -> Result<ResolvedProfile, ApiError> {
    let state = discover(workspace)?.ok_or_else(|| {
        ApiError::new(
            "workspace_not_registered",
            "No .prof-mcp registry was found for this workspace",
            json!({"cwd":path_text(workspace)}),
            "Run prof-mcp register PATH from workspace root.",
        )
    })?;
    let manifest = read_manifest_required(&state)?;
    validate_manifest(&manifest)?;
    let alias = requested.unwrap_or(&manifest.active);
    if !valid_alias(alias) {
        return Err(invalid_alias(alias));
    }
    let entry = manifest.profiles.get(alias).ok_or_else(|| {
        ApiError::new(
            "profile_alias_not_found",
            format!("No registered profile alias: {alias}"),
            json!({"profile":alias,"available":manifest.profiles.keys().collect::<Vec<_>>() }),
            "Register it with prof-mcp register PATH --name ALIAS, or use an existing alias.",
        )
    })?;
    ensure_profiles_dir(&state)?;
    let path = safe_profile_path(&state, entry)?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| {
        corrupt(
            "Registered profile file is missing",
            json!({"file":entry.file}),
        )
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() != entry.byte_len
    {
        return Err(corrupt(
            "Registered profile file does not match its manifest entry",
            json!({"file":entry.file,"expected_byte_len":entry.byte_len,"actual_byte_len":metadata.len()}),
        ));
    }
    Ok(ResolvedProfile {
        alias: alias.to_owned(),
        fingerprint: entry.fingerprint.clone(),
        path,
        byte_len: entry.byte_len,
        sample_period_us: entry.sample_period_us,
    })
}

pub fn status(workspace: &Path) -> Result<RegistryStatus, ApiError> {
    let state = discover(workspace)?.ok_or_else(|| {
        ApiError::new(
            "workspace_not_registered",
            "No .prof-mcp registry was found for this workspace",
            json!({"cwd":path_text(workspace)}),
            "Run prof-mcp register PATH from workspace root.",
        )
    })?;
    let manifest = read_manifest_required(&state)?;
    validate_manifest(&manifest)?;
    let profiles = manifest
        .profiles
        .iter()
        .map(|(alias, profile)| RegistryProfile {
            alias: alias.clone(),
            fingerprint: profile.fingerprint.clone(),
            source_name: profile.source_name.clone(),
            byte_len: profile.byte_len,
            registered_unix_ms: profile.registered_unix_ms,
            sample_period_us: profile.sample_period_us,
        })
        .collect();
    Ok(RegistryStatus {
        registry_root: state,
        active: manifest.active,
        profiles,
    })
}

pub fn set_active(workspace: &Path, alias: &str) -> Result<RegistryStatus, ApiError> {
    if !valid_alias(alias) {
        return Err(invalid_alias(alias));
    }
    let state = discover(workspace)?.ok_or_else(|| {
        ApiError::new(
            "workspace_not_registered",
            "No .prof-mcp registry was found for this workspace",
            json!({"cwd":path_text(workspace)}),
            "Run prof-mcp register PATH from workspace root.",
        )
    })?;
    ensure_registry_layout(&state)?;
    let _lock = RegistrationLock::acquire(&state)?;
    let mut manifest = read_manifest_required(&state)?;
    validate_manifest(&manifest)?;
    if !manifest.profiles.contains_key(alias) {
        return Err(ApiError::new(
            "profile_alias_not_found",
            format!("No registered profile alias: {alias}"),
            json!({"profile":alias,"available":manifest.profiles.keys().collect::<Vec<_>>() }),
            "Run prof-mcp list and select an existing alias.",
        ));
    }
    manifest.active = alias.to_owned();
    atomic_write_json(&state.join(MANIFEST), &manifest)?;
    drop(_lock);
    status(workspace)
}

pub fn gc(workspace: &Path, dry_run: bool) -> Result<GcReport, ApiError> {
    let state = discover(workspace)?.ok_or_else(|| {
        ApiError::new(
            "workspace_not_registered",
            "No .prof-mcp registry was found for this workspace",
            json!({"cwd":path_text(workspace)}),
            "Run prof-mcp register PATH from workspace root.",
        )
    })?;
    ensure_registry_layout(&state)?;
    let _lock = RegistrationLock::acquire(&state)?;
    let manifest = read_manifest_required(&state)?;
    validate_manifest(&manifest)?;
    let referenced: BTreeSet<_> = manifest
        .profiles
        .values()
        .map(|entry| entry.file.clone())
        .collect();
    let profiles = state.join(PROFILES_DIR);
    let mut entries: Vec<_> = fs::read_dir(&profiles)
        .map_err(|error| registry_io(&profiles, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| registry_io(&profiles, error))?;
    entries.sort_by_key(|entry| entry.file_name());

    let mut removable = Vec::new();
    let mut skipped = Vec::new();
    for entry in entries {
        let name = entry.file_name();
        let relative = format!("{PROFILES_DIR}/{}", name.to_string_lossy());
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| registry_io(&path, error))?;
        if !is_profile_blob_name(&name)
            || metadata.file_type().is_symlink()
            || !metadata.file_type().is_file()
        {
            skipped.push(relative);
            continue;
        }
        if !referenced.contains(&relative) {
            removable.push((relative, path));
        }
    }

    let mut removed = Vec::with_capacity(removable.len());
    for (relative, path) in removable {
        if !dry_run {
            fs::remove_file(&path).map_err(|error| registry_io(&path, error))?;
        }
        removed.push(relative);
    }
    Ok(GcReport {
        registry_root: state,
        dry_run,
        removed,
        skipped,
    })
}
