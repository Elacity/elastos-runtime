//! Library's read for "Set as Desktop Background".
//!
//! Home owns the desktop appearance; Library owns read access to the person's
//! objects. This read runs only after the gateway has verified both, and it
//! applies Library's own scope: the signed-in principal's root, never Trash,
//! never Runtime-private areas, never a link, and never protected content.
//! Protection status that cannot be read is treated as protected.
//!
//! Protection is checked twice: by the object's own publish record, and by its
//! bytes. A copy, rename or move gives the bytes a new URI with no record of
//! its own, so the bytes are also compared with the source of every protected
//! record the person holds. A protected publish records the SHA-256 of the
//! exact bytes custody read and protected. For a record written before that
//! (a legacy record), the digest comes from custody's media preparation
//! journal, or else from the record's own file, but only while that file
//! still has the CID the record says was published
//! (`plain_publish_cid::plain_publish_file_cid`). Once found, the digest is
//! kept in the record. A legacy record whose bytes cannot be found, or whose
//! file no longer matches its CID, refuses every picture and names the file
//! to put back. A custody record has the CID of its encrypted payload, so
//! without custody's journal its file can never prove its bytes and it is
//! refused the same way.
//!
//! Protection follows exact bytes. An edited or re-encoded copy of protected
//! content has a new digest, so it is not refused; this gap is accepted.
//!
//! The check reads and hashes files, so callers run it on a blocking thread,
//! and one request does a bounded amount of work (`ProtectionCheckBudget`).
//! The check itself writes nothing: it returns the digests it found
//! (`DesktopBackgroundRead::found`), and the caller keeps them only when the
//! check answered within the request's time limit (`keep_found_source_digests`).

use std::borrow::Cow;
use std::path::{Component, Path};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{
    is_runtime_private_uri, is_trash_uri, library_target, read_publish_record, StoredPublishRecord,
    RUNTIME_CUSTODY_PUBLISHED_PAYLOAD,
};

/// Why Library refused to hand an object to the desktop background.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DesktopBackgroundSourceRefusal {
    OutsideLibrary,
    InTrash,
    Link,
    NotFound,
    NotFile,
    TooLarge,
    Protected,
    ProtectionUnknown,
    /// The check of the person's protected files did not finish within this
    /// request's limits. Digests found so far are kept, so a retry goes on.
    CheckUnfinished,
    /// A protected record written before records named their digest, whose
    /// file is no longer where the record says. Its bytes are unknown.
    ProtectedFileNotInPlace {
        name: String,
        folder: String,
        others: usize,
    },
    Unreadable,
}

impl DesktopBackgroundSourceRefusal {
    pub(crate) fn message(&self) -> Cow<'static, str> {
        Cow::Borrowed(match self {
            Self::OutsideLibrary => "background image source must be one of your own Library files",
            Self::InTrash => "a picture in Trash cannot be the desktop background; restore it first",
            Self::Link => "background image source must not be a link",
            Self::NotFound => "background image source was not found",
            Self::NotFile => "background image source must be a file",
            Self::TooLarge => "background image is larger than 5 MB",
            Self::Protected => {
                "protected content cannot be the desktop background; choose a picture that is not protected"
            }
            Self::ProtectionUnknown => {
                "Library cannot confirm that this picture is not protected content, so it cannot be the desktop background"
            }
            Self::CheckUnfinished => {
                "Library did not finish the check of your protected files. Try again."
            }
            Self::ProtectedFileNotInPlace {
                name,
                folder,
                others,
            } => {
                let more = match others {
                    0 => String::new(),
                    1 => " Then do the same for 1 more protected file.".to_string(),
                    n => format!(" Then do the same for {n} more protected files."),
                };
                return Cow::Owned(format!(
                    "Library cannot check your protected file \"{name}\" because it is not in \"{folder}\". \
                     Move it back to \"{folder}\", then try again.{more} \
                     Or upload the picture in System > Background > Choose image."
                ));
            }
            Self::Unreadable => "background image source could not be read",
        })
    }
}

impl std::fmt::Display for DesktopBackgroundSourceRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for DesktopBackgroundSourceRefusal {}

