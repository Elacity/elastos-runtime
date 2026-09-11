//! Private staged-byte hash checkpoint. Runtime supplies the descriptor and
//! compares the computed root; expected CIDs never enter this primitive.

use super::StagedDirectory;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, CString};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Cursor, Read};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

const MAX_FILES: usize = 33; // 32 model payload files plus the exact object index.
const MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_RESPONSE: u64 = 64 * 1024;
const MAX_TIMEOUT: Duration = Duration::from_secs(300);

// Kubo v0.40.1 core/commands/add.go. All defaults that flags can override are
// explicit. The two remaining HAMT config values are checked separately.
const ADD_OPTIONS: &[(&str, &str)] = &[
    ("only-hash", "true"),
    ("pin", "false"),
    ("wrap-with-directory", "true"),
    ("cid-version", "1"),
    ("hash", "sha2-256"),
    ("raw-leaves", "true"),
    ("chunker", "size-262144"),
    ("trickle", "false"),
    ("max-file-links", "174"),
    ("max-directory-links", "0"),
    ("max-hamt-fanout", "256"),
    ("inline", "false"),
    ("inline-limit", "32"),
    ("nocopy", "false"),
    ("fscache", "false"),
    ("preserve-mode", "false"),
    ("preserve-mtime", "false"),
    ("empty-dirs", "false"),
    ("progress", "false"),
    ("fast-provide-root", "false"),
    ("fast-provide-wait", "false"),
];

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "directory hash deadline"))
}

#[derive(PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    length: u64,
    mode: u32,
    uid: u32,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

fn stamp(meta: &Metadata) -> Stamp {
    Stamp {
        device: meta.dev(),
        inode: meta.ino(),
        length: meta.len(),
        mode: meta.mode(),
        uid: meta.uid(),
        links: meta.nlink(),
        modified: (meta.mtime(), meta.mtime_nsec()),
        changed: (meta.ctime(), meta.ctime_nsec()),
    }
}

fn private_directory(meta: &Metadata) -> io::Result<()> {
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o7777 != 0o700 {
        return Err(invalid("unsafe staged directory"));
    }
    Ok(())
}

fn private_file(meta: &Metadata, size: u64) -> io::Result<()> {
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o7777 != 0o600
        || meta.nlink() != 1
        || meta.len() != size
    {
        return Err(invalid("unsafe or changed staged file"));
    }
    Ok(())
}

fn open_at(dir: &File, name: &str, directory: bool) -> io::Result<File> {
    let name = CString::new(name).map_err(|_| invalid("invalid staged name"))?;
    let flags = libc::O_RDONLY
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | libc::O_CLOEXEC
        | if directory { libc::O_DIRECTORY } else { 0 };
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

enum RootMode {
    Staging,
    BackendObservation,
}

fn open_root(path: &Path) -> io::Result<File> {
    open_root_with_mode(path, RootMode::Staging)
}

fn open_root_with_mode(path: &Path, mode: RootMode) -> io::Result<File> {
    let text = path
        .to_str()
        .filter(|s| s.starts_with('/') && s.len() <= 4096)
        .ok_or_else(|| invalid("absolute staged root required"))?;
    let parts: Vec<_> = text[1..].split('/').collect();
    if parts.len() > 64
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return Err(invalid("invalid staged root"));
    }
    let mut dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")?;
    for (index, name) in parts.iter().enumerate() {
        dir = open_at(&dir, name, true)?;
        let meta = dir.metadata()?;
        if index + 1 == parts.len() {
            match mode {
                RootMode::Staging => private_directory(&meta)?,
                RootMode::BackendObservation => {
                    if !meta.is_dir()
                        || meta.uid() != unsafe { libc::geteuid() }
                        || meta.mode() & 0o7022 != 0
                    {
                        return Err(invalid("unsafe backend repository directory"));
                    }
                }
            }
        } else {
            // Canonical OS temp ancestors can be root-owned sticky directories.
            // Every component is opened without following links, including /tmp aliases.
            let owner = meta.uid() == 0 || meta.uid() == unsafe { libc::geteuid() };
            let protected =
                meta.mode() & 0o022 == 0 || (meta.uid() == 0 && meta.mode() & 0o1000 != 0);
            if !owner || !protected {
                return Err(invalid("unsafe staged ancestor"));
            }
        }
    }
    Ok(dir)
}

