use std::time::{SystemTime, UNIX_EPOCH};

// Thin launcher for the marketplace-content storefront capsule. The UI lives in `browser/` (HTML/CSS/JS);
// the runtime instantiates this `.wasm` as the capsule entrypoint under the capability sandbox (zero
// ambient authority, P3/P16) and serves the frontend. The shell holds no signer/token/CEK/RPC — all
// `/api/market/*` calls flow through the runtime capability plane; signing is routed to the runtime's
// wallet capability seam.
fn main() {
    let info = elastos_guest::CapsuleInfo::from_env();
    let launched_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    eprintln!(
        "marketplace-content capsule launched: name={} id={} ts={}",
        info.name(),
        info.id(),
        launched_at
    );
}
