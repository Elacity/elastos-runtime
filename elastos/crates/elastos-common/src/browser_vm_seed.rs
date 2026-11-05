//! Process-verified seed identity carried in the Runtime launch request.

use std::{fs, io, path::Path};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    dev: u64,
    ino: u64,
    len: u64,
    mode: u32,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl FileIdentity {
    pub fn from_metadata(metadata: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            len: metadata.len(),
            mode: metadata.mode(),
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserVmSeedIdentity {
    pub link: FileIdentity,
    pub target: FileIdentity,
}

impl BrowserVmSeedIdentity {
    pub fn read(path: &Path) -> io::Result<Self> {
        let target = fs::metadata(path)?;
        if !target.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Browser rootfs seed must be a regular file",
            ));
        }
        Ok(Self {
            link: FileIdentity::from_metadata(&fs::symlink_metadata(path)?),
            target: FileIdentity::from_metadata(&target),
        })
    }

    pub fn check(&self, path: &Path) -> Result<(), String> {
        let current =
            Self::read(path).map_err(|error| format!("Browser rootfs seed identity: {error}"))?;
        if &current != self {
            return Err(
                "Browser rootfs seed changed after Runtime admission; retry Browser open".into(),
            );
        }
        Ok(())
    }
}
