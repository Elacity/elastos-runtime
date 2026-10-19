//! One host memory observation for Runtime model admission and execution.
use std::io::{self, Read as _};
use std::path::Path;

const MAX_OBSERVATION_BYTES: u64 = 1024 * 1024;
const MAX_MEMORY_TEXT_BYTES: usize = 64 * 1024;
const MAX_CGROUP_PATH_BYTES: usize = 4096;
const MAX_CGROUP_DEPTH: usize = 64;
const MAX_MOUNT_ENTRIES: usize = 4096;
const MAX_STAT_FIELDS: usize = 512;
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostMemory {
    /// Effective physical/cgroup capacity, rather than currently free memory.
    pub total: u64,
    pub available: u64,
    pub pressure_normal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelMemoryFit {
    Fits,
    Insufficient,
    Unavailable,
}

/// A signed minimum is an admission requirement; it does not promise that a
/// later reply's weights, KV cache and working memory fit the current free RAM.
pub fn model_memory_fit(minimum_memory_mb: u32, memory: Option<HostMemory>) -> ModelMemoryFit {
    if minimum_memory_mb == 0 {
        return ModelMemoryFit::Unavailable;
    }
    match memory {
        Some(memory) if memory.available <= memory.total => {
            if u64::from(minimum_memory_mb) * 1024 * 1024 <= memory.total {
                ModelMemoryFit::Fits
            } else {
                ModelMemoryFit::Insufficient
            }
        }
        _ => ModelMemoryFit::Unavailable,
    }
}

pub fn observe_host_memory(platform: &str) -> io::Result<HostMemory> {
    observe_host_memory_with_reader(platform, read_observation)
}

fn read_observation(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_OBSERVATION_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_OBSERVATION_BYTES {
        return Err(io::Error::other(
            "host memory observation exceeds its limit",
        ));
    }
    String::from_utf8(bytes).map_err(|_| io::Error::other("invalid host memory observation"))
}

/// Explicit platform and reader keep Linux observations testable on any host.
/// macOS uses its native host statistics; unsupported platforms refuse.
pub fn observe_host_memory_with_reader(
    platform: &str,
    read: impl Fn(&Path) -> io::Result<String>,
) -> io::Result<HostMemory> {
    match platform {
        "linux" => linux_host_memory(&read),
        "macos" => {
            #[cfg(target_os = "macos")]
            {
                macos_host_memory()
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(io::Error::other(
                    "macOS memory observation is unavailable on this host",
                ))
            }
        }
        _ => Err(io::Error::other("host memory observation is unsupported")),
    }
}

fn parse_meminfo(text: &str) -> io::Result<HostMemory> {
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
            .and_then(|kib| kib.checked_mul(1024))
            .ok_or_else(|| io::Error::other("invalid host memory observation"))
    };
    let total = field("MemTotal:")?;
    let available = field("MemAvailable:")?;
    if total == 0 || available > total {
        return Err(io::Error::other("invalid host memory observation"));
    }
    Ok(HostMemory {
        total,
        available,
        pressure_normal: false,
    })
}

fn unavailable() -> io::Error {
    io::Error::other("host memory observation unavailable")
}

fn bounded_read(
    read: &impl Fn(&Path) -> io::Result<String>,
    path: &Path,
    limit: usize,
) -> io::Result<String> {
    let text = read(path)?;
    if text.len() > limit {
        return Err(unavailable());
    }
    Ok(text)
}

fn require_absent(read: &impl Fn(&Path) -> io::Result<String>, path: &Path) -> io::Result<()> {
    match read(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        _ => Err(unavailable()),
    }
}

fn cgroup_membership(text: &str) -> io::Result<&str> {
    let mut lines = text.lines();
    let group = lines
        .next()
        .and_then(|line| line.strip_prefix("0::"))
        .ok_or_else(unavailable)?;
    if lines.next().is_some() || !group.starts_with('/') || group.len() > MAX_CGROUP_PATH_BYTES {
        return Err(unavailable());
    }
    if group == "/" {
        return Ok(group);
    }
    let mut depth = 0;
    for part in group[1..].split('/') {
        depth += 1;
        if depth > MAX_CGROUP_DEPTH
            || part.is_empty()
            || part == "."
            || part == ".."
            || part.len() > 255
            || part.chars().any(char::is_control)
        {
            return Err(unavailable());
        }
    }
    Ok(group)
}

