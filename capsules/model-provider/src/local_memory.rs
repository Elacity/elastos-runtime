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

pub(crate) use elastos_common::host_memory::HostMemory;

fn fits(host: HostMemory, required: u64) -> bool {
    required <= host.available
        || (host.pressure_normal && u128::from(required) * 10 <= u128::from(host.total) * 4)
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

/// Refuses with `MemoryObservationUnavailable` when Runtime cannot read the host
/// limits, or with `MemoryUnavailable` unless the host fits the model weights,
/// its KV cache at `context`, compute scratch, and a headroom of
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
        Some(required) if fits(host, required) => Ok(()),
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

/// Provider process fixtures may inject a fixed reading in debug builds.
/// Runtime Get always calls the common observer directly.
#[cfg(not(test))]
pub(crate) fn host_memory() -> Result<HostMemory, LocalLlamaFault> {
    #[cfg(debug_assertions)]
    if let Some(bytes) = std::env::var("ELASTOS_MODEL_PROVIDER_TEST_FREE_MEMORY")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        return Ok(HostMemory {
            total: bytes,
            available: bytes,
            pressure_normal: false,
        });
    }
    observed_memory(elastos_common::host_memory::observe_host_memory(
        std::env::consts::OS,
    ))
}

fn observed_memory(
    observation: std::io::Result<HostMemory>,
) -> Result<HostMemory, LocalLlamaFault> {
    observation.map_err(|_| LocalLlamaFault::MemoryObservationUnavailable)
}

/// Ample fixed memory, so test engines never depend on the host's load.
#[cfg(test)]
pub(crate) fn test_host_memory() -> Result<HostMemory, LocalLlamaFault> {
    Ok(HostMemory {
        total: 8 * 1024 * MIB,
        available: 6 * 1024 * MIB,
        pressure_normal: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fake_gguf;

    const GIB: u64 = 1024 * MIB;

    #[test]
    fn unavailable_memory_refuses_before_opening_model_with_typed_cause() {
        fn unavailable() -> Result<HostMemory, LocalLlamaFault> {
            observed_memory(Err(std::io::ErrorKind::PermissionDenied.into()))
        }
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::Unsupported,
        ] {
            assert_eq!(
                observed_memory(Err(kind.into())),
                Err(LocalLlamaFault::MemoryObservationUnavailable)
            );
        }
        assert_eq!(
            admit(
                Path::new("memory-refused-unopened-model.gguf"),
                4096,
                1,
                unavailable
            ),
            Err(LocalLlamaFault::MemoryObservationUnavailable)
        );
    }

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
    fn signed_minimum_still_requires_the_full_execution_memory_plan() {
        fn one_gib() -> Result<HostMemory, LocalLlamaFault> {
            Ok(HostMemory {
                total: GIB,
                available: GIB,
                pressure_normal: false,
            })
        }
        let root = crate::test_support::temp_root_path("model-provider-memory", "minimum");
        std::fs::create_dir_all(&root).unwrap();
        let model = root.join("weights.gguf");
        std::fs::write(&model, fake_gguf(28, 1536, 12, 2, 8960, 1000)).unwrap();
        assert_eq!(
            elastos_common::host_memory::model_memory_fit(512, Some(one_gib().unwrap())),
            elastos_common::host_memory::ModelMemoryFit::Fits
        );
        assert_eq!(
            admit(&model, 4096, 1, one_gib),
            Err(LocalLlamaFault::MemoryUnavailable)
        );
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
    fn normal_pressure_admits_up_to_40_percent_of_ram_when_estimate_is_low() {
        let host = |available, total, pressure_normal| HostMemory {
            total,
            available,
            pressure_normal,
        };
        // 24 GiB Mac, low estimate, normal pressure: Qwen-1.5B's 4.6 GB need fits.
        assert!(fits(host(3 * GIB, 24 * GIB, true), 4_613_050_297));
        // 8 GiB host: 40% is 3.2 GiB. A 3.04 GB need fits; a 4 GB need does not.
        assert!(fits(host(GIB, 8 * GIB, true), 3_040_000_000));
        assert!(!fits(host(GIB, 8 * GIB, true), 4_000_000_000));
        // Warning, critical or unknown pressure with a low estimate: refused.
        assert!(!fits(host(3 * GIB, 24 * GIB, false), 4_613_050_297));
        // An estimate that covers the need admits regardless of pressure.
        assert!(fits(host(5 * GIB, 8 * GIB, false), 4_000_000_000));
    }
}
