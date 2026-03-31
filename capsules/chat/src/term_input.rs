use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawKey {
    pub code: Key,
    pub ctrl: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Null,
    Char(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Tab,
    BackTab,
    Esc,
    Up,
    Down,
    Insert,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseResult {
    Key(usize),
    Skip(usize),
    Incomplete,
}

#[derive(Debug)]
struct PendingInput {
    bytes: Vec<u8>,
    last_read: Option<Instant>,
}

fn pending_input() -> &'static Mutex<PendingInput> {
    static PENDING: OnceLock<Mutex<PendingInput>> = OnceLock::new();
    PENDING.get_or_init(|| {
        Mutex::new(PendingInput {
            bytes: Vec::new(),
            last_read: None,
        })
    })
}

pub fn has_pending_input() -> bool {
    !pending_input().lock().unwrap().bytes.is_empty()
}

#[cfg(not(target_os = "wasi"))]
pub fn poll_stdin(timeout_ms: i32) -> bool {
    unsafe {
        let mut pfd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        libc::poll(&mut pfd, 1, timeout_ms) > 0 && (pfd.revents & libc::POLLIN) != 0
    }
}

#[cfg(target_os = "wasi")]
pub fn poll_stdin(timeout_ms: i32) -> bool {
    unsafe {
        let timeout = if timeout_ms <= 0 {
            0
        } else {
            timeout_ms as u64 * 1_000_000
        };
        let subscriptions = [
            wasi::Subscription {
                userdata: 0,
                u: wasi::SubscriptionU {
                    tag: wasi::EVENTTYPE_FD_READ.raw(),
                    u: wasi::SubscriptionUU {
                        fd_read: wasi::SubscriptionFdReadwrite { file_descriptor: 0 },
                    },
                },
            },
            wasi::Subscription {
                userdata: 1,
                u: wasi::SubscriptionU {
                    tag: wasi::EVENTTYPE_CLOCK.raw(),
                    u: wasi::SubscriptionUU {
                        clock: wasi::SubscriptionClock {
                            id: wasi::CLOCKID_MONOTONIC,
                            timeout,
                            precision: 1_000_000,
                            flags: 0,
                        },
                    },
                },
            },
        ];
        let mut events = [
            std::mem::zeroed::<wasi::Event>(),
            std::mem::zeroed::<wasi::Event>(),
        ];
        match wasi::poll_oneoff(
            subscriptions.as_ptr(),
            events.as_mut_ptr(),
            subscriptions.len(),
        ) {
            Ok(n) if n > 0 => events[..n as usize]
                .iter()
                .any(|event| event.userdata == 0 && event.type_ == wasi::EVENTTYPE_FD_READ),
            _ => false,
        }
    }
}

pub fn drain_stdin() {
    #[cfg(target_os = "wasi")]
    {
        return;
    }

    #[cfg(not(target_os = "wasi"))]
    {
        let mut tmp = [0u8; 256];
        while poll_stdin(0) {
            if read_into(&mut tmp) <= 0 {
                break;
            }
        }
    }
}

pub fn read_keys() -> Vec<RawKey> {
    let mut pending = pending_input().lock().unwrap();
    if poll_stdin(0) {
        read_available(&mut pending.bytes);
        pending.last_read = Some(Instant::now());
    }

    for _ in 0..3 {
        if !has_incomplete_suffix(&pending.bytes) || !poll_stdin(5) {
            break;
        }
        read_available(&mut pending.bytes);
        pending.last_read = Some(Instant::now());
    }

    let stale_incomplete_suffix = has_incomplete_suffix(&pending.bytes)
        && pending
            .last_read
            .is_some_and(|last_read| last_read.elapsed() >= Duration::from_secs(2));

    if stale_incomplete_suffix {
        if pending.bytes == [0x1B] {
            pending.bytes.clear();
            return vec![RawKey {
                code: Key::Esc,
                ctrl: false,
            }];
        }
        discard_incomplete_suffix(&mut pending.bytes);
    }

    parse_buffer(&mut pending.bytes)
}

fn read_available(buf: &mut Vec<u8>) {
    let mut tmp = [0u8; 64];
    let n = read_into(&mut tmp);
    if n > 0 {
        buf.extend_from_slice(&tmp[..n as usize]);
    }
    while poll_stdin(0) {
        let n = read_into(&mut tmp);
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n as usize]);
    }
}