fn relative_parts(path: &str) -> io::Result<Vec<&str>> {
    let parts: Vec<_> = path.split('/').collect();
    if path.is_empty()
        || path.len() > 256
        || parts.len() > 8
        || parts.iter().any(|p| {
            p.is_empty()
                || *p == "."
                || *p == ".."
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
    {
        return Err(invalid("invalid staged relative path"));
    }
    Ok(parts)
}

fn names(dir: &File) -> io::Result<Vec<String>> {
    // A fresh open description avoids sharing a readdir cursor with retained fds.
    let fd = open_at(dir, ".", true)?.into_raw_fd();
    let ptr = unsafe { libc::fdopendir(fd) };
    if ptr.is_null() {
        let error = io::Error::last_os_error();
        unsafe {
            libc::close(fd);
        }
        return Err(error);
    }
    struct Entries(*mut libc::DIR);
    impl Drop for Entries {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let entries = Entries(ptr);
    let mut result = Vec::new();
    loop {
        #[cfg(target_os = "macos")]
        let errno = unsafe { libc::__error() };
        #[cfg(target_os = "linux")]
        let errno = unsafe { libc::__errno_location() };
        unsafe {
            *errno = 0;
        }
        let entry = unsafe { libc::readdir(entries.0) };
        if entry.is_null() {
            if unsafe { *errno } != 0 {
                return Err(io::Error::last_os_error());
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }
            .to_str()
            .map_err(|_| invalid("invalid staged entry"))?;
        if matches!(name, "." | "..") {
            continue;
        }
        if result.len() >= MAX_FILES * 8 {
            return Err(invalid("staged entry count exceeded"));
        }
        result.push(name.to_owned());
    }
    result.sort();
    Ok(result)
}

struct OpenFile {
    path: String,
    file: File,
    stamp: Stamp,
}
struct Closure {
    root: File,
    root_stamp: Stamp,
    directories: Vec<(String, File, Stamp)>,
    files: Vec<OpenFile>,
}

impl Closure {
    fn open(descriptor: &StagedDirectory) -> io::Result<Self> {
        if descriptor.files.is_empty()
            || descriptor.files.len() > MAX_FILES
            || descriptor.total_bytes == 0
            || descriptor.total_bytes > MAX_BYTES
        {
            return Err(invalid("staged closure bounds"));
        }
        let mut declared = BTreeMap::new();
        let mut directories = BTreeSet::new();
        let mut total = 0u64;
        for file in &descriptor.files {
            let parts = relative_parts(&file.path)?;
            if file.size > MAX_BYTES || declared.insert(file.path.as_str(), file.size).is_some() {
                return Err(invalid("duplicate or oversized staged file"));
            }
            total = total
                .checked_add(file.size)
                .ok_or_else(|| invalid("staged size overflow"))?;
            for depth in 1..parts.len() {
                directories.insert(parts[..depth].join("/"));
            }
        }
        for required in ["capsule.json", "_elastos_object.json"] {
            if !declared
                .get(required)
                .is_some_and(|size| (1..=65536).contains(size))
            {
                return Err(invalid("missing or oversized staged metadata"));
            }
        }
        if total != descriptor.total_bytes
            || directories
                .iter()
                .any(|p| declared.contains_key(p.as_str()))
        {
            return Err(invalid("staged closure mismatch"));
        }
        let root = open_root(&descriptor.root)?;
        let mut closure = Self {
            root_stamp: stamp(&root.metadata()?),
            root: root.try_clone()?,
            directories: Vec::new(),
            files: Vec::new(),
        };
        closure.walk(&root, "", &declared, &directories)?;
        if closure.files.len() != declared.len() || closure.directories.len() != directories.len() {
            return Err(invalid("missing staged entries"));
        }
        closure.files.sort_by(|a, b| a.path.cmp(&b.path));
        closure.check(descriptor)?;
        Ok(closure)
    }

    fn walk(
        &mut self,
        dir: &File,
        prefix: &str,
        declared: &BTreeMap<&str, u64>,
        directories: &BTreeSet<String>,
    ) -> io::Result<()> {
        for name in names(dir)? {
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            relative_parts(&path)?;
            if directories.contains(&path) {
                let child = open_at(dir, path.rsplit('/').next().unwrap(), true)?;
                let metadata = child.metadata()?;
                private_directory(&metadata)?;
                let before = stamp(&metadata);
                self.walk(&child, &path, declared, directories)?;
                self.directories.push((path, child, before));
            } else {
                let size = declared
                    .get(path.as_str())
                    .ok_or_else(|| invalid("extra staged entry"))?;
                let file = open_at(dir, path.rsplit('/').next().unwrap(), false)?;
                let metadata = file.metadata()?;
                private_file(&metadata, *size)?;
                self.files.push(OpenFile {
                    path,
                    stamp: stamp(&metadata),
                    file,
                });
            }
        }
        Ok(())
    }

    fn check(&self, descriptor: &StagedDirectory) -> io::Result<()> {
        if stamp(&open_root(&descriptor.root)?.metadata()?) != self.root_stamp
            || stamp(&self.root.metadata()?) != self.root_stamp
        {
            return Err(invalid("staged root changed"));
        }
        for (path, file, before) in &self.directories {
            if stamp(&file.metadata()?) != *before
                || stamp(&self.reopen(path, true)?.metadata()?) != *before
            {
                return Err(invalid("staged directory changed"));
            }
        }
        for entry in &self.files {
            if stamp(&entry.file.metadata()?) != entry.stamp
                || stamp(&self.reopen(&entry.path, false)?.metadata()?) != entry.stamp
            {
                return Err(invalid("staged file changed"));
            }
        }
        Ok(())
    }

    fn reopen(&self, path: &str, directory: bool) -> io::Result<File> {
        let parts = relative_parts(path)?;
        let mut fd = self.root.try_clone()?;
        for (index, name) in parts.iter().enumerate() {
            fd = open_at(&fd, name, directory || index + 1 != parts.len())?;
        }
        Ok(fd)
    }
}

struct Multipart<'a> {
    files: std::slice::IterMut<'a, OpenFile>,
    current: Option<&'a mut OpenFile>,
    left: u64,
    header: Cursor<Vec<u8>>,
    finished: bool,
    deadline: Instant,
    boundary: String,
}

fn new_boundary() -> io::Result<String> {
    let mut random = [0u8; 32];
    if unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut boundary = String::with_capacity(64);
    const HEX: &[u8] = b"0123456789abcdef";
    for byte in random {
        boundary.push(HEX[(byte >> 4) as usize] as char);
        boundary.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(boundary)
}

fn part_header(path: &str, boundary: &str) -> Vec<u8> {
    format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{path}\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes()
}

impl<'a> Multipart<'a> {
    fn new(files: &'a mut [OpenFile], deadline: Instant, boundary: String) -> Self {
        Self {
            files: files.iter_mut(),
            current: None,
            left: 0,
            header: Cursor::new(Vec::new()),
            finished: false,
            deadline,
            boundary,
        }
    }
}

impl Read for Multipart<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        remaining(self.deadline)?;
        loop {
            let n = self.header.read(output)?;
            if n != 0 {
                return Ok(n);
            }
            if let Some(file) = self.current.as_mut() {
                if self.left != 0 {
                    let length = output.len().min(self.left.min(65536) as usize);
                    let n = file.file.read(&mut output[..length])?;
                    if n == 0 {
                        return Err(invalid("staged file truncated"));
                    }
                    self.left -= n as u64;
                    return Ok(n);
                }
                let mut extra = [0u8];
                if file.file.read(&mut extra)? != 0 || stamp(&file.file.metadata()?) != file.stamp {
                    return Err(invalid("staged file changed during hash"));
                }
                self.current = None;
                self.header = Cursor::new(b"\r\n".to_vec());
                continue;
            }
            if let Some(file) = self.files.next() {
                self.header = Cursor::new(part_header(&file.path, &self.boundary));
                self.left = file.stamp.length;
                self.current = Some(file);
            } else if !self.finished {
                self.finished = true;
                self.header = Cursor::new(format!("--{}--\r\n", self.boundary).into_bytes());
            } else {
                return Ok(0);
            }
        }
    }
}

fn bounded_response(response: ureq::Response) -> io::Result<Vec<u8>> {
    if response.status() != 200
        || response
            .header("Content-Encoding")
            .is_some_and(|v| v != "identity")
        || response
            .header("Content-Length")
            .is_some_and(|v| v.parse::<u64>().map_or(true, |n| n > MAX_RESPONSE))
        || response.header("X-Stream-Error").is_some()
    {
        return Err(invalid("invalid directory hash response"));
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err(invalid("directory hash response exceeds bound"));
    }
    Ok(bytes)
}

fn profile_request(
    agent: &ureq::Agent,
    api: &str,
    operation: &str,
    key: Option<&str>,
    deadline: Instant,
) -> io::Result<serde_json::Value> {
    let mut request = agent
        .post(&format!("{api}/api/v0/{operation}"))
        .set("Accept-Encoding", "identity")
        .set("Connection", "close")
        .timeout(remaining(deadline)?);
    if let Some(key) = key {
        request = request.query("arg", key);
    }
    let response = request
        .call()
        .map_err(|_| invalid("directory hash profile request failed"))?;
    serde_json::from_slice(&bounded_response(response)?)
        .map_err(|_| invalid("invalid directory hash profile response"))
}

fn check_hamt_value(key: &str, value: &serde_json::Value) -> io::Result<()> {
    // v0.40.1 config/import.go defines null/unset as these exact defaults.
    let matches = match key {
        "Import.UnixFSHAMTDirectorySizeThreshold" => {
            value.is_null()
                || value.as_u64() == Some(262144)
                || matches!(value.as_str(), Some("256KiB" | "262144"))
        }
        "Import.UnixFSHAMTDirectorySizeEstimation" => {
            value.is_null() || value.as_str() == Some("links")
        }
        _ => false,
    };
    if !matches {
        return Err(invalid("unsupported directory hash import profile"));
    }
    Ok(())
}

fn verify_profile(agent: &ureq::Agent, api: &str, deadline: Instant) -> io::Result<()> {
    if profile_request(agent, api, "version", None, deadline)?["Version"] != "0.40.1" {
        return Err(invalid("unsupported directory hash Kubo version"));
    }
    for key in [
        "Import.UnixFSHAMTDirectorySizeThreshold",
        "Import.UnixFSHAMTDirectorySizeEstimation",
    ] {
        let response = profile_request(agent, api, "config", Some(key), deadline)?;
        if response["Key"] != key {
            return Err(invalid("directory hash config key mismatch"));
        }
        check_hamt_value(
            key,
            response
                .get("Value")
                .ok_or_else(|| invalid("missing directory hash config value"))?,
        )?;
    }
    Ok(())
}

fn parse_root(bytes: &[u8]) -> io::Result<String> {
    let mut root = None;
    let mut lines = 0;
    for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        lines += 1;
        if line.len() > 4096 || lines > MAX_FILES * 8 + 1 {
            return Err(invalid("directory hash event bound"));
        }
        let event: serde_json::Value =
            serde_json::from_slice(line).map_err(|_| invalid("invalid directory hash event"))?;
        if event.get("Message").is_some() || event.get("Error").is_some() {
            return Err(invalid("directory hash backend failure"));
        }
        let name = event
            .get("Name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| invalid("missing directory hash name"))?;
        let cid = event
            .get("Hash")
            .and_then(|v| v.as_str())
            .ok_or_else(|| invalid("missing directory hash CID"))?;
        if cid.len() != 59
            || !cid.starts_with('b')
            || !cid
                .bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        {
            return Err(invalid("invalid directory hash CID"));
        }
        if name.is_empty() {
            if !cid.starts_with("bafybei") || root.replace(cid.to_owned()).is_some() {
                return Err(invalid("duplicate or invalid directory root"));
            }
        } else {
            relative_parts(name)?;
        }
    }
    root.ok_or_else(|| invalid("missing directory hash root"))
}

pub(super) fn verify_backend(provider: &super::IpfsProvider) -> io::Result<()> {
    if provider.state != super::KuboState::Ready || provider.api_port == 0 {
        return Err(invalid("directory hash backend unavailable"));
    }
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .try_proxy_from_env(false)
        .max_idle_connections(0)
        .build();
    let deadline = Instant::now() + Duration::from_secs(5);
    let version = profile_request(&agent, &provider.api_url(), "version", None, deadline)?;
    if version["Version"] != "0.40.1" {
        return Err(invalid("unsupported directory hash backend"));
    }
    Ok(())
}

pub(super) fn hash_directory(
    provider: &super::IpfsProvider,
    descriptor: &StagedDirectory,
) -> io::Result<String> {
    hash_directory_with_timeout(provider, descriptor, MAX_TIMEOUT)
}

#[derive(serde::Serialize)]
pub(super) struct CapacityObservation {
    volume_id: u64,
    capacity_bytes: u64,
    available_bytes: u64,
    required_bytes: u64,
}

fn capacity_observation(
    volume_id: u64,
    capacity_bytes: u64,
    available_bytes: u64,
    required_bytes: u64,
) -> io::Result<CapacityObservation> {
    let remaining_bytes = available_bytes
        .checked_sub(required_bytes)
        .ok_or_else(|| invalid("insufficient backend capacity"))?;
    if volume_id == 0
        || capacity_bytes == 0
        || available_bytes > capacity_bytes
        || !(1..=super::MAX_CAPACITY_REQUIRED_BYTES).contains(&required_bytes)
        || remaining_bytes < capacity_bytes.div_ceil(10)
    {
        return Err(invalid("invalid or insufficient backend capacity"));
    }
    Ok(CapacityObservation {
        volume_id,
        capacity_bytes,
        available_bytes,
        required_bytes,
    })
}

fn capacity_bytes(blocks: u128, block_size: u128) -> io::Result<u64> {
    if block_size == 0 {
        return Err(invalid("invalid filesystem block size"));
    }
    blocks
        .checked_mul(block_size)
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(|| invalid("filesystem capacity overflow"))
}

fn capacity_datastore_spec() -> serde_json::Value {
    // Pinned Kubo v0.40.1 config/init.go flatfsSpec; other layouts need separate proof.
    serde_json::json!({"type":"mount","mounts":[
        {"mountpoint":"/blocks","type":"flatfs","prefix":"flatfs.datastore","path":"blocks","sync":false,"shardFunc":"/repo/flatfs/shard/v1/next-to-last/2"},
        {"mountpoint":"/","type":"levelds","prefix":"leveldb.datastore","path":"datastore","compression":"none"}
    ]})
}

fn validate_capacity_datastore(response: &serde_json::Value) -> io::Result<()> {
    if response["Key"] != "Datastore.Spec" || response["Value"] != capacity_datastore_spec() {
        return Err(invalid("unsupported backend datastore layout"));
    }
    Ok(())
}

fn require_same_capacity_volume(repo_volume: u64, data_volumes: [u64; 2]) -> io::Result<()> {
    if repo_volume == 0 || data_volumes.iter().any(|volume| *volume != repo_volume) {
        return Err(invalid("backend datastores are on another volume"));
    }
    Ok(())
}

fn observe_repo_capacity(repo: &Path, required_bytes: u64) -> io::Result<CapacityObservation> {
    // Keep the opened no-follow repo handle for the filesystem observation.
    let directory = open_root_with_mode(repo, RootMode::BackendObservation)?;
    let before = directory.metadata()?;
    let data_dirs = [
        open_at(&directory, "blocks", true)?,
        open_at(&directory, "datastore", true)?,
    ];
    let data_before = [data_dirs[0].metadata()?, data_dirs[1].metadata()?];
    for meta in &data_before {
        // Kubo creates its repository and data subdirectories with ordinary modes.
        if !meta.is_dir() || meta.uid() != before.uid() || meta.mode() & 0o7022 != 0 {
            return Err(invalid("unsafe backend datastore directory"));
        }
    }
    require_same_capacity_volume(before.dev(), [data_before[0].dev(), data_before[1].dev()])?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::fstatvfs(directory.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stats = unsafe { stats.assume_init() };
    if stats.f_flag & libc::ST_RDONLY != 0
        || stats.f_bavail > stats.f_bfree
        || stats.f_bfree > stats.f_blocks
    {
        return Err(invalid("unsafe backend filesystem observations"));
    }
    let capacity = capacity_bytes(u128::from(stats.f_blocks), u128::from(stats.f_frsize))?;
    let available = capacity_bytes(u128::from(stats.f_bavail), u128::from(stats.f_frsize))?;
    let observation = capacity_observation(before.dev(), capacity, available, required_bytes)?;
    if stamp(&before) != stamp(&directory.metadata()?)
        || stamp(&before)
            != stamp(&open_root_with_mode(repo, RootMode::BackendObservation)?.metadata()?)
    {
        return Err(invalid("backend repository changed during observation"));
    }
    for (index, name) in ["blocks", "datastore"].iter().enumerate() {
        if stamp(&data_before[index]) != stamp(&data_dirs[index].metadata()?)
            || stamp(&data_before[index]) != stamp(&open_at(&directory, name, true)?.metadata()?)
        {
            return Err(invalid("backend datastore changed during observation"));
        }
    }
    Ok(observation)
}

pub(super) fn check_capacity(
    provider: &super::IpfsProvider,
    required_bytes: u64,
) -> io::Result<CapacityObservation> {
    if !(1..=super::MAX_CAPACITY_REQUIRED_BYTES).contains(&required_bytes)
        || provider.state != super::KuboState::Ready
        || provider.api_port == 0
    {
        return Err(invalid("backend capacity unavailable"));
    }
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .try_proxy_from_env(false)
        .max_idle_connections(0)
        .build();
    let deadline = Instant::now() + Duration::from_secs(5);
    let api = provider.api_url();
    if profile_request(&agent, &api, "version", None, deadline)?["Version"] != "0.40.1" {
        return Err(invalid("unsupported capacity backend"));
    }
    validate_capacity_datastore(&profile_request(
        &agent,
        &api,
        "config",
        Some("Datastore.Spec"),
        deadline,
    )?)?;
    // size-only omits RepoPath. A full stat is still bounded by this call's deadline.
    let stat = profile_request(&agent, &api, "repo/stat", None, deadline)?;
    let repo = stat
        .get("RepoPath")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid("missing actual repository path"))?;
    if stat
        .get("RepoSize")
        .and_then(serde_json::Value::as_u64)
        .is_none()
    {
        return Err(invalid("invalid repository size"));
    }
    remaining(deadline)?;
    let observation = observe_repo_capacity(Path::new(repo), required_bytes)?;
    remaining(deadline)?;
    Ok(observation)
}

fn hash_directory_with_timeout(
    provider: &super::IpfsProvider,
    descriptor: &StagedDirectory,
    timeout: Duration,
) -> io::Result<String> {
    if provider.state != super::KuboState::Ready
        || provider.api_port == 0
        || timeout.is_zero()
        || timeout > MAX_TIMEOUT
    {
        return Err(invalid("directory hash backend unavailable"));
    }
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| invalid("invalid directory hash deadline"))?;
    let mut closure = Closure::open(descriptor)?;
    let boundary = new_boundary()?;
    let body_length = closure
        .files
        .iter()
        .try_fold(format!("--{boundary}--\r\n").len() as u64, |sum, file| {
            sum.checked_add(file.stamp.length)
                .and_then(|n| n.checked_add(part_header(&file.path, &boundary).len() as u64 + 2))
        })
        .ok_or_else(|| invalid("directory hash body size overflow"))?;
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .try_proxy_from_env(false)
        .max_idle_connections(0)
        .build();
    let api = provider.api_url();
    verify_profile(&agent, &api, deadline)?;
    let mut request = agent
        .post(&format!("{api}/api/v0/add"))
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .set("Content-Length", &body_length.to_string())
        .set("Accept-Encoding", "identity")
        .set("Connection", "close")
        .timeout(remaining(deadline)?);
    for (key, value) in ADD_OPTIONS {
        request = request.query(key, value);
    }
    // Synchronous send(Read) settles or errors before handles are dropped. This
    // owner starts no background work. Runtime must wait for return before cleanup.
    let response = request
        .send(Multipart::new(&mut closure.files, deadline, boundary))
        .map_err(|_| invalid("directory hash stream failed"))?;
    let root = parse_root(&bounded_response(response)?)?;
    closure.check(descriptor)?;
    verify_profile(&agent, &api, deadline)?;
    remaining(deadline)?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IpfsProvider, KuboState, StagedFile};
    use std::fs::{self, File};
    use std::io::{BufRead, Read, Write};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    struct OwnedChild(Child);

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            self.0.wait().expect("reap exact fixture child");
        }
    }

    fn put(path: &Path, bytes: &[u8]) {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, bytes))
            .unwrap();
    }

    // One fixture owner. Its children are dropped before TempDir; all command
    // outputs stay small and every wait shares the same 90-second deadline.
    struct KuboFixture {
        binary: PathBuf,
        root: tempfile::TempDir,
        repo: PathBuf,
        deadline: Instant,
    }

    impl KuboFixture {
        fn command(&self, args: &[&str], cwd: &Path) -> Command {
            let mut command = Command::new(&self.binary);
            command
                .args(args)
                .current_dir(cwd)
                .env_clear()
                .env("HOME", self.root.path())
                .env("IPFS_PATH", &self.repo)
                .env("TMPDIR", self.root.path())
                .env("PATH", "/usr/bin:/bin")
                .env("LANG", "C")
                .stdin(Stdio::null());
            command
        }

        fn check_wait(&self, stdout: &File, stderr: &File) {
            assert!(Instant::now() < self.deadline, "fixture deadline");
            for file in [stdout, stderr] {
                assert!(
                    file.metadata().unwrap().len() <= 65536,
                    "fixture output bound"
                );
            }
        }

        fn run(&self, args: &[&str], cwd: &Path) -> String {
            self.run_command(self.command(args, cwd))
        }

        fn run_command(&self, mut command: Command) -> String {
            let mut stdout = tempfile::tempfile_in(self.root.path()).unwrap();
            let mut stderr = tempfile::tempfile_in(self.root.path()).unwrap();
            let mut child = OwnedChild(
                command
                    .stdout(stdout.try_clone().unwrap())
                    .stderr(stderr.try_clone().unwrap())
                    .spawn()
                    .unwrap(),
            );
            let status = loop {
                self.check_wait(&stdout, &stderr);
                if let Some(status) = child.0.try_wait().unwrap() {
                    break status;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            use std::io::{Seek, SeekFrom};
            stdout.seek(SeekFrom::Start(0)).unwrap();
            stderr.seek(SeekFrom::Start(0)).unwrap();
            let mut out = String::new();
            let mut err = String::new();
            stdout.take(65537).read_to_string(&mut out).unwrap();
            stderr.take(65537).read_to_string(&mut err).unwrap();
            assert!(out.len() <= 65536 && err.len() <= 65536);
            assert!(status.success(), "fixture command failed: {err}");
            out.trim().to_string()
        }

        fn digest(&self, path: &Path) -> String {
            let mut command = Command::new(if cfg!(target_os = "macos") {
                "/usr/bin/shasum"
            } else {
                "/usr/bin/sha256sum"
            });
            if cfg!(target_os = "macos") {
                command.args(["-a", "256"]);
            }
            command
                .arg(path)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .stdin(Stdio::null());
            let output = self.run_command(command);
            let digest = output.split_whitespace().next().unwrap();
            assert!(digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()));
            digest.to_owned()
        }

        // One in-flight request. A bounded reader returns the same pipe handles;
        // deadline/error kills and reaps the owned child before joining the reader.
        // None closes stdin and proves exact stdout EOF after Shutdown.
        fn provider_exchange(
            &self,
            child: &mut OwnedChild,
            stderr: &File,
            request: Option<serde_json::Value>,
        ) -> Vec<u8> {
            let mut stdin = child.0.stdin.take().unwrap();
            let stdout = child.0.stdout.take().unwrap();
            let payload = request.map(|request| {
                let mut bytes = serde_json::to_vec(&request).unwrap();
                bytes.push(b'\n');
                assert!(bytes.len() <= 16384, "bounded control request");
                bytes
            });
            let (tx, rx) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                let mut stdout = std::io::BufReader::new(stdout);
                let mut bytes = Vec::new();
                let result = (|| -> io::Result<Option<std::process::ChildStdin>> {
                    if let Some(payload) = payload {
                        stdin.write_all(&payload)?;
                        stdin.flush()?;
                        stdout.by_ref().take(98305).read_until(b'\n', &mut bytes)?;
                        if bytes.len() > 98304
                            || bytes.last() != Some(&b'\n')
                            || !stdout.buffer().is_empty()
                        {
                            return Err(invalid("provider response framing"));
                        }
                    } else {
                        drop(stdin);
                        stdout.by_ref().take(1).read_to_end(&mut bytes)?;
                        if !bytes.is_empty() {
                            return Err(invalid("unexpected output after Shutdown"));
                        }
                        return Ok(None);
                    }
                    Ok(Some(stdin))
                })();
                tx.send(()).ok();
                (stdout.into_inner(), bytes, result)
            });
            loop {
                let remaining = self.deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() || stderr.metadata().unwrap().len() > 65536 {
                    child.0.kill().ok();
                    child.0.wait().unwrap();
                    let _ = reader.join();
                    panic!("provider process deadline/output bound");
                }
                match rx.recv_timeout(remaining.min(Duration::from_millis(10))) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
            let (stdout, bytes, result) = reader.join().expect("provider reader joined");
            child.0.stdout = Some(stdout);
            match result {
                Ok(stdin) => child.0.stdin = stdin,
                Err(error) => {
                    child.0.kill().ok();
                    child.0.wait().unwrap();
                    panic!("provider wire failed: {error}");
                }
            }
            bytes
        }
    }

    fn disk_bytes(path: &Path) -> (u64, u64) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_dir() || metadata.is_file());
        let mut logical = if metadata.is_file() {
            metadata.len()
        } else {
            0
        };
        let mut allocated = metadata.blocks() * 512;
        if metadata.is_dir() {
            let entries: Vec<_> = fs::read_dir(path).unwrap().collect();
            assert!(entries.len() <= 1024);
            for entry in entries {
                let (l, a) = disk_bytes(&entry.unwrap().path());
                logical += l;
                allocated += a;
            }
        }
        (logical, allocated)
    }

    fn peak_rss_bytes(who: libc::c_int) -> u64 {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        assert_eq!(unsafe { libc::getrusage(who, usage.as_mut_ptr()) }, 0);
        let rss = unsafe { usage.assume_init() }.ru_maxrss as u64;
        if cfg!(target_os = "macos") {
            rss
        } else {
            rss * 1024
        }
    }

    #[test]
    #[ignore = "requires explicit ELASTOS_TEST_KUBO_PATH and ELASTOS_TEST_IPFS_PROVIDER_PATH; parent runs"]
    fn staged_directory_hash_matches_normal_cli_import_without_mutation() {
        let binary = PathBuf::from(
            std::env::var_os("ELASTOS_TEST_KUBO_PATH")
                .expect("explicit Kubo prerequisite; a skipped test is not proof"),
        );
        assert!(binary.is_absolute() && fs::symlink_metadata(&binary).unwrap().is_file());
        let provider_binary = PathBuf::from(
            std::env::var_os("ELASTOS_TEST_IPFS_PROVIDER_PATH")
                .expect("explicit native provider prerequisite"),
        );
        assert!(
            provider_binary.is_absolute()
                && fs::symlink_metadata(&provider_binary).unwrap().is_file()
        );
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().canonicalize().unwrap().join("repo");
        let fixture = KuboFixture {
            binary,
            root,
            repo,
            deadline: Instant::now() + Duration::from_secs(90),
        };
        assert_eq!(
            fixture.run(&["version", "--number"], fixture.root.path()),
            "0.40.1"
        );
        fixture.run(&["init", "--empty-repo"], fixture.root.path());
        let config_path = fixture.repo.join("config");
        let mut config: serde_json::Value =
            serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["Bootstrap"] = serde_json::json!([]);
        config["Addresses"]["API"] = serde_json::json!("/ip4/127.0.0.1/tcp/0");
        config["Addresses"]["Gateway"] = serde_json::json!("");
        config["Addresses"]["Swarm"] = serde_json::json!([]);
        config["Routing"]["Type"] = serde_json::json!("none");
        config["Discovery"]["MDNS"]["Enabled"] = serde_json::json!(false);
        config["AutoConf"]["Enabled"] = serde_json::json!(false);
        // Fixed v0.40.1 profile, including HAMT knobs that have no add flag.
        config["Import"] = serde_json::json!({
            "CidVersion": 1, "UnixFSRawLeaves": true, "UnixFSChunker": "size-262144",
            "HashFunction": "sha2-256", "UnixFSFileMaxLinks": 174,
            "UnixFSDirectoryMaxLinks": 0, "UnixFSHAMTDirectoryMaxFanout": 256,
            "UnixFSHAMTDirectorySizeThreshold": "256KiB",
            "UnixFSHAMTDirectorySizeEstimation": "links", "UnixFSDAGLayout": "balanced",
            "FastProvideRoot": false, "FastProvideWait": false
        });
        fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let staged = fixture.root.path().join("staged");
        fs::create_dir(&staged).unwrap();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(staged.join("notices")).unwrap();
        fs::set_permissions(staged.join("notices"), fs::Permissions::from_mode(0o700)).unwrap();
        // Synthetic package bytes, not a signed catalog/model-admission fixture.
        put(
            &staged.join("capsule.json"),
            br#"{"name":"hash-fixture","type":"content"}"#,
        );
        put(&staged.join("notices/source.txt"), b"synthetic notice\n");
        put(&staged.join("_elastos_object.json"), br#"{"schema":"elastos.content.object.manifest/v1","kind":"capsule","content_digest":"sha256:fixture","files":[]}"#);
        let mut weights = vec![0u8; 16 * 1024 * 1024];
        let mut random = 0x7b16_984d_3c20_a5e1u64;
        for (block_index, block) in weights.chunks_mut(262144).enumerate() {
            for bytes in block.chunks_mut(8) {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                let length = bytes.len();
                bytes.copy_from_slice(&random.to_le_bytes()[..length]);
            }
            block[..8].copy_from_slice(&(block_index as u64).to_le_bytes());
        }
        let delimiter = b"\r\n--elastos-private-directory-hash-v1\r\nContent-Disposition: form-data; name=\"file\"; filename=\"injected\"\r\n\r\n";
        weights[100..100 + delimiter.len()].copy_from_slice(delimiter);
        put(&staged.join("weights.gguf"), &weights);
        drop(weights);
        let files: Vec<_> = [
            "_elastos_object.json",
            "capsule.json",
            "notices/source.txt",
            "weights.gguf",
        ]
        .into_iter()
        .map(|name| StagedFile {
            path: name.into(),
            size: fs::metadata(staged.join(name)).unwrap().len(),
        })
        .collect();
        let descriptor = StagedDirectory {
            root: staged.canonicalize().unwrap(),
            total_bytes: files.iter().map(|file| file.size).sum(),
            files,
        };
        // NORMAL import actually writes/pins; the verifier must leave that exact
        // inventory unchanged. Sorted top-level args match multipart nested names.
        let golden = fixture.run(
            &[
                "add",
                "--recursive=true",
                "--quieter=true",
                "--wrap-with-directory=true",
                "--only-hash=false",
                "--pin=true",
                "--cid-version=1",
                "--hash=sha2-256",
                "--raw-leaves=true",
                "--chunker=size-262144",
                "--trickle=false",
                "--max-file-links=174",
                "--max-directory-links=0",
                "--max-hamt-fanout=256",
                "--inline=false",
                "--nocopy=false",
                "--fscache=false",
                "--preserve-mode=false",
                "--preserve-mtime=false",
                "--empty-dirs=false",
                "--progress=false",
                "--fast-provide-root=false",
                "--fast-provide-wait=false",
                "_elastos_object.json",
                "capsule.json",
                "notices",
                "weights.gguf",
            ],
            &staged,
        );
        assert!(golden.starts_with("bafy") && golden.len() == 59);
        let before_blocks = fixture.run(&["refs", "local"], fixture.root.path());
        let before_pins = fixture.run(&["pin", "ls", "--type=all"], fixture.root.path());
        let stdout = tempfile::tempfile_in(fixture.root.path()).unwrap();
        let stderr = tempfile::tempfile_in(fixture.root.path()).unwrap();
        let mut daemon = OwnedChild(
            fixture
                .command(
                    &["daemon", "--offline", "--routing=none", "--enable-gc=false"],
                    fixture.root.path(),
                )
                .stdout(stdout.try_clone().unwrap())
                .stderr(stderr.try_clone().unwrap())
                .spawn()
                .unwrap(),
        );
        let api_file = fixture.repo.join("api");
        let port = loop {
            fixture.check_wait(&stdout, &stderr);
            assert!(
                daemon.0.try_wait().unwrap().is_none(),
                "fixture daemon exited"
            );
            if let Ok(address) = fs::read_to_string(&api_file) {
                if let Some(port) = address
                    .trim()
                    .strip_prefix("/ip4/127.0.0.1/tcp/")
                    .and_then(|p| p.parse::<u16>().ok())
                {
                    let url = format!("http://127.0.0.1:{port}/api/v0/version");
                    if let Ok(response) = ureq::AgentBuilder::new()
                        .redirects(0)
                        .try_proxy_from_env(false)
                        .build()
                        .post(&url)
                        .timeout(Duration::from_millis(100))
                        .call()
                    {
                        let version: serde_json::Value =
                            serde_json::from_reader(response.into_reader().take(4097)).unwrap();
                        assert_eq!(version["Version"], "0.40.1");
                        break port;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // Seeded local-cache wire proof: this is not a cold-network, signed
        // catalog or Runtime/Content admission test.
        let wire_started = Instant::now();
        let backend_before = disk_bytes(&fixture.repo);
        let seed_disk = disk_bytes(&staged);
        assert!(
            backend_before.0 >= 16 * 1024 * 1024,
            "distinct imported payload blocks"
        );
        let received = fixture.root.path().join("received");
        let child_data = fixture.root.path().join("provider-data");
        for path in [&received, &child_data, &received.join("notices")] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let provider_stderr = tempfile::tempfile_in(fixture.root.path()).unwrap();
        let mut native = OwnedChild(
            Command::new(&provider_binary)
                .current_dir(fixture.root.path())
                .env_clear()
                .env("HOME", fixture.root.path())
                .env("TMPDIR", fixture.root.path())
                .env("PATH", "/usr/bin:/bin")
                .env("LANG", "C")
                .env("ELASTOS_IPFS_KUBO_PATH", &fixture.binary)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(provider_stderr.try_clone().unwrap())
                .spawn()
                .unwrap(),
        );
        let mut calls = 0usize;
        let mut largest_frame = 0usize;
        let mut call = |request: serde_json::Value| {
            let op: String = request["op"]
                .as_str()
                .unwrap_or("missing")
                .chars()
                .take(64)
                .collect();
            let frame = fixture.provider_exchange(&mut native, &provider_stderr, Some(request));
            calls += 1;
            largest_frame = largest_frame.max(frame.len());
            let response: serde_json::Value = serde_json::from_slice(&frame).unwrap();
            if response["status"] != "ok" {
                use std::os::unix::fs::FileExt as _;
                let length = provider_stderr.metadata().unwrap().len();
                let mut diagnostic = vec![0; length.min(65536) as usize];
                // Positional read preserves the child writer's shared file offset.
                let read = provider_stderr
                    .read_at(&mut diagnostic, length.saturating_sub(65536))
                    .unwrap();
                panic!(
                    "provider operation {op} failed; fixture stderr: {}",
                    String::from_utf8_lossy(&diagnostic[..read])
                );
            }
            response
        };
        let init = call(serde_json::json!({"op":"init","config":{"base_path":child_data}}));
        assert_eq!(init["data"]["state"], "cold");
        crate::write_coord_file(
            &child_data,
            &crate::CoordFile {
                kubo_pid: daemon.0.id(),
                api_port: port,
                gateway_port: 0,
                started_at: 1,
                last_used: 1,
            },
        );
        assert_eq!(
            call(serde_json::json!({"op":"runtime_prepare_backend"})),
            serde_json::json!({"status":"ok"})
        );
        let repo_metadata = fs::symlink_metadata(&fixture.repo).unwrap();
        let repo_mode = repo_metadata.mode() & 0o7777;
        assert!(repo_metadata.is_dir());
        assert_eq!(repo_metadata.uid(), unsafe { libc::geteuid() });
        assert_eq!(repo_mode & 0o7022, 0);
        let capacity = call(serde_json::json!({"op":"runtime_check_capacity","required_bytes":1}));
        assert_eq!(capacity["data"].as_object().unwrap().len(), 4);
        assert_eq!(
            capacity["data"]["volume_id"],
            fs::metadata(&fixture.repo).unwrap().dev()
        );
        assert_eq!(capacity["data"]["required_bytes"], 1);
        let total = capacity["data"]["capacity_bytes"].as_u64().unwrap();
        let available = capacity["data"]["available_bytes"].as_u64().unwrap();
        assert!(total > 0 && available <= total && available > total.div_ceil(10));
        assert!(
            !child_data.join("ipfs-repo").exists(),
            "capacity must observe the daemon's repo, not the child's default"
        );
        let transfer_started = Instant::now();
        let mut transferred = 0u64;
        let mut digests = Vec::new();
        for file in &descriptor.files {
            let destination = received.join(&file.path);
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_NOFOLLOW)
                .mode(0o600)
                .open(&destination)
                .unwrap();
            let mut offset = 0u64;
            while offset < file.size {
                let length = (file.size - offset).min(65536);
                let end = offset + length - 1;
                let mut request = serde_json::json!({"op":"cat","bounded_read":true,"cid":golden,"path":file.path,
                    "_runtime_invocation":{"schema":"elastos.provider.invocation/v1","source":"content-provider","target":"ipfs","op":"cat","transport":"runtime-local-provider-plane","transfer":"bytes","range":{"start":offset,"end":end},"progress":{"request_id":"seeded-wire-proof","expected_bytes":length}}});
                let complete_metadata = file.path == "_elastos_object.json";
                if complete_metadata {
                    assert_eq!(offset, 0);
                    assert!(file.size <= 65536);
                    request["max_bytes"] = serde_json::json!(65536);
                    let envelope = request["_runtime_invocation"].as_object_mut().unwrap();
                    envelope.remove("range");
                    envelope.remove("progress");
                }
                let response = call(request);
                if complete_metadata {
                    assert_eq!(
                        response["data"]["_runtime_complete_metadata"],
                        serde_json::json!({"schema":"elastos.provider.complete-metadata/v1","cid":golden,"path":file.path,"max_bytes":65536,"actual_bytes":file.size,"completed":true})
                    );
                    assert!(response["data"].get("_runtime_applied_range").is_none());
                } else {
                    assert_eq!(
                        response["data"]["_runtime_applied_range"],
                        serde_json::json!({"schema":"elastos.provider.applied-range/v1","cid":golden,"path":file.path,"start":offset,"end":end})
                    );
                }
                let encoded = response["data"]["data"].as_str().unwrap();
                assert!(encoded.len() <= 87384);
                use base64::Engine as _;
                let bytes = crate::BASE64.decode(encoded).unwrap();
                assert_eq!(bytes.len() as u64, length);
                assert_eq!(crate::BASE64.encode(&bytes), encoded);
                output.write_all(&bytes).unwrap();
                offset += length;
                transferred += length;
            }
            output.sync_all().unwrap();
            private_file(&output.metadata().unwrap(), file.size).unwrap();
            drop(output);
            let expected = fixture.digest(&staged.join(&file.path));
            assert_eq!(fixture.digest(&destination), expected);
            if matches!(file.path.as_str(), "capsule.json" | "_elastos_object.json") {
                assert_eq!(
                    fs::read(&destination).unwrap(),
                    fs::read(staged.join(&file.path)).unwrap()
                );
            }
            digests.push(serde_json::json!({"path":file.path,"sha256":expected}));
        }
        let transfer_elapsed = transfer_started.elapsed();
        assert_eq!(transferred, descriptor.total_bytes);
        let staging_disk = disk_bytes(&received);
        let backend_after_transfer = disk_bytes(&fixture.repo);
        let hash_started = Instant::now();
        let hash = call(
            serde_json::json!({"op":"runtime_hash_staged_directory","directory":{
                "root":received.canonicalize().unwrap(),"total_bytes":descriptor.total_bytes,
                "files":descriptor.files.iter().map(|f| serde_json::json!({"path":f.path,"size":f.size})).collect::<Vec<_>>(),
            }}),
        );
        assert_eq!(
            hash,
            serde_json::json!({"status":"ok","data":{"cid":golden}})
        );
        let hash_elapsed = hash_started.elapsed();
        call(serde_json::json!({"op":"shutdown"}));
        let expected_calls = 5 + descriptor
            .files
            .iter()
            .map(|f| f.size.div_ceil(65536) as usize)
            .sum::<usize>();
        assert_eq!(calls, expected_calls);
        assert!(fixture
            .provider_exchange(&mut native, &provider_stderr, None)
            .is_empty());
        let status = loop {
            fixture.check_wait(&provider_stderr, &provider_stderr);
            if let Some(status) = native.0.try_wait().unwrap() {
                break status;
            }
            std::thread::yield_now();
        };
        assert!(status.success(), "native provider Shutdown exit");
        drop(native); // wait() observes the same reaped status; no live child retained.
        let backend_after_hash = disk_bytes(&fixture.repo);
        eprintln!(
            "seeded_local_cache_process_proof {}",
            serde_json::json!({
                "payload_bytes":16777216,"transferred_bytes":transferred,"calls":calls,
                "repo_mode":repo_mode,
                "largest_response_frame_bytes":largest_frame,"elapsed_ms":wire_started.elapsed().as_millis(),
                "transfer_and_digest_ms":transfer_elapsed.as_millis(),"hash_ms":hash_elapsed.as_millis(),
                "seed_logical_bytes":seed_disk.0,"seed_allocated_bytes":seed_disk.1,
                "staging_logical_bytes":staging_disk.0,"staging_allocated_bytes":staging_disk.1,
                "backend_logical_bytes_before":backend_before.0,"backend_allocated_bytes_before":backend_before.1,
                "backend_logical_bytes_after_transfer":backend_after_transfer.0,"backend_allocated_bytes_after_transfer":backend_after_transfer.1,
                "backend_logical_bytes_after_hash":backend_after_hash.0,"backend_allocated_bytes_after_hash":backend_after_hash.1,
                "test_process_peak_rss_bytes":peak_rss_bytes(libc::RUSAGE_SELF),
                "reaped_children_peak_rss_bytes":peak_rss_bytes(libc::RUSAGE_CHILDREN),
                "measurement_limits":"Disk samples use logical lengths and allocated blocks, not continuous peak. RSS high-water marks cover the test process and aggregate reaped children, not isolated provider/Kubo peaks; live Kubo is excluded. Seed fixture is an extra test-only source copy. Cache was seeded by normal import; no network/catalog/admission readiness claim.",
                "file_digests":digests,"provider_shutdown_reaped":true,"stdout_eof":true,
            })
        );

        let mut provider = IpfsProvider {
            state: KuboState::Cold,
            api_port: 0,
            gateway_port: 0,
            kubo_binary: Some(fixture.binary.clone()),
            kubo_child: None,
            data_dir: fixture.root.path().into(),
            repo_dir: fixture.repo.clone(),
        };
        // Exercise the production wire handler from Cold while the fixture owns
        // the real isolated daemon. Readiness must reuse that lifecycle, not Cat/pin.
        crate::write_coord_file(
            fixture.root.path(),
            &crate::CoordFile {
                kubo_pid: daemon.0.id(),
                api_port: port,
                gateway_port: 0,
                started_at: 1,
                last_used: 1,
            },
        );
        let ready =
            provider.handle(crate::parse_request(r#"{"op":"runtime_prepare_backend"}"#).unwrap());
        assert!(matches!(ready, crate::Response::Ok { data: None }));
        assert_eq!(provider.state, KuboState::Ready);
        assert!(provider.kubo_child.is_none(), "reuse fixture-owned Kubo");
        let wire = serde_json::json!({"op":"runtime_hash_staged_directory","directory":{
            "root":descriptor.root,
            "files":descriptor.files.iter().map(|f| serde_json::json!({"path":f.path,"size":f.size})).collect::<Vec<_>>(),
            "total_bytes":descriptor.total_bytes,
        }});
        let result = provider.handle(crate::parse_request(&wire.to_string()).unwrap());
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({"status":"ok","data":{"cid":golden}})
        );
        let compute = || {
            hash_directory_with_timeout(
                &provider,
                &descriptor,
                fixture.deadline.saturating_duration_since(Instant::now()),
            )
        };
        assert_eq!(compute().expect("compute staged root"), golden);
        let sorted = |value: String| {
            let mut lines: Vec<_> = value.lines().map(str::to_owned).collect();
            lines.sort();
            lines
        };
        let inventory_unchanged = || {
            assert_eq!(
                sorted(fixture.run(&["refs", "local"], fixture.root.path())),
                sorted(before_blocks.clone())
            );
            assert_eq!(
                sorted(fixture.run(&["pin", "ls", "--type=all"], fixture.root.path())),
                sorted(before_pins.clone())
            );
        };
        inventory_unchanged();
        // Same-size changed bytes must produce a different computed root, while
        // normal-add/pin mistakes would add new blocks and fail inventory parity.
        let weights = staged.join("weights.gguf");
        let old = fs::read(&weights).unwrap();
        let mut changed = old.clone();
        changed[262144] ^= 1;
        fs::write(&weights, changed).unwrap();
        let changed_root = compute().unwrap();
        assert_ne!(changed_root, golden);
        inventory_unchanged();
        fs::write(&weights, old).unwrap();
        assert_ne!(compute().unwrap(), changed_root, "wrong expected root");
        let index = staged.join("_elastos_object.json");
        let old_index = fs::read(&index).unwrap();
        let mut changed_index = old_index.clone();
        let i = changed_index.iter().position(|b| *b == b'f').unwrap();
        changed_index[i] = b'g';
        fs::write(&index, changed_index).unwrap();
        assert_ne!(compute().unwrap(), golden);
        inventory_unchanged();
        fs::write(&index, old_index).unwrap();
        let notice = staged.join("notices/source.txt");
        let original_notice = fs::read(&notice).unwrap();
        fs::remove_file(&notice).unwrap();
        assert!(compute().is_err(), "missing file accepted");
        inventory_unchanged();
        put(&notice, &original_notice);
        put(&staged.join("extra"), b"extra");
        assert!(compute().is_err(), "extra file accepted");
        inventory_unchanged();
        fs::remove_file(staged.join("extra")).unwrap();
        assert_eq!(compute().unwrap(), golden);
        inventory_unchanged();
        drop(daemon); // Reap before isolated repo/fixture removal, also on panic.
        let fixture_path = fixture.root.path().to_owned();
        fixture.root.close().expect("remove exact fixture root");
        assert!(!fixture_path.exists(), "temporary root cleanup");
        eprintln!("seeded_local_cache_process_proof cleanup_complete=true");
    }

    fn small_closure() -> (tempfile::TempDir, StagedDirectory) {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::set_permissions(dir.path().join("nested"), fs::Permissions::from_mode(0o700)).unwrap();
        let mut files = Vec::new();
        for name in [
            "_elastos_object.json",
            "capsule.json",
            "nested/weights.gguf",
        ] {
            put(&dir.path().join(name), b"{}");
            files.push(StagedFile {
                path: name.into(),
                size: 2,
            });
        }
        let descriptor = StagedDirectory {
            root: dir.path().canonicalize().unwrap(),
            files,
            total_bytes: 6,
        };
        (dir, descriptor)
    }

    fn ready_provider(root: &Path, port: u16) -> IpfsProvider {
        IpfsProvider {
            state: KuboState::Ready,
            api_port: port,
            gateway_port: 0,
            kubo_binary: None,
            kubo_child: None,
            data_dir: root.into(),
            repo_dir: root.join("unused-repo"),
        }
    }

    #[test]
    fn private_capacity_arithmetic_preserves_ten_percent_floor() {
        let exact = capacity_observation(7, 1000, 110, 10).unwrap();
        assert_eq!(exact.available_bytes - exact.required_bytes, 100);
        assert!(capacity_observation(7, 1000, 110, 11).is_err());
        assert!(capacity_observation(7, 1001, 110, 10).is_err());
        assert!(capacity_observation(7, 1001, 111, 10).is_ok());
        for (volume, total, available, required) in [
            (0, 1000, 110, 10),
            (7, 0, 0, 1),
            (7, 1000, 1001, 1),
            (7, 1000, 5, 10),
            (7, 1000, 110, 0),
            (7, u64::MAX, u64::MAX, u64::MAX),
        ] {
            assert!(capacity_observation(volume, total, available, required).is_err());
        }
        assert!(capacity_observation(
            7,
            u64::MAX,
            u64::MAX,
            super::super::MAX_CAPACITY_REQUIRED_BYTES
        )
        .is_ok());
        assert_eq!(capacity_bytes(10, 4096).unwrap(), 40960);
        assert!(capacity_bytes(10, 0).is_err());
        assert!(capacity_bytes(u128::MAX, 2).is_err());
        assert!(capacity_bytes(u128::from(u64::MAX), 2).is_err());
    }

    #[test]
    fn private_capacity_rejects_redirected_datastore_specs_and_volumes() {
        let valid = serde_json::json!({"Key":"Datastore.Spec","Value":capacity_datastore_spec()});
        validate_capacity_datastore(&valid).unwrap();
        for (pointer, value) in [
            ("/Key", serde_json::json!("Other.Spec")),
            ("/Value", serde_json::Value::Null),
            ("/Value/type", serde_json::json!("pebbleds")),
            ("/Value/mounts/0/path", serde_json::json!("/other/blocks")),
            (
                "/Value/mounts/0/path",
                serde_json::json!("alternate-blocks"),
            ),
            ("/Value/mounts/1/path", serde_json::json!("../datastore")),
            (
                "/Value/mounts/1/path",
                serde_json::json!("/other/datastore"),
            ),
            ("/Value/mounts/0/type", serde_json::json!("measure")),
            ("/Value/mounts/0/mountpoint", serde_json::json!("/")),
        ] {
            let mut invalid = valid.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(validate_capacity_datastore(&invalid).is_err());
        }
        let mut extra = valid;
        extra["Value"]["mounts"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"path":"extra"}));
        assert!(validate_capacity_datastore(&extra).is_err());
        require_same_capacity_volume(7, [7, 7]).unwrap();
        for (repo, stores) in [(7, [8, 7]), (7, [7, 8]), (0, [0, 0])] {
            assert!(require_same_capacity_volume(repo, stores).is_err());
        }
    }

    fn capacity_stat_fixture(root: &Path, body: Vec<u8>) -> io::Result<CapacityObservation> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let provider = ready_provider(root, listener.local_addr().unwrap().port());
        let server = std::thread::spawn(move || {
            for (operation, body) in [
                ("version", br#"{"Version":"0.40.1"}"#.to_vec()),
                (
                    "config?arg=Datastore.Spec",
                    serde_json::json!({"Key":"Datastore.Spec","Value":capacity_datastore_spec()})
                        .to_string()
                        .into_bytes(),
                ),
                ("repo/stat", body),
            ] {
                let mut socket = accept_fixture(&listener);
                let request = headers(&mut socket);
                assert!(request.starts_with(&format!("POST /api/v0/{operation} HTTP/1.1\r\n")));
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                // An oversized response may be refused from its headers before the body is sent.
                let _ = socket.write_all(&body);
            }
        });
        let result = check_capacity(&provider, 1);
        server.join().unwrap();
        assert!(!provider.repo_dir.exists());
        assert!(provider.kubo_child.is_none());
        result
    }

    #[test]
    fn private_capacity_uses_actual_repo_and_rejects_unsafe_or_malformed_facts() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("actual-repo");
        fs::create_dir(&repo).unwrap();
        fs::set_permissions(&repo, fs::Permissions::from_mode(0o700)).unwrap();
        let repo = repo.canonicalize().unwrap();
        for name in ["blocks", "datastore"] {
            fs::create_dir(repo.join(name)).unwrap();
            fs::set_permissions(repo.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let good = serde_json::json!({"RepoPath":repo,"RepoSize":0,"NumObjects":0,"StorageMax":1000,"Version":"16"});
        let observed = capacity_stat_fixture(root.path(), good.to_string().into_bytes()).unwrap();
        assert_eq!(observed.volume_id, fs::metadata(&repo).unwrap().dev());
        assert_eq!(observed.required_bytes, 1);
        assert!(serde_json::to_vec(&observed).unwrap().len() < 256);
        assert!(serde_json::to_value(&observed)
            .unwrap()
            .as_object()
            .unwrap()
            .values()
            .all(|v| v.as_u64().is_some()));
        for bad in [
            serde_json::json!({}),
            serde_json::json!({"RepoPath":repo,"RepoSize":"0"}),
            serde_json::json!({"RepoPath":repo,"RepoSize":-1}),
            serde_json::json!({"RepoPath":7,"RepoSize":0}),
            serde_json::json!({"RepoPath":"relative","RepoSize":0}),
            serde_json::json!({"RepoPath":repo.join("missing"),"RepoSize":0}),
            serde_json::json!({"RepoPath":format!("{}/../actual-repo",repo.display()),"RepoSize":0}),
        ] {
            assert!(capacity_stat_fixture(root.path(), bad.to_string().into_bytes()).is_err());
        }
        for body in [b"{".to_vec(), vec![b' '; MAX_RESPONSE as usize + 1]] {
            assert!(capacity_stat_fixture(root.path(), body).is_err());
        }
        fs::set_permissions(&repo, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(capacity_stat_fixture(root.path(), good.to_string().into_bytes()).is_ok());
        for mode in [0o777, 0o775, 0o757, 0o1700, 0o2700, 0o4700] {
            fs::set_permissions(&repo, fs::Permissions::from_mode(mode)).unwrap();
            assert!(capacity_stat_fixture(root.path(), good.to_string().into_bytes()).is_err());
        }
        fs::set_permissions(&repo, fs::Permissions::from_mode(0o700)).unwrap();
        let link = root.path().join("linked-repo");
        std::os::unix::fs::symlink(&repo, &link).unwrap();
        let file = root.path().join("file-repo");
        put(&file, b"not a directory");
        for path in [link, file] {
            let bad = serde_json::json!({"RepoPath":path,"RepoSize":0});
            assert!(capacity_stat_fixture(root.path(), bad.to_string().into_bytes()).is_err());
        }
        for name in ["blocks", "datastore"] {
            let directory = repo.join(name);
            let saved = repo.join("saved-directory");
            fs::rename(&directory, &saved).unwrap();
            std::os::unix::fs::symlink(&saved, &directory).unwrap();
            assert!(capacity_stat_fixture(root.path(), good.to_string().into_bytes()).is_err());
            fs::remove_file(&directory).unwrap();
            fs::rename(&saved, &directory).unwrap();
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap();
            assert!(capacity_stat_fixture(root.path(), good.to_string().into_bytes()).is_err());
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut provider = ready_provider(root.path(), 0);
        assert!(check_capacity(&provider, 1).is_err());
        provider.state = KuboState::Cold;
        assert!(check_capacity(&provider, 1).is_err());
        assert!(!provider.repo_dir.exists());
    }

    #[test]
    fn private_capacity_deadline_closes_repo_stat_socket() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let provider = ready_provider(root.path(), listener.local_addr().unwrap().port());
        let server = std::thread::spawn(move || {
            let mut version = accept_fixture(&listener);
            assert!(headers(&mut version).starts_with("POST /api/v0/version "));
            version.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{\"Version\":\"0.40.1\"}").unwrap();
            drop(version);
            let mut config = accept_fixture(&listener);
            assert!(headers(&mut config).starts_with("POST /api/v0/config?arg=Datastore.Spec "));
            let body =
                serde_json::json!({"Key":"Datastore.Spec","Value":capacity_datastore_spec()})
                    .to_string();
            write!(
                config,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            drop(config);
            let mut stat = accept_fixture(&listener);
            assert!(headers(&mut stat).starts_with("POST /api/v0/repo/stat "));
            stat.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{")
                .unwrap();
            stat.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
            let mut byte = [0];
            match stat.read(&mut byte) {
                Ok(0) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                    ) => {}
                other => panic!("capacity request did not settle: {other:?}"),
            }
        });
        assert!(check_capacity(&provider, 1).is_err());
        server.join().unwrap();
    }

    #[test]
    fn staged_directory_hash_rejects_descriptor_and_closure_mismatches() {
        let (dir, mut descriptor) = small_closure();
        assert!(Closure::open(&descriptor).is_ok());
        descriptor.total_bytes = 7;
        assert!(Closure::open(&descriptor).is_err());
        descriptor.total_bytes = 6;
        descriptor.files[2].size = MAX_BYTES + 1;
        assert!(Closure::open(&descriptor).is_err());
        descriptor.files[2].size = 2;
        for path in [
            "../weights",
            "/weights",
            "nested//weights",
            "nested/./weights",
            "nested/../weights",
            "bad\nname",
            "_elastos_object.json",
        ] {
            descriptor.files[2].path = path.into();
            assert!(Closure::open(&descriptor).is_err(), "accepted {path}");
        }
        descriptor.files[2].path = "nested/weights.gguf".into();
        put(&dir.path().join("extra"), b"{}");
        assert!(Closure::open(&descriptor).is_err());
        fs::remove_file(dir.path().join("extra")).unwrap();
        fs::create_dir(dir.path().join("empty")).unwrap();
        assert!(Closure::open(&descriptor).is_err());
        fs::remove_dir(dir.path().join("empty")).unwrap();
        fs::remove_file(dir.path().join("capsule.json")).unwrap();
        assert!(Closure::open(&descriptor).is_err());
        descriptor.files = (0..34)
            .map(|i| StagedFile {
                path: format!("f{i}"),
                size: 1,
            })
            .collect();
        descriptor.total_bytes = 34;
        assert!(Closure::open(&descriptor).is_err());
        let mut provider = ready_provider(dir.path(), 1);
        provider.state = KuboState::Cold;
        assert!(hash_directory(&provider, &descriptor).is_err());
        assert!(
            !provider.repo_dir.exists(),
            "hash started or configured Kubo"
        );
        assert!(!dir.path().join("ipfs-coord.json").exists());
        assert!(
            serde_json::from_value::<crate::Request>(serde_json::json!({
                "op": "hash_directory", "root": descriptor.root
            }))
            .is_err(),
            "private primitive became a generic provider operation"
        );
    }

    #[test]
    fn staged_directory_hash_rejects_symlink_hardlink_fifo_and_unsafe_modes() {
        use std::os::unix::fs::symlink;
        for case in [
            "file_link",
            "directory_link",
            "hardlink",
            "fifo",
            "file_mode",
            "directory_mode",
            "writable_directory",
            "root_mode",
        ] {
            let (dir, descriptor) = small_closure();
            let file = dir.path().join("nested/weights.gguf");
            match case {
                "file_link" => {
                    fs::remove_file(&file).unwrap();
                    symlink("../capsule.json", &file).unwrap();
                }
                "directory_link" => {
                    fs::rename(dir.path().join("nested"), dir.path().join("saved")).unwrap();
                    symlink("saved", dir.path().join("nested")).unwrap();
                }
                "hardlink" => {
                    fs::remove_file(&file).unwrap();
                    fs::hard_link(dir.path().join("capsule.json"), &file).unwrap();
                }
                "fifo" => {
                    fs::remove_file(&file).unwrap();
                    let name = CString::new(file.to_str().unwrap()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
                }
                "file_mode" => {
                    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap()
                }
                "directory_mode" => fs::set_permissions(
                    dir.path().join("nested"),
                    fs::Permissions::from_mode(0o755),
                )
                .unwrap(),
                "writable_directory" => fs::set_permissions(
                    dir.path().join("nested"),
                    fs::Permissions::from_mode(0o777),
                )
                .unwrap(),
                "root_mode" => {
                    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap()
                }
                _ => unreachable!(),
            }
            assert!(Closure::open(&descriptor).is_err(), "accepted {case}");
        }
        let (dir, mut descriptor) = small_closure();
        symlink(dir.path(), dir.path().join("alias")).unwrap();
        descriptor.root.push("alias");
        assert!(Closure::open(&descriptor).is_err(), "followed root symlink");
    }

    #[test]
    fn staged_directory_hash_detects_replacement_and_changes_during_stream() {
        let (dir, descriptor) = small_closure();
        let mut closure = Closure::open(&descriptor).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        {
            let mut reader = Multipart::new(&mut closure.files, deadline, new_boundary().unwrap());
            let mut byte = [0];
            assert_eq!(reader.read(&mut byte).unwrap(), 1);
            fs::write(dir.path().join("capsule.json"), b"changed length").unwrap();
            assert!(io::copy(&mut reader, &mut io::sink()).is_err());
        }
        assert!(closure.check(&descriptor).is_err());
        let (_dir, descriptor) = small_closure();
        let closure = Closure::open(&descriptor).unwrap();
        fs::rename(
            descriptor.root.join("capsule.json"),
            descriptor.root.join("old"),
        )
        .unwrap();
        put(&descriptor.root.join("capsule.json"), b"{}");
        assert!(
            closure.check(&descriptor).is_err(),
            "replacement inode accepted"
        );
        let (dir, descriptor) = small_closure();
        let closure = Closure::open(&descriptor).unwrap();
        let moved = dir.path().join("moved");
        // Replace a retained nested directory, not a fixture-owned temp root.
        fs::rename(dir.path().join("nested"), &moved).unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        assert!(closure.check(&descriptor).is_err());
    }

    #[test]
    fn staged_directory_hash_requires_pinned_profile_and_bounded_response() {
        let first = new_boundary().unwrap();
        let second = new_boundary().unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.len() <= 70);
        assert_ne!(first, second);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
        for key in [
            "Import.UnixFSHAMTDirectorySizeThreshold",
            "Import.UnixFSHAMTDirectorySizeEstimation",
        ] {
            assert!(check_hamt_value(key, &serde_json::Value::Null).is_ok());
            assert!(check_hamt_value(key, &serde_json::json!(false)).is_err());
            assert!(check_hamt_value(key, &serde_json::json!("disabled")).is_err());
        }
        assert!(check_hamt_value(
            "Import.UnixFSHAMTDirectorySizeThreshold",
            &serde_json::json!(262144)
        )
        .is_ok());
        assert!(check_hamt_value(
            "Import.UnixFSHAMTDirectorySizeThreshold",
            &serde_json::json!(1)
        )
        .is_err());
        let cid = "bafybeihgnsjhpoktqbyspaqv6moblyny3txs5nkjdxfx7wm346odxkhlrm";
        let line = format!("{{\"Name\":\"\",\"Hash\":\"{cid}\"}}\n");
        assert_eq!(parse_root(line.as_bytes()).unwrap(), cid);
        for bytes in [
            b"not json".to_vec(),
            b"{\"Name\":\"\"}".to_vec(),
            format!("{line}{line}").into_bytes(),
            vec![b'x'; 4097],
        ] {
            assert!(parse_root(&bytes).is_err());
        }
        let oversized = ureq::Response::new(200, "OK", &"x".repeat(65537)).unwrap();
        assert!(bounded_response(oversized).is_err());
        let redirect = ureq::Response::new(302, "Found", "").unwrap();
        assert!(bounded_response(redirect).is_err());
    }

    fn accept_fixture(listener: &std::net::TcpListener) -> std::net::TcpStream {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            match listener.accept() {
                Ok((socket, _)) => {
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(4)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(4)))
                        .unwrap();
                    return socket;
                }
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                other => panic!("fixture accept failed: {other:?}"),
            }
        }
    }

    fn headers(socket: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut byte = [0];
        while !bytes.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
            assert!(bytes.len() <= 8192);
        }
        String::from_utf8(bytes).unwrap()
    }

    fn mock_profile(listener: &std::net::TcpListener) {
        use std::io::Write;
        for (operation, value) in [
            ("version", serde_json::json!({"Version":"0.40.1"})),
            (
                "config",
                serde_json::json!({"Key":"Import.UnixFSHAMTDirectorySizeThreshold", "Value":null}),
            ),
            (
                "config",
                serde_json::json!({"Key":"Import.UnixFSHAMTDirectorySizeEstimation", "Value":null}),
            ),
        ] {
            let mut socket = accept_fixture(listener);
            let request = headers(&mut socket);
            assert!(request.starts_with(&format!("POST /api/v0/{operation}")));
            if operation == "config" {
                assert!(request.contains(value["Key"].as_str().unwrap()));
            }
            let body = value.to_string();
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    }

    #[test]
    fn staged_directory_hash_deadline_settles_stalled_upload_and_response() {
        use std::io::Write;
        for stall_upload in [false, true] {
            let (dir, mut descriptor) = small_closure();
            if stall_upload {
                // Larger than both TCP buffers; sparse bytes keep fixture I/O small.
                let file = OpenOptions::new()
                    .write(true)
                    .open(dir.path().join("nested/weights.gguf"))
                    .unwrap();
                file.set_len(16 * 1024 * 1024).unwrap();
                descriptor.files[2].size = 16 * 1024 * 1024;
                descriptor.total_bytes = 16 * 1024 * 1024 + 4;
            }
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let (returned, observe_return) = std::sync::mpsc::channel();
            let server = std::thread::spawn(move || {
                mock_profile(&listener);
                let mut socket = accept_fixture(&listener);
                let request = headers(&mut socket);
                assert!(request.starts_with("POST /api/v0/add?"));
                for (key, value) in ADD_OPTIONS {
                    assert!(request.contains(&format!("{key}={value}")));
                }
                let length: u64 = request
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .unwrap()
                    .parse()
                    .unwrap();
                if !stall_upload {
                    assert_eq!(
                        io::copy(&mut (&mut socket).take(length), &mut io::sink()).unwrap(),
                        length
                    );
                    // Deliver an unfinished response after the complete upload.
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 200\r\nConnection: close\r\n\r\n{",
                        )
                        .unwrap();
                }
                observe_return.recv_timeout(Duration::from_secs(5)).unwrap();
                // Provider return must close the actual transport. Draining an
                // upload prefix distinguishes a stalled send from response-only wait.
                let mut total = 0;
                let mut buffer = [0; 65536];
                loop {
                    match socket.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            total += n as u64;
                            assert!(total <= length);
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                            ) =>
                        {
                            break
                        }
                        other => panic!("native hash transport did not settle: {other:?}"),
                    }
                }
                if stall_upload {
                    assert!(total > 0 && total < length, "upload did not stall");
                }
            });
            let start = Instant::now();
            let result = hash_directory_with_timeout(
                &ready_provider(dir.path(), port),
                &descriptor,
                Duration::from_secs(1),
            );
            let elapsed = start.elapsed();
            returned.send(()).unwrap();
            server.join().unwrap();
            assert!(result.is_err());
            assert!(elapsed < Duration::from_secs(5));
            assert!(!dir.path().join("unused-repo").exists());
        }
    }
}
