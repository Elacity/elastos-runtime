//! Admission of a local GGUF engine against the host's free memory, checked
//! right before the engine starts.
use crate::local_llama::LocalLlamaFault;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

const MIB: u64 = 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * MIB;
// The engine is launched with these bounds; llama.cpp defaults are larger.
pub(crate) const BATCH_SIZE: u64 = 128;
pub(crate) const UBATCH_SIZE: u64 = 128;
const BASE_SCRATCH: u64 = 512 * MIB;
const MIN_HEADROOM: u64 = 1024 * MIB;

/// Host memory as seen right now: total RAM and memory the OS can hand out
/// without swapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HostMemory {
    pub total: u64,
    pub available: u64,
}

pub(crate) type HostMemoryReader = fn() -> Result<HostMemory, LocalLlamaFault>;

#[derive(Debug, PartialEq, Eq)]
struct MemoryPlan {
    weights: u64,
    kv_cache: u64,
    scratch: u64,
    headroom: u64,
}

impl MemoryPlan {
    fn required(&self) -> Option<u64> {
        self.weights
            .checked_add(self.kv_cache)?
            .checked_add(self.scratch)?
            .checked_add(self.headroom)
    }
}

/// Refuses with `MemoryUnavailable` unless free memory covers the model
/// weights, its KV cache at `context`, compute scratch, and a headroom of
/// max(10% of RAM, 1 GiB) left for the rest of the device.
pub(crate) fn admit(
    model: &Path,
    context: u32,
    parallel: u32,
    read_host: HostMemoryReader,
) -> Result<(), LocalLlamaFault> {
    let host = read_host()?;
    let file = std::fs::File::open(model).map_err(|_| LocalLlamaFault::Failed)?;
    let weights = file.metadata().map_err(|_| LocalLlamaFault::Failed)?.len();
    let plan = memory_plan(file, weights, context, parallel, host.total)?;
    match plan.required() {
        Some(required) if required <= host.available => Ok(()),
        _ => Err(LocalLlamaFault::MemoryUnavailable),
    }
}

fn memory_plan(
    reader: impl Read,
    weights: u64,
    context: u32,
    parallel: u32,
    total: u64,
) -> Result<MemoryPlan, LocalLlamaFault> {
    let fail = LocalLlamaFault::Failed;
    let Metadata {
        architecture,
        numbers,
        keys,
        vocabulary,
    } = metadata(reader).map_err(|_| fail)?;
    let number = |suffix: &str| {
        numbers
            .get(&format!("{architecture}.{suffix}"))
            .copied()
            .ok_or(fail)
    };
    let optional = |suffix: &str, default| {
        if keys.contains(&format!("{architecture}.{suffix}")) {
            number(suffix)
        } else {
            Ok(default)
        }
    };
    let layers = number("block_count")?;
    let embedding = number("embedding_length")?;
    let heads = number("attention.head_count")?;
    let kv_heads = optional("attention.head_count_kv", heads)?;
    let feed_forward = number("feed_forward_length")?;
    let vocabulary = vocabulary.ok_or(fail)?;
    if layers == 0
        || layers > 512
        || heads == 0
        || kv_heads == 0
        || kv_heads > heads
        || embedding == 0
        || embedding > 131_072
        || embedding % heads != 0
        || feed_forward == 0
        || feed_forward > 1_048_576
        || context == 0
        || parallel == 0
        || parallel > 64
    {
        return Err(fail);
    }
    let key = optional("attention.key_length", embedding / heads)?;
    let value = optional("attention.value_length", embedding / heads)?;
    if key == 0 || value == 0 || key > 131_072 || value > 131_072 {
        return Err(fail);
    }
    // f16 K/V cache over the total --ctx-size, padded per slot to 256 cells.
    let padded_context = u64::from(context)
        .div_ceil(u64::from(parallel))
        .div_ceil(256)
        .checked_mul(256)
        .and_then(|n| n.checked_mul(u64::from(parallel)))
        .ok_or(fail)?;
    let kv_cache = (key + value)
        .checked_mul(kv_heads)
        .and_then(|n| n.checked_mul(layers))
        .and_then(|n| n.checked_mul(padded_context))
        .and_then(|n| n.checked_mul(2))
        .ok_or(fail)?;
    // f32 attention scores, logits and per-layer activations for one ubatch.
    // Weights are counted once, in `weights`: Metal and mmap share the pages.
    let attention = padded_context
        .checked_mul(UBATCH_SIZE)
        .and_then(|n| n.checked_mul(heads))
        .and_then(|n| n.checked_mul(4 * 4))
        .ok_or(fail)?;
    let logits = vocabulary
        .checked_mul(BATCH_SIZE)
        .and_then(|n| n.checked_mul(4))
        .ok_or(fail)?;
    let activation = (heads * key.max(value))
        .max(embedding)
        .checked_add(feed_forward)
        .and_then(|n| n.checked_mul(UBATCH_SIZE))
        .and_then(|n| n.checked_mul(4 * 16))
        .ok_or(fail)?;
    let scratch = BASE_SCRATCH
        .checked_add(attention)
        .and_then(|n| n.checked_add(logits))
        .and_then(|n| n.checked_add(activation))
        .ok_or(fail)?;
    Ok(MemoryPlan {
        weights,
        kv_cache,
        scratch,
        headroom: (total / 10).max(MIN_HEADROOM),
    })
}

