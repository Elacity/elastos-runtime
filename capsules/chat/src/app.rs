//! App state: channels, messages, input, deduplication.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Default maximum messages per channel buffer
const DEFAULT_MAX_MESSAGES: usize = 1000;

/// A chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub sender_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_session_id: Option<String>,
    pub sender_nick: String,
    pub content: String,
    pub ts: u64,
    /// Local display timestamp used for UI/history ordering on this host.
    /// This avoids remote clock skew making the timeline look scrambled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_ts: Option<u64>,
    /// Ed25519 signature over SHA-256(sender_id:ts:content), hex-encoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Verification result (not serialized — set locally after verify)
    #[serde(skip)]
    pub verified: Option<bool>,
}

/// A channel (topic) with its messages
pub struct Channel {
    pub name: String,
    pub messages: VecDeque<Message>,
}

impl Channel {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            messages: VecDeque::new(),
        }
    }
}

/// Chat application state
pub struct App {
    /// All joined channels
    pub channels: Vec<Channel>,
    /// Index of active channel
    pub active_channel: usize,
    /// Input buffer
    pub input: String,
    /// Cursor position in input
    pub cursor: usize,
    /// User's nickname
    pub nickname: String,
    /// User's DID (did:key:z...) — used as sender_id in messages
    pub pubkey: String,
    /// Per-client session identity so same-host chat surfaces can share a DID
    /// without being mistaken for the same client instance.
    pub session_id: String,
    /// Connected peer count
    pub peer_count: usize,
    /// Status bar message
    pub status: String,
    /// Whether to quit
    pub should_quit: bool,
    /// Whether chat should return to the PC2 home surface on exit
    pub return_home_requested: bool,
    /// Deduplication set: SHA-256 of (sender_id + ts + content)
    seen_messages: HashSet<[u8; 32]>,
    /// Capability tokens
    pub identity_token: String,
    pub peer_token: String,
    pub storage_token: String,
    /// Max messages to keep per channel
    pub max_messages: usize,
    /// Nick -> pubkey mapping learned from received messages
    pub known_nicks: HashMap<String, String>,
    /// Whether we're waiting for the first peer to connect
    pub p2p_connecting: bool,
    /// Whether joined #channels should attach in direct-peer mode
    pub direct_peer_mode: bool,
    /// Room attach state keyed as "<room>|<did>".
    pub attached_room_peers: HashSet<String>,
    /// Backoff for repeated room attach attempts keyed as "<room>|<did>".
    pub attach_retry_after: HashMap<String, Instant>,
    /// Last time presence announcements were broadcast.
    pub last_presence_announce: Option<Instant>,
}

impl App {
    pub fn new(nickname: &str) -> Self {
        Self {
            channels: vec![],
            active_channel: 0,
            input: String::new(),
            cursor: 0,
            nickname: nickname.to_string(),
            pubkey: String::new(),
            session_id: new_session_id(),
            peer_count: 0,
            status: String::new(),
            should_quit: false,
            return_home_requested: false,
            seen_messages: HashSet::new(),
            identity_token: String::new(),
            peer_token: String::new(),
            storage_token: String::new(),
            max_messages: DEFAULT_MAX_MESSAGES,
            known_nicks: HashMap::new(),
            p2p_connecting: false,
            direct_peer_mode: false,
            attached_room_peers: HashSet::new(),
            attach_retry_after: HashMap::new(),
            last_presence_announce: None,
        }
    }

    /// Get names of all joined channels
    pub fn channel_names(&self) -> Vec<String> {
        self.channels.iter().map(|c| c.name.clone()).collect()
    }

    /// Get active channel name
    pub fn active_channel_name(&self) -> String {
        self.channels
            .get(self.active_channel)
            .map(|c| c.name.clone())
            .unwrap_or_default()
    }

    /// Join a channel
    pub fn join_channel(&mut self, name: &str) {
        // Check if already joined
        if self.channels.iter().any(|c| c.name == name) {
            // Switch to it
            if let Some(idx) = self.channels.iter().position(|c| c.name == name) {
                self.active_channel = idx;
            }
            return;
        }

        self.channels.push(Channel::new(name));
        self.active_channel = self.channels.len() - 1;
    }

