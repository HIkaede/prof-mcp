//! Workspace-local, validated profile registration.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Cursor, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use crate::{
    error::{ApiError, ProfileError},
    profile::{BuildLimits, ProfileBuilder},
};

pub const REGISTRY_DIR: &str = ".prof-mcp";
const MANIFEST: &str = "manifest.json";
const PROFILES_DIR: &str = "profiles";
const LOCK: &str = ".register.lock";
const IGNORE: &str = ".gitignore";
const IGNORE_CONTENT: &str = "\
# prof-mcp data files are local to this workspace.
# Keep this file visible so the registry directory can be intentionally ignored.
*
!.gitignore
";

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
}

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

#[derive(Clone, Debug, Serialize)]
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

fn profile_file(fingerprint: &str) -> String {
    format!("{PROFILES_DIR}/{fingerprint}.folded")
}

fn ensure_registry_layout(state: &Path) -> Result<(), ApiError> {
    ensure_real_directory(state, true, "Workspace .prof-mcp path")?;
    ensure_profiles_dir(state)?;
    ensure_ignore_file(state)
}

fn ensure_profiles_dir(state: &Path) -> Result<(), ApiError> {
    ensure_real_directory(&state.join(PROFILES_DIR), true, "Registry profiles path")
}

fn ensure_ignore_file(state: &Path) -> Result<(), ApiError> {
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

fn ensure_real_directory(
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

fn read_manifest_optional(state: &Path) -> Result<Option<Manifest>, ApiError> {
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

fn read_manifest_required(state: &Path) -> Result<Manifest, ApiError> {
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

fn validate_manifest(manifest: &Manifest) -> Result<(), ApiError> {
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

fn valid_source_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn safe_profile_path(state: &Path, entry: &ManifestProfile) -> Result<PathBuf, ApiError> {
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

fn valid_fingerprint(fingerprint: &str) -> bool {
    fingerprint.len() == 64
        && fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn is_profile_blob_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_suffix(".folded"))
        .is_some_and(valid_fingerprint)
}

fn invalid_alias(alias: &str) -> ApiError {
    ApiError::new(
        "invalid_profile_alias",
        "Profile alias must match [A-Za-z0-9][A-Za-z0-9._-]{0,63}",
        json!({"alias":alias}),
        "Use an ASCII alias starting with a letter or digit.",
    )
}

fn corrupt(message: &str, details: serde_json::Value) -> ApiError {
    ApiError::new(
        "registry_corrupt",
        message,
        details,
        "Re-register the profile in this workspace.",
    )
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

fn serialize_path<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&path_text(path))
}

fn source_error(path: &Path, error: std::io::Error) -> ApiError {
    ApiError::from(ProfileError::Io {
        path: path.to_path_buf(),
        source: error,
    })
}

fn registry_io(path: &Path, error: std::io::Error) -> ApiError {
    ApiError::new(
        "internal_error",
        format!("Could not update registry at {}: {error}", path.display()),
        json!({"path":path_text(path)}),
        "Check workspace permissions and retry.",
    )
}

fn profile_too_large(byte_len: u64, max_bytes: u64) -> ApiError {
    ApiError::new(
        "profile_too_large",
        "Profile exceeds configured maximum size",
        json!({"byte_len":byte_len,"max_bytes":max_bytes}),
        "Use a smaller profile or raise --max-file-size-mib.",
    )
}

fn atomic_write_bytes(destination: &Path, bytes: &[u8]) -> Result<(), ApiError> {
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

fn atomic_write_json(destination: &Path, manifest: &Manifest) -> Result<(), ApiError> {
    let mut bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|_| ApiError::internal("Could not serialize registry manifest"))?;
    bytes.push(b'\n');
    atomic_replace(destination, &bytes)
}

fn atomic_replace(destination: &Path, bytes: &[u8]) -> Result<(), ApiError> {
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

struct RegistrationLock(fs::File);
impl RegistrationLock {
    fn acquire(state: &Path) -> Result<Self, ApiError> {
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
fn lock_file_matches_path(file: &fs::File, path: &Path) -> Result<bool, ApiError> {
    let opened = file.metadata().map_err(|error| registry_io(path, error))?;
    let current = fs::symlink_metadata(path).map_err(|error| registry_io(path, error))?;
    Ok(!current.file_type().is_symlink()
        && current.file_type().is_file()
        && opened.dev() == current.dev()
        && opened.ino() == current.ino())
}

#[cfg(not(unix))]
fn lock_file_matches_path(_file: &fs::File, path: &Path) -> Result<bool, ApiError> {
    Err(ApiError::new(
        "internal_error",
        "Safe registry advisory locking is unavailable on this platform",
        json!({"path":path_text(path)}),
        "Run prof-mcp on a platform with stable lock-file identity checks.",
    ))
}

fn unix_ms() -> u64 {
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
