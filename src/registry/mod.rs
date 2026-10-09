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
use std::io::{self, BufReader, Read, Seek};

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
    valid_source_name, validate_manifest,
};
use persist::{atomic_copy, files_equal, sync_directory};

#[derive(Clone, Debug)]
pub struct Registration {
    pub alias: String,
    pub fingerprint: String,
    pub byte_len: u64,
}

#[derive(Clone, Debug)]
pub struct ResolvedProfile {
    pub alias: String,
    pub fingerprint: String,
    pub path: PathBuf,
    pub byte_len: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegistryStatus {
    #[serde(serialize_with = "serialize_path")]
    pub registry_root: PathBuf,
    pub active: String,
    pub profiles: Vec<RegistryProfile>,
}

#[derive(Clone, Debug, serde::Deserialize, schemars::JsonSchema, Serialize)]
pub struct RegistryProfile {
    pub alias: String,
    pub fingerprint: String,
    pub source_name: String,
    pub byte_len: u64,
    pub registered_unix_ms: u64,
}

#[derive(Clone, Debug)]
pub struct GcReport {
    pub registry_root: PathBuf,
    pub dry_run: bool,
    pub removed: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Removal {
    pub alias: String,
    pub active: String,
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
) -> Result<Registration, ApiError> {
    let metadata = fs::symlink_metadata(source).map_err(|error| source_error(source, error))?;
    if !metadata.file_type().is_file() {
        return Err(ApiError::new(
            "not_a_regular_file",
            "Profile must be a regular file",
            json!({"profile":path_text(source)}),
            "Select a regular folded stack file.",
        ));
    }
    if metadata.len() > max_file_size {
        return Err(profile_too_large(metadata.len(), max_file_size));
    }
    let file = fs::File::open(source).map_err(|error| source_error(source, error))?;
    let source_name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| ApiError::internal("Profile path has no file name"))?;
    let alias = explicit_alias
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| default_alias(source));
    register_reader(workspace, file, &source_name, Some(&alias), max_file_size)
}

pub fn register_reader(
    workspace: &Path,
    reader: impl Read,
    source_name: &str,
    explicit_alias: Option<&str>,
    max_file_size: u64,
) -> Result<Registration, ApiError> {
    let alias = explicit_alias
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| default_alias(Path::new(source_name)));
    if !valid_alias(&alias) {
        return Err(invalid_alias(&alias));
    }
    if !valid_source_name(source_name) {
        return Err(ApiError::new(
            "invalid_source_name",
            "Profile source must be a file name",
            json!({"source_name":source_name}),
            "Use a single file name.",
        ));
    }
    // Spool before parsing so raw input and parsed indexes do not share the heap.
    let mut spool =
        tempfile::tempfile().map_err(|error| source_error(Path::new(source_name), error))?;
    let byte_len = io::copy(
        &mut reader.take(max_file_size.saturating_add(1)),
        &mut spool,
    )
    .map_err(|error| source_error(Path::new(source_name), error))?;
    if byte_len > max_file_size {
        return Err(profile_too_large(byte_len, max_file_size));
    }
    spool
        .rewind()
        .map_err(|error| source_error(Path::new(source_name), error))?;
    let parsed = ProfileBuilder::new(BuildLimits {
        max_file_bytes: max_file_size,
        ..BuildLimits::default()
    })
    .from_reader(
        BufReader::new(&mut spool),
        PathBuf::from(source_name),
        byte_len,
        None,
    )
    .map_err(ApiError::from)?;
    let fingerprint = parsed.source.fingerprint.clone();
    drop(parsed);
    spool
        .rewind()
        .map_err(|error| source_error(Path::new(source_name), error))?;
    let source_name = source_name.to_owned();

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
            byte_len,
            registered_unix_ms: unix_ms(),
        },
    );
    validate_manifest(&manifest)?;
    let destination = state.join(&file);
    let created_blob = match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            atomic_copy(&destination, &mut spool)?;
            true
        }
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            return Err(corrupt(
                "Registered profile path is not a regular file",
                json!({"file":file}),
            ));
        }
        Ok(_) => {
            if !files_equal(&mut spool, &destination)
                .map_err(|error| registry_io(&destination, error))?
            {
                return Err(corrupt(
                    "Existing deduplicated profile bytes do not match their fingerprint",
                    json!({"file":file}),
                ));
            }
            fs::File::open(&destination)
                .and_then(|file| file.sync_all())
                .map_err(|error| registry_io(&destination, error))?;
            let profiles = state.join(PROFILES_DIR);
            sync_directory(&profiles).map_err(|error| registry_io(&profiles, error))?;
            false
        }
        Err(error) => return Err(registry_io(&destination, error)),
    };
    if let Err(error) = atomic_write_json(&state.join(MANIFEST), &manifest) {
        if created_blob && error.details["committed"] != true {
            let _ = fs::remove_file(&destination);
        }
        return Err(error);
    }
    Ok(Registration {
        alias,
        fingerprint,
        byte_len,
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
            alias_details(&manifest, alias),
            "Register it with prof-mcp register PATH --name ALIAS, or use an existing alias.",
        )
    })?;
    ensure_profiles_dir(&state, false)?;
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
            alias_details(&manifest, alias),
            "Run prof-mcp list and select an existing alias.",
        ));
    }
    manifest.active = alias.to_owned();
    atomic_write_json(&state.join(MANIFEST), &manifest)?;
    drop(_lock);
    status(workspace)
}

