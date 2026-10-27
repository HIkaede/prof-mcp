//! Shared error constructors and path helpers for registry operations.

use std::path::Path;

use serde_json::json;

use crate::error::{ApiError, ProfileError};

pub(crate) fn invalid_alias(alias: &str) -> ApiError {
    ApiError::new(
        "invalid_profile_alias",
        "Profile alias must match [A-Za-z0-9][A-Za-z0-9._-]{0,63}",
        json!({"alias":alias}),
        "Use an ASCII alias starting with a letter or digit.",
    )
}

pub(crate) fn corrupt(message: &str, details: serde_json::Value) -> ApiError {
    ApiError::new(
        "registry_corrupt",
        message,
        details,
        "Re-register the profile in this workspace.",
    )
}

pub(crate) fn path_text(path: &Path) -> String {
    path.display().to_string()
}

pub(crate) fn serialize_path<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.collect_str(&path.display())
}

pub(crate) fn source_error(path: &Path, error: std::io::Error) -> ApiError {
    ApiError::from(ProfileError::Io {
        path: path.to_path_buf(),
        source: error,
    })
}

pub(crate) fn registry_io(path: &Path, error: std::io::Error) -> ApiError {
    ApiError::new(
        "internal_error",
        format!("Could not update registry at {}: {error}", path.display()),
        json!({"path":path_text(path)}),
        "Check workspace permissions and retry.",
    )
}

pub(crate) fn profile_too_large(byte_len: u64, max_bytes: u64) -> ApiError {
    ApiError::new(
        "profile_too_large",
        "Profile exceeds configured maximum size",
        json!({"byte_len":byte_len,"max_bytes":max_bytes}),
        "Use a smaller profile or raise --max-file-size-mib.",
    )
}

/// Helper trait for registry I/O error context.
/// Usage: `fs::write(path, data).context_path(&path)?`
pub(crate) trait RegistryContext<T> {
    fn context_path(self, path: &Path) -> Result<T, ApiError>;
}

impl<T> RegistryContext<T> for std::io::Result<T> {
    fn context_path(self, path: &Path) -> Result<T, ApiError> {
        self.map_err(|error| registry_io(path, error))
    }
}
