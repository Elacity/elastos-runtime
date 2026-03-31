//! ElastOS Localhost Provider Capsule
//!
//! This provider capsule gives controlled access to localhost resources.
//! Storage operations require capability tokens and are scoped to allowed paths.
//! When an encryption key is provided, files are encrypted at rest with AES-256-GCM.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use elastos_guest::prelude::*;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;

use elastos_common::localhost::{is_plaintext_root, parse_localhost_path, parse_localhost_uri};

/// Provider protocol version
const PROTOCOL_VERSION: &str = "1.0";
/// Release version for startup logging
const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

/// Request from runtime to provider
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ProviderRequest {
    /// Initialize the provider
    Init { config: ProviderConfig },

    /// Read file contents
    Read {
        path: String,
        token: String,
        offset: Option<u64>,
        length: Option<u64>,
    },

    /// Write file contents
    Write {
        path: String,
        token: String,
        content: Vec<u8>,
        append: bool,
    },

    /// List directory contents
    List { path: String, token: String },

    /// Delete file or directory
    Delete {
        path: String,
        token: String,
        recursive: bool,
    },

    /// Get file/directory metadata
    Stat { path: String, token: String },

    /// Create directory
    Mkdir {
        path: String,
        token: String,
        parents: bool,
    },

    /// Check if path exists
    Exists { path: String, token: String },

    /// Shutdown the provider
    Shutdown,
}

/// Response from provider to runtime
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProviderResponse {
    /// Operation succeeded
    Ok { data: Option<serde_json::Value> },

    /// Operation failed
    Error { code: String, message: String },
}

/// Provider configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Base path for all operations (sandbox root)
    #[serde(default)]
    pub base_path: String,

    /// Allowed path prefixes (relative to base_path)
    #[serde(default)]
    pub allowed_paths: Vec<String>,

    /// Read-only mode
    #[serde(default)]
    pub read_only: bool,

    /// Hex-encoded AES-256 encryption key (empty = no encryption)
    #[serde(default)]
    pub encryption_key: String,
}

/// File metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStat {
    pub path: String,
    pub is_file: bool,
    pub is_dir: bool,
    pub size: u64,
    pub readonly: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<u64>,
}

/// Directory entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_file: bool,
    pub is_dir: bool,
    pub size: u64,
}

/// On-disk encrypted envelope (same format as identity crate)
#[derive(Serialize, Deserialize)]
struct EncryptedEnvelope {
    version: u8,
    nonce: String,
    ciphertext: String,
}

/// Encrypt raw bytes with AES-256-GCM, returning JSON envelope bytes.
fn encrypt_bytes(key: &[u8; 32], plaintext: &[u8]) -> io::Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| io::Error::other(format!("AES key init: {}", e)))?;
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| io::Error::other(format!("encryption: {}", e)))?;

    let envelope = EncryptedEnvelope {
        version: 1,
        nonce: hex::encode(nonce_bytes),
        ciphertext: hex::encode(ciphertext),
    };
    serde_json::to_vec(&envelope)
        .map_err(|e| io::Error::other(format!("envelope serialize: {}", e)))
}

/// Decrypt bytes from an EncryptedEnvelope.
fn decrypt_bytes(key: &[u8; 32], data: &[u8]) -> io::Result<Vec<u8>> {
    let envelope: EncryptedEnvelope = serde_json::from_slice(data).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("not an encrypted envelope: {}", e),
        )
    })?;

    let nonce_bytes = hex::decode(&envelope.nonce)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("bad nonce hex: {}", e)))?;
    let ct = hex::decode(&envelope.ciphertext).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad ciphertext hex: {}", e),
        )
    })?;

    if nonce_bytes.len() != 12 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid nonce length {}", nonce_bytes.len()),
        ));
    }

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| io::Error::other(format!("AES key init: {}", e)))?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    cipher
        .decrypt(nonce, ct.as_ref())
        .map_err(|e| io::Error::other(format!("decryption: {}", e)))
}

/// Local filesystem provider
pub struct LocalProvider {
    config: ProviderConfig,
    base_path: PathBuf,
    /// Parsed AES-256 encryption key (None = no encryption)
    encryption_key: Option<[u8; 32]>,
}

