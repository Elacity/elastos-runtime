//! Publisher admission of an independently pinned, already signed byte set.
//! Candidate files are data: this module runs neither candidates nor signers.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use anyhow::{ensure, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_METADATA: u64 = 256 * 1024;
const MAX_COMPONENTS: u64 = 2 * 1024 * 1024;
const MAX_FILES: usize = 512;
const METADATA: [&str; 3] = ["release-head.json", "release.json", "install.sh"];

#[cfg(test)]
thread_local! {
    static STREAM_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactDescriptor {
    pub name: String,
    pub cid: String,
    pub sha256: String,
    pub size: u64,
}

struct Artifact {
    descriptor: ArtifactDescriptor,
    file: File,
    admitted: StatStamp,
}

struct AdmittedMetadata {
    file: File,
    admitted: StatStamp,
}

/// Open files retain the admitted directory and inode identities. Artifact
/// reads and snapshots also verify the signed bytes before publication/use.
pub struct Publication {
    root_path: PathBuf,
    _root: File,
    _artifact_directory: File,
    root_stamp: DirectoryStamp,
    artifact_directory_stamp: DirectoryStamp,
    metadata_files: [AdmittedMetadata; 3],
    published: bool,
    version: String,
    channel: String,
    release_cid: String,
    head: Vec<u8>,
    release: Vec<u8>,
    installer: Vec<u8>,
    descriptors: Vec<ArtifactDescriptor>,
    artifacts: BTreeMap<String, Artifact>,
}

impl Publication {
    pub fn open_flat(input: &Path, pinned_did: &str) -> Result<Self> {
        Self::open(input, pinned_did, false)
    }

    pub fn open_published(root: &Path, pinned_did: &str) -> Result<Self> {
        Self::open(root, pinned_did, true)
    }

    fn open(path: &Path, pinned_did: &str, published: bool) -> Result<Self> {
        let key = crate::crypto::decode_did_key(pinned_did)?;
        ensure!(
            crate::crypto::encode_did_key(&key)? == pinned_did,
            "noncanonical publisher pin"
        );
        let root = directory(path)?;
        let root_stamp = directory_stamp(&root)?;
        let artifact_directory = if published {
            open_at(&root, "artifacts", libc::O_RDONLY | libc::O_DIRECTORY)?
        } else {
            root.try_clone()?
        };
        let artifact_directory_stamp = directory_stamp(&artifact_directory)?;
        let (head_file, head) = admitted_metadata(&root, METADATA[0], MAX_METADATA)?;
        let (release_file, release) = admitted_metadata(&root, METADATA[1], MAX_METADATA)?;
        let (installer_file, installer) = admitted_metadata(&root, METADATA[2], MAX_COMPONENTS)?;
        let head_envelope =
            crate::crypto::verify_release_envelope(&head, "elastos.release.head.v1", pinned_did)?;
        let release_envelope =
            crate::crypto::verify_release_envelope(&release, "elastos.release.v1", pinned_did)?;
        let h = &head_envelope["payload"];
        let r = &release_envelope["payload"];
        integer_json(h)?;
        integer_json(r)?;
        ensure!(
            text(h, "schema")? == "elastos.release.head/v1"
                && text(r, "schema")? == "elastos.release/v1",
            "release schema refused"
        );
        let version = text(r, "version")?.to_owned();
        semver::Version::parse(&version).context("release version refused")?;
        let channel = text(r, "channel")?.to_owned();
        ensure!(
            matches!(channel.as_str(), "stable" | "canary" | "jetson-test"),
            "release channel refused"
        );
        ensure!(
            text(h, "version")? == version
                && text(h, "channel")? == channel
                && text(h, "signer_did")? == pinned_did,
            "head identity differs from release/pin"
        );
        let source = r
            .get("source")
            .and_then(Value::as_object)
            .context("release source required")?;
        ensure!(source.len() == 2, "release source fields refused");
        for field in ["commit", "tree"] {
            let oid = source
                .get(field)
                .and_then(Value::as_str)
                .context("source OID required")?;
            checked_hex(oid, 40)?;
        }
        if let Some(head_source) = h.get("source") {
            ensure!(head_source == &r["source"], "head source differs");
        }
        checked_time(r, "released_at")?;
        checked_time(h, "updated_at")?;
        previous_cid(r, "prev_release_cid")?;
        previous_cid(h, "prev_head_cid")?;
        ensure!(
            checked_hash(text(h, "release_sha256")?)? == digest(&release),
            "head release hash differs"
        );
        ensure!(
            checked_hash(text(r, "installer_sha256")?)? == digest(&installer),
            "installer hash differs"
        );
        let release_cid = text(h, "latest_release_cid")?.to_owned();
        checked_cid(&release_cid)?;
        crate::update::verify_release_metadata_cid(&release_cid, &release)?;
        let names = directory_names(&artifact_directory)?;
        let mut artifacts = BTreeMap::new();
        let platforms = r
            .get("platforms")
            .and_then(Value::as_object)
            .context("release platforms required")?;
        ensure!(
            !platforms.is_empty() && platforms.len() <= 3,
            "release platforms refused"
        );
        for (platform, refs) in platforms {
            ensure!(
                matches!(
                    platform.as_str(),
                    "x86_64-linux" | "aarch64-linux" | "aarch64-darwin"
                ),
                "unsupported release platform"
            );
            let refs = refs.as_object().context("platform descriptors required")?;
            ensure!(
                refs.len() == 2 && refs.contains_key("binary") && refs.contains_key("components"),
                "platform descriptor fields refused"
            );
            for kind in ["binary", "components"] {
                let name = if kind == "binary" {
                    format!("elastos-{platform}")
                } else {
                    format!("components-{platform}.json")
                };
                admit_descriptor(
                    &artifact_directory,
                    &names,
                    &mut artifacts,
                    &refs[kind],
                    Some(&name),
                )?;
            }
        }
        let mut visited = BTreeSet::new();
        loop {
            let next = artifacts
                .keys()
                .find(|name| name.ends_with(".json") && !visited.contains(*name))
                .cloned();
            let Some(name) = next else { break };
            visited.insert(name.clone());
            let bytes = read_artifact(&artifacts[&name], Some(MAX_COMPONENTS))?;
            let component: Value =
                serde_json::from_slice(&bytes).context("artifact JSON refused")?;
            integer_json(&component)?;
            if name.starts_with("components-") {
                ensure!(
                    text(&component, "schema")? == "elastos.components/v1",
                    "components schema refused"
                );
            }
            if component.get("schema").and_then(Value::as_str) != Some("elastos.components/v1") {
                continue;
            }
            if let Some(platform) = name
                .strip_prefix("components-")
                .and_then(|name| name.strip_suffix(".json"))
            {
                let manifest = serde_json::from_value(component.clone())?;
                crate::setup::admit_release_components(&manifest, platform)?;
            }
            component_refs(&component, &artifact_directory, &names, &mut artifacts)?;
            if let Some(catalog) = component.get("model_catalog") {
                let cid = text(catalog, "head_cid")?;
                ensure!(
                    checked_cid(cid)?.codec() == 0x55,
                    "catalog head must be raw CID"
                );
                let file = regular(&artifact_directory, "model-catalog.json", MAX_COMPONENTS)?;
                let bytes = bounded_bytes(&file, MAX_COMPONENTS)?;
                crate::update::verify_release_metadata_cid(cid, &bytes)?;
                let descriptor =
                    serde_json::json!({"cid":cid,"sha256":digest(&bytes),"size":bytes.len()});
                admit_descriptor(
                    &artifact_directory,
                    &names,
                    &mut artifacts,
                    &descriptor,
                    Some("model-catalog.json"),
                )?;
            }
        }
        ensure!(
            !artifacts.is_empty() && artifacts.len() <= MAX_FILES,
            "artifact count refused"
        );
        if !published {
            let expected: BTreeSet<_> = artifacts
                .keys()
                .cloned()
                .chain(METADATA.iter().map(|s| (*s).to_owned()))
                .collect();
            ensure!(names == expected, "unadvertised flat publication file");
        }
        let descriptors = artifacts.values().map(|a| a.descriptor.clone()).collect();
        let publication = Self {
            root_path: path.to_owned(),
            _root: root,
            _artifact_directory: artifact_directory,
            root_stamp,
            artifact_directory_stamp,
            metadata_files: [head_file, release_file, installer_file],
            published,
            version,
            channel,
            release_cid,
            head,
            release,
            installer,
            descriptors,
            artifacts,
        };
        publication.unchanged()?;
        if !published {
            ensure!(
                directory_names(&publication._artifact_directory)? == names,
                "flat publication namespace changed"
            );
        }
        Ok(publication)
    }

    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn channel(&self) -> &str {
        &self.channel
    }
    pub fn release_cid(&self) -> &str {
        &self.release_cid
    }
    pub fn head_bytes(&self) -> &[u8] {
        &self.head
    }
    pub fn release_bytes(&self) -> &[u8] {
        &self.release
    }
    pub fn installer_bytes(&self) -> &[u8] {
        &self.installer
    }
    pub fn artifacts(&self) -> &[ArtifactDescriptor] {
        &self.descriptors
    }

    /// Cached publication bytes belong to this admitted immutable generation.
    /// This check observes file identities and metadata without reading content.
    pub fn unchanged_published(&self) -> Result<()> {
        ensure!(self.published, "published layout required");
        self.unchanged()
    }

    fn held_unchanged(&self) -> Result<()> {
        ensure!(
            directory_stamp(&self._root)? == self.root_stamp
                && directory_stamp(&self._artifact_directory)? == self.artifact_directory_stamp,
            "publication directory changed"
        );
        for metadata in &self.metadata_files {
            ensure!(
                stamp(&metadata.file)? == metadata.admitted,
                "publication metadata changed"
            );
        }
        for artifact in self.artifacts.values() {
            ensure!(
                stamp(&artifact.file)? == artifact.admitted,
                "publication artifact changed"
            );
        }
        Ok(())
    }

    fn unchanged(&self) -> Result<()> {
        self.held_unchanged()?;
        let root = directory(&self.root_path)?;
        ensure!(
            directory_stamp(&root)? == self.root_stamp,
            "publication root changed"
        );
        let artifacts = if self.published {
            open_at(&root, "artifacts", libc::O_RDONLY | libc::O_DIRECTORY)?
        } else {
            root.try_clone()?
        };
        ensure!(
            directory_stamp(&artifacts)? == self.artifact_directory_stamp,
            "publication artifact directory changed"
        );
        for (name, metadata) in METADATA.iter().zip(&self.metadata_files) {
            ensure!(
                stamp(&regular(&root, name, metadata.admitted.size)?)? == metadata.admitted,
                "publication metadata path changed: {name}"
            );
        }
        for (name, artifact) in &self.artifacts {
            ensure!(
                stamp(&regular(&artifacts, name, artifact.descriptor.size)?)? == artifact.admitted,
                "publication artifact path changed: {name}"
            );
        }
        self.held_unchanged()?;
        let root = directory(&self.root_path)?;
        ensure!(
            directory_stamp(&root)? == self.root_stamp,
            "publication root changed during check"
        );
        if self.published {
            ensure!(
                directory_stamp(&open_at(
                    &root,
                    "artifacts",
                    libc::O_RDONLY | libc::O_DIRECTORY
                )?)? == self.artifact_directory_stamp,
                "publication artifact directory changed during check"
            );
        }
        Ok(())
    }

    pub fn read_verified_artifact(&self, name: &str) -> Result<Vec<u8>> {
        safe_name(name)?;
        read_artifact(
            self.artifacts
                .get(name)
                .context("artifact outside current signed set")?,
            None,
        )
    }

    /// Apply the caller's memory budget before reserving or reading bytes.
    /// The held descriptor and signed hash still govern an admitted read.
    pub fn read_verified_artifact_bounded(&self, name: &str, max_bytes: u64) -> Result<Vec<u8>> {
        safe_name(name)?;
        read_artifact(
            self.artifacts
                .get(name)
                .context("artifact outside current signed set")?,
            Some(max_bytes),
        )
    }

    /// Create a new private snapshot outside the admitted root. Callers promote
    /// only this successful result; failure removes this method's partial set.
    pub fn snapshot_into(&self, output: &Path) -> Result<()> {
        ensure!(
            output.is_absolute() && !output.starts_with(&self.root_path),
            "snapshot must be outside input root"
        );
        let parent_path = output.parent().context("snapshot parent required")?;
        let name = output
            .file_name()
            .and_then(|n| n.to_str())
            .context("snapshot name required")?;
        safe_name(name)?;
        let parent = directory(parent_path)?;
        let meta = parent.metadata()?;
        ensure!(
            meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o022 == 0,
            "snapshot parent must be operator-owned and protected"
        );
        // Also reject aliases of the input root reached through directory moves.
        reject_ancestor(&parent, &self._root)?;
        let total = self.artifacts.values().try_fold(
            (self.head.len() + self.release.len() + self.installer.len()) as u64,
            |total, a| {
                total
                    .checked_add(a.descriptor.size)
                    .context("snapshot size overflow")
            },
        )?;
        available_space(&parent, total, self.artifacts.len() + 3)?;
        mkdir_at(&parent, name)?;
        let target = open_at(&parent, name, libc::O_RDONLY | libc::O_DIRECTORY)?;
        let mut copied = Vec::new();
        let mut artifact_target = None;
        let result = (|| -> Result<()> {
            mkdir_at(&target, "artifacts")?;
            let dir = open_at(&target, "artifacts", libc::O_RDONLY | libc::O_DIRECTORY)?;
            artifact_target = Some(dir);
            let dir = artifact_target.as_ref().unwrap();
            for (name, artifact) in &self.artifacts {
                let mut out = create_at(dir, name)?;
                copied.push(name.clone());
                stream_verified(artifact, |bytes| {
                    out.write_all(bytes)?;
                    Ok(())
                })?;
                seal(&out)?;
            }
            for (name, bytes) in METADATA
                .iter()
                .zip([&self.head, &self.release, &self.installer])
            {
                let mut out = create_at(&target, name)?;
                out.write_all(bytes)?;
                seal(&out)?;
            }
            dir.sync_all()?;
            target.sync_all()?;
            parent.sync_all()?;
            let visible = directory(output)?;
            let expected = target.metadata()?;
            let actual = visible.metadata()?;
            ensure!(
                (expected.dev(), expected.ino()) == (actual.dev(), actual.ino()),
                "snapshot destination changed"
            );
            Ok(())
        })();
        if result.is_err() {
            if let Some(dir) = artifact_target {
                for name in copied {
                    let _ = unlink_at(&dir, &name, false);
                }
            }
            for name in METADATA {
                let _ = unlink_at(&target, name, false);
            }
            let _ = unlink_at(&target, "artifacts", true);
            let _ = unlink_at(&parent, name, true);
        }
        result
    }
}

fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value
        .get(name)
        .and_then(Value::as_str)
        .with_context(|| format!("{name} string required"))
}
fn integer_json(value: &Value) -> Result<()> {
    match value {
        Value::Number(number) => ensure!(
            number.is_i64() || number.is_u64(),
            "noninteger JSON refused"
        ),
        Value::Array(array) => {
            for child in array {
                integer_json(child)?;
            }
        }
        Value::Object(object) => {
            for child in object.values() {
                integer_json(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn checked_time(value: &Value, name: &str) -> Result<()> {
    ensure!(
        value.get(name).and_then(Value::as_u64).is_some(),
        "{name} integer required"
    );
    Ok(())
}
fn previous_cid(value: &Value, name: &str) -> Result<()> {
    let v = value
        .get(name)
        .with_context(|| format!("{name} required"))?;
    if !v.is_null() {
        checked_cid(v.as_str().context("previous CID type refused")?)?;
    }
    Ok(())
}
fn checked_hex(value: &str, len: usize) -> Result<()> {
    ensure!(
        value.len() == len
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "lowercase hex identity required"
    );
    Ok(())
}
fn checked_hash(value: &str) -> Result<&str> {
    checked_hex(value, 64)?;
    Ok(value)
}
fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn checked_cid(value: &str) -> Result<cid::Cid> {
    let cid = cid::Cid::try_from(value).context("malformed artifact CID")?;
    ensure!(
        matches!(cid.codec(), 0x55 | 0x70)
            && cid.hash().code() == 0x12
            && cid.hash().digest().len() == 32
            && cid.to_string() == value,
        "unsupported/noncanonical artifact CID"
    );
    Ok(cid)
}
fn safe_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 240
            && !name.starts_with('.')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
        "unsafe flat file name"
    );
    Ok(())
}

fn admit_descriptor(
    dir: &File,
    names: &BTreeSet<String>,
    artifacts: &mut BTreeMap<String, Artifact>,
    value: &Value,
    path: Option<&str>,
) -> Result<()> {
    let cid = text(value, "cid")?;
    let digest = if let Some(checksum) = value.get("checksum") {
        let checksum = checksum
            .as_str()
            .context("component checksum type refused")?;
        let digest = checksum
            .strip_prefix("sha256:")
            .context("component checksum refused")?;
        if let Some(sha) = value.get("sha256") {
            ensure!(sha.as_str() == Some(digest), "component checksum differs");
        }
        ensure!(path.is_some(), "component release_path required");
        digest
    } else {
        text(value, "sha256")?
    };
    checked_hash(digest)?;
    let decoded = checked_cid(cid)?;
    if decoded.codec() == 0x55 {
        ensure!(
            hex::encode(decoded.hash().digest()) == digest,
            "raw CID hash differs"
        );
    }
    let size = value
        .get("size")
        .and_then(Value::as_u64)
        .context("artifact size integer required")?;
    ensure!(size > 0 && size <= i64::MAX as u64, "artifact size refused");
    let matches = if let Some(name) = path {
        safe_name(name)?;
        vec![name.to_owned()]
    } else {
        // Legacy capsule descriptors omit release_path. Their signed digest and
        // size identify bytes; each matching flat name becomes explicitly owned.
        let mut matches = Vec::new();
        for name in names {
            if METADATA.contains(&name.as_str()) || safe_name(name).is_err() {
                continue;
            }
            let Ok(file) = regular(dir, name, size) else {
                continue;
            };
            if file.metadata()?.len() != size {
                continue;
            }
            let descriptor = ArtifactDescriptor {
                name: name.clone(),
                cid: cid.to_owned(),
                sha256: digest.to_owned(),
                size,
            };
            let admitted = stamp(&file)?;
            let artifact = Artifact {
                descriptor,
                file,
                admitted,
            };
            if stream_verified(&artifact, |_| Ok(())).is_ok() && stamp(&artifact.file)? == admitted
            {
                matches.push(name.clone());
            }
        }
        ensure!(
            !matches.is_empty(),
            "artifact reference absent from snapshot"
        );
        matches
    };
    for name in matches {
        ensure!(
            !METADATA.contains(&name.as_str()) && names.contains(&name),
            "artifact path absent/reserved"
        );
        let descriptor = ArtifactDescriptor {
            name: name.clone(),
            cid: cid.to_owned(),
            sha256: digest.to_owned(),
            size,
        };
        if let Some(prior) = artifacts.get(&name) {
            ensure!(
                prior.descriptor == descriptor,
                "artifact references disagree"
            );
            continue;
        }
        ensure!(artifacts.len() < MAX_FILES, "artifact count refused");
        let file = regular(dir, &name, size)?;
        let admitted = stamp(&file)?;
        let artifact = Artifact {
            descriptor,
            file,
            admitted,
        };
        stream_verified(&artifact, |_| Ok(()))?;
        ensure!(
            stamp(&artifact.file)? == admitted,
            "artifact changed during admission"
        );
        artifacts.insert(name, artifact);
    }
    Ok(())
}

fn component_refs(
    value: &Value,
    dir: &File,
    names: &BTreeSet<String>,
    artifacts: &mut BTreeMap<String, Artifact>,
) -> Result<()> {
    match value {
        Value::Object(object) => {
            if object.contains_key("cid") {
                let path = if object.contains_key("release_path") {
                    Some(text(value, "release_path")?)
                } else {
                    None
                };
                admit_descriptor(dir, names, artifacts, value, path)?;
            }
            for child in object.values() {
                component_refs(child, dir, names, artifacts)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                component_refs(child, dir, names, artifacts)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn open_at(dir: &File, name: &str, flags: i32) -> Result<File> {
    let name = CString::new(name)?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )
    };
    ensure!(
        fd >= 0,
        "open publication file: {}",
        std::io::Error::last_os_error()
    );
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn directory(path: &Path) -> Result<File> {
    ensure!(path.is_absolute(), "absolute publication path required");
    let mut dir = File::open("/")?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                dir = open_at(
                    &dir,
                    name.to_str().context("non-UTF8 directory")?,
                    libc::O_RDONLY | libc::O_DIRECTORY,
                )?
            }
            _ => anyhow::bail!("canonical publication path required"),
        }
    }
    Ok(dir)
}
fn regular(dir: &File, name: &str, limit: u64) -> Result<File> {
    safe_name(name)?;
    let file = open_at(dir, name, libc::O_RDONLY)?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.nlink() == 1 && meta.len() > 0 && meta.len() <= limit,
        "regular single-link bounded file required: {name}"
    );
    Ok(file)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StatStamp {
    dev: u64,
    ino: u64,
    size: u64,
    mode: u32,
    nlink: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

impl StatStamp {
    fn from_metadata(m: &std::fs::Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            size: m.len(),
            mode: m.mode(),
            nlink: m.nlink(),
            mtime: m.mtime(),
            mtime_nsec: m.mtime_nsec(),
            ctime: m.ctime(),
            ctime_nsec: m.ctime_nsec(),
        }
    }
}

fn stamp(file: &File) -> Result<StatStamp> {
    let m = file.metadata()?;
    ensure!(m.is_file() && m.nlink() == 1, "file link/type changed");
    Ok(StatStamp::from_metadata(&m))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DirectoryStamp {
    dev: u64,
    ino: u64,
    mode: u32,
    uid: u32,
    gid: u32,
}

fn directory_stamp(file: &File) -> Result<DirectoryStamp> {
    let m = file.metadata()?;
    ensure!(m.is_dir(), "publication directory type changed");
    // Publisher bookkeeping and historical entries share these directories;
    // directory ownership and identity bind the signed files' namespace.
    Ok(DirectoryStamp {
        dev: m.dev(),
        ino: m.ino(),
        mode: m.mode(),
        uid: m.uid(),
        gid: m.gid(),
    })
}

fn admitted_metadata(dir: &File, name: &str, limit: u64) -> Result<(AdmittedMetadata, Vec<u8>)> {
    let file = regular(dir, name, limit)?;
    let admitted = stamp(&file)?;
    let bytes = bounded_bytes(&file, limit)?;
    ensure!(
        stamp(&file)? == admitted,
        "metadata changed during admission"
    );
    Ok((AdmittedMetadata { file, admitted }, bytes))
}
fn stream_file(
    file: &File,
    size: u64,
    mut consume: impl FnMut(&[u8]) -> Result<()>,
) -> Result<String> {
    let before = stamp(file)?;
    ensure!(before.size == size, "artifact size differs");
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut offset = 0;
    while offset < size {
        let wanted = (size - offset).min(buffer.len() as u64) as usize;
        let count = file.read_at(&mut buffer[..wanted], offset)?;
        #[cfg(test)]
        STREAM_READS.with(|reads| reads.set(reads.get() + 1));
        ensure!(count > 0, "artifact truncated");
        hasher.update(&buffer[..count]);
        consume(&buffer[..count])?;
        offset += count as u64;
    }
    ensure!(before == stamp(file)?, "artifact changed during read");
    Ok(hex::encode(hasher.finalize()))
}
fn stream_verified(artifact: &Artifact, consume: impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
    ensure!(
        stream_file(&artifact.file, artifact.descriptor.size, consume)?
            == artifact.descriptor.sha256,
        "artifact hash differs: {}",
        artifact.descriptor.name
    );
    Ok(())
}
fn bounded_bytes(file: &File, limit: u64) -> Result<Vec<u8>> {
    let size = file.metadata()?.len();
    ensure!(size <= limit, "metadata too large");
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(usize::try_from(size)?)?;
    stream_file(file, size, |part| {
        bytes.extend_from_slice(part);
        Ok(())
    })?;
    Ok(bytes)
}
fn read_artifact(artifact: &Artifact, limit: Option<u64>) -> Result<Vec<u8>> {
    if let Some(limit) = limit {
        ensure!(
            artifact.descriptor.size <= limit,
            "artifact exceeds read limit"
        );
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(usize::try_from(artifact.descriptor.size)?)?;
    stream_verified(artifact, |part| {
        bytes.extend_from_slice(part);
        Ok(())
    })?;
    Ok(bytes)
}

fn directory_names(dir: &File) -> Result<BTreeSet<String>> {
    let fd = open_at(dir, ".", libc::O_RDONLY | libc::O_DIRECTORY)?;
    let raw = unsafe { libc::dup(fd.as_raw_fd()) };
    ensure!(raw >= 0, "duplicate directory failed");
    let entries = unsafe { libc::fdopendir(raw) };
    if entries.is_null() {
        unsafe { libc::close(raw) };
        return Err(std::io::Error::last_os_error().into());
    }
    let result = (|| {
        let mut names = BTreeSet::new();
        loop {
            // readdir uses a null pointer for both EOF and error.
            #[cfg(target_os = "macos")]
            unsafe {
                *libc::__error() = 0;
            }
            #[cfg(target_os = "linux")]
            unsafe {
                *libc::__errno_location() = 0;
            }
            let entry = unsafe { libc::readdir(entries) };
            if entry.is_null() {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                ensure!(
                    std::io::Error::last_os_error().raw_os_error() == Some(0),
                    "read publication directory failed"
                );
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }
                .to_str()
                .context("non-UTF8 publication file")?;
            if matches!(name, "." | "..") {
                continue;
            }
            ensure!(names.len() < 4096, "publication directory too large");
            names.insert(name.to_owned());
        }
        Ok(names)
    })();
    unsafe { libc::closedir(entries) };
    result
}
fn mkdir_at(dir: &File, name: &str) -> Result<()> {
    let name = CString::new(name)?;
    ensure!(
        unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) } == 0,
        "create snapshot directory: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}
fn create_at(dir: &File, name: &str) -> Result<File> {
    open_at(dir, name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)
}
fn seal(file: &File) -> Result<()> {
    file.set_permissions(std::fs::Permissions::from_mode(0o444))?;
    file.sync_all()?;
    Ok(())
}
fn unlink_at(dir: &File, name: &str, directory: bool) -> Result<()> {
    let name = CString::new(name)?;
    ensure!(
        unsafe {
            libc::unlinkat(
                dir.as_raw_fd(),
                name.as_ptr(),
                if directory { libc::AT_REMOVEDIR } else { 0 },
            )
        } == 0,
        "remove snapshot failed"
    );
    Ok(())
}
fn reject_ancestor(parent: &File, root: &File) -> Result<()> {
    let expected = root.metadata()?;
    let mut dir = parent.try_clone()?;
    loop {
        let meta = dir.metadata()?;
        ensure!(
            (meta.dev(), meta.ino()) != (expected.dev(), expected.ino()),
            "snapshot parent inside input root"
        );
        let next = open_at(&dir, "..", libc::O_RDONLY | libc::O_DIRECTORY)?;
        let above = next.metadata()?;
        if (meta.dev(), meta.ino()) == (above.dev(), above.ino()) {
            return Ok(());
        }
        dir = next;
    }
}
fn space_fits(capacity: u128, available: u128, reserved: u128) -> bool {
    capacity > 0 && available <= capacity && reserved <= available
}
fn available_space(parent: &File, total: u64, count: usize) -> Result<()> {
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    ensure!(
        unsafe { libc::fstatvfs(parent.as_raw_fd(), stats.as_mut_ptr()) } == 0,
        "snapshot disk observation failed"
    );
    let stats = unsafe { stats.assume_init() };
    let unit = u128::from(stats.f_frsize);
    let reserved = u128::from(total) + unit * (count as u128 + 4);
    ensure!(
        space_fits(
            u128::from(stats.f_blocks) * unit,
            u128::from(stats.f_bavail) * unit,
            reserved
        ),
        "snapshot needs more free space than the volume has"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::signature::{generate_keypair, SigningKey};
    use serde_json::json;
    use std::os::unix::fs::symlink;

    // Each test owns an isolated, memory-only disposable signing key. The
    // temporary filesystem contains public envelopes and harmless fixture data.
    struct PublicPublicationFixture {
        _temporary: tempfile::TempDir,
        parent: PathBuf,
        input: PathBuf,
        key: SigningKey,
        did: String,
        release: Value,
    }

    fn raw_cid(bytes: &[u8]) -> String {
        let hash = cid::multihash::Multihash::<64>::wrap(0x12, &Sha256::digest(bytes)).unwrap();
        cid::Cid::new_v1(0x55, hash).to_string()
    }

    fn descriptor(bytes: &[u8]) -> Value {
        json!({"cid":raw_cid(bytes),"sha256":digest(bytes),"size":bytes.len()})
    }

    fn envelope(key: &SigningKey, domain: &str, payload: &Value) -> Vec<u8> {
        let bytes = serde_json::to_vec(payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(key, domain, &bytes);
        serde_json::to_vec(
            &json!({"payload":payload,"signature":signature,"signer_did":signer_did}),
        )
        .unwrap()
    }

    impl PublicPublicationFixture {
        fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let parent = temporary.path().canonicalize().unwrap();
            let input = parent.join("input");
            std::fs::create_dir(&input).unwrap();
            let (key, _) = generate_keypair();
            let did = crate::crypto::encode_signing_key_did(&key);
            let binary = b"#!/bin/sh\ncat /custodian/unopened-signing.pem > candidate-executed\n";
            let support = b"public qualified support archive";
            let catalog = br#"{"payload":{"entries":[],"schema":"elastos.model.catalog/v1"}}"#;
            let mut support_ref = descriptor(support);
            support_ref["release_path"] = json!("home.tar.gz");
            support_ref["checksum"] = json!(format!("sha256:{}", digest(support)));
            support_ref.as_object_mut().unwrap().remove("sha256");
            let components = serde_json::to_vec(&json!({
                "schema":"elastos.components/v1", "capsules":{}, "profiles":{},
                "external":{"home":{"platforms":{"*":support_ref}}},
                "model_catalog":{"head_cid":raw_cid(catalog),"publisher_dids":[did]}
            }))
            .unwrap();
            let installer = b"#!/bin/sh\n# public frozen installer\n";
            for (name, data) in [
                ("elastos-aarch64-darwin", &binary[..]),
                ("components-aarch64-darwin.json", &components[..]),
                ("home.tar.gz", &support[..]),
                ("model-catalog.json", &catalog[..]),
                ("install.sh", &installer[..]),
            ] {
                std::fs::write(input.join(name), data).unwrap();
            }
            let release = json!({"schema":"elastos.release/v1","version":"0.7.1","channel":"canary",
                "source":{"commit":"a".repeat(40),"tree":"b".repeat(40)},
                "released_at":1,"prev_release_cid":null,"installer_sha256":digest(installer),
                "platforms":{"aarch64-darwin":{"binary":descriptor(binary),"components":descriptor(&components)}}});
            let fixture = Self {
                _temporary: temporary,
                parent,
                input,
                key,
                did,
                release,
            };
            fixture.write_signed();
            fixture
        }

        fn write_signed(&self) {
            let release = envelope(&self.key, "elastos.release.v1", &self.release);
            let head = json!({"schema":"elastos.release.head/v1","version":self.release["version"],
                "channel":self.release["channel"],"signer_did":self.did,"updated_at":2,
                "prev_head_cid":null,"latest_release_cid":raw_cid(&release),"release_sha256":digest(&release)});
            std::fs::write(self.input.join("release.json"), release).unwrap();
            self.write_head(&head);
        }

        fn write_head(&self, head: &Value) {
            std::fs::write(
                self.input.join("release-head.json"),
                envelope(&self.key, "elastos.release.head.v1", head),
            )
            .unwrap();
        }

        fn head(&self) -> Value {
            let bytes = std::fs::read(self.input.join("release-head.json")).unwrap();
            serde_json::from_slice::<Value>(&bytes).unwrap()["payload"].clone()
        }

        fn open(&self) -> Result<Publication> {
            Publication::open_flat(&self.input, &self.did)
        }
    }

    #[test]
    fn real_crypto_admits_complete_public_set_and_exact_snapshot() {
        let fixture = PublicPublicationFixture::new();
        let publication = fixture.open().unwrap();
        assert_eq!(publication.version(), "0.7.1");
        assert_eq!(publication.channel(), "canary");
        assert_eq!(publication.artifacts().len(), 4);
        let output = fixture.parent.join("snapshot");
        publication.snapshot_into(&output).unwrap();
        let published = Publication::open_published(&output, &fixture.did).unwrap();
        assert_eq!(published.head_bytes(), publication.head_bytes());
        assert_eq!(published.release_bytes(), publication.release_bytes());
        assert_eq!(published.installer_bytes(), publication.installer_bytes());
        for record in publication.artifacts() {
            assert_eq!(
                published.read_verified_artifact(&record.name).unwrap(),
                publication.read_verified_artifact(&record.name).unwrap()
            );
        }
        assert!(!fixture.input.join("candidate-executed").exists());
    }

    #[test]
    fn published_cache_check_keeps_hash_reads_at_admission_and_requested_artifact() {
        let fixture = PublicPublicationFixture::new();
        let output = fixture.parent.join("published");
        fixture.open().unwrap().snapshot_into(&output).unwrap();
        let publication = Publication::open_published(&output, &fixture.did).unwrap();
        let reads = STREAM_READS.with(|reads| reads.get());
        for _ in 0..3 {
            publication.unchanged_published().unwrap();
        }
        assert_eq!(STREAM_READS.with(|reads| reads.get()), reads);
        publication.read_verified_artifact("home.tar.gz").unwrap();
        assert!(STREAM_READS.with(|reads| reads.get()) > reads);
    }

    #[test]
    fn published_cache_accepts_ledger_updates_and_unadvertised_history_without_hash_reads() {
        let fixture = PublicPublicationFixture::new();
        let output = fixture.parent.join("published");
        fixture.open().unwrap().snapshot_into(&output).unwrap();
        let publication = Publication::open_published(&output, &fixture.did).unwrap();
        let reads = STREAM_READS.with(|reads| reads.get());
        let ledger = output.join("release-ledger.json");
        std::fs::write(&ledger, br#"{"receipts":[]}"#).unwrap();
        publication.unchanged_published().unwrap();
        let next_ledger = output.join("release-ledger.next");
        std::fs::write(&next_ledger, br#"{"receipts":["public receipt"]}"#).unwrap();
        std::fs::rename(&next_ledger, &ledger).unwrap();
        std::fs::write(
            output.join("artifacts/historical-runtime"),
            b"public historical bytes",
        )
        .unwrap();
        publication.unchanged_published().unwrap();
        assert_eq!(STREAM_READS.with(|reads| reads.get()), reads);
        assert_eq!(
            publication.read_verified_artifact("home.tar.gz").unwrap(),
            b"public qualified support archive"
        );
        assert!(STREAM_READS.with(|reads| reads.get()) > reads);
        assert!(publication
            .read_verified_artifact("historical-runtime")
            .is_err());
    }

    #[test]
    fn published_cache_refuses_same_size_in_place_metadata_and_artifact_changes() {
        for name in METADATA.into_iter().chain([
            "artifacts/elastos-aarch64-darwin",
            "artifacts/components-aarch64-darwin.json",
            "artifacts/home.tar.gz",
            "artifacts/model-catalog.json",
        ]) {
            let fixture = PublicPublicationFixture::new();
            let output = fixture.parent.join("published");
            fixture.open().unwrap().snapshot_into(&output).unwrap();
            let path = output.join(name);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            let publication = Publication::open_published(&output, &fixture.did).unwrap();
            let before = std::fs::metadata(&path).unwrap().len();
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            let first = std::fs::read(&path).unwrap()[0];
            file.write_at(&[first ^ 1], 0).unwrap();
            file.sync_all().unwrap();
            assert_eq!(std::fs::metadata(&path).unwrap().len(), before);
            assert!(publication.unchanged_published().is_err(), "{name}");
        }
    }

    #[test]
    fn bounded_artifact_read_refuses_before_reading_and_keeps_signed_tamper_checks() {
        let fixture = PublicPublicationFixture::new();
        let publication = fixture.open().unwrap();
        let name = "home.tar.gz";
        let size = publication
            .artifacts()
            .iter()
            .find(|record| record.name == name)
            .unwrap()
            .size;
        let reads = STREAM_READS.with(|reads| reads.get());
        let error = publication
            .read_verified_artifact_bounded(name, size - 1)
            .unwrap_err();
        assert!(error.to_string().contains("read limit"));
        assert_eq!(STREAM_READS.with(|reads| reads.get()), reads);
        assert_eq!(
            publication
                .read_verified_artifact_bounded(name, size)
                .unwrap(),
            std::fs::read(fixture.input.join(name)).unwrap()
        );
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(fixture.input.join(name))
            .unwrap();
        file.write_at(b"X", 0).unwrap();
        file.sync_all().unwrap();
        assert!(publication
            .read_verified_artifact_bounded(name, size)
            .is_err());
        assert!(publication
            .read_verified_artifact_bounded("../install.sh", size)
            .is_err());
    }

    #[test]
    fn published_cache_refuses_replacement_links_and_mode_changes() {
        for name in ["install.sh", "artifacts/home.tar.gz"] {
            for mutation in ["replacement", "symlink", "hardlink", "mode"] {
                let fixture = PublicPublicationFixture::new();
                let output = fixture.parent.join("published");
                fixture.open().unwrap().snapshot_into(&output).unwrap();
                let publication = Publication::open_published(&output, &fixture.did).unwrap();
                let path = output.join(name);
                let retained = fixture.parent.join("retained-public-file");
                match mutation {
                    "replacement" | "symlink" => {
                        std::fs::rename(&path, &retained).unwrap();
                        if mutation == "symlink" {
                            symlink(&retained, &path).unwrap();
                        } else {
                            std::fs::copy(&retained, &path).unwrap();
                        }
                    }
                    "hardlink" => std::fs::hard_link(&path, retained).unwrap(),
                    _ => std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                        .unwrap(),
                }
                assert!(
                    publication.unchanged_published().is_err(),
                    "{name}: {mutation}"
                );
            }
        }
    }

    #[test]
    fn published_cache_refuses_root_and_artifact_directory_namespace_changes() {
        for name in ["", "artifacts"] {
            for replacement in ["directory", "symlink", "mode"] {
                let fixture = PublicPublicationFixture::new();
                let output = fixture.parent.join("published");
                fixture.open().unwrap().snapshot_into(&output).unwrap();
                let publication = Publication::open_published(&output, &fixture.did).unwrap();
                let path = if name.is_empty() {
                    output.clone()
                } else {
                    output.join(name)
                };
                if replacement == "mode" {
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                        .unwrap();
                } else {
                    let retained = fixture.parent.join("retained-public-directory");
                    std::fs::rename(&path, &retained).unwrap();
                    if replacement == "symlink" {
                        symlink(retained, &path).unwrap();
                    } else {
                        std::fs::create_dir(&path).unwrap();
                    }
                }
                assert!(
                    publication.unchanged_published().is_err(),
                    "{name}: {replacement}"
                );
            }
        }
    }

    #[test]
    fn independent_wrong_pin_and_corrupt_signature_refused() {
        let fixture = PublicPublicationFixture::new();
        let (other, _) = generate_keypair();
        assert!(Publication::open_flat(
            &fixture.input,
            &crate::crypto::encode_signing_key_did(&other)
        )
        .is_err());
        let path = fixture.input.join("release-head.json");
        let mut head: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        head["signature"] = json!("00".repeat(64));
        std::fs::write(path, serde_json::to_vec(&head).unwrap()).unwrap();
        assert!(fixture.open().is_err());
    }

    #[test]
    fn installer_binary_and_support_tamper_refused() {
        for name in [
            "install.sh",
            "elastos-aarch64-darwin",
            "home.tar.gz",
            "model-catalog.json",
        ] {
            let fixture = PublicPublicationFixture::new();
            std::fs::write(fixture.input.join(name), b"tampered public bytes").unwrap();
            assert!(fixture.open().is_err(), "{name}");
        }
    }

    #[test]
    fn signed_head_must_bind_release_hash_cid_and_identity() {
        for field in [
            "release_sha256",
            "latest_release_cid",
            "version",
            "channel",
            "signer_did",
        ] {
            let fixture = PublicPublicationFixture::new();
            let mut head = fixture.head();
            head[field] = match field {
                "release_sha256" => json!("0".repeat(64)),
                "latest_release_cid" => json!(raw_cid(b"other")),
                "version" => json!("0.7.2"),
                "channel" => json!("stable"),
                _ => json!("untrusted"),
            };
            fixture.write_head(&head);
            assert!(fixture.open().is_err(), "{field}");
        }
    }

    #[test]
    fn malformed_cid_raw_mismatch_and_signed_size_refused() {
        for (field, value) in [
            ("cid", json!("not-a-cid")),
            ("cid", json!(raw_cid(b"different"))),
            ("size", json!(1)),
            ("size", json!(-1)),
        ] {
            let mut fixture = PublicPublicationFixture::new();
            fixture.release["platforms"]["aarch64-darwin"]["binary"][field] = value;
            fixture.write_signed();
            assert!(fixture.open().is_err());
        }
    }

    #[test]
    fn approved_dag_pb_artifact_preserves_root_cid_without_file_hash_equivalence() {
        let mut fixture = PublicPublicationFixture::new();
        let hash = cid::multihash::Multihash::<64>::wrap(
            0x12,
            &Sha256::digest(b"independent UnixFS root"),
        )
        .unwrap();
        let unixfs = cid::Cid::new_v0(hash).unwrap().to_string();
        fixture.release["platforms"]["aarch64-darwin"]["binary"]["cid"] = json!(unixfs);
        fixture.write_signed();
        let publication = fixture.open().unwrap();
        assert_eq!(
            publication
                .artifacts()
                .iter()
                .find(|r| r.name == "elastos-aarch64-darwin")
                .unwrap()
                .cid,
            unixfs
        );
    }

    #[test]
    fn unsupported_platform_and_source_identity_refused() {
        let mut fixture = PublicPublicationFixture::new();
        let refs = fixture.release["platforms"]["aarch64-darwin"].take();
        fixture.release["platforms"] = json!({"x86_64-windows":refs});
        fixture.write_signed();
        assert!(fixture.open().is_err());
        let mut fixture = PublicPublicationFixture::new();
        fixture.release["source"]["tree"] = json!("B".repeat(40));
        fixture.write_signed();
        assert!(fixture.open().is_err());
    }

    #[test]
    fn symlink_hardlink_unadvertised_and_oversized_metadata_refused() {
        let fixture = PublicPublicationFixture::new();
        let binary = fixture.input.join("elastos-aarch64-darwin");
        let outside = fixture.parent.join("public-marker");
        std::fs::rename(&binary, &outside).unwrap();
        symlink(&outside, &binary).unwrap();
        assert!(fixture.open().is_err());
        let fixture = PublicPublicationFixture::new();
        std::fs::hard_link(
            fixture.input.join("home.tar.gz"),
            fixture.parent.join("link"),
        )
        .unwrap();
        assert!(fixture.open().is_err());
        let fixture = PublicPublicationFixture::new();
        std::fs::write(fixture.input.join("unadvertised"), b"public marker").unwrap();
        assert!(fixture.open().is_err());
        let fixture = PublicPublicationFixture::new();
        std::fs::write(
            fixture.input.join("release-head.json"),
            vec![b' '; MAX_METADATA as usize + 1],
        )
        .unwrap();
        assert!(fixture.open().is_err());
    }

    #[test]
    fn publication_component_admission_refuses_optional_checksum_and_development_strategy() {
        for info in [
            json!({}),
            json!({"strategy":"source-build"}),
            json!({"strategy":"local-copy"}),
        ] {
            let mut fixture = PublicPublicationFixture::new();
            let path = fixture.input.join("components-aarch64-darwin.json");
            let mut components: Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            components["external"]["optional"] = json!({"platforms":{"*":info}});
            let bytes = serde_json::to_vec(&components).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            fixture.release["platforms"]["aarch64-darwin"]["components"] = descriptor(&bytes);
            fixture.write_signed();
            let error = fixture
                .open()
                .err()
                .expect("invalid release component admitted");
            let message = format!("{error:#}");
            assert!(
                message.contains("checksum") || message.contains("strategy"),
                "{message}"
            );
        }
    }

    #[test]
    fn component_path_checksum_size_and_catalog_pin_refused() {
        for field in ["release_path", "checksum", "size", "catalog"] {
            let mut fixture = PublicPublicationFixture::new();
            let path = fixture.input.join("components-aarch64-darwin.json");
            let mut components: Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            if field == "catalog" {
                components["model_catalog"]["head_cid"] = json!(raw_cid(b"wrong catalog"));
            } else {
                components["external"]["home"]["platforms"]["*"][field] = match field {
                    "release_path" => json!("../public-marker"),
                    "checksum" => json!(format!("sha256:{}", "0".repeat(64))),
                    _ => json!(999),
                };
            }
            let bytes = serde_json::to_vec(&components).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            fixture.release["platforms"]["aarch64-darwin"]["components"] = descriptor(&bytes);
            fixture.write_signed();
            assert!(fixture.open().is_err(), "{field}");
        }
    }

    #[test]
    fn held_file_and_directory_fds_defeat_path_replacement() {
        let fixture = PublicPublicationFixture::new();
        let publication = fixture.open().unwrap();
        let binary = fixture.input.join("elastos-aarch64-darwin");
        let expected = publication
            .read_verified_artifact("elastos-aarch64-darwin")
            .unwrap();
        std::fs::rename(&binary, fixture.parent.join("retained-public-binary")).unwrap();
        let marker = fixture.parent.join("marker");
        std::fs::write(&marker, b"different public marker").unwrap();
        symlink(&marker, &binary).unwrap();
        std::fs::rename(&fixture.input, fixture.parent.join("retained-input")).unwrap();
        symlink(&fixture.parent, &fixture.input).unwrap();
        let output = fixture.parent.join("snapshot");
        publication.snapshot_into(&output).unwrap();
        assert_eq!(
            std::fs::read(output.join("artifacts/elastos-aarch64-darwin")).unwrap(),
            expected
        );
    }

    #[test]
    fn next_canary_admits_a_changed_runtime_with_identical_qualified_support() {
        let mut fixture = PublicPublicationFixture::new();
        let first = fixture.open().unwrap();
        let support = first
            .artifacts()
            .iter()
            .filter(|record| record.name != "elastos-aarch64-darwin")
            .cloned()
            .collect::<Vec<_>>();
        let next_binary = b"public next-version Runtime fixture";
        std::fs::write(fixture.input.join("elastos-aarch64-darwin"), next_binary).unwrap();
        fixture.release["version"] = json!("0.7.2");
        fixture.release["prev_release_cid"] = json!(first.release_cid());
        fixture.release["platforms"]["aarch64-darwin"]["binary"] = descriptor(next_binary);
        fixture.write_signed();
        let mut next_head = fixture.head();
        next_head["prev_head_cid"] = json!(raw_cid(first.head_bytes()));
        fixture.write_head(&next_head);
        let next = fixture.open().unwrap();
        assert_eq!(next.version(), "0.7.2");
        assert_eq!(
            next.artifacts()
                .iter()
                .filter(|record| record.name != "elastos-aarch64-darwin")
                .cloned()
                .collect::<Vec<_>>(),
            support
        );
        for record in support {
            assert_eq!(
                next.read_verified_artifact(&record.name).unwrap(),
                first.read_verified_artifact(&record.name).unwrap()
            );
        }
        assert_eq!(next.installer_bytes(), first.installer_bytes());
        assert_eq!(
            next.read_verified_artifact("elastos-aarch64-darwin")
                .unwrap(),
            next_binary
        );
    }

    #[test]
    fn changed_held_inode_refused_and_partial_snapshot_removed() {
        let fixture = PublicPublicationFixture::new();
        let publication = fixture.open().unwrap();
        std::fs::write(fixture.input.join("home.tar.gz"), b"in-place tamper").unwrap();
        assert!(publication.read_verified_artifact("home.tar.gz").is_err());
        let output = fixture.parent.join("snapshot");
        assert!(publication.snapshot_into(&output).is_err());
        assert!(!output.exists());
    }

    #[test]
    fn published_historical_extras_are_unservable() {
        let fixture = PublicPublicationFixture::new();
        let output = fixture.parent.join("published");
        fixture.open().unwrap().snapshot_into(&output).unwrap();
        std::fs::write(
            output.join("artifacts/old-platform"),
            b"historical public bytes",
        )
        .unwrap();
        let publication = Publication::open_published(&output, &fixture.did).unwrap();
        assert!(publication.read_verified_artifact("old-platform").is_err());
        assert!(publication.read_verified_artifact("../install.sh").is_err());
    }

    #[test]
    fn snapshot_existing_symlink_and_inside_input_refused() {
        let fixture = PublicPublicationFixture::new();
        let publication = fixture.open().unwrap();
        let output = fixture.parent.join("existing");
        std::fs::create_dir(&output).unwrap();
        assert!(publication.snapshot_into(&output).is_err());
        symlink(&output, fixture.parent.join("symlink")).unwrap();
        assert!(publication
            .snapshot_into(&fixture.parent.join("symlink"))
            .is_err());
        assert!(publication
            .snapshot_into(&fixture.input.join("inside"))
            .is_err());
        assert!(std::fs::read_dir(output).unwrap().next().is_none());
    }

    #[test]
    fn snapshot_reservation_must_fit_available_bytes() {
        assert!(space_fits(1000, 200, 50));
        assert!(space_fits(1000, 200, 200));
        assert!(!space_fits(1000, 200, 201));
        // Volume size alone never refuses a snapshot that fits.
        assert!(space_fits(1 << 50, 10, 10));
        assert!(!space_fits(1000, 1001, 0));
        assert!(!space_fits(0, 0, 0));
    }
}
