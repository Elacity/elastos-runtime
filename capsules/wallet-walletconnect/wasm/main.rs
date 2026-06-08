use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let info = elastos_guest::CapsuleInfo::from_env();
    let launched_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    eprintln!(
        "wallet-walletconnect capsule launched: name={} id={} ts={}",
        info.name(),
        info.id(),
        launched_at
    );
}
