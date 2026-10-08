//! Per-Profile sliding-window limits on Community messages.
//!
//! The sending Home limits each local Profile, so a person sees "Slow down"
//! before anything leaves the Home. Every receiving Home limits each remote
//! Profile at the same rate, so a modified Home that skips its own limit still
//! cannot flood the room. A receiving Home holds a refused message and retries
//! it itself, because its sender stops resending once any other Home accepts
//! it. Receive windows and held messages live in memory and reset on restart.

use std::collections::{HashMap, VecDeque};

use elastos_common::collaboration_protocol::MAX_COLLABORATION_ENVELOPE_BYTES;
use std::sync::{Mutex, PoisonError};

/// Payload types the Community limits count. Presence is periodic and bounded
/// by its own cadence.
pub(crate) const COMMUNITY_RATE_LIMITED_PAYLOAD_TYPE: &str = "elastos.chat.message/v1";
pub(crate) const COMMUNITY_RATE_WINDOW_SECS: u64 = 10;
pub(crate) const COMMUNITY_SENDS_PER_WINDOW: usize = 5;
/// Equal to the send limit, as #139 records. Honest messages that network
/// delay bunches into one window are held and retried by the receiving Home.
pub(crate) const COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW: usize = 5;
/// Only accepted messages open a window, and the core's durable backlog caps
/// how many land in one window, so honest use stays far below this bound.
const MAX_TRACKED_PROFILES: usize = 4_096;
/// A sender within its limits never needs more than one window of held
/// messages; more than that is a flood and is dropped.
const MAX_HELD_FRAMES_PER_SENDER: usize = COMMUNITY_RECEIVES_PER_SENDER_PER_WINDOW;
const MAX_HELD_FRAMES: usize = 64;
const MAX_HELD_BYTES: usize = 16 * MAX_COLLABORATION_ENVELOPE_BYTES;

/// A local Community message refused by the send limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommunitySendRateLimited {
    pub(crate) retry_after_secs: u64,
}

impl std::fmt::Display for CommunitySendRateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Slow down. You can send another message in {} seconds.",
            self.retry_after_secs
        )
    }
}

impl std::error::Error for CommunitySendRateLimited {}

pub(crate) struct ProfileRateLimiter {
    per_window: usize,
    window_secs: u64,
    profiles: Mutex<HashMap<String, VecDeque<u64>>>,
}

impl ProfileRateLimiter {
    pub(crate) fn new(per_window: usize, window_secs: u64) -> Self {
        Self {
            per_window,
            window_secs,
            profiles: Mutex::new(HashMap::new()),
        }
    }

    /// Returns the seconds until `profile_did` may send again, or `Ok` when a
    /// message at `now` fits. Checking records nothing. An open window is never
    /// forgotten early, so a full table refuses a new Profile instead.
    pub(crate) fn check(&self, profile_did: &str, now: u64) -> Result<(), u64> {
        let mut profiles = self.profiles.lock().unwrap_or_else(PoisonError::into_inner);
        self.prune(&mut profiles, now);
        match profiles.get(profile_did) {
            Some(times) if times.len() >= self.per_window => Err(self.retry_after(times, now)),
            Some(_) => Ok(()),
            None if profiles.len() >= MAX_TRACKED_PROFILES => Err(profiles
                .values()
                .map(|times| self.retry_after(times, now))
                .min()
                .unwrap_or(self.window_secs)),
            None => Ok(()),
        }
    }

    /// Counts one admitted message. Call it only after `check` passed under the
    /// same serialization, so the table stays within its bound.
    pub(crate) fn record(&self, profile_did: &str, now: u64) {
        let mut profiles = self.profiles.lock().unwrap_or_else(PoisonError::into_inner);
        profiles
            .entry(profile_did.to_string())
            .or_default()
            .push_back(now);
    }

    fn retry_after(&self, times: &VecDeque<u64>, now: u64) -> u64 {
        times
            .front()
            .map_or(self.window_secs, |oldest| {
                oldest.saturating_add(self.window_secs).saturating_sub(now)
            })
            .max(1)
    }

    fn prune(&self, profiles: &mut HashMap<String, VecDeque<u64>>, now: u64) {
        profiles.retain(|_, times| {
            while times
                .front()
                .is_some_and(|time| time.saturating_add(self.window_secs) <= now)
            {
                times.pop_front();
            }
            !times.is_empty()
        });
    }
}