/// Mount root `/` and the absence of the non-root-only `cgroup.type` prove
/// visible ancestry. A namespace or subtree mount can hide a tighter parent.
fn require_global_cgroup_mount(text: &str) -> io::Result<()> {
    let mut cgroup_mounts = 0;
    let mut entries = 0;
    let root = Path::new(CGROUP_ROOT);
    let mut ancestors = std::collections::BTreeMap::new();
    for line in text.lines() {
        entries += 1;
        if entries > MAX_MOUNT_ENTRIES {
            return Err(unavailable());
        }
        let fields: Vec<_> = line.split_whitespace().take(65).collect();
        if fields.len() > 64 {
            return Err(unavailable());
        }
        let split = fields
            .iter()
            .position(|field| *field == "-")
            .ok_or_else(unavailable)?;
        if split < 6 || fields.len() != split + 4 {
            return Err(unavailable());
        }
        let id = unsigned(fields[0])?;
        let parent = unsigned(fields[1])?;
        let (major, minor) = fields[2].split_once(':').ok_or_else(unavailable)?;
        unsigned(major)?;
        unsigned(minor)?;
        let mountpoint = mountinfo_path(fields[4])?;
        if fields[split + 1] == "cgroup2" {
            cgroup_mounts += 1;
            if mountinfo_path(fields[3])? != Path::new("/") || mountpoint != root {
                return Err(unavailable());
            }
        } else if mountpoint.starts_with(root) {
            // A listed underlying cgroup mount does not prove the files read
            // below it when another filesystem hides their mountpoints.
            return Err(unavailable());
        }
        if root.starts_with(&mountpoint) {
            // Only four possible paths: /, /sys, /sys/fs, and the cgroup root.
            if ancestors.len() >= 4
                || ancestors.values().any(|(known, _)| *known == id)
                || ancestors.insert(mountpoint, (id, parent)).is_some()
            {
                return Err(unavailable());
            }
        }
    }
    if cgroup_mounts != 1 {
        return Err(unavailable());
    }
    // A later mount at an ancestor can cover the listed cgroup mount. Every
    // relevant visible path must belong to its actual mount-ID parent chain.
    let mut path = root;
    let mut visited = 0;
    loop {
        let (_, parent) = ancestors.get(path).ok_or_else(unavailable)?;
        visited += 1;
        if path == Path::new("/") {
            break;
        }
        let (parent_path, _) = ancestors
            .iter()
            .find(|(_, (id, _))| id == parent)
            .ok_or_else(unavailable)?;
        if parent_path == path || !path.starts_with(parent_path) {
            return Err(unavailable());
        }
        path = parent_path;
    }
    if visited != ancestors.len() {
        return Err(unavailable());
    }
    Ok(())
}

fn mountinfo_path(field: &str) -> io::Result<std::path::PathBuf> {
    // proc escapes spaces, tabs, newlines and backslashes in pathname fields.
    let mut bytes = Vec::with_capacity(field.len());
    let input = field.as_bytes();
    let mut offset = 0;
    while offset < input.len() {
        if input[offset] == b'\\' {
            let escaped = input.get(offset + 1..offset + 4).ok_or_else(unavailable)?;
            bytes.push(match escaped {
                b"040" => b' ',
                b"011" => b'\t',
                b"012" => b'\n',
                b"134" => b'\\',
                _ => return Err(unavailable()),
            });
            offset += 4;
        } else {
            bytes.push(input[offset]);
            offset += 1;
        }
    }
    let path = std::path::PathBuf::from(String::from_utf8(bytes).map_err(|_| unavailable())?);
    if !path.is_absolute()
        || path.components().any(|part| {
            !matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(unavailable());
    }
    Ok(path)
}

fn unsigned(text: &str) -> io::Result<u64> {
    let text = text.trim();
    if text.is_empty() || text.len() > 20 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(unavailable());
    }
    text.parse().map_err(|_| unavailable())
}