// GGUF v2/v3 metadata layout: https://github.com/ggml-org/ggml/blob/master/docs/gguf.md
// Only bounded metadata is read; tensor payloads stay on disk.
struct Metadata {
    architecture: String,
    numbers: BTreeMap<String, u64>,
    keys: BTreeSet<String>,
    vocabulary: Option<u64>,
}

fn read_u32(reader: &mut impl Read) -> std::io::Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> std::io::Result<u64> {
    let mut bytes = [0; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn invalid() -> std::io::Error {
    std::io::ErrorKind::InvalidData.into()
}

fn string(reader: &mut impl Read, max: u64) -> std::io::Result<String> {
    let len = read_u64(reader)?;
    if len > max {
        return Err(invalid());
    }
    let mut bytes = vec![0; len as usize];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|_| invalid())
}

fn skip(reader: &mut impl Read, bytes: u64) -> std::io::Result<()> {
    if bytes > MAX_METADATA_BYTES
        || std::io::copy(&mut reader.take(bytes), &mut std::io::sink())? != bytes
    {
        return Err(invalid());
    }
    Ok(())
}

fn skip_value(reader: &mut impl Read, kind: u32, in_array: bool) -> std::io::Result<()> {
    match kind {
        0 | 1 | 7 => skip(reader, 1),
        2 | 3 => skip(reader, 2),
        4..=6 => skip(reader, 4),
        10..=12 => skip(reader, 8),
        8 => {
            let len = read_u64(reader)?;
            skip(reader, len)
        }
        9 if !in_array => {
            let kind = read_u32(reader)?;
            let count = read_u64(reader)?;
            if count > 1_048_576 {
                return Err(invalid());
            }
            for _ in 0..count {
                skip_value(reader, kind, true)?;
            }
            Ok(())
        }
        _ => Err(invalid()),
    }
}

fn metadata(reader: impl Read) -> std::io::Result<Metadata> {
    let mut reader = reader.take(MAX_METADATA_BYTES);
    let mut magic = [0; 4];
    reader.read_exact(&mut magic)?;
    if &magic != b"GGUF" || !matches!(read_u32(&mut reader)?, 2 | 3) {
        return Err(invalid());
    }
    let _tensors = read_u64(&mut reader)?;
    let count = read_u64(&mut reader)?;
    if count > 4096 {
        return Err(invalid());
    }
    let mut architecture = None;
    let mut numbers = BTreeMap::new();
    let mut keys = BTreeSet::new();
    let mut vocabulary = None;
    for _ in 0..count {
        let key = string(&mut reader, 512)?;
        if !keys.insert(key.clone()) {
            return Err(invalid());
        }
        let kind = read_u32(&mut reader)?;
        match (key.as_str(), kind) {
            ("general.architecture", 8) => architecture = Some(string(&mut reader, 64)?),
            ("general.architecture", _) => return Err(invalid()),
            ("tokenizer.ggml.tokens", 9) => {
                if read_u32(&mut reader)? != 8 {
                    return Err(invalid());
                }
                let count = read_u64(&mut reader)?;
                if count == 0 || count > 1_048_576 {
                    return Err(invalid());
                }
                for _ in 0..count {
                    skip_value(&mut reader, 8, true)?;
                }
                vocabulary = Some(count);
            }
            ("tokenizer.ggml.tokens", _) => return Err(invalid()),
            (_, 0) => {
                let mut value = [0; 1];
                reader.read_exact(&mut value)?;
                numbers.insert(key, u64::from(value[0]));
            }
            (_, 2) => {
                let mut value = [0; 2];
                reader.read_exact(&mut value)?;
                numbers.insert(key, u64::from(u16::from_le_bytes(value)));
            }
            (_, 4) => {
                numbers.insert(key, u64::from(read_u32(&mut reader)?));
            }
            (_, 10) => {
                numbers.insert(key, read_u64(&mut reader)?);
            }
            _ => skip_value(&mut reader, kind, false)?,
        }
    }
    Ok(Metadata {
        architecture: architecture.ok_or_else(invalid)?,
        numbers,
        keys,
        vocabulary,
    })
}

/// Reads the live host memory. Linux: `MemAvailable`, lowered by any cgroup v2
/// ancestor limit. macOS: free, purgeable and file-backed pages only.
pub(crate) fn host_memory() -> Result<HostMemory, LocalLlamaFault> {
    // Debug builds only, for the process tests that run this binary: a fixed
    // reading keeps them independent of the host's load. Release ignores it.
    #[cfg(debug_assertions)]
    if let Some(bytes) = std::env::var("ELASTOS_MODEL_PROVIDER_TEST_FREE_MEMORY")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        return Ok(HostMemory {
            total: bytes,
            available: bytes,
        });
    }
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").map_err(|_| LocalLlamaFault::Failed)?;
        let host = parse_meminfo(&text)?;
        // cgroup v1 hosts have no `0::` line; they keep plain MemAvailable.
        let available = match std::fs::read_to_string("/proc/self/cgroup") {
            Ok(groups) => cgroup_available(
                &groups,
                Path::new("/sys/fs/cgroup"),
                host.available,
                |path| std::fs::read_to_string(path).ok(),
            ),
            Err(_) => host.available,
        };
        Ok(HostMemory {
            total: host.total,
            available,
        })
    }
    #[cfg(target_os = "macos")]
    {
        macos_host_memory()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(LocalLlamaFault::Failed)
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_meminfo(text: &str) -> Result<HostMemory, LocalLlamaFault> {
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
            .and_then(|kib| kib.checked_mul(1024))
            .ok_or(LocalLlamaFault::Failed)
    };
    let total = field("MemTotal:")?;
    let available = field("MemAvailable:")?;
    if total == 0 || available > total {
        return Err(LocalLlamaFault::Failed);
    }
    Ok(HostMemory { total, available })
}

