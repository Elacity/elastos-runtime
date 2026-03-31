//! Compute provider trait

use async_trait::async_trait;
use std::path::Path;

use elastos_common::{CapsuleId, CapsuleManifest, CapsuleStatus, CapsuleType, Result};

/// Handle to a loaded/running capsule
#[derive(Debug, Clone)]
pub struct CapsuleHandle {
    pub id: CapsuleId,
    pub manifest: CapsuleManifest,
    pub args: Vec<String>,
}

/// Information about a running capsule
#[derive(Debug, Clone)]
pub struct CapsuleInfo {
    pub id: CapsuleId,
    pub name: String,
    pub status: CapsuleStatus,
    pub memory_used_mb: u32,
}

/// Abstract compute provider interface
#[async_trait]
pub trait ComputeProvider: Send + Sync {
    /// Load a capsule from a directory path
    async fn load(&self, path: &Path, manifest: CapsuleManifest) -> Result<CapsuleHandle>;

    /// Start a loaded capsule
    async fn start(&self, handle: &CapsuleHandle) -> Result<()>;

    /// Stop a running capsule
    async fn stop(&self, handle: &CapsuleHandle) -> Result<()>;

    /// Get capsule status
    async fn status(&self, handle: &CapsuleHandle) -> Result<CapsuleStatus>;

    /// Get capsule info
    async fn info(&self, handle: &CapsuleHandle) -> Result<CapsuleInfo>;

    /// Check if this provider supports the capsule type
    fn supports(&self, capsule_type: &CapsuleType) -> bool;
}
