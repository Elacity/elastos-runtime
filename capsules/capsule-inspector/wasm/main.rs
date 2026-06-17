use std::time::{SystemTime, UNIX_EPOCH};

// Capsule Inspector (Phase 1): a read-only, object-centered view of live
// capsules. The WASM entrypoint announces the capsule; the inspector UI is
// served from `inspector/` and reads `elastos://inspect/*` (capability
// `elastos://inspect/read`). Holds no ambient authority and no write effect.
fn main() {
    let info = elastos_guest::CapsuleInfo::from_env();
    let launched_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    eprintln!(
        "capsule-inspector launched: name={} id={} ts={}",
        info.name(),
        info.id(),
        launched_at
    );
}
