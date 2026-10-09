//! Durable byte persistence via file sync, atomic rename, and directory sync.

use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

use super::errors::registry_io;
use crate::error::ApiError;

pub(crate) fn atomic_write_bytes(destination: &Path, bytes: &[u8]) -> Result<(), ApiError> {
    atomic_copy(destination, bytes)
}

pub(crate) fn atomic_copy(destination: &Path, reader: impl Read) -> Result<(), ApiError> {
    atomic_copy_with_sync(destination, reader, |parent| {
        #[cfg(test)]
        tests::sync_result(destination)?;
        sync_directory(parent)
    })
}

fn atomic_copy_with_sync(
    destination: &Path,
    mut reader: impl Read,
    sync_parent: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), ApiError> {
    let parent = destination.parent().expect("registry file has parent");
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        destination
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("registry"),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| registry_io(destination, error))?;
    let result = (|| -> std::io::Result<()> {
        io::copy(&mut reader, &mut file)?;
        file.sync_all()?;
        fs::rename(&temp, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|error| registry_io(destination, error))?;
    sync_parent(parent).map_err(|error| {
        let mut error = registry_io(destination, error);
        // Rename already committed; callers must not delete its referenced blobs.
        error.details["committed"] = true.into();
        error.retry_hint = "The replacement is visible but durability is uncertain. Inspect the registry before retrying.".into();
        error
    })
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

pub(crate) fn files_equal(source: &mut fs::File, destination: &Path) -> io::Result<bool> {
    let mut existing = fs::File::open(destination)?;
    if source.metadata()?.len() != existing.metadata()?.len() {
        return Ok(false);
    }
    let mut left = [0; 8192];
    let mut right = [0; 8192];
    loop {
        let size = source.read(&mut left)?;
        if size == 0 {
            return Ok(true);
        }
        existing.read_exact(&mut right[..size])?;
        if left[..size] != right[..size] {
            return Ok(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry;
    use std::cell::RefCell;
    use std::path::PathBuf;

    thread_local! {
        static FAIL_SYNC: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    pub(super) fn sync_result(destination: &Path) -> io::Result<()> {
        FAIL_SYNC.with(|target| {
            let mut target = target.borrow_mut();
            if target.as_deref() == Some(destination) {
                target.take();
                return Err(io::Error::other("injected directory sync failure"));
            }
            Ok(())
        })
    }

    #[test]
    fn sync_failure() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("manifest.json");
        fs::write(&destination, b"old").unwrap();
        let error = atomic_copy_with_sync(&destination, &b"new"[..], |parent| {
            assert_eq!(parent, root.path());
            assert_eq!(fs::read(&destination).unwrap(), b"new");
            Err(io::Error::other("injected directory sync failure"))
        })
        .unwrap_err();
        assert_eq!(error.details["committed"], true);
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn manifest_sync() {
        let root = tempfile::tempdir().unwrap();
        registry::register_reader(
            root.path(),
            &b"root;old 1\n"[..],
            "old.folded",
            Some("old"),
            1024,
        )
        .unwrap();
        FAIL_SYNC.with(|target| {
            *target.borrow_mut() = Some(root.path().join(".prof-mcp/manifest.json"));
        });
        let error = registry::register_reader(
            root.path(),
            &b"root;new 2\n"[..],
            "new.folded",
            Some("new"),
            1024,
        )
        .unwrap_err();
        assert_eq!(error.details["committed"], true);
        let resolved = registry::resolve(root.path(), None).unwrap();
        assert_eq!(resolved.alias, "new");
        assert_eq!(fs::read(resolved.path).unwrap(), b"root;new 2\n");
        assert_eq!(
            registry::resolve(root.path(), Some("old")).unwrap().alias,
            "old"
        );
    }

    #[test]
    fn blob_retry() {
        let root = tempfile::tempdir().unwrap();
        registry::register_reader(
            root.path(),
            &b"root;old 1\n"[..],
            "old.folded",
            Some("old"),
            1024,
        )
        .unwrap();
        let bytes = b"root;new 2\n";
        let fingerprint = blake3::hash(bytes).to_hex();
        let blob = root
            .path()
            .join(format!(".prof-mcp/profiles/{fingerprint}.folded"));
        FAIL_SYNC.with(|target| *target.borrow_mut() = Some(blob.clone()));
        let error =
            registry::register_reader(root.path(), &bytes[..], "new.folded", Some("new"), 1024)
                .unwrap_err();
        assert_eq!(error.details["committed"], true);
        assert_eq!(registry::resolve(root.path(), None).unwrap().alias, "old");
        assert_eq!(fs::read(&blob).unwrap(), bytes);
        registry::register_reader(root.path(), &bytes[..], "new.folded", Some("new"), 1024)
            .unwrap();
        assert_eq!(registry::resolve(root.path(), None).unwrap().alias, "new");
    }

    #[test]
    fn resolve_missing_dir() {
        let root = tempfile::tempdir().unwrap();
        registry::register_reader(root.path(), &b"root;old 1\n"[..], "old.folded", None, 1024)
            .unwrap();
        let profiles = root.path().join(".prof-mcp/profiles");
        fs::remove_dir_all(&profiles).unwrap();
        assert_eq!(
            registry::resolve(root.path(), None).unwrap_err().code,
            "registry_corrupt"
        );
        assert!(!profiles.exists());
    }

    #[test]
    fn recover_failed_rename() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("blocked");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep"), b"original").unwrap();
        assert!(atomic_write_bytes(&destination, b"replacement").is_err());
        assert_eq!(fs::read(destination.join("keep")).unwrap(), b"original");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        fs::remove_dir_all(&destination).unwrap();
        atomic_write_bytes(&destination, b"replacement").unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"replacement");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_copy_preserves_destination() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("injected write input failure"))
            }
        }
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("profile");
        fs::write(&destination, b"original").unwrap();
        assert!(atomic_copy(&destination, (&b"partial"[..]).chain(Broken)).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"original");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
