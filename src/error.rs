use std::{fmt, io, path::PathBuf};

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ApiError {
    pub code: &'static str,
    pub message: String,
    pub details: Value,
    pub retry_hint: String,
}

impl ApiError {
    pub fn new(
        code: &'static str,
        message: impl Into<String>,
        details: Value,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            details,
            retry_hint: hint.into(),
        }
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(
            "internal_error",
            message,
            Value::Null,
            "Check server stderr and retry.",
        )
    }
    /// Frame ID not found by index or name.
    pub fn frame_not_found(hint: impl Into<String>) -> Self {
        Self::new(
            "frame_not_found",
            "Frame not found in profile",
            Value::Null,
            hint,
        )
    }
    /// Invalid regex pattern.
    pub fn invalid_regex(pattern: &str, reason: impl Into<String>) -> Self {
        Self::new(
            "invalid_regex",
            format!("Invalid regex: {}", reason.into()),
            serde_json::json!({"pattern": pattern}),
            "Simplify the regex or fix its syntax and retry.",
        )
    }
    /// Parameter out of range.
    pub fn out_of_range(field: &str, value: usize, min: usize, max: usize) -> Self {
        Self::new(
            "invalid_budget",
            format!("{field} must be between {min} and {max}"),
            serde_json::json!({"field": field, "min": min, "max": max, "value": value}),
            "Use a documented output budget.",
        )
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ApiError {}

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("{0}")]
    Api(#[from] ApiError),
    #[error("I/O for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl From<ProfileError> for ApiError {
    fn from(value: ProfileError) -> Self {
        match value {
            ProfileError::Api(error) => error,
            ProfileError::Io { path, source } if source.kind() == io::ErrorKind::NotFound => {
                ApiError::new(
                    "profile_not_found",
                    format!("Profile does not exist: {}", path.display()),
                    serde_json::json!({"profile": path.display().to_string()}),
                    "Check the profile path and retry.",
                )
            }
            ProfileError::Io { path, .. } => ApiError::new(
                "internal_error",
                format!("Could not read profile: {}", path.display()),
                serde_json::json!({"profile": path.display().to_string()}),
                "Check file permissions and retry.",
            ),
        }
    }
}
