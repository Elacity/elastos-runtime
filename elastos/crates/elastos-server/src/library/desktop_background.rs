//! Library's read for "Set as Desktop Background".
//!
//! Home owns the desktop appearance; Library owns read access to the person's
//! objects. This read runs only after the gateway has verified both, and it
//! applies Library's own scope: the signed-in principal's root, never Trash,
//! never Runtime-private areas, never a link, and never protected content.
//! Protection status that cannot be read is treated as protected.

use std::path::{Component, Path};

use serde_json::Value;

use super::{
    is_runtime_private_uri, is_trash_uri, library_target, read_publish_record,
    RUNTIME_CUSTODY_PUBLISHED_PAYLOAD,
};

/// Why Library refused to hand an object to the desktop background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DesktopBackgroundSourceRefusal {
    OutsideLibrary,
    InTrash,
    Link,
    NotFound,
    NotFile,
    TooLarge,
    Protected,
    Unreadable,
}

impl DesktopBackgroundSourceRefusal {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::OutsideLibrary => "background image source must be one of your own Library files",
            Self::InTrash => "a picture in Trash cannot be the desktop background; restore it first",
            Self::Link => "background image source must not be a link",
            Self::NotFound => "background image source was not found",
            Self::NotFile => "background image source must be a file",
            Self::TooLarge => "background image is larger than 5 MB",
            Self::Protected => {
                "protected content cannot be the desktop background; choose a picture that is not protected"
            }
            Self::Unreadable => "background image source could not be read",
        }
    }
}

impl std::fmt::Display for DesktopBackgroundSourceRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for DesktopBackgroundSourceRefusal {}

/// Reads one of `principal_id`'s own Library files for the desktop background.
/// `max_stored_bytes` bounds the stored (encrypted) size before any read.
pub(crate) fn read_library_object_for_desktop_background(
    data_dir: &Path,
    principal_id: &str,
    uri: &str,
    max_stored_bytes: u64,
) -> Result<Vec<u8>, DesktopBackgroundSourceRefusal> {
    use DesktopBackgroundSourceRefusal as Refusal;

    if uri.len() > 2048 || !uri_segments_are_plain(uri) {
        return Err(Refusal::OutsideLibrary);
    }
    // Same scope gate as Library's read: the principal's root, no `.`/`..`
    // segments, no backslash.
    let target =
        library_target(data_dir, principal_id, uri).map_err(|_| Refusal::OutsideLibrary)?;
    if target.uri == target.localhost_root {
        return Err(Refusal::NotFile);
    }
    if is_trash_uri(&target.localhost_root, &target.uri) {
        return Err(Refusal::InTrash);
    }
    if is_runtime_private_uri(&target.localhost_root, &target.uri) {
        return Err(Refusal::OutsideLibrary);
    }
    let root_path = library_target(data_dir, principal_id, &target.localhost_root)
        .map_err(|_| Refusal::OutsideLibrary)?
        .path;
    require_no_links(&root_path, &target.path)?;
    let metadata = std::fs::symlink_metadata(&target.path).map_err(|_| Refusal::NotFound)?;
    if !metadata.is_file() {
        return Err(Refusal::NotFile);
    }
    if metadata.len() > max_stored_bytes {
        return Err(Refusal::TooLarge);
    }
    require_unprotected(data_dir, principal_id, &target.localhost_root, &target.uri)?;
    // Read without Library's legacy-plaintext migration: a file that is not
    // under principal-root protection is blocked in Library, so it is refused
    // here and left untouched. The read is bounded through the one open
    // descriptor, so a file that grows after the size check is still refused.
    let bytes = crate::auth::read_principal_root_object_limited(
        data_dir,
        principal_id,
        &target.localhost_root,
        &target.uri,
        &target.path,
        max_stored_bytes,
    )
    .map_err(|_| Refusal::Unreadable)?;
    Ok(bytes)
}