fn linux_host_memory(read: &impl Fn(&Path) -> io::Result<String>) -> io::Result<HostMemory> {
    let mut memory = parse_meminfo(&bounded_read(
        read,
        Path::new("/proc/meminfo"),
        MAX_MEMORY_TEXT_BYTES,
    )?)?;
    let groups = bounded_read(read, Path::new("/proc/self/cgroup"), MAX_CGROUP_PATH_BYTES)?;
    let group = cgroup_membership(&groups)?;
    let mounts = bounded_read(
        read,
        Path::new("/proc/self/mountinfo"),
        MAX_OBSERVATION_BYTES as usize,
    )?;
    require_global_cgroup_mount(&mounts)?;
    let root = Path::new(CGROUP_ROOT);
    let controllers = bounded_read(read, &root.join("cgroup.controllers"), 4096)?;
    let mut names = std::collections::BTreeSet::new();
    for name in controllers.split_whitespace() {
        if name.len() > 64
            || names.len() >= 64
            || !names.insert(name)
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(unavailable());
        }
    }
    if !names.contains("memory") {
        return Err(unavailable());
    }
    // Linux exposes these files only on non-root cgroups. Missing is allowed
    // exactly here after proving the global mount and memory controller.
    for name in ["cgroup.type", "memory.max", "memory.current"] {
        require_absent(read, &root.join(name))?;
    }
    let mut path = root.join(&group[1..]);
    while path != root {
        let limit = bounded_read(read, &path.join("memory.max"), 64)?;
        let current = unsigned(&bounded_read(read, &path.join("memory.current"), 64)?)?;
        if limit.trim() != "max" {
            let limit = unsigned(&limit)?;
            let cache = bounded_read(read, &path.join("memory.stat"), MAX_MEMORY_TEXT_BYTES)
                .ok()
                .and_then(|stat| file_cache(&stat))
                .unwrap_or(0)
                .min(current);
            memory.total = memory.total.min(limit);
            memory.available = memory.available.min(limit.saturating_sub(current - cache));
        }
        if !path.pop() || !path.starts_with(root) {
            return Err(unavailable());
        }
    }
    // Moving between groups during observation invalidates the ancestor census.
    let current_groups = bounded_read(read, Path::new("/proc/self/cgroup"), MAX_CGROUP_PATH_BYTES)?;
    if cgroup_membership(&current_groups)? != group {
        return Err(unavailable());
    }
    if bounded_read(
        read,
        Path::new("/proc/self/mountinfo"),
        MAX_OBSERVATION_BYTES as usize,
    )? != mounts
    {
        return Err(unavailable());
    }
    Ok(memory)
}

/// File LRU pages are reclaimable; dirty/writeback pages and tmpfs stay used.
/// Any malformed statistic gives zero credit rather than inventing headroom.
fn file_cache(stat: &str) -> Option<u64> {
    let mut seen = std::collections::BTreeSet::new();
    let (mut active, mut inactive, mut dirty, mut writeback) = (None, None, None, None);
    for line in stat.lines() {
        let mut fields = line.split_whitespace();
        let key = fields.next()?;
        let value = unsigned(fields.next()?).ok()?;
        if fields.next().is_some()
            || key.len() > 64
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || seen.len() >= MAX_STAT_FIELDS
            || !seen.insert(key)
        {
            return None;
        }
        match key {
            "active_file" => active = Some(value),
            "inactive_file" => inactive = Some(value),
            "file_dirty" => dirty = Some(value),
            "file_writeback" => writeback = Some(value),
            _ => {}
        }
    }
    Some(
        active?
            .checked_add(inactive?)?
            .saturating_sub(dirty?.checked_add(writeback?)?),
    )
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn macos_host_memory() -> io::Result<HostMemory> {
    let fail = || io::Error::other("host memory observation unavailable");
    let mut total = 0_u64;
    let mut len = std::mem::size_of_val(&total);
    let mut stats = std::mem::MaybeUninit::<libc::vm_statistics64>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: host_statistics64 writes at most `count` words into `stats`;
    // the host port is released below.
    let host = unsafe { libc::mach_host_self() };
    let result = unsafe {
        libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            stats.as_mut_ptr().cast(),
            &mut count,
        )
    };
    unsafe extern "C" {
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }
    // SAFETY: `host` is a send right owned by this call.
    unsafe {
        mach_port_deallocate(libc::mach_task_self(), host);
    }
    // SAFETY: hw.memsize is a u64 and `len` is its size.
    let sysctl = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut total as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if result != 0 || sysctl != 0 || page <= 0 || total == 0 {
        return Err(fail());
    }
    // SAFETY: host_statistics64 succeeded.
    let stats = unsafe { stats.assume_init() };
    let available = macos_available(
        u64::from(stats.free_count),
        u64::from(stats.speculative_count),
        u64::from(stats.purgeable_count),
        u64::from(stats.external_page_count),
        page as u64,
    )
    .ok_or_else(fail)?;
    // kern.memorystatus_vm_pressure_level: 1 normal, 2 warning, 4 critical.
    // Unreadable counts as not normal.
    let mut level: libc::c_int = 0;
    let mut level_len = std::mem::size_of_val(&level);
    // SAFETY: the sysctl is an int and `level_len` is its size.
    let pressure = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_vm_pressure_level".as_ptr(),
            (&mut level as *mut libc::c_int).cast(),
            &mut level_len,
            std::ptr::null_mut(),
            0,
        )
    };
    Ok(HostMemory {
        total,
        available: available.min(total),
        pressure_normal: pressure == 0 && level == 1,
    })
}

