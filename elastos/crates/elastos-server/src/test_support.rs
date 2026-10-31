//! Helpers shared by the library and binary test targets.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes a fixture that a test later executes, from a child process, then
/// sets `mode`. A writer fd held by the test process is copied into any child
/// that another test thread forks meanwhile; until that child execs, exec of
/// the fixture fails with ETXTBSY. Here only the short-lived `cat` holds it.
pub(crate) fn write_from_child(path: &Path, contents: impl AsRef<[u8]>, mode: u32) {
    let mut writer = Command::new("/bin/sh")
        .args(["-c", "cat > \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    writer
        .stdin
        .take()
        .unwrap()
        .write_all(contents.as_ref())
        .unwrap();
    assert!(writer.wait().unwrap().success());
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}