#[cfg(not(target_os = "wasi"))]
fn read_into(buf: &mut [u8]) -> isize {
    unsafe {
        libc::read(
            libc::STDIN_FILENO,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
        )
    }
}

#[cfg(target_os = "wasi")]
fn read_into(buf: &mut [u8]) -> isize {
    use std::io::Read;

    match std::io::stdin().lock().read(buf) {
        Ok(n) => n as isize,
        Err(_) => -1,
    }
}

fn parse_buffer(buf: &mut Vec<u8>) -> Vec<RawKey> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let mut key = RawKey {
            code: Key::Null,
            ctrl: false,
        };
        match parse_one(&buf[i..], &mut key) {
            ParseResult::Key(consumed) => {
                keys.push(key);
                i += consumed;
            }
            ParseResult::Skip(consumed) => {
                i += consumed;
            }
            ParseResult::Incomplete => break,
        }
    }

    if i > 0 {
        buf.drain(..i);
    }

    keys
}

fn discard_incomplete_suffix(bytes: &mut Vec<u8>) {
    if bytes.is_empty() {
        return;
    }

    let original_len = bytes.len();
    let mut idx = bytes.len();
    while idx > 0 {
        idx -= 1;
        if bytes[idx] == 0x1B {
            bytes.truncate(idx);
            return;
        }
    }

    if matches!(bytes.last(), Some(0xC2..=0xF4)) {
        bytes.truncate(original_len - 1);
    }
}

fn parse_one(bytes: &[u8], key: &mut RawKey) -> ParseResult {
    let b = bytes[0];
    match b {
        0x1B => parse_escape(bytes, key),
        0x9B => match parse_csi(&bytes[1..], key) {
            ParseResult::Key(consumed) => ParseResult::Key(consumed + 1),
            ParseResult::Skip(consumed) => ParseResult::Skip(consumed + 1),
            ParseResult::Incomplete => ParseResult::Incomplete,
        },
        0x08 | 0x7F => {
            *key = RawKey {
                code: Key::Backspace,
                ctrl: false,
            };
            ParseResult::Key(1)
        }
        0x0A | 0x0D => {
            let consumed = if bytes.get(1).copied() == Some(other_newline(b)) {
                2
            } else {
                1
            };
            *key = RawKey {
                code: Key::Enter,
                ctrl: false,
            };
            ParseResult::Key(consumed)
        }
        0x01..=0x1A => {
            let (code, ctrl) = match b {
                0x01 => (Key::Home, false),
                0x03 => (Key::Char('c'), true),
                0x04 => (Key::Delete, false),
                0x05 => (Key::End, false),
                0x09 => (Key::Tab, false),
                0x0E => (Key::Char('n'), true),
                0x10 => (Key::Char('p'), true),
                0x11 => (Key::Char('q'), true),
                _ => (Key::Char((b + b'a' - 1) as char), true),
            };
            *key = RawKey { code, ctrl };
            ParseResult::Key(1)
        }
        0x20..=0x7E => {
            *key = RawKey {
                code: Key::Char(b as char),
                ctrl: false,
            };
            ParseResult::Key(1)
        }
        0x80..=0xFF => parse_utf8(bytes, key),
        _ => ParseResult::Skip(1),
    }
}

fn parse_escape(bytes: &[u8], key: &mut RawKey) -> ParseResult {
    if bytes.len() == 1 {
        return ParseResult::Incomplete;
    }

    match bytes[1] {
        b'[' => match parse_csi(&bytes[2..], key) {
            ParseResult::Key(consumed) => ParseResult::Key(consumed + 2),
            ParseResult::Skip(consumed) => ParseResult::Skip(consumed + 2),
            ParseResult::Incomplete => ParseResult::Incomplete,
        },
        b'O' => {
            if bytes.len() < 3 {
                return ParseResult::Incomplete;
            }
            let code = match bytes[2] {
                b'A' => Some(Key::Up),
                b'B' => Some(Key::Down),
                b'C' => Some(Key::Right),
                b'D' => Some(Key::Left),
                b'H' => Some(Key::Home),
                b'F' => Some(Key::End),
                _ => None,
            };
            if let Some(code) = code {
                *key = RawKey { code, ctrl: false };
                ParseResult::Key(3)
            } else {
                ParseResult::Skip(3)
            }
        }
        _ => {
            *key = RawKey {
                code: Key::Esc,
                ctrl: false,
            };
            ParseResult::Key(1)
        }
    }
}

