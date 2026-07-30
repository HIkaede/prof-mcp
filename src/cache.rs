use std::{num::NonZeroUsize, path::PathBuf, sync::Arc, time::UNIX_EPOCH};

use lru::LruCache;
use tokio::sync::Mutex;

use crate::{
    error::{ApiError, ProfileError},
    profile::{BuildLimits, Profile, ProfileBuilder},
    registry,
};

#[derive(Clone, Debug)]
pub struct LoadedProfile {
    pub alias: String,
    pub profile: Arc<Profile>,
}

#[derive(Clone)]
pub struct ProfileCache {
    workspace: PathBuf,
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
    pub fn new(workspace: PathBuf, max_file_size: u64, capacity: usize) -> Result<Self, ApiError> {
        let capacity = NonZeroUsize::new(capacity).ok_or_else(|| {
            ApiError::new(
                "invalid_budget",
                "cache_capacity must be at least 1",
                serde_json::json!({"cache_capacity":capacity}),
                "Set --cache-capacity to a positive integer.",
            )
        })?;
        Ok(Self {
            workspace,
            max_file_size,
            entries: Arc::new(Mutex::new(LruCache::new(capacity))),
        })
    }

    pub async fn load(&self, alias: Option<&str>) -> Result<LoadedProfile, ApiError> {
        let resolved = registry::resolve(&self.workspace, alias)?;
        let metadata = std::fs::metadata(&resolved.path).map_err(|source| {
            ApiError::from(ProfileError::Io {
                path: resolved.path.clone(),
                source,
            })
        })?;
        if !metadata.file_type().is_file() {
            return Err(ApiError::new(
                "registry_corrupt",
                "Registered profile is not a regular file",
                serde_json::json!({"path":resolved.path.display().to_string()}),
                "Re-register the profile in this workspace.",
            ));
        }
        if metadata.len() > self.max_file_size {
            return Err(ApiError::new(
                "profile_too_large",
                "Registered profile exceeds configured maximum size",
                serde_json::json!({"byte_len":metadata.len(),"max_bytes":self.max_file_size}),
                "Raise --max-file-size-mib or re-register a smaller profile.",
            ));
        }
        let modified_unix_ms = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| u64::try_from(duration.as_millis()).ok());
        {
            let mut cache = self.entries.lock().await;
            if let Some(entry) = cache.get(&resolved.path)
                && entry.byte_len == resolved.byte_len
                && entry.modified_unix_ms == modified_unix_ms
            {
                return Ok(LoadedProfile {
                    alias: resolved.alias,
                    profile: entry.profile.clone(),
                });
            }
        }
        let parse_path = resolved.path.clone();
        let max_file_size = self.max_file_size;
        let profile = tokio::task::spawn_blocking(move || {
            ProfileBuilder::new(BuildLimits {
                max_file_bytes: max_file_size,
                ..BuildLimits::default()
            })
            .from_file(parse_path.clone(), metadata.len(), modified_unix_ms)
        })
        .await
        .map_err(|_| ApiError::internal("Profile parsing task failed"))?
        .map_err(ApiError::from)?;
        if profile.source.fingerprint != resolved.fingerprint {
            return Err(ApiError::new(
                "registry_corrupt",
                "Registered profile contents do not match its manifest fingerprint",
                serde_json::json!({"alias":resolved.alias}),
                "Re-register the profile in this workspace.",
            ));
        }
        let profile = Arc::new(profile);
        self.entries.lock().await.put(
            resolved.path,
            CacheEntry {
                byte_len: resolved.byte_len,
                modified_unix_ms,
                profile: profile.clone(),
            },
        );
        Ok(LoadedProfile {
            alias: resolved.alias,
            profile,
        })
    }
}
