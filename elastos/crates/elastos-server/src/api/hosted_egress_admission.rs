//! Admission shared by Runtime's durable hosted End and the Mac transport.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{watch, OwnedRwLockWriteGuard, RwLock};

type AdmissionKey = (PathBuf, String);
static ADMISSION_GATES: OnceLock<Mutex<HashMap<AdmissionKey, Weak<AdmissionGate>>>> =
    OnceLock::new();

pub(super) struct AdmissionGate {
    pub(super) lock: Arc<RwLock<()>>,
    pub(super) ending: AtomicUsize,
    pub(super) changed: watch::Sender<u64>,
}

pub(crate) struct HostedEndGuard {
    gate: Arc<AdmissionGate>,
    _lock: OwnedRwLockWriteGuard<()>,
}

struct PendingEnd {
    gate: Arc<AdmissionGate>,
    active: bool,
}

impl Drop for PendingEnd {
    fn drop(&mut self) {
        if self.active {
            self.gate.ending.fetch_sub(1, Ordering::Release);
        }
    }
}

impl Drop for HostedEndGuard {
    fn drop(&mut self) {
        self.gate.ending.fetch_sub(1, Ordering::Release);
    }
}

pub(super) fn admission_gate(data_dir: &Path, offer_id: &str) -> Arc<AdmissionGate> {
    let gates = ADMISSION_GATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut gates = gates.lock().unwrap_or_else(|error| error.into_inner());
    let key = (data_dir.to_path_buf(), offer_id.to_string());
    if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
        return gate;
    }
    gates.retain(|_, gate| gate.strong_count() > 0);
    let (changed, _) = watch::channel(0);
    let gate = Arc::new(AdmissionGate {
        lock: Arc::new(RwLock::new(())),
        ending: AtomicUsize::new(0),
        changed,
    });
    gates.insert(key, Arc::downgrade(&gate));
    gate
}

#[cfg(test)]
pub(super) struct HeldAdmissionForTest {
    _gate: Arc<AdmissionGate>,
    _read: tokio::sync::OwnedRwLockReadGuard<()>,
}

#[cfg(test)]
pub(super) async fn hold_admission_for_test(
    data_dir: &Path,
    offer_id: &str,
) -> HeldAdmissionForTest {
    let gate = admission_gate(data_dir, offer_id);
    let read = gate.lock.clone().read_owned().await;
    HeldAdmissionForTest {
        _gate: gate,
        _read: read,
    }
}

/// After durable End succeeds, stop admitted sends and drain their admission.
pub(crate) async fn begin_hosted_end(data_dir: &Path, offer_id: &str) -> HostedEndGuard {
    let gate = admission_gate(data_dir, offer_id);
    gate.ending.fetch_add(1, Ordering::AcqRel);
    let mut pending = PendingEnd {
        gate: gate.clone(),
        active: true,
    };
    gate.changed
        .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    let lock = gate.lock.clone().write_owned().await;
    pending.active = false;
    HostedEndGuard { gate, _lock: lock }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cancelled_end_releases_pending_admission() {
        let dir = tempfile::tempdir().unwrap();
        let gate = admission_gate(dir.path(), "model:fixture");
        let read = gate.lock.read().await;
        let mut end = Box::pin(begin_hosted_end(dir.path(), "model:fixture"));
        assert!(tokio::time::timeout(Duration::from_millis(100), &mut end)
            .await
            .is_err());
        assert_eq!(gate.ending.load(Ordering::Acquire), 1);
        drop(end);
        assert_eq!(gate.ending.load(Ordering::Acquire), 0);
        drop(read);
        drop(
            tokio::time::timeout(
                Duration::from_secs(1),
                begin_hosted_end(dir.path(), "model:fixture"),
            )
            .await
            .unwrap(),
        );
    }
}