impl LocalProvider {
    fn is_plaintext_path(path: &str) -> bool {
        let trimmed = path.trim_start_matches('/');
        parse_localhost_path(trimmed)
            .map(|(root, _)| is_plaintext_root(root))
            .unwrap_or(false)
    }

    /// Create a new provider with configuration
    pub fn new(config: ProviderConfig) -> io::Result<Self> {
        let base_path = if config.base_path.is_empty() {
            std::env::current_dir()?
        } else {
            PathBuf::from(&config.base_path)
        };

        // Ensure base path exists and canonicalize for symlink-safe comparisons
        if !base_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Base path does not exist: {:?}", base_path),
            ));
        }
        let base_path = base_path.canonicalize().map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Failed to canonicalize base path: {}", e),
            )
        })?;

        // Parse encryption key from hex
        let encryption_key = if config.encryption_key.is_empty() {
            None
        } else {
            let bytes = hex::decode(&config.encryption_key).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("bad encryption_key hex: {}", e),
                )
            })?;
            if bytes.len() != 32 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "encryption_key has invalid length {} (expected 32)",
                        bytes.len()
                    ),
                ));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            Some(key)
        };

        Ok(Self {
            config,
            base_path,
            encryption_key,
        })
    }

    /// Resolve and validate a path (hardened against symlink traversal)
    fn resolve_path(&self, path: &str) -> io::Result<PathBuf> {
        // Normalize the path
        let normalized = if let Some((root, rest)) = parse_localhost_uri(path) {
            if rest.is_empty() {
                root.to_string()
            } else {
                format!("{}/{}", root, rest)
            }
        } else {
            path.trim_start_matches('/').to_string()
        };
        let path = normalized.as_str();
        if path.is_empty() {
            return Ok(self.base_path.clone());
        }

        // Reject explicit ".." components (defense in depth)
        for component in std::path::Path::new(path).components() {
            match component {
                std::path::Component::ParentDir => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Path traversal not allowed",
                    ));
                }
                std::path::Component::Normal(_) => {}
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("Invalid path component in: {}", path),
                    ));
                }
            }
        }

        let full_path = self.base_path.join(path);

        // Double-check the joined path is under base_path
        if !full_path.starts_with(&self.base_path) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Path escapes sandbox",
            ));
        }

        // Resolve symlinks: walk up to deepest existing ancestor, canonicalize,
        // and verify it stays within base_path. No silent fallback.
        let mut check = full_path.as_path();
        while !check.exists() {
            match check.parent() {
                Some(p) => check = p,
                None => break,
            }
        }
        if check.exists() {
            let canonical = check.canonicalize().map_err(|e| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("Failed to resolve path: {}", e),
                )
            })?;
            if !canonical.starts_with(&self.base_path) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Path escapes sandbox via symlink",
                ));
            }
        }

        // If full path exists, canonicalize it too and re-verify
        if full_path.exists() {
            let canonical = full_path.canonicalize().map_err(|e| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("Failed to resolve path: {}", e),
                )
            })?;
            if !canonical.starts_with(&self.base_path) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Path escapes sandbox via symlink",
                ));
            }
        }

        // Check against allowed paths if configured
        if !self.config.allowed_paths.is_empty() {
            let relative = full_path
                .strip_prefix(&self.base_path)
                .unwrap_or(full_path.as_path());
            let rel_str = relative.to_string_lossy();

            let allowed = self.config.allowed_paths.iter().any(|allowed| {
                if allowed == "*" {
                    return true;
                }
                rel_str.as_ref() == allowed || rel_str.starts_with(&format!("{}/", allowed))
            });

            if !allowed {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Path not in allowed list",
                ));
            }
        }

        Ok(full_path)
    }

    /// Validate a capability token (placeholder - runtime validates tokens)
    fn validate_token(&self, _token: &str, _action: &str, _path: &str) -> io::Result<()> {
        // In the full implementation, this would:
        // 1. Send a message to the runtime to validate the token
        // 2. Check the token grants the required action on the path
        // For Phase 3, we trust the runtime has already validated
        Ok(())
    }

    /// Read file contents
    pub fn read_file(
        &self,
        path: &str,
        token: &str,
        offset: Option<u64>,
        length: Option<u64>,
    ) -> io::Result<Vec<u8>> {
        self.validate_token(token, "read", path)?;
        let full_path = self.resolve_path(path)?;

        let metadata = fs::metadata(&full_path)?;
        if metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cannot read directory as file",
            ));
        }

        // With encryption: read entire file, decrypt, then apply offset/length
        if let Some(ref key) = self.encryption_key {
            if Self::is_plaintext_path(path) {
                let mut file = fs::File::open(&full_path)?;
                let file_size = metadata.len();

                if let Some(off) = offset {
                    use std::io::Seek;
                    file.seek(std::io::SeekFrom::Start(off))?;
                }

                let to_read = match length {
                    Some(len) => len as usize,
                    None => (file_size - offset.unwrap_or(0)) as usize,
                };

                let mut buffer = vec![0u8; to_read];
                let bytes_read = file.read(&mut buffer)?;
                buffer.truncate(bytes_read);
                return Ok(buffer);
            }

            let raw = fs::read(&full_path)?;
            let plaintext = decrypt_bytes(key, &raw)?;

            let off = offset.unwrap_or(0) as usize;
            if off >= plaintext.len() {
                return Ok(Vec::new());
            }
            let end = match length {
                Some(len) => (off + len as usize).min(plaintext.len()),
                None => plaintext.len(),
            };
            return Ok(plaintext[off..end].to_vec());
        }

        // No encryption: stream directly
        let mut file = fs::File::open(&full_path)?;
        let file_size = metadata.len();

        if let Some(off) = offset {
            use std::io::Seek;
            file.seek(std::io::SeekFrom::Start(off))?;
        }

        let to_read = match length {
            Some(len) => len as usize,
            None => (file_size - offset.unwrap_or(0)) as usize,
        };

        let mut buffer = vec![0u8; to_read];
        let bytes_read = file.read(&mut buffer)?;
        buffer.truncate(bytes_read);

        Ok(buffer)
    }

    /// Write file contents
    pub fn write_file(
        &self,
        path: &str,
        token: &str,
        content: &[u8],
        append: bool,
    ) -> io::Result<u64> {
        if self.config.read_only {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Provider is read-only",
            ));
        }

        self.validate_token(token, "write", path)?;
        let full_path = self.resolve_path(path)?;

        // Ensure parent directory exists
        if let Some(parent) = full_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }

        if let Some(ref key) = self.encryption_key {
            if Self::is_plaintext_path(path) {
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .append(append)
                    .truncate(!append)
                    .open(&full_path)?;

                file.write_all(content)?;
                file.flush()?;
                return Ok(content.len() as u64);
            }

            let plaintext = if append && full_path.exists() {
                // Read-decrypt-append-re-encrypt
                let raw = fs::read(&full_path)?;
                let mut existing = decrypt_bytes(key, &raw)?;
                existing.extend_from_slice(content);
                existing
            } else {
                content.to_vec()
            };

            let encrypted = encrypt_bytes(key, &plaintext)?;
            fs::write(&full_path, &encrypted)?;
            return Ok(content.len() as u64);
        }

        // No encryption: write directly
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .append(append)
            .truncate(!append)
            .open(&full_path)?;

        file.write_all(content)?;
        file.flush()?;

        Ok(content.len() as u64)
    }

    /// List directory contents
    pub fn list_dir(&self, path: &str, token: &str) -> io::Result<Vec<DirEntry>> {
        self.validate_token(token, "list", path)?;
        let full_path = self.resolve_path(path)?;

        if !full_path.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Path is not a directory",
            ));
        }

        let mut entries = Vec::new();
        for entry in fs::read_dir(&full_path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                is_file: metadata.is_file(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
            });
        }

        // Sort by name
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(entries)
    }

    /// Delete file or directory
    pub fn delete(&self, path: &str, token: &str, recursive: bool) -> io::Result<()> {
        if self.config.read_only {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Provider is read-only",
            ));
        }

        self.validate_token(token, "delete", path)?;
        let full_path = self.resolve_path(path)?;

        if full_path.is_dir() {
            if recursive {
                fs::remove_dir_all(&full_path)?;
            } else {
                fs::remove_dir(&full_path)?;
            }
        } else {
            fs::remove_file(&full_path)?;
        }

        Ok(())
    }

    /// Get file/directory metadata
    pub fn stat(&self, path: &str, token: &str) -> io::Result<FileStat> {
        self.validate_token(token, "stat", path)?;
        let full_path = self.resolve_path(path)?;

        let metadata = fs::metadata(&full_path)?;

        // Get timestamps if available
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        let created = metadata
            .created()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        Ok(FileStat {
            path: path.to_string(),
            is_file: metadata.is_file(),
            is_dir: metadata.is_dir(),
            size: metadata.len(),
            readonly: metadata.permissions().readonly(),
            modified,
            created,
        })
    }

    /// Create directory
    pub fn mkdir(&self, path: &str, token: &str, parents: bool) -> io::Result<()> {
        if self.config.read_only {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Provider is read-only",
            ));
        }

        self.validate_token(token, "write", path)?;
        let full_path = self.resolve_path(path)?;

        if parents {
            fs::create_dir_all(&full_path)?;
        } else {
            fs::create_dir(&full_path)?;
        }

        Ok(())
    }

    /// Check if path exists
    pub fn exists(&self, path: &str, token: &str) -> io::Result<bool> {
        self.validate_token(token, "stat", path)?;
        let full_path = self.resolve_path(path)?;
        Ok(full_path.exists())
    }

    /// Handle a request
    pub fn handle_request(&mut self, request: ProviderRequest) -> ProviderResponse {
        match request {
            ProviderRequest::Init { config } => match LocalProvider::new(config) {
                Ok(provider) => {
                    *self = provider;
                    ProviderResponse::Ok {
                        data: Some(serde_json::json!({
                            "protocol_version": PROTOCOL_VERSION,
                            "provider": "localhost",
                        })),
                    }
                }
                Err(e) => ProviderResponse::Error {
                    code: "init_failed".to_string(),
                    message: e.to_string(),
                },
            },

            ProviderRequest::Read {
                path,
                token,
                offset,
                length,
            } => match self.read_file(&path, &token, offset, length) {
                Ok(data) => ProviderResponse::Ok {
                    data: Some(serde_json::json!({
                        "content": data,
                        "size": data.len(),
                    })),
                },
                Err(e) => ProviderResponse::Error {
                    code: "read_failed".to_string(),
                    message: e.to_string(),
                },
            },

            ProviderRequest::Write {
                path,
                token,
                content,
                append,
            } => match self.write_file(&path, &token, &content, append) {
                Ok(written) => ProviderResponse::Ok {
                    data: Some(serde_json::json!({
                        "bytes_written": written,
                    })),
                },
                Err(e) => ProviderResponse::Error {
                    code: "write_failed".to_string(),
                    message: e.to_string(),
                },
            },

            ProviderRequest::List { path, token } => match self.list_dir(&path, &token) {
                Ok(entries) => ProviderResponse::Ok {
                    data: Some(serde_json::to_value(entries).unwrap()),
                },
                Err(e) => ProviderResponse::Error {
                    code: "list_failed".to_string(),
                    message: e.to_string(),
                },
            },

            ProviderRequest::Delete {
                path,
                token,
                recursive,
            } => match self.delete(&path, &token, recursive) {
                Ok(()) => ProviderResponse::Ok { data: None },
                Err(e) => ProviderResponse::Error {
                    code: "delete_failed".to_string(),
                    message: e.to_string(),
                },
            },

            ProviderRequest::Stat { path, token } => match self.stat(&path, &token) {
                Ok(stat) => ProviderResponse::Ok {
                    data: Some(serde_json::to_value(stat).unwrap()),
                },
                Err(e) => ProviderResponse::Error {
                    code: "stat_failed".to_string(),
                    message: e.to_string(),
                },
            },

            ProviderRequest::Mkdir {
                path,
                token,
                parents,
            } => match self.mkdir(&path, &token, parents) {
                Ok(()) => ProviderResponse::Ok { data: None },
                Err(e) => ProviderResponse::Error {
                    code: "mkdir_failed".to_string(),
                    message: e.to_string(),
                },
            },

            ProviderRequest::Exists { path, token } => match self.exists(&path, &token) {
                Ok(exists) => ProviderResponse::Ok {
                    data: Some(serde_json::json!({ "exists": exists })),
                },
                Err(e) => ProviderResponse::Error {
                    code: "exists_failed".to_string(),
                    message: e.to_string(),
                },
            },

            ProviderRequest::Shutdown => ProviderResponse::Ok {
                data: Some(serde_json::json!({ "message": "Provider shutting down" })),
            },
        }
    }
}