/// Lowers `available` to the tightest `memory.max - memory.current` among the
/// process's cgroup v2 ancestors. A missing or unreadable level has no limit.
#[cfg(any(target_os = "linux", test))]
fn cgroup_available(
    groups: &str,
    root: &Path,
    mut available: u64,
    read: impl Fn(&Path) -> Option<String>,
) -> u64 {
    let Some(group) = groups.lines().find_map(|line| line.strip_prefix("0::")) else {
        return available;
    };
    let mut path = root.to_path_buf();
    for part in Path::new(group).components() {
        if let std::path::Component::Normal(name) = part {
            path.push(name);
        }
    }
    while path.starts_with(root) {
        let number = |file: &str| read(&path.join(file))?.trim().parse::<u64>().ok();
        if let (Some(limit), Some(used)) = (number("memory.max"), number("memory.current")) {
            available = available.min(limit.saturating_sub(used));
        }
        if path == root || !path.pop() {
            break;
        }
    }
    available
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn macos_host_memory() -> Result<HostMemory, LocalLlamaFault> {
    let fail = LocalLlamaFault::Failed;
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
        return Err(fail);
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
    .ok_or(fail)?;
    Ok(HostMemory {
        total,
        available: available.min(total),
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

/// Ample fixed memory, so test engines never depend on the host's load.
#[cfg(test)]
pub(crate) fn test_host_memory() -> Result<HostMemory, LocalLlamaFault> {
    Ok(HostMemory {
        total: 8 * 1024 * MIB,
        available: 6 * 1024 * MIB,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fake_gguf;

    const GIB: u64 = 1024 * MIB;

    #[test]
    fn plan_counts_weights_once_plus_kv_scratch_and_headroom() {
        // Qwen2.5-1.5B-Instruct geometry: 28 layers, 1536 wide, 12/2 heads.
        let gguf = fake_gguf(28, 1536, 12, 2, 8960, 1000);
        let weights = 1_117_320_736;
        let plan = memory_plan(gguf.as_slice(), weights, 4096, 1, 16 * GIB).unwrap();
        assert_eq!(plan.weights, weights);
        assert_eq!(plan.kv_cache, 112 * MIB);
        assert!(plan.scratch >= BASE_SCRATCH && plan.scratch < weights);
        assert_eq!(plan.headroom, 16 * GIB / 10);
        // Small RAM still keeps the 1 GiB floor.
        let small = memory_plan(gguf.as_slice(), weights, 4096, 1, 4 * GIB).unwrap();
        assert_eq!(small.headroom, GIB);
        // A longer context needs more.
        let long = memory_plan(gguf.as_slice(), weights, 8192, 1, 16 * GIB).unwrap();
        assert_eq!(long.kv_cache, 2 * plan.kv_cache);
        assert!(long.required() > plan.required());
    }

    #[test]
    fn malformed_metadata_is_not_admitted() {
        for gguf in [
            fake_gguf(0, 1536, 12, 2, 8960, 10),
            fake_gguf(28, 1536, 12, 13, 8960, 10),
            fake_gguf(28, 1537, 12, 2, 8960, 10),
            b"GGUF".to_vec(),
            b"healthy".to_vec(),
        ] {
            assert_eq!(
                memory_plan(gguf.as_slice(), MIB, 4096, 1, 16 * GIB),
                Err(LocalLlamaFault::Failed)
            );
        }
    }

    #[test]
    fn meminfo_reports_mem_available_not_mem_free() {
        let text = "MemTotal:        8000000 kB\nMemFree:          100000 kB\nMemAvailable:    3000000 kB\n";
        assert_eq!(
            parse_meminfo(text),
            Ok(HostMemory {
                total: 8_000_000 * 1024,
                available: 3_000_000 * 1024
            })
        );
        assert_eq!(
            parse_meminfo("MemTotal: 8000000 kB\n"),
            Err(LocalLlamaFault::Failed)
        );
    }

    #[test]
    fn macos_estimate_excludes_inactive_anonymous_pages() {
        // vm_statistics64 fixture: 100 free (20 of them speculative), 5
        // purgeable, 300 file-backed; inactive anonymous pages are not an input.
        assert_eq!(macos_available(100, 20, 5, 300, 16384), Some(385 * 16384));
        assert_eq!(macos_available(10, 20, 0, 0, 16384), Some(0));
    }

    #[test]
    fn cgroup_v2_ancestor_limit_lowers_available() {
        let root = crate::test_support::temp_root_path("model-provider-memory", "cgroup");
        let leaf = root.join("system.slice/home.service");
        std::fs::create_dir_all(&leaf).unwrap();
        let read = |path: &Path| std::fs::read_to_string(path).ok();
        let groups = "0::/system.slice/home.service\n";
        // No limit files anywhere: MemAvailable stands.
        assert_eq!(cgroup_available(groups, &root, 6 * GIB, read), 6 * GIB);
        // The leaf is unlimited; its parent caps at 2 GiB with 1.5 GiB used.
        std::fs::write(leaf.join("memory.max"), "max\n").unwrap();
        std::fs::write(leaf.join("memory.current"), "100\n").unwrap();
        let parent = root.join("system.slice");
        std::fs::write(parent.join("memory.max"), format!("{}\n", 2 * GIB)).unwrap();
        std::fs::write(parent.join("memory.current"), format!("{}\n", 3 * GIB / 2)).unwrap();
        assert_eq!(cgroup_available(groups, &root, 6 * GIB, read), GIB / 2);
        // A tighter MemAvailable still wins; cgroup v1 (no 0:: line) is unchanged.
        assert_eq!(cgroup_available(groups, &root, MIB, read), MIB);
        assert_eq!(
            cgroup_available("4:memory:/home\n", &root, 6 * GIB, read),
            6 * GIB
        );
    }

    #[test]
    fn live_host_memory_is_readable() {
        let host = host_memory().unwrap();
        assert!(host.total > 0 && host.available <= host.total);
    }
}