/// Refuses control characters anywhere, and any segment that is a dot
/// segment or a separator once percent-decoded (once or twice), so `%2e%2e`,
/// `%252e%252e`, `%2f` and `%5c` cannot reach a later decoder as traversal.
fn uri_segments_are_plain(uri: &str) -> bool {
    if uri.chars().any(|c| c.is_control()) {
        return false;
    }
    let Some(rest) = uri.strip_prefix("localhost://") else {
        return false;
    };
    rest.split('/').all(|segment| {
        let mut current = segment.to_string();
        for _ in 0..4 {
            if current == "." || current == ".." {
                return false;
            }
            if current.contains('\\') || current.chars().any(|c| c.is_control()) {
                return false;
            }
            let Some(decoded) = percent_decode_once(&current) else {
                return false;
            };
            if decoded.contains('/') {
                return false;
            }
            if decoded == current {
                return true;
            }
            current = decoded;
        }
        // Still changing after four passes: too deeply encoded to trust.
        false
    })
}

/// One pass of percent-decoding. `None` when the result is not UTF-8.
fn percent_decode_once(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8(out).ok()
}

/// Refuses a symbolic link at the root or at any component below it, so a
/// link inside Library cannot point the read at another place.
fn require_no_links(root: &Path, path: &Path) -> Result<(), DesktopBackgroundSourceRefusal> {
    use DesktopBackgroundSourceRefusal as Refusal;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| Refusal::OutsideLibrary)?;
    let mut current = root.to_path_buf();
    let check = |candidate: &Path| match std::fs::symlink_metadata(candidate) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(Refusal::Link),
        Ok(_) => Ok(()),
        Err(_) => Err(Refusal::NotFound),
    };
    check(&current)?;
    for component in relative.components() {
        match component {
            Component::Normal(part) => current.push(part),
            _ => return Err(Refusal::OutsideLibrary),
        }
        check(&current)?;
    }
    Ok(())
}

/// Allows only content with no publish record, or one published as plain
/// content with no key release. Anything else, including a record that cannot
/// be read, is treated as protected.
fn require_unprotected(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    uri: &str,
) -> Result<(), DesktopBackgroundSourceRefusal> {
    use DesktopBackgroundSourceRefusal as Refusal;
    let record_uri = super::publish_record_uri(localhost_root, uri);
    let record_path = elastos_common::localhost::rooted_localhost_fs_path(data_dir, &record_uri)
        .ok_or(Refusal::Protected)?;
    match std::fs::symlink_metadata(&record_path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(Refusal::Protected),
        Ok(_) => {}
    }
    let record =
        read_publish_record(data_dir, principal_id, uri).map_err(|_| Refusal::Protected)?;
    let security = &record.content_security;
    let payload = security.get("published_payload").and_then(Value::as_str);
    let key_release = security
        .get("key_release_required")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if payload == Some(RUNTIME_CUSTODY_PUBLISHED_PAYLOAD)
        || payload != Some("plain_content")
        || key_release
    {
        return Err(Refusal::Protected);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_dot_segments_and_separators_are_refused() {
        let root = "localhost://Users/did:key:z6Mk/Pictures";
        assert!(uri_segments_are_plain(&format!("{root}/hills.png")));
        assert!(uri_segments_are_plain(&format!("{root}/100%25 done.png")));
        assert!(uri_segments_are_plain(&format!("{root}/caf\u{e9}.png")));
        assert!(uri_segments_are_plain(&format!("{root}/cafe\u{301}.png")));
        for bad in [
            "..",
            ".",
            "%2e%2e",
            "%2E%2e",
            ".%2e",
            "%252e%252e",
            "%25252e%25252e",
            "%2525252e%2525252e",
            "a%2fb",
            "a%252fb",
            "a%5cb",
            "..\\..\\x.png",
            "a\\b",
            "nul\0.png",
            "a%00b",
            "line\nbreak",
            "%ff%fe",
        ] {
            assert!(
                !uri_segments_are_plain(&format!("{root}/{bad}")),
                "accepted {bad:?}"
            );
        }
        assert!(!uri_segments_are_plain("file:///etc/passwd"));
    }
}