fn main() {
    // Print startup message
    eprintln!("localhost-provider: starting v{}", PROVIDER_VERSION);

    // Get capsule info from environment
    let info = CapsuleInfo::from_env();
    if info.is_elastos_runtime() {
        eprintln!("Running as: {} ({})", info.name(), info.id());
    } else {
        eprintln!("Running in standalone mode");
    }

    // Create provider with default config (will be initialized via Init request)
    let mut provider = LocalProvider {
        config: ProviderConfig::default(),
        base_path: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        encryption_key: None,
    };

    // Message loop - read JSON requests from stdin, write responses to stdout
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                break;
            }
        };

        if line.is_empty() {
            continue;
        }

        // Parse request
        let request: ProviderRequest = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                let response = ProviderResponse::Error {
                    code: "parse_error".to_string(),
                    message: e.to_string(),
                };
                let json = serde_json::to_string(&response).unwrap();
                writeln!(stdout, "{}", json).unwrap();
                stdout.flush().unwrap();
                continue;
            }
        };

        // Check for shutdown
        let is_shutdown = matches!(request, ProviderRequest::Shutdown);

        // Handle request
        let response = provider.handle_request(request);

        // Send response
        let json = serde_json::to_string(&response).unwrap();
        writeln!(stdout, "{}", json).unwrap();
        stdout.flush().unwrap();

        // Exit on shutdown
        if is_shutdown {
            break;
        }
    }

    eprintln!("Localhost provider exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_test_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("test.txt"), b"hello world").unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();
        fs::write(dir.path().join("subdir/nested.txt"), b"nested content").unwrap();
        dir
    }

    fn test_key() -> [u8; 32] {
        // Deterministic test key
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8;
        }
        key
    }

    fn make_encrypted_provider(dir: &tempfile::TempDir) -> LocalProvider {
        let key = test_key();
        LocalProvider::new(ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: hex::encode(key),
        })
        .unwrap()
    }

    #[test]
    fn test_read_file() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();
        let content = provider.read_file("test.txt", "token", None, None).unwrap();
        assert_eq!(content, b"hello world");
    }

    #[test]
    fn test_read_file_accepts_rooted_localhost_uri() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["Users".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };
        fs::create_dir_all(dir.path().join("Users/self/Documents")).unwrap();
        fs::write(
            dir.path().join("Users/self/Documents/readme.txt"),
            b"hello rooted world",
        )
        .unwrap();

        let provider = LocalProvider::new(config).unwrap();
        let content = provider
            .read_file(
                "localhost://Users/self/Documents/readme.txt",
                "token",
                None,
                None,
            )
            .unwrap();
        assert_eq!(content, b"hello rooted world");
    }

    #[test]
    fn test_read_file_with_offset() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();
        let content = provider
            .read_file("test.txt", "token", Some(6), Some(5))
            .unwrap();
        assert_eq!(content, b"world");
    }

    #[test]
    fn test_write_file() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();
        provider
            .write_file("new.txt", "token", b"new content", false)
            .unwrap();

        let content = fs::read_to_string(dir.path().join("new.txt")).unwrap();
        assert_eq!(content, "new content");
    }

    #[test]
    fn test_write_file_append() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();
        provider
            .write_file("test.txt", "token", b" appended", true)
            .unwrap();

        let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "hello world appended");
    }

    #[test]
    fn test_read_only_mode() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: true,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();
        let result = provider.write_file("test.txt", "token", b"data", false);
        assert!(result.is_err());
    }

    #[test]
    fn test_list_dir() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();
        let entries = provider.list_dir("", "token").unwrap();

        assert_eq!(entries.len(), 2);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"test.txt"));
        assert!(names.contains(&"subdir"));
    }

    #[test]
    fn test_stat() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();
        let stat = provider.stat("test.txt", "token").unwrap();

        assert!(stat.is_file);
        assert!(!stat.is_dir);
        assert_eq!(stat.size, 11); // "hello world"
    }

    #[test]
    fn test_mkdir() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();
        provider.mkdir("newdir/nested", "token", true).unwrap();

        assert!(dir.path().join("newdir/nested").is_dir());
    }

    #[test]
    fn test_delete() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();
        provider.delete("test.txt", "token", false).unwrap();

        assert!(!dir.path().join("test.txt").exists());
    }

    #[test]
    fn test_path_escape_prevention() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();
        let result = provider.read_file("../../../etc/passwd", "token", None, None);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn test_path_escape_intermediate_dotdot() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();
        // foo/../../etc/passwd has ".." components that should be rejected
        let result = provider.read_file("foo/../../etc/passwd", "token", None, None);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_traversal_blocked() {
        let dir = setup_test_dir();

        // Create a target file outside the storage base
        let outside_dir = tempfile::tempdir().unwrap();
        let outside_file = outside_dir.path().join("secret.txt");
        fs::write(&outside_file, b"secret data").unwrap();

        // Create a symlink inside base_path pointing outside
        let symlink_path = dir.path().join("escape");
        std::os::unix::fs::symlink(outside_dir.path(), &symlink_path).unwrap();

        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();

        // Read through symlink should fail
        let result = provider.read_file("escape/secret.txt", "token", None, None);
        assert_eq!(
            result.unwrap_err().kind(),
            io::ErrorKind::PermissionDenied,
            "symlink read should be blocked"
        );

        // Write through symlink should also fail
        let result = provider.write_file("escape/new.txt", "token", b"evil", false);
        assert_eq!(
            result.unwrap_err().kind(),
            io::ErrorKind::PermissionDenied,
            "symlink write should be blocked"
        );
    }

    #[test]
    fn test_exists() {
        let dir = setup_test_dir();
        let config = ProviderConfig {
            base_path: dir.path().to_string_lossy().to_string(),
            allowed_paths: vec!["*".to_string()],
            read_only: false,
            encryption_key: String::new(),
        };

        let provider = LocalProvider::new(config).unwrap();
        assert!(provider.exists("test.txt", "token").unwrap());
        assert!(!provider.exists("nonexistent.txt", "token").unwrap());
    }

    // === Encryption tests ===

    #[test]
    fn test_encrypted_read_write() {
        let dir = setup_test_dir();
        let provider = make_encrypted_provider(&dir);

        provider
            .write_file("secret.txt", "token", b"sensitive data", false)
            .unwrap();

        // Raw file should NOT contain plaintext
        let raw = fs::read_to_string(dir.path().join("secret.txt")).unwrap();
        assert!(
            !raw.contains("sensitive data"),
            "raw file should be encrypted"
        );
        assert!(raw.contains("ciphertext"), "raw file should be an envelope");

        // Read back through provider should return plaintext
        let content = provider
            .read_file("secret.txt", "token", None, None)
            .unwrap();
        assert_eq!(content, b"sensitive data");
    }

    #[test]
    fn test_encrypted_read_with_offset() {
        let dir = setup_test_dir();
        let provider = make_encrypted_provider(&dir);

        provider
            .write_file("offset.txt", "token", b"hello world", false)
            .unwrap();

        let content = provider
            .read_file("offset.txt", "token", Some(6), Some(5))
            .unwrap();
        assert_eq!(content, b"world");
    }

    #[test]
    fn test_encrypted_append() {
        let dir = setup_test_dir();
        let provider = make_encrypted_provider(&dir);

        provider
            .write_file("append.txt", "token", b"hello", false)
            .unwrap();
        provider
            .write_file("append.txt", "token", b" world", true)
            .unwrap();

        let content = provider
            .read_file("append.txt", "token", None, None)
            .unwrap();
        assert_eq!(content, b"hello world");
    }

    #[test]
    fn test_public_paths_stay_plaintext_with_encryption_enabled() {
        let dir = setup_test_dir();
        let provider = make_encrypted_provider(&dir);

        provider
            .write_file(
                "Public/elastos/index.html",
                "token",
                b"<h1>Hello</h1>",
                false,
            )
            .unwrap();

        let raw = fs::read(dir.path().join("Public/elastos/index.html")).unwrap();
        assert_eq!(raw, b"<h1>Hello</h1>");

        let content = provider
            .read_file("Public/elastos/index.html", "token", None, None)
            .unwrap();
        assert_eq!(content, b"<h1>Hello</h1>");
    }
}
