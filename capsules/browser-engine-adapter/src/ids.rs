pub(super) fn default_supervisor_timeout_ms() -> u64 {
    5_000
}

pub(super) fn is_safe_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
}

pub(super) fn stable_page_id(url: &str, stream_id: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in url.bytes().chain(stream_id.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("page:{hash:016x}")
}