pub fn remove(
    workspace: &Path,
    alias: &str,
    new_active: Option<&str>,
) -> Result<Removal, ApiError> {
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
            alias_details(&manifest, alias),
            "Run prof-mcp list and select an existing alias.",
        ));
    }
    if manifest.profiles.len() == 1 {
        return Err(ApiError::new(
            "cannot_remove_last_alias",
            "The last registered profile alias cannot be removed",
            json!({"profile":alias}),
            "Register a replacement profile before removing this alias.",
        ));
    }
    let removing_active = manifest.active == alias;
    let replacement = if removing_active {
        let replacement = new_active.ok_or_else(|| {
            ApiError::new(
                "active_alias_requires_replacement",
                "Removing the active alias requires --new-active",
                json!({"profile":alias}),
                "Pass --new-active with another registered alias.",
            )
        })?;
        if !valid_alias(replacement) {
            return Err(invalid_alias(replacement));
        }
        if replacement == alias {
            return Err(ApiError::new(
                "replacement_alias_same_as_removed",
                "The replacement active alias must differ from the removed alias",
                json!({"profile":alias}),
                "Pass another registered alias with --new-active.",
            ));
        }
        if !manifest.profiles.contains_key(replacement) {
            return Err(ApiError::new(
                "profile_alias_not_found",
                format!("No registered replacement alias: {replacement}"),
                alias_details(&manifest, replacement),
                "Run prof-mcp list and select an existing alias.",
            ));
        }
        Some(replacement)
    } else {
        if new_active.is_some() {
            return Err(ApiError::new(
                "new_active_only_for_active_alias",
                "--new-active is only valid when removing the active alias",
                json!({"profile":alias}),
                "Remove the flag or remove the active alias instead.",
            ));
        }
        None
    };
    manifest.profiles.remove(alias);
    if let Some(replacement) = replacement {
        manifest.active = replacement.to_owned();
    }
    validate_manifest(&manifest)?;
    atomic_write_json(&state.join(MANIFEST), &manifest)?;
    Ok(Removal {
        alias: alias.to_owned(),
        active: manifest.active,
    })
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

fn alias_details(manifest: &Manifest, alias: &str) -> serde_json::Value {
    json!({"profile":alias,
        "available":manifest.profiles.keys().take(100).collect::<Vec<_>>(),
        "available_count":manifest.profiles.len(),
        "omitted":manifest.profiles.len().saturating_sub(100)})
}
