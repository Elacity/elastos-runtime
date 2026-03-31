//! Mutable storage abstraction for ElastOS
//!
//! Provides a filesystem-like interface for mutable storage.
//! Unlike the content-addressed StorageProvider, this supports
//! in-place updates via paths.
//!
//! Each mutable rooted namespace (for example `localhost://Users/...` or
//! `elastos://namespace/...`) implements this trait as a provider capsule.

mod local;

pub use local::LocalMutableStorage;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Result type for mutable storage operations
pub type Result<T> = std::result::Result<T, StorageError>;

/// Errors that can occur during storage operations
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("Path not found: {0}")]
    NotFound(String),

    #[error("Path is a directory: {0}")]
    IsDirectory(String),

    #[error("Path is a file: {0}")]
    IsFile(String),

    #[error("Directory not empty: {0}")]
    DirectoryNotEmpty(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Storage error: {0}")]
    Other(String),
}

/// Type of directory entry
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    File,
    Directory,
}

/// A directory entry returned by list()
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    /// Entry name (not full path)
    pub name: String,

    /// Type of entry
    pub entry_type: EntryType,

    /// Size in bytes (0 for directories)
    pub size: u64,

    /// Last modified time (unix timestamp in seconds)
    pub modified: u64,
}

/// File or directory metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    /// Type of entry
    pub entry_type: EntryType,

    /// Size in bytes (0 for directories)
    pub size: u64,

    /// Last modified time (unix timestamp in seconds)
    pub modified: u64,

    /// Creation time (unix timestamp in seconds, if available)
    pub created: Option<u64>,
}

impl Metadata {
    /// Check if this is a file
    pub fn is_file(&self) -> bool {
        self.entry_type == EntryType::File
    }

    /// Check if this is a directory
    pub fn is_dir(&self) -> bool {
        self.entry_type == EntryType::Directory
    }
}

/// Convert SystemTime to unix timestamp
fn system_time_to_unix(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Mutable storage trait
///
/// Provides filesystem-like operations for mutable storage.
/// Implementations handle the actual storage backend (local fs, cloud, etc.)
#[async_trait]
pub trait MutableStorage: Send + Sync {
    /// Read file contents
    ///
    /// Returns the full file contents as bytes.
    /// Returns `NotFound` if path doesn't exist.
    /// Returns `IsDirectory` if path is a directory.
    async fn read(&self, path: &str) -> Result<Vec<u8>>;

    /// Write file contents
    ///
    /// Creates the file if it doesn't exist.
    /// Overwrites if it exists.
    /// Creates parent directories as needed.
    /// Returns `IsDirectory` if path is an existing directory.
    async fn write(&self, path: &str, data: &[u8]) -> Result<()>;

    /// Delete a file or empty directory
    ///
    /// Returns `NotFound` if path doesn't exist.
    /// Returns `DirectoryNotEmpty` if trying to delete non-empty directory.
    async fn delete(&self, path: &str) -> Result<()>;

    /// Delete a file or directory recursively
    ///
    /// If path is a directory, deletes all contents.
    /// Returns `NotFound` if path doesn't exist.
    async fn delete_recursive(&self, path: &str) -> Result<()>;

    /// List directory contents
    ///
    /// Returns entries in the directory (not recursive).
    /// Returns `NotFound` if path doesn't exist.
    /// Returns `IsFile` if path is a file.
    async fn list(&self, path: &str) -> Result<Vec<DirEntry>>;

    /// Check if path exists
    async fn exists(&self, path: &str) -> Result<bool>;

    /// Create a directory
    ///
    /// Creates parent directories as needed.
    /// No-op if directory already exists.
    /// Returns `IsFile` if path is an existing file.
    async fn mkdir(&self, path: &str) -> Result<()>;

    /// Get file or directory metadata
    ///
    /// Returns `NotFound` if path doesn't exist.
    async fn stat(&self, path: &str) -> Result<Metadata>;

    /// Copy a file
    ///
    /// Returns `NotFound` if source doesn't exist.
    /// Returns `IsDirectory` if source is a directory.
    async fn copy(&self, src: &str, dst: &str) -> Result<()> {
        let data = self.read(src).await?;
        self.write(dst, &data).await
    }

    /// Move/rename a file or directory
    ///
    /// Returns `NotFound` if source doesn't exist.
    async fn rename(&self, src: &str, dst: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_type_serialization() {
        assert_eq!(serde_json::to_string(&EntryType::File).unwrap(), "\"file\"");
        assert_eq!(
            serde_json::to_string(&EntryType::Directory).unwrap(),
            "\"directory\""
        );
    }

    #[test]
    fn test_dir_entry_serialization() {
        let entry = DirEntry {
            name: "test.txt".to_string(),
            entry_type: EntryType::File,
            size: 1024,
            modified: 1234567890,
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"name\":\"test.txt\""));
        assert!(json.contains("\"entry_type\":\"file\""));
    }

    #[test]
    fn test_metadata_helpers() {
        let file_meta = Metadata {
            entry_type: EntryType::File,
            size: 100,
            modified: 0,
            created: None,
        };
        assert!(file_meta.is_file());
        assert!(!file_meta.is_dir());

        let dir_meta = Metadata {
            entry_type: EntryType::Directory,
            size: 0,
            modified: 0,
            created: None,
        };
        assert!(!dir_meta.is_file());
        assert!(dir_meta.is_dir());
    }
}
