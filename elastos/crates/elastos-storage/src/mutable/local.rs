//! Local filesystem implementation of MutableStorage
//!
//! Stores files in a base directory on the local filesystem.
//! Per-user isolation is handled by the caller (HTTP layer) by
//! providing different base paths for different users.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;

use super::{
    system_time_to_unix, DirEntry, EntryType, Metadata, MutableStorage, Result, StorageError,
};

/// Local filesystem storage
///
/// All paths are relative to the base_path.
/// Example: if base_path is `/home/user/.elastos/storage/user123/`
/// then path `config.json` maps to `/home/user/.elastos/storage/user123/config.json`
pub struct LocalMutableStorage {
    base_path: PathBuf,
}

impl LocalMutableStorage {
    /// Create a new local storage with the given base path
    ///
    /// Creates the base directory if it doesn't exist.
    pub async fn new(base_path: impl Into<PathBuf>) -> Result<Self> {
        let base_path = base_path.into();
        fs::create_dir_all(&base_path).await?;
        Ok(Self { base_path })
    }

    /// Create without ensuring the directory exists (for testing)
    pub fn new_unchecked(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    /// Get the base path
    pub fn base_path(&self) -> &Path {
        &self.base_path
    }

    /// Resolve a relative path to an absolute path
    ///
    /// Validates that the path doesn't escape the base directory.
    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        // Normalize the path - remove leading slashes and handle empty
        let path = path.trim_start_matches('/');
        if path.is_empty() {
            return Ok(self.base_path.clone());
        }

        // Check for path traversal attempts
        let normalized = Path::new(path);
        for component in normalized.components() {
            match component {
                std::path::Component::ParentDir => {
                    return Err(StorageError::InvalidPath(
                        "Path traversal not allowed".to_string(),
                    ));
                }
                std::path::Component::Normal(_) => {}
                _ => {
                    return Err(StorageError::InvalidPath(format!(
                        "Invalid path component in: {}",
                        path
                    )));
                }
            }
        }

        let full_path = self.base_path.join(path);

        // Double-check the resolved path is under base_path
        if !full_path.starts_with(&self.base_path) {
            return Err(StorageError::InvalidPath(
                "Path escapes base directory".to_string(),
            ));
        }

        // Resolve symlinks to detect escape via symlink.
        // Find the deepest existing ancestor (may be full_path itself) and
        // canonicalize it to resolve any symlinks in the chain.
        let mut check = full_path.as_path();
        while !check.exists() {
            match check.parent() {
                Some(p) => check = p,
                None => break,
            }
        }
        if check.exists() {
            let canonical = check
                .canonicalize()
                .map_err(|e| StorageError::InvalidPath(format!("Failed to resolve path: {}", e)))?;
            let canonical_base = self.base_path.canonicalize().map_err(|e| {
                StorageError::InvalidPath(format!("Failed to resolve base path: {}", e))
            })?;
            if !canonical.starts_with(&canonical_base) {
                return Err(StorageError::InvalidPath(
                    "Path escapes base directory via symlink".to_string(),
                ));
            }
        }

        Ok(full_path)
    }
}

