//! Conservative admission for the local GGUF text profiles before engine spawn.
use crate::local_llama::LocalLlamaFault;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Take};
use std::path::Path;

const MIB: u64 = 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * MIB;

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
    if plan.total()? > available {
        return Err(LocalLlamaFault::MemoryUnavailable);
    }
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
    let (architecture, numbers) = metadata(reader).map_err(|_| fail)?;
    // These profiles use the ordinary attention KV layout. A different layout
    // needs its own measured profile before Runtime can admit it.
    if !matches!(architecture.as_str(), "llama" | "qwen2" | "qwen3") {
        return Err(fail);
    }
    if numbers.get("split.count").is_some_and(|count| *count != 1) {
        return Err(fail);
    }
    let number = |suffix: &str| {
        numbers
            .get(&format!("{architecture}.{suffix}"))
            .copied()
            .ok_or(fail)
    };
    let layers = number("block_count")?;
    let embedding = number("embedding_length")?;
    let heads = number("attention.head_count")?;
    let kv_heads = number("attention.head_count_kv")?;
    if layers == 0
        || layers > 512
        || heads == 0
        || kv_heads == 0
        || kv_heads > heads
        || embedding == 0
        || embedding > 131_072
        || embedding % heads != 0
        || context == 0
        || parallel == 0
        || parallel > 64
    {
        return Err(fail);
    }
    let key = numbers
        .get(&format!("{architecture}.attention.key_length"))
        .copied()
        .unwrap_or(embedding / heads);
    let value = numbers
        .get(&format!("{architecture}.attention.value_length"))
        .copied()
        .unwrap_or(embedding / heads);
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
    Ok(MemoryPlan {
        weights,
        context,
        // Reserve an additional weight-sized workspace for loading and compute,
        // with a floor for the engine and its fixed batch buffers.
        scratch: weights.max(512 * MIB),
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
fn metadata(reader: impl Read) -> std::io::Result<(String, BTreeMap<String, u64>)> {
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
    Ok((architecture.ok_or_else(invalid)?, numbers))
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
    let mut total = field("MemTotal:")?;
    let mut available = field("MemAvailable:")?;
    // On cgroup v2, both the namespace root and this process's subgroup can
    // restrict the bytes available to the engine.
    if let Ok(groups) = std::fs::read_to_string("/proc/self/cgroup") {
        if groups.lines().any(|line| {
            line.split(':')
                .nth(1)
                .is_some_and(|controllers| controllers.split(',').any(|name| name == "memory"))
        }) {
            // The admitted Linux profile uses unified cgroup v2 accounting.
            return Err(fail);
        }
        for group in groups.lines().filter_map(|line| line.strip_prefix("0::")) {
            if Path::new(group).components().any(|part| {
                !matches!(
                    part,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            }) {
                return Err(fail);
            }
            let root = Path::new("/sys/fs/cgroup");
            let mut path = root.join(group.trim_start_matches('/'));
            while path.starts_with(root) {
                if let Ok(limit) = std::fs::read_to_string(path.join("memory.max")) {
                    if limit.trim() != "max" {
                        let limit = limit.trim().parse::<u64>().map_err(|_| fail)?;
                        let used = std::fs::read_to_string(path.join("memory.current"))
                            .map_err(|_| fail)?
                            .trim()
                            .parse::<u64>()
                            .map_err(|_| fail)?;
                        total = total.min(limit);
                        available = available.min(limit.saturating_sub(used));
                    }
                }
                if path == root || !path.pop() {
                    break;
                }
            }
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
        assert_eq!(plan.scratch, 2 * 1024 * MIB);
        assert_eq!(plan.home, 16 * 1024 * MIB / 10);
        let doubled =
            memory_plan(metadata.as_slice(), plan.weights, 8192, 1, 16 * 1024 * MIB).unwrap();
        assert_eq!(doubled.context, 2 * plan.context);
        let small = memory_plan(metadata.as_slice(), MIB, 256, 1, 4 * 1024 * MIB).unwrap();
        assert_eq!(small.scratch, 512 * MIB);
        assert_eq!(small.home, 1024 * MIB);
        let padded = memory_plan(metadata.as_slice(), MIB, 257, 2, 4 * 1024 * MIB).unwrap();
        assert_eq!(padded.context, small.context * 2);
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