/// Limits on the work one desktop background request may do to check the
/// person's protected files. Past any limit the check fails closed with
/// `CheckUnfinished`; the digests found so far are kept in their records, so
/// a retry continues where this one stopped.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProtectionCheckBudget {
    /// The check stops when this passes.
    pub(crate) deadline: Instant,
    /// Most legacy sources hashed in one request.
    pub(crate) legacy_sources: usize,
    /// Most stored bytes of legacy sources read in one request.
    pub(crate) legacy_bytes: u64,
}

/// Time one request may spend on the protection check.
pub(crate) const PROTECTION_CHECK_TIME_LIMIT: Duration = Duration::from_secs(5);
/// Legacy sources one request may hash.
pub(crate) const PROTECTION_CHECK_LEGACY_SOURCES: usize = 8;
/// Stored bytes of legacy sources one request may read (32 MiB).
pub(crate) const PROTECTION_CHECK_LEGACY_BYTES: u64 = 32 * 1024 * 1024;

impl ProtectionCheckBudget {
    pub(crate) fn for_request(now: Instant) -> Self {
        Self {
            deadline: now + PROTECTION_CHECK_TIME_LIMIT,
            legacy_sources: PROTECTION_CHECK_LEGACY_SOURCES,
            legacy_bytes: PROTECTION_CHECK_LEGACY_BYTES,
        }
    }

    fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }
}

/// A protected record's source digest that a check found and that is not yet
/// kept in the record.
pub(crate) struct FoundSourceDigest {
    stored: StoredPublishRecord,
    digest: String,
}

/// What one desktop background read answers: the bytes or why they were
/// refused, and the source digests the protection check found on the way.
/// Nothing is written while reading; see `keep_found_source_digests`.
pub(crate) struct DesktopBackgroundRead {
    pub(crate) result: Result<Vec<u8>, DesktopBackgroundSourceRefusal>,
    pub(crate) found: Vec<FoundSourceDigest>,
}

/// Reads one of `principal_id`'s own Library files for the desktop background.
/// `max_stored_bytes` bounds the stored (encrypted) size before any read.
/// This does blocking file I/O and hashing: call it from a blocking thread.
pub(crate) fn read_library_object_for_desktop_background(
    data_dir: &Path,
    principal_id: &str,
    uri: &str,
    max_stored_bytes: u64,
    budget: ProtectionCheckBudget,
) -> DesktopBackgroundRead {
    let mut found = Vec::new();
    let result = read_checked(
        data_dir,
        principal_id,
        uri,
        max_stored_bytes,
        budget,
        &mut found,
    );
    DesktopBackgroundRead { result, found }
}

/// Keeps each found digest in its record so later checks need no read. A
/// record changed since it was read is left as it is. Call this only after
/// the check answered within its request's time limit, and before answering.
pub(crate) fn keep_found_source_digests(
    data_dir: &Path,
    principal_id: &str,
    found: Vec<FoundSourceDigest>,
) {
    for FoundSourceDigest { stored, digest } in found {
        if let Err(err) =
            super::remember_publish_record_source_digest(data_dir, principal_id, &stored, &digest)
        {
            tracing::debug!(error = %err, "library could not keep a protected record's source digest");
        }
    }
}

