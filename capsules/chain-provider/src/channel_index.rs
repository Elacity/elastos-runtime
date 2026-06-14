//! Persistent, resumable channel index.
//!
//! Mirrors PC2's `ContentIndexerService` (`src/services/ContentIndexerService.ts`) cursor
//! model — a persisted "scout" that scans the channel factory for `ChannelCreated` events
//! and remembers how far it has scanned, so it NEVER re-scans chain history per request:
//!
//!   * `head`  — highest block scanned (forward/incremental cursor). New blocks since the
//!               last call are scanned cheaply on every request (PC2's `indexer_last_block`).
//!   * `floor` — lowest block scanned (backfill cursor). Lowered toward `deploy_block` in
//!               bounded, resumable steps so a fresh wallet surfaces recent channels fast
//!               and converges to full history across calls (PC2's one-time backfill, made
//!               resumable instead of one blocking pass).
//!   * `complete` — `floor` has reached `deploy_block`; backfill done, only forward scans run.
//!
//! Per the runtime principles this index is an UNTRUSTED CONVENIENCE CACHE (#5 Small Trusted
//! Core): it is never an authority. The chain stays canonical (#10) — a selected channel is
//! re-confirmed on-chain before any mint, and ownership for dDRM/rights is verified on-chain
//! at access time. A stale/empty index fails toward "show nothing + offer manual entry",
//! never toward minting into an unverified channel (#11 Fail Closed).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

/// On-disk index keyed by `network|factory|creator`. One entry per (network, factory,
/// creator) since the `ChannelCreated` scan is topic-filtered to a single creator.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ChannelIndexFile {
    #[serde(default)]
    pub(super) entries: BTreeMap<String, ChannelIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ChannelIndexEntry {
    /// Lower bound of the factory's history (the deploy block); backfill stops here.
    pub(super) deploy_block: u64,
    /// Lowest block scanned so far (inclusive). Equals `deploy_block` once fully backfilled.
    pub(super) floor: u64,
    /// Highest block scanned so far (inclusive); the forward/incremental cursor.
    pub(super) head: u64,
    /// `floor` has reached `deploy_block` — full history covered.
    pub(super) complete: bool,
    pub(super) channels: Vec<IndexedChannel>,
    pub(super) updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct IndexedChannel {
    pub(super) address: String,
    pub(super) block_number: u64,
    #[serde(default)]
    pub(super) channel_type: Option<u8>,
    #[serde(default)]
    pub(super) scope: Option<u8>,
}

impl ChannelIndexEntry {
    /// Insert a discovered channel, de-duplicated by address (case-insensitive); keeps the
    /// earliest seen block so ordering stays stable across rescans.
    pub(super) fn upsert(&mut self, address: &str, block_number: u64, ct: Option<u8>, scope: Option<u8>) {
        if let Some(existing) = self
            .channels
            .iter_mut()
            .find(|c| c.address.eq_ignore_ascii_case(address))
        {
            if block_number < existing.block_number {
                existing.block_number = block_number;
            }
            if existing.channel_type.is_none() {
                existing.channel_type = ct;
            }
            if existing.scope.is_none() {
                existing.scope = scope;
            }
            return;
        }
        self.channels.push(IndexedChannel {
            address: address.to_string(),
            block_number,
            channel_type: ct,
            scope,
        });
    }

    /// Channels newest-first (highest block first) — the order the picker shows them.
    pub(super) fn channels_newest_first(&self) -> Vec<&IndexedChannel> {
        let mut out: Vec<&IndexedChannel> = self.channels.iter().collect();
        out.sort_by(|a, b| b.block_number.cmp(&a.block_number));
        out
    }
}

/// Index key. Addresses lower-cased so the same creator/factory always maps to one entry.
pub(super) fn channel_index_key(network: &str, factory: &str, creator: &str) -> String {
    format!(
        "{}|{}|{}",
        network,
        factory.to_ascii_lowercase(),
        creator.to_ascii_lowercase()
    )
}

pub(super) fn channel_index_path(data_dir: &Path) -> PathBuf {
    data_dir.join("chain-provider").join("channel-index.json")
}

pub(super) fn read_channel_index_file(path: &Path) -> Result<ChannelIndexFile, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(ChannelIndexFile::default()),
        Err(err) => return Err(format!("read channel index: {err}")),
    };
    serde_json::from_str(&content).map_err(|err| format!("parse channel index: {err}"))
}

pub(super) fn write_channel_index_file(path: &Path, file: &ChannelIndexFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create channel index dir: {err}"))?;
    }
    let json =
        serde_json::to_vec_pretty(file).map_err(|err| format!("serialize channel index: {err}"))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, json).map_err(|err| format!("write channel index: {err}"))?;
    fs::rename(&tmp, path).map_err(|err| format!("commit channel index: {err}"))?;
    Ok(())
}