/// Pages macOS can hand out without compressing or swapping: free pages
/// (whose count includes speculative read-ahead), purgeable pages, and
/// file-backed pages (speculative pages are file-backed, so counted once).
/// Inactive anonymous memory is excluded: reclaiming it needs the compressor.
#[cfg(any(target_os = "macos", test))]
fn macos_available(
    free: u64,
    speculative: u64,
    purgeable: u64,
    file_backed: u64,
    page: u64,
) -> Option<u64> {
    free.saturating_sub(speculative)
        .checked_add(purgeable)?
        .checked_add(file_backed)?
        .checked_mul(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    fn read_fixture(files: &BTreeMap<String, String>, path: &Path) -> io::Result<String> {
        files
            .get(path.to_str().unwrap())
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
    fn memory_files() -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "/proc/meminfo".into(),
                format!(
                    "MemTotal: {} kB\nMemFree: 1 kB\nMemAvailable: {} kB\n",
                    8 * GIB / 1024,
                    6 * GIB / 1024
                ),
            ),
            (
                "/proc/self/cgroup".into(),
                "0::/system.slice/home.service\n".into(),
            ),
            (
                "/proc/self/mountinfo".into(),
                "1 1 8:1 / / rw - ext4 /dev/root rw\n\
                 23 1 0:23 / /sys rw - sysfs sysfs rw\n\
                 29 23 0:26 / /sys/fs/cgroup rw,nosuid,nodev,noexec,relatime - cgroup2 cgroup rw\n"
                    .into(),
            ),
            (
                "/sys/fs/cgroup/cgroup.controllers".into(),
                "cpuset cpu io memory pids\n".into(),
            ),
            (
                "/sys/fs/cgroup/system.slice/memory.max".into(),
                "max\n".into(),
            ),
            (
                "/sys/fs/cgroup/system.slice/memory.current".into(),
                "0\n".into(),
            ),
            (
                "/sys/fs/cgroup/system.slice/home.service/memory.max".into(),
                "max\n".into(),
            ),
            (
                "/sys/fs/cgroup/system.slice/home.service/memory.current".into(),
                "0\n".into(),
            ),
        ])
    }
    #[test]
    fn meminfo_reports_mem_available_not_mem_free() {
        let host =
            parse_meminfo("MemTotal: 8000000 kB\nMemFree: 100000 kB\nMemAvailable: 3000000 kB\n")
                .unwrap();
        assert_eq!(
            host,
            HostMemory {
                total: 8_000_000 * 1024,
                available: 3_000_000 * 1024,
                pressure_normal: false
            }
        );
        for text in [
            "MemTotal: 8000000 kB\n",
            "MemTotal: 0 kB\nMemAvailable: 0 kB\n",
            "MemTotal: 1 kB\nMemAvailable: 2 kB\n",
        ] {
            assert!(parse_meminfo(text).is_err());
        }
    }
    #[test]
    fn cgroup_v2_ancestor_limit_lowers_available() {
        let mut files = memory_files();
        assert_eq!(
            observe_host_memory_with_reader("linux", |p| read_fixture(&files, p))
                .unwrap()
                .available,
            6 * GIB
        );
        files.insert(
            "/sys/fs/cgroup/system.slice/home.service/memory.max".into(),
            "max\n".into(),
        );
        files.insert(
            "/sys/fs/cgroup/system.slice/home.service/memory.current".into(),
            "100\n".into(),
        );
        files.insert(
            "/sys/fs/cgroup/system.slice/memory.max".into(),
            (2 * GIB).to_string(),
        );
        files.insert(
            "/sys/fs/cgroup/system.slice/memory.current".into(),
            (3 * GIB / 2).to_string(),
        );
        let host = observe_host_memory_with_reader("linux", |p| read_fixture(&files, p)).unwrap();
        assert_eq!(host.total, 2 * GIB);
        assert_eq!(host.available, GIB / 2);
        files.insert(
            "/proc/meminfo".into(),
            format!(
                "MemTotal: {} kB\nMemAvailable: {} kB\n",
                8 * GIB / 1024,
                MIB / 1024
            ),
        );
        assert_eq!(
            observe_host_memory_with_reader("linux", |p| read_fixture(&files, p))
                .unwrap()
                .available,
            MIB
        );
        files.insert(
            "/proc/meminfo".into(),
            format!(
                "MemTotal: {} kB\nMemAvailable: {} kB\n",
                8 * GIB / 1024,
                6 * GIB / 1024
            ),
        );
        assert_eq!(model_memory_fit(2048, Some(host)), ModelMemoryFit::Fits);
        assert_eq!(
            model_memory_fit(2049, Some(host)),
            ModelMemoryFit::Insufficient
        );
        files.insert("/proc/self/cgroup".into(), "4:memory:/home\n".into());
        assert!(observe_host_memory_with_reader("linux", |p| read_fixture(&files, p)).is_err());
    }
    #[test]
    fn cgroup_v2_page_cache_is_not_counted_as_used() {
        let mut files = memory_files();
        files.insert(
            "/sys/fs/cgroup/system.slice/memory.max".into(),
            (7680 * MIB).to_string(),
        );
        files.insert(
            "/sys/fs/cgroup/system.slice/memory.current".into(),
            (3395 * MIB).to_string(),
        );
        let observe = |files: &BTreeMap<String, String>| {
            observe_host_memory_with_reader("linux", |p| read_fixture(files, p))
                .unwrap()
                .available
        };
        assert_eq!(observe(&files), 4285 * MIB);
        // The file counter includes tmpfs; only the two file LRUs are credited.
        let stat = format!(
            "active_file {}\ninactive_file {}\nfile {}\nshmem {}\nfile_dirty 0\nfile_writeback 0\n",
            1953 * MIB,
            1321 * MIB,
            3338 * MIB,
            64 * MIB
        );
        files.insert(
            "/sys/fs/cgroup/system.slice/memory.stat".into(),
            stat.clone(),
        );
        assert_eq!(observe(&files), 6 * GIB);
        files.insert(
            "/proc/meminfo".into(),
            format!(
                "MemTotal: {} kB\nMemAvailable: {} kB\n",
                8 * GIB / 1024,
                8 * GIB / 1024
            ),
        );
        assert_eq!(observe(&files), (7680 - 121) * MIB);
        files.insert(
            "/sys/fs/cgroup/system.slice/memory.stat".into(),
            stat.replace("file_dirty 0", &format!("file_dirty {}", 200 * MIB))
                .replace("file_writeback 0", &format!("file_writeback {}", 56 * MIB)),
        );
        assert_eq!(observe(&files), (7680 - 377) * MIB);
        files.insert(
            "/sys/fs/cgroup/system.slice/memory.stat".into(),
            stat.replace("file_dirty 0", &format!("file_dirty {}", 4000 * MIB)),
        );
        assert_eq!(observe(&files), 4285 * MIB);
    }
    #[test]
    fn macos_estimate_excludes_inactive_anonymous_pages() {
        assert_eq!(macos_available(100, 20, 5, 300, 16384), Some(385 * 16384));
        assert_eq!(macos_available(10, 20, 0, 0, 16384), Some(0));
    }
    #[test]
    fn live_host_memory_is_readable() {
        // Fixed OS input covers the observation without depending on the test host.
        let files = memory_files();
        let host = observe_host_memory_with_reader("linux", |p| read_fixture(&files, p)).unwrap();
        assert!(host.total > 0 && host.available <= host.total);
        assert!(observe_host_memory_with_reader("unsupported", |_| panic!(
            "unsupported platform reads no files"
        ))
        .is_err());
        assert!(
            observe_host_memory_with_reader("linux", |_| Err(io::Error::from(
                io::ErrorKind::PermissionDenied
            )))
            .is_err()
        );
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_macos_memory_observation_is_readable() {
        let host = observe_host_memory("macos").unwrap();
        assert!(host.total > 0 && host.available <= host.total);
    }
    #[test]
    fn signed_minimum_checks_capacity_without_promising_free_memory() {
        let host = HostMemory {
            total: 4 * GIB,
            available: 0,
            pressure_normal: false,
        };
        assert_eq!(model_memory_fit(0, Some(host)), ModelMemoryFit::Unavailable);
        assert_eq!(model_memory_fit(4096, Some(host)), ModelMemoryFit::Fits);
        assert_eq!(
            model_memory_fit(4097, Some(host)),
            ModelMemoryFit::Insufficient
        );
        assert_eq!(model_memory_fit(512, None), ModelMemoryFit::Unavailable);
        assert_eq!(
            model_memory_fit(
                512,
                Some(HostMemory {
                    total: 0,
                    available: 0,
                    pressure_normal: false
                })
            ),
            ModelMemoryFit::Insufficient
        );
    }
    #[test]
    fn linux_memory_requires_a_proven_global_v2_root() {
        for (path, replacement) in [
            ("/proc/self/mountinfo", None),
            (
                "/proc/self/mountinfo",
                Some("29 23 0:26 /hidden /sys/fs/cgroup rw - cgroup2 cgroup rw\n"),
            ),
            (
                "/proc/self/mountinfo",
                Some("29 23 0:26 / /other rw - cgroup2 cgroup rw\n"),
            ),
            (
                "/proc/self/mountinfo",
                Some("invalid 23 0:26 / /sys/fs/cgroup rw - cgroup2 cgroup rw\n"),
            ),
            ("/sys/fs/cgroup/cgroup.controllers", None),
            ("/sys/fs/cgroup/cgroup.controllers", Some("cpu io pids\n")),
            ("/sys/fs/cgroup/cgroup.controllers", Some("memory memory\n")),
            (
                "/sys/fs/cgroup/cgroup.controllers",
                Some("memory invalid/controller\n"),
            ),
            ("/sys/fs/cgroup/cgroup.type", Some("domain\n")),
            ("/sys/fs/cgroup/cgroup.type", Some("")),
            ("/sys/fs/cgroup/memory.max", Some("max\n")),
            ("/sys/fs/cgroup/memory.current", Some("0\n")),
        ] {
            let mut files = memory_files();
            match replacement {
                Some(value) => {
                    files.insert(path.into(), value.into());
                }
                None => {
                    files.remove(path);
                }
            }
            assert!(
                observe_host_memory_with_reader("linux", |p| read_fixture(&files, p)).is_err(),
                "{path} {replacement:?}"
            );
        }
        let mut files = memory_files();
        let duplicate = files["/proc/self/mountinfo"].clone();
        files
            .get_mut("/proc/self/mountinfo")
            .unwrap()
            .push_str(&duplicate);
        assert!(observe_host_memory_with_reader("linux", |p| read_fixture(&files, p)).is_err());
        // A true global-root membership needs no non-root limit files.
        let mut files = memory_files();
        files.insert("/proc/self/cgroup".into(), "0::/\n".into());
        assert_eq!(
            observe_host_memory_with_reader("linux", |p| read_fixture(&files, p))
                .unwrap()
                .total,
            8 * GIB
        );
    }

    #[test]
    fn unreadable_root_and_required_ancestor_files_refuse() {
        let files = memory_files();
        for denied in [
            "/proc/self/cgroup",
            "/proc/self/mountinfo",
            "/sys/fs/cgroup/cgroup.controllers",
            "/sys/fs/cgroup/cgroup.type",
            "/sys/fs/cgroup/memory.max",
            "/sys/fs/cgroup/memory.current",
            "/sys/fs/cgroup/system.slice/memory.max",
            "/sys/fs/cgroup/system.slice/memory.current",
            "/sys/fs/cgroup/system.slice/home.service/memory.max",
            "/sys/fs/cgroup/system.slice/home.service/memory.current",
        ] {
            assert!(
                observe_host_memory_with_reader("linux", |p| {
                    if p == Path::new(denied) {
                        Err(io::Error::from(io::ErrorKind::PermissionDenied))
                    } else {
                        read_fixture(&files, p)
                    }
                })
                .is_err(),
                "{denied}"
            );
        }
        for key in [
            "/sys/fs/cgroup/system.slice/memory.max",
            "/sys/fs/cgroup/system.slice/memory.current",
            "/sys/fs/cgroup/system.slice/home.service/memory.max",
            "/sys/fs/cgroup/system.slice/home.service/memory.current",
        ] {
            for bad in [
                None,
                Some(""),
                Some("-1"),
                Some("18446744073709551616"),
                Some("1 2"),
                Some("invalid"),
            ] {
                let mut files = files.clone();
                match bad {
                    Some(value) => {
                        files.insert(key.into(), value.into());
                    }
                    None => {
                        files.remove(key);
                    }
                }
                assert!(
                    observe_host_memory_with_reader("linux", |p| read_fixture(&files, p)).is_err(),
                    "{key} {bad:?}"
                );
            }
        }
    }

    #[test]
    fn cgroup_overmounts_refuse_hidden_memory_files() {
        for mount in [
            "30 1 0:27 / /sys rw - tmpfs tmpfs rw\n",
            "30 23 0:27 / /sys/fs rw - tmpfs tmpfs rw\n",
            "30 29 0:27 / /sys/fs/cgroup rw - tmpfs tmpfs rw\n",
            "30 29 0:27 / /sys/fs/cgroup/system.slice rw - tmpfs tmpfs rw\n",
            "30 29 0:27 / /sys/fs/cgroup/cgroup.type rw - tmpfs tmpfs rw\n",
            "30 29 0:27 / /sys/fs/cgroup/hidden\\040limit rw - tmpfs tmpfs rw\n",
            "30 29 0:27 / /sys/fs/cgroup\\057system.slice rw - tmpfs tmpfs rw\n",
        ] {
            let mut files = memory_files();
            files
                .get_mut("/proc/self/mountinfo")
                .unwrap()
                .push_str(mount);
            assert!(
                observe_host_memory_with_reader("linux", |p| read_fixture(&files, p)).is_err(),
                "{mount:?}"
            );
        }
        let mut files = memory_files();
        files.get_mut("/proc/self/mountinfo").unwrap().push_str(
            "30 23 0:27 / /sys/fs/cgroup-other rw - tmpfs tmpfs rw\n\
             31 23 0:28 / /unrelated\\040directory rw - tmpfs tmpfs rw\n",
        );
        assert_eq!(
            observe_host_memory_with_reader("linux", |p| read_fixture(&files, p))
                .unwrap()
                .available,
            6 * GIB
        );
        // An ancestor mount installed before cgroup is valid when the actual
        // parent chain passes through it.
        let mut files = memory_files();
        files
            .get_mut("/proc/self/mountinfo")
            .unwrap()
            .push_str("24 23 0:27 / /sys/fs rw - tmpfs tmpfs rw\n");
        let mounts = files.get_mut("/proc/self/mountinfo").unwrap();
        *mounts = mounts.replace("29 23 0:26", "29 24 0:26");
        assert_eq!(
            observe_host_memory_with_reader("linux", |p| read_fixture(&files, p))
                .unwrap()
                .available,
            6 * GIB
        );
    }

    #[test]
    fn cgroup_membership_is_unique_bounded_and_stable() {
        for membership in [
            "",
            "4:memory:/home\n",
            "0::/home\n4:memory:/home\n",
            "0::/\n0::/\n",
            "0::relative\n",
            "0::/a/../b\n",
            "0::/a/./b\n",
            "0:://a\n",
            "0::/a/\n",
        ] {
            let mut files = memory_files();
            files.insert("/proc/self/cgroup".into(), membership.into());
            assert!(
                observe_host_memory_with_reader("linux", |p| read_fixture(&files, p)).is_err(),
                "{membership:?}"
            );
        }
        let mut files = memory_files();
        files.insert(
            "/proc/self/cgroup".into(),
            format!("0::/{}\n", vec!["a"; MAX_CGROUP_DEPTH + 1].join("/")),
        );
        assert!(observe_host_memory_with_reader("linux", |p| read_fixture(&files, p)).is_err());
        let files = memory_files();
        let reads = std::cell::Cell::new(0);
        assert!(observe_host_memory_with_reader("linux", |p| {
            if p == Path::new("/proc/self/cgroup") {
                reads.set(reads.get() + 1);
                if reads.get() > 1 {
                    return Ok("0::/\n".into());
                }
            }
            read_fixture(&files, p)
        })
        .is_err());
        assert_eq!(
            reads.get(),
            2,
            "membership is checked again after the ancestor read"
        );
        let reads = std::cell::Cell::new(0);
        assert!(observe_host_memory_with_reader("linux", |p| {
            if p == Path::new("/proc/self/mountinfo") {
                reads.set(reads.get() + 1);
                if reads.get() > 1 {
                    return Ok(format!(
                        "{}30 23 0:27 / /unrelated rw - tmpfs tmpfs rw\n",
                        files["/proc/self/mountinfo"]
                    ));
                }
            }
            read_fixture(&files, p)
        })
        .is_err());
        assert_eq!(reads.get(), 2, "mount ancestry is checked again");
    }

    #[test]
    fn observer_payload_and_mount_work_are_bounded() {
        for (path, limit) in [
            ("/proc/meminfo", MAX_MEMORY_TEXT_BYTES),
            ("/proc/self/cgroup", MAX_CGROUP_PATH_BYTES),
            ("/proc/self/mountinfo", MAX_OBSERVATION_BYTES as usize),
            ("/sys/fs/cgroup/cgroup.controllers", 4096),
            ("/sys/fs/cgroup/system.slice/memory.max", 64),
            ("/sys/fs/cgroup/system.slice/memory.current", 64),
        ] {
            let mut files = memory_files();
            files.insert(path.into(), "1".repeat(limit + 1));
            assert!(
                observe_host_memory_with_reader("linux", |p| read_fixture(&files, p)).is_err(),
                "{path}"
            );
        }
        let mut files = memory_files();
        let line = "29 23 0:26 / /elsewhere rw - tmpfs tmpfs rw\n";
        files
            .get_mut("/proc/self/mountinfo")
            .unwrap()
            .push_str(&line.repeat(MAX_MOUNT_ENTRIES));
        assert!(observe_host_memory_with_reader("linux", |p| read_fixture(&files, p)).is_err());
    }

    #[test]
    fn malformed_or_unreadable_page_cache_gives_zero_credit() {
        let mut files = memory_files();
        let base = "/sys/fs/cgroup/system.slice";
        files.insert(format!("{base}/memory.max"), (4 * GIB).to_string());
        files.insert(format!("{base}/memory.current"), (3 * GIB).to_string());
        let key = format!("{base}/memory.stat");
        for stat in [
            "active_file 1\ninactive_file 2\n",
            "active_file broken\ninactive_file 2\nfile_dirty 0\nfile_writeback 0\n",
            "active_file 1\nactive_file 2\ninactive_file 2\nfile_dirty 0\nfile_writeback 0\n",
            "active_file 18446744073709551615\ninactive_file 2\nfile_dirty 0\nfile_writeback 0\n",
        ] {
            files.insert(key.clone(), stat.into());
            assert_eq!(
                observe_host_memory_with_reader("linux", |p| read_fixture(&files, p))
                    .unwrap()
                    .available,
                GIB
            );
        }
        files.insert(key.clone(), "x".repeat(MAX_MEMORY_TEXT_BYTES + 1));
        assert_eq!(
            observe_host_memory_with_reader("linux", |p| read_fixture(&files, p))
                .unwrap()
                .available,
            GIB
        );
        assert_eq!(
            observe_host_memory_with_reader("linux", |p| {
                if p == Path::new(&key) {
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                } else {
                    read_fixture(&files, p)
                }
            })
            .unwrap()
            .available,
            GIB
        );
        // Even valid cache credit is capped by the observed current use.
        files.insert(
            key,
            format!(
                "active_file {}\ninactive_file 0\nfile_dirty 0\nfile_writeback 0\n",
                10 * GIB
            ),
        );
        assert_eq!(
            observe_host_memory_with_reader("linux", |p| read_fixture(&files, p))
                .unwrap()
                .available,
            4 * GIB
        );
    }
}