fn read_checked(
    data_dir: &Path,
    principal_id: &str,
    uri: &str,
    max_stored_bytes: u64,
    budget: ProtectionCheckBudget,
    found: &mut Vec<FoundSourceDigest>,
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
    require_unprotected(
        data_dir,
        principal_id,
        &target.localhost_root,
        &target.uri,
        &bytes,
        budget,
        found,
    )?;
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

/// Most publish records this check reads. More than this counts as unknown.
const MAX_PUBLISH_RECORDS_CHECKED: usize = 16_384;

/// Allows only content with no publish record, or one published as plain
/// content with no key release, and whose bytes are not the source of any
/// protected record. Anything else, including a record that cannot be read,
/// is treated as protected.
fn require_unprotected(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    uri: &str,
    bytes: &[u8],
    budget: ProtectionCheckBudget,
    found: &mut Vec<FoundSourceDigest>,
) -> Result<(), DesktopBackgroundSourceRefusal> {
    use DesktopBackgroundSourceRefusal as Refusal;
    // The object's own record.
    let record_uri = super::publish_record_uri(localhost_root, uri);
    let record_path = elastos_common::localhost::rooted_localhost_fs_path(data_dir, &record_uri)
        .ok_or(Refusal::Protected)?;
    match std::fs::symlink_metadata(&record_path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(Refusal::Protected),
        Ok(_) => {
            let record =
                read_publish_record(data_dir, principal_id, uri).map_err(|_| Refusal::Protected)?;
            if is_protected(&record.content_security) {
                return Err(Refusal::Protected);
            }
        }
    }
    // The bytes: protection follows the content through copy, rename and move.
    let digest = super::content_sha256_label(bytes);
    let records = super::read_all_publish_records(
        data_dir,
        principal_id,
        MAX_PUBLISH_RECORDS_CHECKED,
        budget.deadline,
    )
    .map_err(|_| {
        if budget.expired() {
            Refusal::CheckUnfinished
        } else {
            Refusal::ProtectionUnknown
        }
    })?;
    let mut legacy = LegacyCheck {
        data_dir,
        principal_id,
        budget,
        sources_hashed: 0,
        bytes_read: 0,
        found,
    };
    let mut not_in_place: Vec<&super::LibraryPublishRecord> = Vec::new();
    for stored in records
        .iter()
        .filter(|stored| is_protected(&stored.record.content_security))
    {
        let source_digests = match &stored.record.source_sha256 {
            Some(source_digest) => vec![source_digest.clone()],
            None => match legacy.source_digests(stored)? {
                LegacySource::Digests(digests) => digests,
                LegacySource::NotInPlace => {
                    not_in_place.push(&stored.record);
                    Vec::new()
                }
            },
        };
        if source_digests.contains(&digest) {
            return Err(Refusal::Protected);
        }
    }
    if let Some(first) = not_in_place.first() {
        let (name, folder) = display_location(localhost_root, &first.object_uri);
        return Err(Refusal::ProtectedFileNotInPlace {
            name,
            folder,
            others: not_in_place.len() - 1,
        });
    }
    Ok(())
}

/// What a legacy record (one written before records named their digest) says
/// about its protected bytes.
enum LegacySource {
    /// The protected bytes have one of these digests.
    Digests(Vec<String>),
    /// No file proves the protected bytes: the record's file is not where the
    /// record says, or it no longer has the record's published CID, and
    /// custody has no digest for it. Its bytes are unknown.
    NotInPlace,
}

/// Finds the protected bytes of legacy records within one request's budget.
/// Each digest found is added to `found`, for the caller to keep in its
/// record, so this is done once.
struct LegacyCheck<'a> {
    data_dir: &'a Path,
    principal_id: &'a str,
    budget: ProtectionCheckBudget,
    sources_hashed: usize,
    bytes_read: u64,
    found: &'a mut Vec<FoundSourceDigest>,
}

