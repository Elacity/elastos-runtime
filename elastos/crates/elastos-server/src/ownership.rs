use anyhow::Context;
use std::path::Path;

#[cfg(unix)]
fn sudo_target_owner() -> Option<(u32, u32)> {
    if unsafe { libc::geteuid() } != 0 {
        return None;
    }
    let uid = std::env::var("SUDO_UID").ok()?.parse::<u32>().ok()?;
    let gid = std::env::var("SUDO_GID").ok()?.parse::<u32>().ok()?;
    Some((uid, gid))
}

#[cfg(not(unix))]
fn sudo_target_owner() -> Option<(u32, u32)> {
    None
}

pub fn repair_path_recursive(path: &Path) -> anyhow::Result<()> {
    let Some((uid, gid)) = sudo_target_owner() else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }
    repair_inner(path, uid, gid)
}

#[cfg(unix)]
fn repair_inner(path: &Path, uid: u32, gid: u32) -> anyhow::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("invalid path for chown: {}", path.display()))?;
    let rc = unsafe { libc::chown(c_path.as_ptr(), uid, gid) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        anyhow::bail!("chown {} failed: {}", path.display(), err);
    }

    if path.is_dir() {
        for entry in std::fs::read_dir(path)
            .with_context(|| format!("reading directory {}", path.display()))?
        {
            let entry = entry?;
            repair_inner(&entry.path(), uid, gid)?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn repair_inner(_path: &Path, _uid: u32, _gid: u32) -> anyhow::Result<()> {
    Ok(())
}