/// One received transport frame refused by a per-sender limit, kept for retry.
pub(crate) struct HeldFrame {
    frame: Vec<u8>,
    envelope_sha256: String,
    sender_profile_did: String,
    expires_at: u64,
}

impl HeldFrame {
    pub(crate) fn new(
        frame: &[u8],
        envelope_sha256: &str,
        sender_profile_did: &str,
        expires_at: u64,
    ) -> Self {
        Self {
            frame: frame.to_vec(),
            envelope_sha256: envelope_sha256.to_string(),
            sender_profile_did: sender_profile_did.to_string(),
            expires_at,
        }
    }

    pub(crate) fn frame(&self) -> &[u8] {
        &self.frame
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoldOutcome {
    /// Kept for retry, or already held.
    Held,
    /// Expired, or its sender already holds a full window: a flood, dropped.
    Dropped,
    /// The Home is full of senders holding no more than this one would. The
    /// caller keeps the frame in transport and retries the batch later.
    HomeFull,
}

/// Bounded, in-order store of refused frames that the receiving Home retries.
pub(crate) struct HeldFrames {
    frames: Mutex<VecDeque<HeldFrame>>,
    max_frames: usize,
}

impl Default for HeldFrames {
    fn default() -> Self {
        Self {
            frames: Mutex::new(VecDeque::new()),
            max_frames: MAX_HELD_FRAMES,
        }
    }
}

impl HeldFrames {
    /// A smaller Home bound, so a test can fill it with few frames.
    #[cfg(test)]
    pub(crate) fn with_max_frames(max_frames: usize) -> Self {
        Self {
            max_frames,
            ..Self::default()
        }
    }

    /// Keeps `held` for a later retry. When the Home's bound is full, the
    /// sender holding the most gives up its newest frame first, so a flood
    /// across many Profiles cannot push out a sender holding little.
    pub(crate) fn hold(&self, held: HeldFrame, now: u64) -> HoldOutcome {
        let mut frames = self.frames.lock().unwrap_or_else(PoisonError::into_inner);
        frames.retain(|frame| frame.expires_at > now);
        if frames
            .iter()
            .any(|frame| frame.envelope_sha256 == held.envelope_sha256)
        {
            return HoldOutcome::Held;
        }
        let count_from = |frames: &VecDeque<HeldFrame>, sender: &str| {
            frames
                .iter()
                .filter(|frame| frame.sender_profile_did == sender)
                .count()
        };
        let from_sender = count_from(&frames, &held.sender_profile_did);
        if held.expires_at <= now || from_sender >= MAX_HELD_FRAMES_PER_SENDER {
            return HoldOutcome::Dropped;
        }
        // Plan every eviction first and apply it only if the frame then fits,
        // so a refusal leaves the queue exactly as it was.
        let mut counts = std::collections::BTreeMap::<&str, usize>::new();
        for frame in frames.iter() {
            *counts.entry(frame.sender_profile_did.as_str()).or_default() += 1;
        }
        let mut len = frames.len();
        let mut bytes: usize = frames.iter().map(|frame| frame.frame.len()).sum();
        let mut evicted = Vec::new();
        while len >= self.max_frames || bytes.saturating_add(held.frame.len()) > MAX_HELD_BYTES {
            let Some((largest, count)) = counts
                .iter()
                .max_by(|left, right| left.1.cmp(right.1).then(right.0.cmp(left.0)))
                .map(|(sender, count)| (sender.to_string(), *count))
            else {
                return HoldOutcome::HomeFull;
            };
            if count <= from_sender + 1 {
                return HoldOutcome::HomeFull;
            }
            let Some(index) = (0..frames.len()).rev().find(|index| {
                frames[*index].sender_profile_did == largest && !evicted.contains(index)
            }) else {
                return HoldOutcome::HomeFull;
            };
            evicted.push(index);
            *counts
                .entry(frames[index].sender_profile_did.as_str())
                .or_default() -= 1;
            len -= 1;
            bytes -= frames[index].frame.len();
        }
        drop(counts);
        evicted.sort_unstable();
        for index in evicted.into_iter().rev() {
            frames.remove(index);
        }
        frames.push_back(held);
        HoldOutcome::Held
    }

    /// Takes every unexpired held frame, oldest first.
    pub(crate) fn take(&self, now: u64) -> Vec<HeldFrame> {
        let mut frames = self.frames.lock().unwrap_or_else(PoisonError::into_inner);
        frames.retain(|frame| frame.expires_at > now);
        frames.drain(..).collect()
    }

    /// Puts frames taken by `take` back in front, keeping their order, when a
    /// retry stopped before reaching them.
    pub(crate) fn restore(&self, held: Vec<HeldFrame>) {
        let mut frames = self.frames.lock().unwrap_or_else(PoisonError::into_inner);
        for frame in held.into_iter().rev() {
            if !frames
                .iter()
                .any(|stored| stored.envelope_sha256 == frame.envelope_sha256)
            {
                frames.push_front(frame);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_each_profile_within_its_window() {
        let limiter = ProfileRateLimiter::new(2, 10);
        for now in [100, 101] {
            limiter.check("did:alice", now).unwrap();
            limiter.record("did:alice", now);
        }
        assert_eq!(limiter.check("did:alice", 102), Err(8));
        // Another Profile keeps its own window.
        limiter.check("did:bob", 102).unwrap();
        // A refused check counts nothing; the window slides.
        assert_eq!(limiter.check("did:alice", 109), Err(1));
        limiter.check("did:alice", 110).unwrap();
    }

    #[test]
    fn a_full_table_refuses_new_profiles_and_keeps_open_windows() {
        let limiter = ProfileRateLimiter::new(1, 10);
        limiter.record("did:flooder", 100);
        for index in 1..MAX_TRACKED_PROFILES {
            limiter.check(&format!("did:{index}"), 101).unwrap();
            limiter.record(&format!("did:{index}"), 101);
        }
        // Fresh Profiles cannot push out a sender whose window is still open.
        assert_eq!(limiter.check("did:fresh", 102), Err(8));
        assert_eq!(limiter.check("did:flooder", 102), Err(8));
        // Expired windows free their rows.
        limiter.check("did:fresh", 111).unwrap();
    }

    fn held(id: usize, sender: &str, expires_at: u64) -> HeldFrame {
        sized(id, sender, expires_at, 8)
    }

    fn sized(id: usize, sender: &str, expires_at: u64, bytes: usize) -> HeldFrame {
        HeldFrame::new(&vec![0; bytes], &format!("hash-{id}"), sender, expires_at)
    }

    fn senders(frames: &HeldFrames) -> Vec<String> {
        frames
            .frames
            .lock()
            .unwrap()
            .iter()
            .map(|frame| frame.sender_profile_did.clone())
            .collect()
    }

    #[test]
    fn held_frames_are_bounded_per_sender_deduplicated_and_expire() {
        let frames = HeldFrames::default();
        for id in 0..MAX_HELD_FRAMES_PER_SENDER {
            assert_eq!(
                frames.hold(held(id, "did:flood", 200), 100),
                HoldOutcome::Held
            );
        }
        assert_eq!(
            frames.hold(held(99, "did:flood", 200), 100),
            HoldOutcome::Dropped
        );
        assert_eq!(
            frames.hold(held(0, "did:flood", 200), 100),
            HoldOutcome::Held
        );
        assert_eq!(
            frames.hold(held(100, "did:other", 150), 100),
            HoldOutcome::Held
        );
        assert_eq!(
            frames.hold(held(101, "did:other", 100), 100),
            HoldOutcome::Dropped
        );

        let taken = frames.take(160);
        assert_eq!(taken.len(), MAX_HELD_FRAMES_PER_SENDER);
        assert!(frames.take(160).is_empty());
        frames.restore(taken);
        let order = frames
            .take(160)
            .into_iter()
            .map(|frame| frame.envelope_sha256)
            .collect::<Vec<_>>();
        assert_eq!(order.first().map(String::as_str), Some("hash-0"));
        assert_eq!(order.len(), MAX_HELD_FRAMES_PER_SENDER);
    }

    #[test]
    fn a_full_home_takes_room_from_the_largest_holder_first() {
        let frames = HeldFrames::default();
        let mut id = 0;
        'fill: for flooder in 0.. {
            for _ in 0..MAX_HELD_FRAMES_PER_SENDER {
                if frames.frames.lock().unwrap().len() == MAX_HELD_FRAMES {
                    break 'fill;
                }
                frames.hold(held(id, &format!("did:flood-{flooder}"), 200), 100);
                id += 1;
            }
        }
        let full_holders = |frames: &HeldFrames| {
            senders(frames)
                .iter()
                .fold(
                    std::collections::BTreeMap::<String, usize>::new(),
                    |mut counts, sender| {
                        *counts.entry(sender.clone()).or_default() += 1;
                        counts
                    },
                )
                .into_values()
                .filter(|count| *count == MAX_HELD_FRAMES_PER_SENDER)
                .count()
        };
        let full_before = full_holders(&frames);
        assert_eq!(
            frames.hold(held(id, "did:honest", 200), 100),
            HoldOutcome::Held
        );
        let held_senders = senders(&frames);
        assert_eq!(held_senders.len(), MAX_HELD_FRAMES);
        assert_eq!(held_senders.last().map(String::as_str), Some("did:honest"));
        // One of the largest holders gave up its newest frame.
        assert_eq!(full_holders(&frames), full_before - 1);
    }

    #[test]
    fn a_full_byte_budget_takes_room_from_the_largest_holder_first() {
        let frames = HeldFrames::default();
        let large = MAX_COLLABORATION_ENVELOPE_BYTES;
        // did:a is the one largest holder; the others hold one frame fewer.
        for id in 0..MAX_HELD_BYTES / large {
            let sender = if id < MAX_HELD_FRAMES_PER_SENDER {
                "did:a".to_string()
            } else {
                format!(
                    "did:b{}",
                    (id - MAX_HELD_FRAMES_PER_SENDER) / (MAX_HELD_FRAMES_PER_SENDER - 1)
                )
            };
            assert_eq!(
                frames.hold(sized(id, &sender, 200, large), 100),
                HoldOutcome::Held
            );
        }
        assert_eq!(
            frames.hold(sized(999, "did:honest", 200, large), 100),
            HoldOutcome::Held
        );
        let held_senders = senders(&frames);
        assert_eq!(
            held_senders
                .iter()
                .filter(|sender| *sender == "did:a")
                .count(),
            MAX_HELD_FRAMES_PER_SENDER - 1
        );
        assert!(held_senders.iter().any(|sender| sender == "did:honest"));
    }

    #[test]
    fn a_refused_hold_keeps_every_held_frame() {
        // Codex round 4: many large single frames plus two small frames from
        // one sender, just under the byte budget. Evicting the small frames
        // cannot make room for another large one, so nothing may be removed.
        let frames = HeldFrames::default();
        let large = 63 * 1024;
        let flooders = (MAX_HELD_BYTES - 2 * 3 * 1024) / large;
        for id in 0..flooders {
            assert_eq!(
                frames.hold(sized(id, &format!("did:flood-{id}"), 200, large), 100),
                HoldOutcome::Held
            );
        }
        for id in 0..2 {
            assert_eq!(
                frames.hold(sized(1_000 + id, "did:honest", 200, 3 * 1024), 100),
                HoldOutcome::Held
            );
        }
        let before = senders(&frames);
        assert_eq!(
            frames.hold(sized(2_000, "did:new-flooder", 200, large), 100),
            HoldOutcome::HomeFull
        );
        assert_eq!(senders(&frames), before);
    }

    #[test]
    fn a_home_full_of_equal_holders_asks_the_transport_to_wait() {
        let frames = HeldFrames::default();
        for id in 0..MAX_HELD_FRAMES {
            assert_eq!(
                frames.hold(held(id, &format!("did:{id}"), 200), 100),
                HoldOutcome::Held
            );
        }
        assert_eq!(
            frames.hold(held(MAX_HELD_FRAMES, "did:late", 200), 100),
            HoldOutcome::HomeFull
        );
        assert_eq!(senders(&frames).len(), MAX_HELD_FRAMES);
    }

    #[test]
    fn send_refusal_reads_as_slow_down() {
        let text = CommunitySendRateLimited {
            retry_after_secs: 4,
        }
        .to_string();
        assert!(text.starts_with("Slow down."), "{text}");
    }
}