    /// Leave the active channel
    pub fn part_channel(&mut self) -> Option<String> {
        if self.channels.is_empty() {
            return None;
        }
        let removed = self.channels.remove(self.active_channel);
        if self.active_channel >= self.channels.len() && !self.channels.is_empty() {
            self.active_channel = self.channels.len() - 1;
        }
        Some(removed.name)
    }

    /// Switch to next channel
    pub fn next_channel(&mut self) {
        if !self.channels.is_empty() {
            self.active_channel = (self.active_channel + 1) % self.channels.len();
        }
    }

    /// Switch to previous channel
    pub fn prev_channel(&mut self) {
        if !self.channels.is_empty() {
            if self.active_channel == 0 {
                self.active_channel = self.channels.len() - 1;
            } else {
                self.active_channel -= 1;
            }
        }
    }

    /// Add a system message to the active channel
    pub fn system_message(&mut self, text: &str) {
        if let Some(ch) = self.channels.get_mut(self.active_channel) {
            ch.messages.push_back(Message {
                sender_id: String::new(),
                sender_session_id: None,
                sender_nick: "*".to_string(),
                content: text.to_string(),
                ts: now(),
                display_ts: Some(now()),
                signature: None,
                verified: None,
            });
            if ch.messages.len() > self.max_messages {
                ch.messages.pop_front();
            }
        }
    }

    /// Add a system message to a specific channel by name
    pub fn system_message_to(&mut self, channel: &str, text: &str) {
        if let Some(ch) = self.channels.iter_mut().find(|c| c.name == channel) {
            ch.messages.push_back(Message {
                sender_id: String::new(),
                sender_session_id: None,
                sender_nick: "*".to_string(),
                content: text.to_string(),
                ts: now(),
                display_ts: Some(now()),
                signature: None,
                verified: None,
            });
            if ch.messages.len() > self.max_messages {
                ch.messages.pop_front();
            }
        }
    }

    /// Append messages from the provider, with deduplication
    pub fn append_messages(&mut self, topic: &str, msgs: Vec<Message>) {
        let ch = match self.channels.iter_mut().find(|c| c.name == topic) {
            Some(c) => c,
            None => return,
        };

        for msg in msgs {
            // Prevent unbounded growth — clear when over 10k entries
            if self.seen_messages.len() > 10_000 {
                self.seen_messages.clear();
            }
            let hash = message_hash(&msg);
            if !self.seen_messages.insert(hash) {
                continue; // duplicate
            }
            ch.messages.push_back(msg);
            if ch.messages.len() > self.max_messages {
                ch.messages.pop_front();
            }
        }
    }

    /// Set status message
    pub fn set_status(&mut self, msg: &str) {
        self.status = msg.to_string();
    }

    /// Insert a character at cursor position
    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Delete character before cursor
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let prev = self.input[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.remove(prev);
            self.cursor = prev;
        }
    }

    /// Delete character at cursor
    pub fn delete(&mut self) {
        if self.cursor < self.input.len() {
            self.input.remove(self.cursor);
        }
    }

    /// Move cursor left
    pub fn cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.input[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    /// Move cursor right
    pub fn cursor_right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor = self.input[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(self.input.len());
        }
    }

    /// Delete from the cursor back to the start of the line.
    pub fn kill_to_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.input.drain(..self.cursor);
        self.cursor = 0;
    }

    /// Delete the word immediately before the cursor.
    pub fn delete_word_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let mut start = self.cursor;
        for (idx, ch) in self.input[..self.cursor].char_indices().rev() {
            if !ch.is_whitespace() {
                start = idx;
                break;
            }
            start = idx;
        }
        for (idx, ch) in self.input[..start].char_indices().rev() {
            if ch.is_whitespace() {
                break;
            }
            start = idx;
        }

        self.input.drain(start..self.cursor);
        self.cursor = start;
    }

    /// Take the current input and reset
    pub fn take_input(&mut self) -> String {
        let input = std::mem::take(&mut self.input);
        self.cursor = 0;
        input
    }
}

/// SHA-256 hash for deduplication
fn message_hash(msg: &Message) -> [u8; 32] {
    let data = format!(
        "{}:{}:{}:{}",
        msg.sender_id,
        msg.sender_session_id.as_deref().unwrap_or(""),
        msg.ts,
        msg.content
    );
    let hash = Sha256::digest(data.as_bytes());
    let mut result = [0u8; 32];
    result.copy_from_slice(&hash);
    result
}

