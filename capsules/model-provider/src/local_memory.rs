//! Conservative admission for the local GGUF text profiles before engine spawn.
use crate::local_llama::LocalLlamaFault;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Take};
use std::path::Path;

const MIB: u64 = 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * MIB;
// Launch and admission share these bounds; llama.cpp defaults are larger.
pub(crate) const BATCH_SIZE: u64 = 128;
pub(crate) const UBATCH_SIZE: u64 = 128;

#[derive(Debug, PartialEq, Eq)]
struct MemoryPlan {
    weights: u64,
    context: u64,
    scratch: u64,
    home: u64,
}

impl MemoryPlan {
    fn total(&self) -> Result<u64, LocalLlamaFault> {
        self.weights
            .checked_add(self.context)
            .and_then(|n| n.checked_add(self.scratch))
            .and_then(|n| n.checked_add(self.home))
            .ok_or(LocalLlamaFault::ResourcesUnavailable)
    }

    fn admit_available(&self, available: u64) -> Result<(), LocalLlamaFault> {
        if self.total()? > available {
            Err(LocalLlamaFault::MemoryUnavailable)
        } else {
            Ok(())
        }
    }
}

pub(crate) fn admit(
    model: &Path,
    context: u32,
    parallel: u32,
    home: &Path,
) -> Result<(), LocalLlamaFault> {
    let (total, available) = host_memory()?;
    let file = File::open(model).map_err(|_| LocalLlamaFault::ResourcesUnavailable)?;
    let weights = file
        .metadata()
        .map_err(|_| LocalLlamaFault::ResourcesUnavailable)?
        .len();
    let plan = memory_plan(file, weights, context, parallel, total)?;
    plan.admit_available(available)?;
    disk_reserve(model)?;
    disk_reserve(home)
}