impl LegacyCheck<'_> {
    fn source_digests(
        &mut self,
        stored: &StoredPublishRecord,
    ) -> Result<LegacySource, DesktopBackgroundSourceRefusal> {
        use DesktopBackgroundSourceRefusal as Refusal;
        if self.budget.expired() {
            return Err(Refusal::CheckUnfinished);
        }
        let record = &stored.record;
        // Runtime custody keeps the digest of the exact bytes it read and
        // protected in its media preparation journal. That digest does not
        // depend on where the file is now.
        if super::record_is_runtime_custody(record) {
            let digests = self.custody_source_digests(&record.object_uri)?;
            if let [only] = digests.as_slice() {
                self.found.push(FoundSourceDigest {
                    stored: stored.clone(),
                    digest: only.clone(),
                });
            }
            if !digests.is_empty() {
                return Ok(LegacySource::Digests(digests));
            }
            // A custody record's CID names its encrypted payload, so no
            // plaintext file can match it: the file proves nothing.
            return Ok(LegacySource::NotInPlace);
        }
        // Otherwise the record's own file is the evidence left, but only
        // while it still holds the bytes that were published.
        let Ok(source) = library_target(self.data_dir, self.principal_id, &record.object_uri)
        else {
            return Err(Refusal::ProtectionUnknown);
        };
        let metadata = match std::fs::symlink_metadata(&source.path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LegacySource::NotInPlace);
            }
            Err(_) => return Err(Refusal::ProtectionUnknown),
        };
        if !metadata.is_file() {
            return Ok(LegacySource::NotInPlace);
        }
        // A file that no request may read whole can never be checked.
        if metadata.len() > self.budget.legacy_bytes {
            return Ok(LegacySource::NotInPlace);
        }
        if self.sources_hashed >= self.budget.legacy_sources
            || self.bytes_read.saturating_add(metadata.len()) > self.budget.legacy_bytes
        {
            return Err(Refusal::CheckUnfinished);
        }
        self.sources_hashed += 1;
        self.bytes_read = self.bytes_read.saturating_add(metadata.len());
        let bytes = crate::auth::read_principal_root_object_limited(
            self.data_dir,
            self.principal_id,
            &source.localhost_root,
            &source.uri,
            &source.path,
            metadata.len(),
        )
        .map_err(|_| Refusal::ProtectionUnknown)?;
        // The file changed after it was published (or the CID is of a form
        // this check does not compute): its bytes say nothing about the
        // protected bytes, so nothing is kept and the check fails closed.
        if super::plain_publish_cid::plain_publish_file_cid(&bytes).as_deref()
            != Some(record.cid.as_str())
        {
            return Ok(LegacySource::NotInPlace);
        }
        let digest = super::content_sha256_label(&bytes);
        self.found.push(FoundSourceDigest {
            stored: stored.clone(),
            digest: digest.clone(),
        });
        Ok(LegacySource::Digests(vec![digest]))
    }

    /// The source digests custody's media preparation journal holds for
    /// `object_uri`, for either way the source may have been stored.
    fn custody_source_digests(
        &self,
        object_uri: &str,
    ) -> Result<Vec<String>, DesktopBackgroundSourceRefusal> {
        use elastos_protected_content_runtime::{RuntimeMintIntent, RuntimeMintJournalError};
        use DesktopBackgroundSourceRefusal as Refusal;
        let journal_root =
            crate::protected_content_runtime::runtime_mint_journal_root(self.data_dir);
        match std::fs::symlink_metadata(&journal_root) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(Refusal::ProtectionUnknown),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(Refusal::ProtectionUnknown),
        }
        let journal = crate::protected_content_runtime::runtime_mint_journal(self.data_dir);
        let mut digests = Vec::new();
        for source_storage in ["plain_localhost_root", "protected_principal_root"] {
            let Ok(request_id) = RuntimeMintIntent::request_id_for_source(
                self.principal_id,
                object_uri,
                source_storage,
            ) else {
                return Err(Refusal::ProtectionUnknown);
            };
            match journal.load_media_preparation(request_id) {
                Ok(preparation) => digests.push(super::digest32_sha256_label(
                    preparation.source_object_digest(),
                )),
                Err(RuntimeMintJournalError::NotFound) => {}
                Err(_) => return Err(Refusal::ProtectionUnknown),
            }
        }
        Ok(digests)
    }
}

/// The file name and folder of `object_uri` as a person sees them in
/// Library: relative to their root, decoded once, without control characters.
fn display_location(localhost_root: &str, object_uri: &str) -> (String, String) {
    let relative = object_uri
        .strip_prefix(localhost_root)
        .unwrap_or(object_uri)
        .trim_start_matches('/');
    let clean = |part: &str| -> String {
        let decoded = percent_decode_once(part).unwrap_or_else(|| part.to_string());
        let text: String = decoded.chars().filter(|c| !c.is_control()).collect();
        if text.chars().count() > 80 {
            let short: String = text.chars().take(79).collect();
            format!("{short}\u{2026}")
        } else {
            text
        }
    };
    match relative.rsplit_once('/') {
        Some((folder, name)) => (clean(name), clean(folder)),
        None => (clean(relative), "Library".to_string()),
    }
}

/// Only plain content with no key release is unprotected; anything else,
/// including a form this check does not know, is protected.
fn is_protected(security: &Value) -> bool {
    let payload = security.get("published_payload").and_then(Value::as_str);
    let key_release = security
        .get("key_release_required")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    payload == Some(RUNTIME_CUSTODY_PUBLISHED_PAYLOAD)
        || payload != Some("plain_content")
        || key_release
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