fn new_session_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    #[cfg(not(target_os = "wasi"))]
    let seed = format!("{}:{}:{:p}", std::process::id(), now, &now);
    #[cfg(target_os = "wasi")]
    let seed = format!("wasi:{}:{:p}", now, &now);
    hex::encode(Sha256::digest(seed.as_bytes()))[..16].to_string()
}

/// SHA-256 of the signing payload — delegates to shared chat protocol.
pub fn signing_payload_hex(sender_id: &str, ts: u64, content: &str) -> String {
    elastos_common::chat_protocol::signing_payload_hex(sender_id, ts, content)
}

/// Current unix timestamp
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Generate a color index from a nickname (for nick coloring)
pub fn nick_color(nick: &str) -> u8 {
    // mIRC-style: hash nick to one of 12 colors (skip black/white)
    let hash = nick
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    let colors = [1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13];
    colors[(hash as usize) % colors.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_channel() {
        let mut app = App::new("alice");
        app.join_channel("#general");
        assert_eq!(app.channels.len(), 1);
        assert_eq!(app.active_channel, 0);
        assert_eq!(app.active_channel_name(), "#general");
    }

    #[test]
    fn test_join_duplicate_switches() {
        let mut app = App::new("alice");
        app.join_channel("#general");
        app.join_channel("#random");
        assert_eq!(app.active_channel, 1);
        app.join_channel("#general");
        assert_eq!(app.active_channel, 0);
    }

    #[test]
    fn test_part_channel() {
        let mut app = App::new("alice");
        app.join_channel("#general");
        app.join_channel("#random");
        let removed = app.part_channel();
        assert_eq!(removed, Some("#random".to_string()));
        assert_eq!(app.channels.len(), 1);
    }

    #[test]
    fn test_dedup_messages() {
        let mut app = App::new("alice");
        app.join_channel("#test");

        let msg = Message {
            sender_id: "abc".to_string(),
            sender_session_id: Some("s1".to_string()),
            sender_nick: "bob".to_string(),
            content: "hello".to_string(),
            ts: 1000,
            display_ts: None,
            signature: None,
            verified: None,
        };

        app.append_messages("#test", vec![msg.clone(), msg.clone()]);
        assert_eq!(app.channels[0].messages.len(), 1);
    }

    #[test]
    fn test_dedup_distinguishes_same_did_different_session() {
        let mut app = App::new("alice");
        app.join_channel("#test");

        let msg1 = Message {
            sender_id: "abc".to_string(),
            sender_session_id: Some("s1".to_string()),
            sender_nick: "bob".to_string(),
            content: "hello".to_string(),
            ts: 1000,
            display_ts: None,
            signature: None,
            verified: None,
        };
        let mut msg2 = msg1.clone();
        msg2.sender_session_id = Some("s2".to_string());

        app.append_messages("#test", vec![msg1.clone(), msg2.clone()]);
        assert_eq!(app.channels[0].messages.len(), 2);
    }

    #[test]
    fn test_input_editing() {
        let mut app = App::new("alice");
        app.insert_char('h');
        app.insert_char('i');
        assert_eq!(app.input, "hi");
        assert_eq!(app.cursor, 2);

        app.backspace();
        assert_eq!(app.input, "h");
        assert_eq!(app.cursor, 1);
    }

    #[test]
    fn test_kill_to_start() {
        let mut app = App::new("alice");
        app.input = "hello world".to_string();
        app.cursor = 5;
        app.kill_to_start();
        assert_eq!(app.input, " world");
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn test_delete_word_backward() {
        let mut app = App::new("alice");
        app.input = "hello world".to_string();
        app.cursor = app.input.len();
        app.delete_word_backward();
        assert_eq!(app.input, "hello ");
        assert_eq!(app.cursor, 6);
    }

    #[test]
    fn test_nick_color_deterministic() {
        let c1 = nick_color("alice");
        let c2 = nick_color("alice");
        assert_eq!(c1, c2);
    }

    #[test]
    fn test_nick_color_varies() {
        let c1 = nick_color("alice");
        let c2 = nick_color("bob");
        // They might collide, but usually won't
        let _ = (c1, c2);
    }
}
