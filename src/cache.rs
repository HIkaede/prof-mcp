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
    max_cached_bytes: usize,
    entries: Arc<Mutex<LruCache<PathBuf, CacheEntry>>>,
    loading: Arc<Mutex<()>>,
}

#[derive(Clone)]
struct CacheEntry {
    byte_len: u64,
    modified_unix_ms: Option<u64>,
    estimated_bytes: usize,
    profile: Arc<Profile>,
}

impl ProfileCache {
    pub fn new(workspace: PathBuf, max_file_size: u64, capacity: usize) -> Result<Self, ApiError> {
        let capacity = NonZeroUsize::new(capacity).ok_or_else(|| {
            ApiError::new(
                "invalid_budget",
                "cache_capacity must be at least 1",
                serde_json::json!({"cache_capacity":capacity}),
                "Use a positive cache capacity.",
            )
        })?;
        Ok(Self {
            workspace,
            max_file_size,
            max_cached_bytes: 512 * 1024 * 1024,
            entries: Arc::new(Mutex::new(LruCache::new(capacity))),
            loading: Arc::new(Mutex::new(())),
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
        if let Some(profile) = self.cached(&resolved, modified_unix_ms).await {
            return Ok(profile);
        }
        let loading = self.loading.clone().lock_owned().await;
        if let Some(profile) = self.cached(&resolved, modified_unix_ms).await {
            return Ok(profile);
        }
        let entries = self.entries.clone();
        let max_file_size = self.max_file_size;
        let max_cached_bytes = self.max_cached_bytes;
        tokio::task::spawn_blocking(move || {
            // Keep the miss lock through publication, even if the request is cancelled.
            let _loading = loading;
            let profile = ProfileBuilder::new(BuildLimits {
                max_file_bytes: max_file_size,
                ..BuildLimits::default()
            })
            .from_file(resolved.path.clone(), metadata.len(), modified_unix_ms)
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
            let estimated_bytes = profile.estimated_size_bytes();
            {
                let mut entries = entries.blocking_lock();
                entries.pop(&resolved.path);
                if estimated_bytes <= max_cached_bytes {
                    let mut cached_bytes: usize =
                        entries.iter().map(|(_, entry)| entry.estimated_bytes).sum();
                    while cached_bytes > max_cached_bytes - estimated_bytes {
                        let (_, entry) = entries.pop_lru().expect("nonempty over-budget cache");
                        cached_bytes -= entry.estimated_bytes;
                    }
                    entries.put(
                        resolved.path,
                        CacheEntry {
                            byte_len: resolved.byte_len,
                            modified_unix_ms,
                            estimated_bytes,
                            profile: profile.clone(),
                        },
                    );
                }
            }
            Ok(LoadedProfile {
                alias: resolved.alias,
                profile,
            })
        })
        .await
        .map_err(|_| ApiError::internal("Profile parsing task failed"))?
    }

    async fn cached(
        &self,
        resolved: &registry::ResolvedProfile,
        modified_unix_ms: Option<u64>,
    ) -> Option<LoadedProfile> {
        self.entries
            .lock()
            .await
            .get(&resolved.path)
            .filter(|entry| {
                entry.byte_len == resolved.byte_len && entry.modified_unix_ms == modified_unix_ms
            })
            .map(|entry| LoadedProfile {
                alias: resolved.alias.clone(),
                profile: entry.profile.clone(),
            })
    }

    pub fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_budget() {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("input.folded");
        for (alias, bytes) in [("base", "root;one 1\n"), ("next", "root;two 1\n")] {
            std::fs::write(&source, bytes).unwrap();
            registry::register(workspace.path(), &source, Some(alias), 1024).unwrap();
        }
        let mut cache = ProfileCache::new(workspace.path().to_owned(), 1024, 8).unwrap();
        let base = cache.load(Some("base")).await.unwrap();
        cache.max_cached_bytes = base.profile.estimated_size_bytes();
        let next = cache.load(Some("next")).await.unwrap();
        {
            let entries = cache.entries.lock().await;
            assert_eq!(entries.len(), 1);
            assert!(
                entries
                    .iter()
                    .all(|(_, entry)| Arc::ptr_eq(&entry.profile, &next.profile))
            );
        }
        assert!(!Arc::ptr_eq(
            &base.profile,
            &cache.load(Some("base")).await.unwrap().profile
        ));
        cache.max_cached_bytes = 0;
        let first = cache.load(Some("next")).await.unwrap();
        let second = cache.load(Some("next")).await.unwrap();
        assert!(!Arc::ptr_eq(&first.profile, &second.profile));
        assert!(
            cache
                .entries
                .lock()
                .await
                .iter()
                .all(|(_, entry)| !Arc::ptr_eq(&entry.profile, &second.profile))
        );
    }

    #[tokio::test]
    async fn warm_load_skips_parser_lock() {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("input.folded");
        std::fs::write(&source, "root;leaf 1\n").unwrap();
        registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
        let cache = ProfileCache::new(workspace.path().to_owned(), 1024, 2).unwrap();
        let first = cache.load(None).await.unwrap();
        let _loading = cache.loading.lock().await;
        let loaded = tokio::time::timeout(std::time::Duration::from_secs(1), cache.load(None))
            .await
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&first.profile, &loaded.profile));
    }

    #[test]
    fn cancelled_load_keeps_parser_lock() {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("input.folded");
        std::fs::write(&source, "root;leaf 1\n").unwrap();
        registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
        let cache = ProfileCache::new(workspace.path().to_owned(), 1024, 2).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = wait.recv();
            });
            let loader = cache.clone();
            let request = tokio::spawn(async move { loader.load(None).await });
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while cache.loading.try_lock().is_ok() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            request.abort();
            assert!(request.await.unwrap_err().is_cancelled());
            assert!(cache.loading.try_lock().is_err());
            release.send(()).unwrap();
            let loaded = cache.load(None).await.unwrap();
            blocker.await.unwrap();
            assert_eq!(loaded.profile.total_weight, 1);
            assert!(Arc::ptr_eq(
                &loaded.profile,
                &cache.load(None).await.unwrap().profile
            ));
        });
    }
}
