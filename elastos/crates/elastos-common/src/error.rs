//! Error types for ElastOS

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ElastosError {
    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Compute error: {0}")]
    Compute(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Capsule not found: {0}")]
    CapsuleNotFound(String),

    #[error("Invalid manifest: {0}")]
    InvalidManifest(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ElastosError>;