fn parse_csi(bytes: &[u8], key: &mut RawKey) -> ParseResult {
    let mut params = Vec::new();
    let mut num = 0u32;
    let mut has_num = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'0'..=b'9' => {
                num = num * 10 + (b - b'0') as u32;
                has_num = true;
                i += 1;
            }
            b';' => {
                params.push(if has_num { num } else { 0 });
                num = 0;
                has_num = false;
                i += 1;
            }
            _ => {
                if has_num {
                    params.push(num);
                }
                break;
            }
        }
    }

    if i >= bytes.len() {
        return ParseResult::Incomplete;
    }

    let final_byte = bytes[i];
    i += 1;

    let code = match final_byte {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'Z' => Key::BackTab,
        b'~' => match params.first() {
            Some(1) | Some(7) => Key::Home,
            Some(2) => Key::Insert,
            Some(3) => Key::Delete,
            Some(4) | Some(8) => Key::End,
            Some(5) => Key::PageUp,
            Some(6) => Key::PageDown,
            _ => return ParseResult::Skip(i),
        },
        _ => return ParseResult::Skip(i),
    };

    *key = RawKey { code, ctrl: false };
    ParseResult::Key(i)
}

fn parse_utf8(bytes: &[u8], key: &mut RawKey) -> ParseResult {
    let width = utf8_width(bytes[0]);
    if width == 0 {
        return ParseResult::Skip(1);
    }
    if bytes.len() < width {
        return ParseResult::Incomplete;
    }
    match std::str::from_utf8(&bytes[..width]) {
        Ok(s) => match s.chars().next() {
            Some(ch) if !ch.is_control() => {
                *key = RawKey {
                    code: Key::Char(ch),
                    ctrl: false,
                };
                ParseResult::Key(width)
            }
            _ => ParseResult::Skip(width),
        },
        Err(_) => ParseResult::Skip(width),
    }
}

fn utf8_width(first: u8) -> usize {
    match first {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 0,
    }
}

fn other_newline(b: u8) -> u8 {
    if b == 0x0D {
        0x0A
    } else {
        0x0D
    }
}

fn has_incomplete_suffix(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }

    match bytes[bytes.len() - 1] {
        0x1B => return true,
        0xC2..=0xF4 => return true,
        _ => {}
    }

    if bytes.len() >= 2 && bytes[bytes.len() - 2] == 0x1B {
        return matches!(bytes[bytes.len() - 1], b'[' | b'O');
    }

    let mut idx = bytes.len();
    while idx > 0 {
        idx -= 1;
        if bytes[idx] == 0x1B {
            if idx + 1 >= bytes.len() {
                return true;
            }
            return match bytes[idx + 1] {
                b'[' => bytes[idx + 2..]
                    .iter()
                    .all(|b| b.is_ascii_digit() || *b == b';'),
                b'O' => idx + 2 >= bytes.len(),
                _ => false,
            };
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(bytes: &[u8]) -> (Vec<RawKey>, Vec<u8>) {
        let mut buf = bytes.to_vec();
        let keys = parse_buffer(&mut buf);
        (keys, buf)
    }

    #[test]
    fn ascii_input_parses() {
        let (keys, rest) = parse_all(b"ab");
        assert_eq!(
            keys,
            vec![
                RawKey {
                    code: Key::Char('a'),
                    ctrl: false
                },
                RawKey {
                    code: Key::Char('b'),
                    ctrl: false
                },
            ]
        );
        assert!(rest.is_empty());
    }

    #[test]
    fn csi_left_parses() {
        let (keys, rest) = parse_all(b"\x1b[D");
        assert_eq!(
            keys,
            vec![RawKey {
                code: Key::Left,
                ctrl: false
            }]
        );
        assert!(rest.is_empty());
    }
}