fn memory_plan(
    reader: impl Read,
    weights: u64,
    context: u32,
    parallel: u32,
    total: u64,
) -> Result<MemoryPlan, LocalLlamaFault> {
    let fail = LocalLlamaFault::ResourcesUnavailable;
    let Metadata {
        architecture,
        numbers,
        keys,
        vocabulary,
    } = metadata(reader).map_err(|_| fail)?;
    // These profiles use the ordinary attention KV layout. A different layout
    // needs its own measured profile before Runtime can admit it.
    if !matches!(architecture.as_str(), "llama" | "qwen2" | "qwen3") {
        return Err(fail);
    }
    if keys.contains("split.count") && numbers.get("split.count") != Some(&1) {
        return Err(fail);
    }
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
        || vocabulary == 0
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
    // The engine uses its default f16 K/V cache. context_size is the total
    // engine context across its parallel slots, as passed to --ctx-size.
    let padded_context = u64::from(context)
        .div_ceil(u64::from(parallel))
        .div_ceil(256)
        .checked_mul(256)
        .and_then(|n| n.checked_mul(u64::from(parallel)))
        .ok_or(fail)?;
    let context = key
        .checked_add(value)
        .and_then(|n| n.checked_mul(kv_heads))
        .and_then(|n| n.checked_mul(layers))
        .and_then(|n| n.checked_mul(padded_context))
        .and_then(|n| n.checked_mul(2))
        .ok_or(fail)?;
    // Cover non-flash f32 attention scores and softmax intermediates, logits,
    // and the layer's projected/FFN activation working sets. These scale with
    // selected context and the pinned batch sizes, even for small weights.
    let attention = padded_context
        .checked_mul(UBATCH_SIZE)
        .and_then(|n| n.checked_mul(heads))
        .and_then(|n| n.checked_mul(4 * 4))
        .ok_or(fail)?;
    let logits = vocabulary
        .checked_mul(BATCH_SIZE)
        .and_then(|n| n.checked_mul(4))
        .ok_or(fail)?;
    let activation = heads
        .checked_mul(key.max(value))
        .map(|n| n.max(embedding))
        .and_then(|n| n.checked_add(feed_forward))
        .and_then(|n| n.checked_mul(UBATCH_SIZE))
        .and_then(|n| n.checked_mul(4 * 16))
        .ok_or(fail)?;
    let scratch = weights
        .max(512 * MIB)
        .checked_add(attention)
        .and_then(|n| n.checked_add(logits))
        .and_then(|n| n.checked_add(activation))
        .ok_or(fail)?;
    Ok(MemoryPlan {
        weights,
        context,
        scratch,
        home: (total / 10).max(1024 * MIB),
    })
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
    if bytes > MAX_METADATA_BYTES {
        return Err(invalid());
    }
    if std::io::copy(&mut reader.take(bytes), &mut std::io::sink())? != bytes {
        return Err(invalid());
    }
    Ok(())
}
fn skip_value(reader: &mut Take<impl Read>, kind: u32, array: bool) -> std::io::Result<()> {
    match kind {
        0 | 1 | 7 => skip(reader, 1),
        2 | 3 => skip(reader, 2),
        4..=6 => skip(reader, 4),
        10..=12 => skip(reader, 8),
        8 => {
            let len = read_u64(reader)?;
            skip(reader, len)
        }
        9 if !array => {
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

// GGUF v2/v3 metadata layout: https://github.com/ggml-org/ggml/blob/master/docs/gguf.md
// Read only bounded metadata; tensor payloads remain on disk.
struct Metadata {
    architecture: String,
    numbers: BTreeMap<String, u64>,
    keys: std::collections::BTreeSet<String>,
    vocabulary: Option<u64>,
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
    let mut keys = std::collections::BTreeSet::new();
    let mut vocabulary = None;
    for _ in 0..count {
        let key = string(&mut reader, 512)?;
        if !keys.insert(key.clone()) {
            return Err(invalid());
        }
        let kind = read_u32(&mut reader)?;
        if key == "general.architecture" {
            if kind != 8 {
                return Err(invalid());
            }
            architecture = Some(string(&mut reader, 64)?);
        } else if key == "tokenizer.ggml.tokens" {
            if kind != 9 || read_u32(&mut reader)? != 8 {
                return Err(invalid());
            }
            let count = read_u64(&mut reader)?;
            if count > 1_048_576 {
                return Err(invalid());
            }
            for _ in 0..count {
                skip_value(&mut reader, 8, true)?;
            }
            vocabulary = Some(count);
        } else if kind == 0 {
            let mut value = [0; 1];
            reader.read_exact(&mut value)?;
            numbers.insert(key, u64::from(value[0]));
        } else if kind == 2 {
            let mut value = [0; 2];
            reader.read_exact(&mut value)?;
            numbers.insert(key, u64::from(u16::from_le_bytes(value)));
        } else if kind == 4 {
            numbers.insert(key, u64::from(read_u32(&mut reader)?));
        } else if kind == 10 {
            numbers.insert(key, read_u64(&mut reader)?);
        } else {
            skip_value(&mut reader, kind, false)?;
        }
    }
    Ok(Metadata {
        architecture: architecture.ok_or_else(invalid)?,
        numbers,
        keys,
        vocabulary,
    })
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn host_memory() -> Result<(u64, u64), LocalLlamaFault> {
    let fail = LocalLlamaFault::ResourcesUnavailable;
    let mut total = 0_u64;
    let mut len = std::mem::size_of_val(&total);
    let mut stats = std::mem::MaybeUninit::<libc::vm_statistics64>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
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
    unsafe {
        mach_port_deallocate(libc::mach_task_self(), host);
    }
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
    let stats = unsafe { stats.assume_init() };
    let available = (u64::from(stats.free_count) + u64::from(stats.inactive_count))
        .checked_mul(page as u64)
        .ok_or(fail)?;
    Ok((total, available.min(total)))
}

#[cfg(target_os = "linux")]
fn host_memory() -> Result<(u64, u64), LocalLlamaFault> {
    let fail = LocalLlamaFault::ResourcesUnavailable;
    let text = std::fs::read_to_string("/proc/meminfo").map_err(|_| fail)?;
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
            .and_then(|n| n.checked_mul(1024))
            .ok_or(fail)
    };
    cgroup_memory(
        std::fs::read_to_string("/proc/self/cgroup"),
        Path::new("/sys/fs/cgroup"),
        field("MemTotal:")?,
        field("MemAvailable:")?,
        |path| std::fs::read_to_string(path),
    )
}

#[cfg(any(target_os = "linux", test))]
fn cgroup_memory(
    groups: std::io::Result<String>,
    root: &Path,
    mut total: u64,
    mut available: u64,
    read: impl Fn(&Path) -> std::io::Result<String>,
) -> Result<(u64, u64), LocalLlamaFault> {
    let fail = LocalLlamaFault::ResourcesUnavailable;
    let groups = groups.map_err(|_| fail)?;
    if groups.lines().any(|line| {
        line.split(':')
            .nth(1)
            .is_some_and(|controllers| controllers.split(',').any(|name| name == "memory"))
    }) {
        // This Linux profile requires unified cgroup v2 memory accounting.
        return Err(fail);
    }
    let mut unified = groups.lines().filter_map(|line| line.strip_prefix("0::"));
    let group = unified.next().ok_or(fail)?;
    if unified.next().is_some()
        || !group.starts_with('/')
        || Path::new(group).components().any(|part| {
            !matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(fail);
    }
    let mut path = root.join(group.trim_start_matches('/'));
    loop {
        // A missing controller file is meaningful only within a visible unified
        // hierarchy. Hidden paths, failed reads and v1 limits fail admission.
        if !path.is_dir() {
            return Err(fail);
        }
        let controllers = read(&path.join("cgroup.controllers")).map_err(|_| fail)?;
        if path == root {
            // A cgroup namespace can hide stricter ancestors. Only the global
            // memory-controller root (which has no memory.max) proves that this
            // walk observed the entire enforced hierarchy.
            if !controllers.split_whitespace().any(|name| name == "memory")
                || !matches!(read(&path.join("memory.max")), Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound)
            {
                return Err(fail);
            }
            break;
        }
        match read(&path.join("memory.max")) {
            Ok(limit) if limit.trim() == "max" => {}
            Ok(limit) => {
                let limit = limit.trim().parse::<u64>().map_err(|_| fail)?;
                let used = read(&path.join("memory.current"))
                    .map_err(|_| fail)?
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| fail)?;
                total = total.min(limit);
                available = available.min(limit.saturating_sub(used));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let enabled = read(&path.parent().ok_or(fail)?.join("cgroup.subtree_control"))
                    .map_err(|_| fail)?;
                if enabled.split_whitespace().any(|name| name == "memory")
                    || controllers.split_whitespace().any(|name| name == "memory")
                {
                    return Err(fail);
                }
                // Both views must show that memory accounting is disabled.
            }
            Err(_) => return Err(fail),
        }
        if !path.pop() || !path.starts_with(root) {
            return Err(fail);
        }
    }
    if total == 0 || available > total {
        return Err(fail);
    }
    Ok((total, available))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn host_memory() -> Result<(u64, u64), LocalLlamaFault> {
    Err(LocalLlamaFault::ResourcesUnavailable)
}

#[cfg(unix)]
fn disk_reserve(path: &Path) -> Result<(), LocalLlamaFault> {
    use std::os::unix::io::AsRawFd;
    let fail = LocalLlamaFault::ResourcesUnavailable;
    let file = File::open(path).map_err(|_| fail)?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::fstatvfs(file.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(fail);
    }
    let stats = unsafe { stats.assume_init() };
    let capacity = u128::from(stats.f_blocks) * u128::from(stats.f_frsize);
    let available = u128::from(stats.f_bavail) * u128::from(stats.f_frsize);
    if capacity == 0 || available > capacity {
        return Err(fail);
    }
    // Leave room for bounded run journal updates as well as the volume floor.
    if available.saturating_sub(u128::from(8 * MIB)) * 10 < capacity {
        return Err(LocalLlamaFault::DiskUnavailable);
    }
    Ok(())
}

#[cfg(not(unix))]
fn disk_reserve(_path: &Path) -> Result<(), LocalLlamaFault> {
    Err(LocalLlamaFault::ResourcesUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fake_gguf_metadata;

    #[test]
    fn admission_counts_weights_total_context_scratch_and_home() {
        let metadata = fake_gguf_metadata(32, 2048, 32, 8);
        let plan = memory_plan(
            metadata.as_slice(),
            2 * 1024 * MIB,
            4096,
            1,
            16 * 1024 * MIB,
        )
        .unwrap();
        assert_eq!(plan.weights, 2 * 1024 * MIB);
        assert_eq!(plan.context, 256 * MIB);
        assert!(plan.scratch > 2 * 1024 * MIB);
        assert_eq!(plan.home, 16 * 1024 * MIB / 10);
        let doubled =
            memory_plan(metadata.as_slice(), plan.weights, 8192, 1, 16 * 1024 * MIB).unwrap();
        assert_eq!(doubled.context, 2 * plan.context);
        assert!(doubled.scratch > plan.scratch);
        let small = memory_plan(metadata.as_slice(), MIB, 256, 1, 4 * 1024 * MIB).unwrap();
        assert!(small.scratch > 512 * MIB);
        assert_eq!(small.home, 1024 * MIB);
        let padded = memory_plan(metadata.as_slice(), MIB, 257, 2, 4 * 1024 * MIB).unwrap();
        assert_eq!(padded.context, small.context * 2);
    }

    #[test]
    fn small_weights_with_large_context_require_compute_space() {
        let metadata = fake_gguf_metadata(1, 2048, 32, 1);
        let short = memory_plan(metadata.as_slice(), MIB, 256, 1, 4 * 1024 * MIB).unwrap();
        let maximum = memory_plan(metadata.as_slice(), MIB, 32768, 1, 4 * 1024 * MIB).unwrap();
        assert!(maximum.scratch >= 2 * 1024 * MIB);
        assert!(maximum.context > short.context);
        assert!(maximum.scratch > short.scratch);
        assert_eq!(short.admit_available(3 * 1024 * MIB), Ok(()));
        assert_eq!(
            maximum.admit_available(3 * 1024 * MIB),
            Err(LocalLlamaFault::MemoryUnavailable)
        );
    }

    #[test]
    fn known_smollm2_profile_fits_small_host() {
        // Geometry from the qualified SmolLM2-135M GGUF, without its weights.
        let metadata = crate::test_support::fake_gguf_profile(30, 576, 9, 3, 1536, 49152);
        let plan = memory_plan(metadata.as_slice(), 144_811_072, 4096, 1, 4 * 1024 * MIB).unwrap();
        assert_eq!(plan.context, 90 * MIB);
        assert_eq!(plan.admit_available(2 * 1024 * MIB), Ok(()));
    }

    fn numeric_field(bytes: &[u8], key: &str) -> usize {
        bytes
            .windows(key.len())
            .position(|part| part == key.as_bytes())
            .unwrap()
            + key.len()
    }

    #[test]
    fn ordinary_attention_uses_missing_kv_head_default_but_refuses_malformed_fields() {
        let mut missing = fake_gguf_metadata(32, 2048, 32, 32);
        let key = "llama.attention.head_count_kv";
        let offset = numeric_field(&missing, key);
        missing.drain(offset - key.len() - 8..offset + 8);
        missing[16..24].copy_from_slice(&6_u64.to_le_bytes());
        let explicit = fake_gguf_metadata(32, 2048, 32, 32);
        assert_eq!(
            memory_plan(missing.as_slice(), MIB, 4096, 1, 4 * 1024 * MIB),
            memory_plan(explicit.as_slice(), MIB, 4096, 1, 4 * 1024 * MIB)
        );
        let mut malformed = explicit;
        let offset = numeric_field(&malformed, key);
        malformed[offset..offset + 4].copy_from_slice(&5_u32.to_le_bytes());
        assert_eq!(
            memory_plan(malformed.as_slice(), MIB, 4096, 1, 4 * 1024 * MIB),
            Err(LocalLlamaFault::ResourcesUnavailable)
        );
    }

    #[test]
    fn cgroup_admission_observes_ancestors_and_requires_visible_controllers() {
        let root = crate::test_support::temp_root_path("model-provider-resources", "cgroups");
        let leaf = root.join("parent/child");
        std::fs::create_dir_all(&leaf).unwrap();
        for path in [&root, &root.join("parent"), &leaf] {
            std::fs::write(path.join("cgroup.controllers"), "memory cpu").unwrap();
            std::fs::write(path.join("cgroup.subtree_control"), "memory").unwrap();
        }
        std::fs::write(root.join("parent/memory.max"), "500").unwrap();
        std::fs::write(root.join("parent/memory.current"), "200").unwrap();
        std::fs::write(leaf.join("memory.max"), "400").unwrap();
        std::fs::write(leaf.join("memory.current"), "50").unwrap();
        let observe = || {
            cgroup_memory(Ok("0::/parent/child".into()), &root, 1000, 900, |path| {
                std::fs::read_to_string(path)
            })
        };
        assert_eq!(observe(), Ok((400, 300)));
        assert_eq!(
            cgroup_memory(
                Err(std::io::ErrorKind::PermissionDenied.into()),
                &root,
                1000,
                900,
                |path| std::fs::read_to_string(path)
            ),
            Err(LocalLlamaFault::ResourcesUnavailable)
        );
        assert_eq!(
            cgroup_memory(Ok("0::/parent/child".into()), &root, 1000, 900, |path| {
                if path == leaf.join("memory.max") {
                    Err(std::io::ErrorKind::PermissionDenied.into())
                } else {
                    std::fs::read_to_string(path)
                }
            }),
            Err(LocalLlamaFault::ResourcesUnavailable)
        );
        std::fs::remove_file(leaf.join("memory.max")).unwrap();
        assert_eq!(observe(), Err(LocalLlamaFault::ResourcesUnavailable));
        std::fs::write(root.join("parent/cgroup.subtree_control"), "cpu").unwrap();
        std::fs::write(leaf.join("cgroup.controllers"), "cpu").unwrap();
        assert_eq!(observe(), Ok((500, 300)));
        // A namespace root can report unlimited memory while an invisible
        // parent has a finite limit. Any root memory.max marks an incomplete
        // hierarchy, including a finite local limit.
        for limit in ["max", "900"] {
            std::fs::write(root.join("memory.max"), limit).unwrap();
            assert_eq!(observe(), Err(LocalLlamaFault::ResourcesUnavailable));
        }
        std::fs::remove_file(root.join("memory.max")).unwrap();
        std::fs::write(root.join("cgroup.controllers"), "cpu").unwrap();
        assert_eq!(observe(), Err(LocalLlamaFault::ResourcesUnavailable));
        std::fs::remove_file(root.join("cgroup.controllers")).unwrap();
        assert_eq!(observe(), Err(LocalLlamaFault::ResourcesUnavailable));
        assert_eq!(
            cgroup_memory(Ok("0::/hidden".into()), &root, 1000, 900, |path| {
                std::fs::read_to_string(path)
            }),
            Err(LocalLlamaFault::ResourcesUnavailable)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_refuses_invalid_or_incomplete_profile_and_overflow() {
        for metadata in [
            fake_gguf_metadata(0, 2048, 32, 8),
            fake_gguf_metadata(32, 2048, 0, 8),
            fake_gguf_metadata(32, 2048, 32, 33),
            fake_gguf_metadata(32, 2049, 32, 8),
            b"GGUF".to_vec(),
        ] {
            assert_eq!(
                memory_plan(metadata.as_slice(), MIB, 4096, 1, 16 * 1024 * MIB),
                Err(LocalLlamaFault::ResourcesUnavailable)
            );
        }
        let plan = MemoryPlan {
            weights: u64::MAX,
            context: 1,
            scratch: 1,
            home: 1,
        };
        assert_eq!(plan.total(), Err(LocalLlamaFault::ResourcesUnavailable));
    }

    #[test]
    fn oversized_metadata_stops_at_bounded_input() {
        let mut metadata = fake_gguf_metadata(32, 2048, 32, 8);
        metadata[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(super::metadata(metadata.as_slice()).is_err());
        let mut metadata = fake_gguf_metadata(32, 2048, 32, 8);
        metadata[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(super::metadata(metadata.as_slice()).is_err());
    }

    #[test]
    fn host_observation_and_small_fixture_admission() {
        let (total, available) = host_memory().unwrap();
        assert!(total > 0 && available <= total);
        let root = crate::test_support::temp_root_path("model-provider-resources", "memory");
        std::fs::create_dir_all(&root).unwrap();
        let model = root.join("model.gguf");
        std::fs::write(&model, fake_gguf_metadata(32, 2048, 32, 8)).unwrap();
        // The result depends on current host pressure; every refusal is explicit.
        assert!(matches!(
            admit(&model, 256, 1, &root),
            Ok(())
                | Err(LocalLlamaFault::MemoryUnavailable)
                | Err(LocalLlamaFault::DiskUnavailable)
        ));
    }
}
