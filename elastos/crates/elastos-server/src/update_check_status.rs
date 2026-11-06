//! The last completed signed-release check, for a supervisor and for Inbox.
//!
//! `elastos update --check` exits 0 both when the installed release is current
//! and when a newer signed release exists. This file is the distinction.
//! Inbox reads the same record and keeps one notice for the offered release.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::notifications::{self, SignedReleaseOffer};
use crate::operator_control::{OperatorUpdateCheck, UpdateVersionStatus};

pub const SCHEMA: &str = "elastos.update-check/v1";
pub const FILE_NAME: &str = "update-check.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateCheckOutcome {
    UpToDate,
    UpdateAvailable,
    InstalledNewer,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateCheckStatus {
    pub schema: String,
    pub outcome: UpdateCheckOutcome,
    pub current_version: String,
    pub latest_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub publisher_did: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_cid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_cid: Option<String>,
    /// The owner can install this release from System. A refused release stays
    /// `update_available` and is not offered.
    pub offered: bool,
}

impl UpdateCheckStatus {
    pub fn from_home_check(check: &OperatorUpdateCheck, offered: bool) -> Self {
        let outcome = match check.version_status {
            UpdateVersionStatus::Available => UpdateCheckOutcome::UpdateAvailable,
            UpdateVersionStatus::InstalledNewer => UpdateCheckOutcome::InstalledNewer,
            UpdateVersionStatus::Current => UpdateCheckOutcome::UpToDate,
        };
        let offered = offered && outcome == UpdateCheckOutcome::UpdateAvailable;
        Self {
            schema: SCHEMA.to_string(),
            outcome,
            current_version: check.current_version.clone(),
            latest_version: check.latest_version.clone(),
            publisher_did: check.publisher_did.clone(),
            release_cid: check.release_cid.clone(),
            head_cid: check.head_cid.clone(),
            offered,
        }
    }

    fn offer(&self) -> SignedReleaseOffer {
        if self.offered && !self.publisher_did.is_empty() {
            if let Some(release_cid) = self.release_cid.clone() {
                return SignedReleaseOffer::Available {
                    release_cid,
                    version: self.latest_version.clone(),
                };
            }
        }
        SignedReleaseOffer::NoneAvailable
    }
}

pub fn status_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(FILE_NAME)
}

/// Writes the check and reconciles the Inbox notice. A repeat with the same
/// bytes leaves the file and the notice unchanged.
pub fn record_update_check(data_dir: &Path, status: &UpdateCheckStatus) -> anyhow::Result<()> {
    let path = status_path(data_dir);
    let mut bytes = serde_json::to_vec_pretty(status).context("update check status")?;
    bytes.push(b'\n');
    let unchanged = fs::read(&path).ok().is_some_and(|current| current == bytes);
    if !unchanged {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .with_context(|| format!("create {}", tmp.display()))?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))?;
    }
    notifications::reconcile_signed_release_notice(data_dir, status.offer())
}

pub fn record_home_check(
    data_dir: &Path,
    check: &OperatorUpdateCheck,
    offered: bool,
) -> anyhow::Result<()> {
    record_update_check(
        data_dir,
        &UpdateCheckStatus::from_home_check(check, offered),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(outcome: UpdateCheckOutcome, offered: bool, cid: &str) -> UpdateCheckStatus {
        UpdateCheckStatus {
            schema: SCHEMA.to_string(),
            outcome,
            current_version: "2.0.0".into(),
            latest_version: if offered {
                "2.0.1".into()
            } else {
                "2.0.0".into()
            },
            publisher_did: "did:key:publisher".into(),
            release_cid: Some(cid.into()),
            head_cid: Some("head".into()),
            offered,
        }
    }

    #[test]
    fn the_file_distinguishes_a_current_release_from_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        record_update_check(
            dir.path(),
            &status(UpdateCheckOutcome::UpToDate, false, "cid-current"),
        )
        .unwrap();
        let current: UpdateCheckStatus =
            serde_json::from_slice(&fs::read(status_path(dir.path())).unwrap()).unwrap();
        assert_eq!(current.outcome, UpdateCheckOutcome::UpToDate);
        assert!(!current.offered);
        assert!(crate::notifications::load_summary(dir.path())
            .unwrap()
            .entries
            .is_empty());

        record_update_check(
            dir.path(),
            &status(UpdateCheckOutcome::UpdateAvailable, true, "cid-next"),
        )
        .unwrap();
        let newer: UpdateCheckStatus =
            serde_json::from_slice(&fs::read(status_path(dir.path())).unwrap()).unwrap();
        assert_eq!(newer.outcome, UpdateCheckOutcome::UpdateAvailable);
        assert!(newer.offered);
        let summary = crate::notifications::load_summary(dir.path()).unwrap();
        assert_eq!(summary.entries.len(), 1);
        assert_eq!(summary.entries[0].id, "signed-release:cid-next");
    }
}
