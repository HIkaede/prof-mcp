use std::{
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use lru::LruCache;
use tokio::sync::Mutex;

use crate::{
    error::{ApiError, ProfileError},
    profile::{BuildLimits, Profile, ProfileBuilder},
};

#[derive(Clone)]
pub struct ProfileCache {
    roots: Arc<Vec<PathBuf>>,
    max_file_size: u64,
    entries: Arc<Mutex<LruCache<PathBuf, CacheEntry>>>,
}

#[derive(Clone)]
struct CacheEntry {
    byte_len: u64,
    modified_unix_ms: Option<u64>,
    profile: Arc<Profile>,
}

impl ProfileCache {
    pub fn new(roots: Vec<PathBuf>, max_file_size: u64, capacity: usize) -> Result<Self, ApiError> {
        if roots.is_empty() {
            return Err(ApiError::new(
                "invalid_budget",
                "At least one allowed root is required",
                serde_json::json!({}),
                "Pass --root PATH at least once.",
            ));
        }
        let capacity = NonZeroUsize::new(capacity).ok_or_else(|| {
            ApiError::new(
                "invalid_budget",
                "cache_capacity must be at least 1",
                serde_json::json!({"cache_capacity":capacity}),
                "Set --cache-capacity to a positive integer.",
            )
        })?;
        let roots = roots
            .into_iter()
            .map(|root| {
                std::fs::canonicalize(&root).map_err(|_| {
                    ApiError::new(
                        "profile_not_found",
                        format!("Configured root does not exist: {}", root.display()),
                        serde_json::json!({"root":root}),
                        "Pass an existing directory with --root.",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            roots: Arc::new(roots),
            max_file_size,
            entries: Arc::new(Mutex::new(LruCache::new(capacity))),
        })
    }

    pub async fn load(&self, reference: &str) -> Result<Arc<Profile>, ApiError> {
        let (path, byte_len, modified_unix_ms) = self.resolve(reference)?;
        {
            let mut cache = self.entries.lock().await;
            if let Some(entry) = cache.get(&path)
                && entry.byte_len == byte_len
                && entry.modified_unix_ms == modified_unix_ms
            {
                return Ok(entry.profile.clone());
            }
        }
        let parse_path = path.clone();
        let max_file_size = self.max_file_size;
        let profile = tokio::task::spawn_blocking(move || {
            ProfileBuilder::new(BuildLimits {
                max_file_bytes: max_file_size,
                ..BuildLimits::default()
            })
            .from_file(parse_path.clone(), byte_len, modified_unix_ms)
        })
        .await
        .map_err(|_| ApiError::internal("Profile parsing task failed"))?
        .map_err(ApiError::from)?;
        let profile = Arc::new(profile);
        self.entries.lock().await.put(
            path,
            CacheEntry {
                byte_len,
                modified_unix_ms,
                profile: profile.clone(),
            },
        );
        Ok(profile)
    }

    fn resolve(&self, reference: &str) -> Result<(PathBuf, u64, Option<u64>), ApiError> {
        let raw = Path::new(reference);
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.roots[0].join(raw)
        };
        let canonical = std::fs::canonicalize(&candidate).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ApiError::new(
                    "profile_not_found",
                    format!("Profile does not exist: {reference}"),
                    serde_json::json!({"profile":reference}),
                    "Check the path relative to the configured root.",
                )
            } else {
                ApiError::new(
                    "profile_not_found",
                    format!("Profile cannot be resolved: {reference}"),
                    serde_json::json!({"profile":reference}),
                    "Check the path and permissions relative to the configured root.",
                )
            }
        })?;
        if !self.roots.iter().any(|root| canonical.starts_with(root)) {
            return Err(ApiError::new(
                "path_outside_root",
                format!("Profile is outside an allowed root: {reference}"),
                serde_json::json!({"profile":reference}),
                "Use a path inside a configured --root.",
            ));
        }
        let metadata = std::fs::metadata(&canonical).map_err(|source| {
            ApiError::from(ProfileError::Io {
                path: canonical.clone(),
                source,
            })
        })?;
        if !metadata.file_type().is_file() {
            return Err(ApiError::new(
                "not_a_regular_file",
                format!("Profile is not a regular file: {reference}"),
                serde_json::json!({"profile":reference}),
                "Select a regular folded stack file.",
            ));
        }
        if metadata.len() > self.max_file_size {
            return Err(ApiError::new(
                "profile_too_large",
                format!("Profile exceeds configured maximum size: {reference}"),
                serde_json::json!({"profile":reference, "byte_len":metadata.len(), "max_bytes":self.max_file_size}),
                "Use a smaller profile or raise --max-file-size-mib.",
            ));
        }
        let modified_unix_ms = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| u64::try_from(duration.as_millis()).ok());
        Ok((canonical, metadata.len(), modified_unix_ms))
    }
}