#[async_trait]
impl MutableStorage for LocalMutableStorage {
    async fn read(&self, path: &str) -> Result<Vec<u8>> {
        let full_path = self.resolve_path(path)?;

        // Check if it's a directory
        if full_path.is_dir() {
            return Err(StorageError::IsDirectory(path.to_string()));
        }

        fs::read(&full_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound(path.to_string())
            } else {
                StorageError::Io(e)
            }
        })
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        let full_path = self.resolve_path(path)?;

        // Check if it's a directory
        if full_path.is_dir() {
            return Err(StorageError::IsDirectory(path.to_string()));
        }

        // Create parent directories if needed
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::write(&full_path, data).await?;
        Ok(())
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let full_path = self.resolve_path(path)?;

        let metadata = fs::metadata(&full_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound(path.to_string())
            } else {
                StorageError::Io(e)
            }
        })?;

        if metadata.is_dir() {
            // Try to remove as empty directory
            fs::remove_dir(&full_path).await.map_err(|e| {
                if e.kind() == std::io::ErrorKind::Other
                    || e.to_string().contains("not empty")
                    || e.to_string().contains("Directory not empty")
                {
                    StorageError::DirectoryNotEmpty(path.to_string())
                } else {
                    StorageError::Io(e)
                }
            })?;
        } else {
            fs::remove_file(&full_path).await?;
        }

        Ok(())
    }

    async fn delete_recursive(&self, path: &str) -> Result<()> {
        let full_path = self.resolve_path(path)?;

        let metadata = fs::metadata(&full_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound(path.to_string())
            } else {
                StorageError::Io(e)
            }
        })?;

        if metadata.is_dir() {
            fs::remove_dir_all(&full_path).await?;
        } else {
            fs::remove_file(&full_path).await?;
        }

        Ok(())
    }

    async fn list(&self, path: &str) -> Result<Vec<DirEntry>> {
        let full_path = self.resolve_path(path)?;

        // Check if path exists and is a directory
        let metadata = fs::metadata(&full_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound(path.to_string())
            } else {
                StorageError::Io(e)
            }
        })?;

        if !metadata.is_dir() {
            return Err(StorageError::IsFile(path.to_string()));
        }

        let mut entries = Vec::new();
        let mut read_dir = fs::read_dir(&full_path).await?;

        while let Some(entry) = read_dir.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry.metadata().await?;

            let entry_type = if metadata.is_dir() {
                EntryType::Directory
            } else {
                EntryType::File
            };

            let size = if metadata.is_file() {
                metadata.len()
            } else {
                0
            };

            let modified = metadata.modified().map(system_time_to_unix).unwrap_or(0);

            entries.push(DirEntry {
                name,
                entry_type,
                size,
                modified,
            });
        }

        // Sort by name for consistent ordering
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(entries)
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        let full_path = self.resolve_path(path)?;
        Ok(full_path.exists())
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        let full_path = self.resolve_path(path)?;

        // Check if it's an existing file
        if full_path.exists() && full_path.is_file() {
            return Err(StorageError::IsFile(path.to_string()));
        }

        fs::create_dir_all(&full_path).await?;
        Ok(())
    }

    async fn stat(&self, path: &str) -> Result<Metadata> {
        let full_path = self.resolve_path(path)?;

        let metadata = fs::metadata(&full_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound(path.to_string())
            } else {
                StorageError::Io(e)
            }
        })?;

        let entry_type = if metadata.is_dir() {
            EntryType::Directory
        } else {
            EntryType::File
        };

        let size = if metadata.is_file() {
            metadata.len()
        } else {
            0
        };

        let modified = metadata.modified().map(system_time_to_unix).unwrap_or(0);

        let created = metadata.created().ok().map(system_time_to_unix);

        Ok(Metadata {
            entry_type,
            size,
            modified,
            created,
        })
    }

    async fn rename(&self, src: &str, dst: &str) -> Result<()> {
        let src_path = self.resolve_path(src)?;
        let dst_path = self.resolve_path(dst)?;

        // Check source exists
        if !src_path.exists() {
            return Err(StorageError::NotFound(src.to_string()));
        }

        // Create parent directories for destination
        if let Some(parent) = dst_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::rename(&src_path, &dst_path).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_storage() -> (LocalMutableStorage, TempDir) {
        let dir = TempDir::new().unwrap();
        let storage = LocalMutableStorage::new(dir.path()).await.unwrap();
        (storage, dir)
    }

    #[tokio::test]
    async fn test_write_and_read() {
        let (storage, _dir) = create_test_storage().await;

        let data = b"hello world";
        storage.write("test.txt", data).await.unwrap();

        let read_data = storage.read("test.txt").await.unwrap();
        assert_eq!(read_data, data);
    }

    #[tokio::test]
    async fn test_write_creates_parents() {
        let (storage, _dir) = create_test_storage().await;

        storage
            .write("a/b/c/deep.txt", b"deep content")
            .await
            .unwrap();

        let data = storage.read("a/b/c/deep.txt").await.unwrap();
        assert_eq!(data, b"deep content");
    }

    #[tokio::test]
    async fn test_read_not_found() {
        let (storage, _dir) = create_test_storage().await;

        let result = storage.read("nonexistent.txt").await;
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_read_directory_error() {
        let (storage, _dir) = create_test_storage().await;

        storage.mkdir("mydir").await.unwrap();

        let result = storage.read("mydir").await;
        assert!(matches!(result, Err(StorageError::IsDirectory(_))));
    }

    #[tokio::test]
    async fn test_delete_file() {
        let (storage, _dir) = create_test_storage().await;

        storage.write("to_delete.txt", b"data").await.unwrap();
        assert!(storage.exists("to_delete.txt").await.unwrap());

        storage.delete("to_delete.txt").await.unwrap();
        assert!(!storage.exists("to_delete.txt").await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_empty_directory() {
        let (storage, _dir) = create_test_storage().await;

        storage.mkdir("empty_dir").await.unwrap();
        storage.delete("empty_dir").await.unwrap();
        assert!(!storage.exists("empty_dir").await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_non_empty_directory_fails() {
        let (storage, _dir) = create_test_storage().await;

        storage.write("dir/file.txt", b"content").await.unwrap();

        let result = storage.delete("dir").await;
        assert!(matches!(result, Err(StorageError::DirectoryNotEmpty(_))));
    }

    #[tokio::test]
    async fn test_delete_recursive() {
        let (storage, _dir) = create_test_storage().await;

        storage.write("dir/a.txt", b"a").await.unwrap();
        storage.write("dir/sub/b.txt", b"b").await.unwrap();

        storage.delete_recursive("dir").await.unwrap();
        assert!(!storage.exists("dir").await.unwrap());
    }

    #[tokio::test]
    async fn test_list() {
        let (storage, _dir) = create_test_storage().await;

        storage.write("dir/file1.txt", b"1").await.unwrap();
        storage.write("dir/file2.txt", b"22").await.unwrap();
        storage.mkdir("dir/subdir").await.unwrap();

        let entries = storage.list("dir").await.unwrap();
        assert_eq!(entries.len(), 3);

        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"file1.txt"));
        assert!(names.contains(&"file2.txt"));
        assert!(names.contains(&"subdir"));

        // Check types
        let file1 = entries.iter().find(|e| e.name == "file1.txt").unwrap();
        assert_eq!(file1.entry_type, EntryType::File);
        assert_eq!(file1.size, 1);

        let subdir = entries.iter().find(|e| e.name == "subdir").unwrap();
        assert_eq!(subdir.entry_type, EntryType::Directory);
    }

    #[tokio::test]
    async fn test_list_root() {
        let (storage, _dir) = create_test_storage().await;

        storage.write("root_file.txt", b"root").await.unwrap();
        storage.mkdir("root_dir").await.unwrap();

        // List root with empty path
        let entries = storage.list("").await.unwrap();
        assert_eq!(entries.len(), 2);

        // Also works with "/"
        let entries = storage.list("/").await.unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn test_exists() {
        let (storage, _dir) = create_test_storage().await;

        assert!(!storage.exists("nope.txt").await.unwrap());

        storage.write("yes.txt", b"yes").await.unwrap();
        assert!(storage.exists("yes.txt").await.unwrap());

        storage.mkdir("mydir").await.unwrap();
        assert!(storage.exists("mydir").await.unwrap());
    }

    #[tokio::test]
    async fn test_mkdir() {
        let (storage, _dir) = create_test_storage().await;

        storage.mkdir("a/b/c").await.unwrap();
        assert!(storage.exists("a/b/c").await.unwrap());

        let meta = storage.stat("a/b/c").await.unwrap();
        assert!(meta.is_dir());
    }

    #[tokio::test]
    async fn test_mkdir_on_file_fails() {
        let (storage, _dir) = create_test_storage().await;

        storage.write("file.txt", b"content").await.unwrap();

        let result = storage.mkdir("file.txt").await;
        assert!(matches!(result, Err(StorageError::IsFile(_))));
    }

    #[tokio::test]
    async fn test_stat() {
        let (storage, _dir) = create_test_storage().await;

        storage.write("file.txt", b"hello").await.unwrap();
        let meta = storage.stat("file.txt").await.unwrap();
        assert!(meta.is_file());
        assert_eq!(meta.size, 5);
        assert!(meta.modified > 0);

        storage.mkdir("dir").await.unwrap();
        let meta = storage.stat("dir").await.unwrap();
        assert!(meta.is_dir());
        assert_eq!(meta.size, 0);
    }

    #[tokio::test]
    async fn test_copy() {
        let (storage, _dir) = create_test_storage().await;

        storage.write("original.txt", b"original").await.unwrap();
        storage.copy("original.txt", "copy.txt").await.unwrap();

        let original = storage.read("original.txt").await.unwrap();
        let copy = storage.read("copy.txt").await.unwrap();
        assert_eq!(original, copy);
    }

    #[tokio::test]
    async fn test_rename() {
        let (storage, _dir) = create_test_storage().await;

        storage.write("old.txt", b"content").await.unwrap();
        storage.rename("old.txt", "new.txt").await.unwrap();

        assert!(!storage.exists("old.txt").await.unwrap());
        assert!(storage.exists("new.txt").await.unwrap());

        let data = storage.read("new.txt").await.unwrap();
        assert_eq!(data, b"content");
    }

    #[tokio::test]
    async fn test_path_traversal_blocked() {
        let (storage, _dir) = create_test_storage().await;

        // Try to escape with ..
        let result = storage.read("../etc/passwd").await;
        assert!(matches!(result, Err(StorageError::InvalidPath(_))));

        let result = storage.write("foo/../../etc/evil", b"bad").await;
        assert!(matches!(result, Err(StorageError::InvalidPath(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_symlink_path_traversal_blocked() {
        let (storage, dir) = create_test_storage().await;

        // Create a target file outside the storage base
        let outside_dir = TempDir::new().unwrap();
        let outside_file = outside_dir.path().join("secret.txt");
        std::fs::write(&outside_file, b"secret data").unwrap();

        // Create a symlink inside base_path pointing outside
        let symlink_path = dir.path().join("escape");
        std::os::unix::fs::symlink(outside_dir.path(), &symlink_path).unwrap();

        // Attempting to read through the symlink should fail
        let result = storage.read("escape/secret.txt").await;
        assert!(
            matches!(result, Err(StorageError::InvalidPath(_))),
            "Expected InvalidPath error for symlink escape, got: {:?}",
            result
        );

        // Writing through the symlink should also fail
        let result = storage.write("escape/new.txt", b"evil").await;
        assert!(
            matches!(result, Err(StorageError::InvalidPath(_))),
            "Expected InvalidPath error for symlink write, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_leading_slash_normalized() {
        let (storage, _dir) = create_test_storage().await;

        storage.write("/file.txt", b"data").await.unwrap();
        let data = storage.read("file.txt").await.unwrap();
        assert_eq!(data, b"data");

        let data = storage.read("/file.txt").await.unwrap();
        assert_eq!(data, b"data");
    }
}
