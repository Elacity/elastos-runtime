#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::anyhow;
use anyhow::Result;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::io::IsTerminal;
use std::io::{self, Write};
use std::time::Duration;

const ESCAPE_SEQUENCE_SETTLE_WINDOW: Duration = Duration::from_millis(25);
const ESCAPE_SEQUENCE_BYTE_TIMEOUT_MS: i32 = 10;
pub(crate) const ESCAPE_SEQUENCE_MAX_BYTES: usize = 64;
pub(crate) const LIVE_REFRESH_POLL_MS: i32 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    MarkRead,
    Dismiss,
    Refresh,
    Quit,
    Help,
    Digit(usize),
    Mouse(MouseEvent),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MouseEvent {
    pub(crate) button: u16,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) released: bool,
}

pub(crate) struct TerminalGuard {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    original_termios: Option<libc::termios>,
}

impl TerminalGuard {
    pub(crate) fn enter() -> Result<Self> {
        let guard = Self::new()?;
        print!("\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[H");
        io::stdout().flush()?;
        Ok(guard)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn new() -> Result<Self> {
        Ok(Self {
            original_termios: enable_terminal_raw_mode()?,
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn new() -> Result<Self> {
        Ok(Self {})
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(original) = self.original_termios.as_ref() {
            let _ = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, original) };
        }
        let _ = write!(io::stdout(), "\x1b[?1006l\x1b[?1000l\x1b[?25h\x1b[?1049l");
        let _ = io::stdout().flush();
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn enable_terminal_raw_mode() -> Result<Option<libc::termios>> {
    if !io::stdin().is_terminal() {
        return Ok(None);
    }

    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    let rc = unsafe { libc::tcgetattr(libc::STDIN_FILENO, termios.as_mut_ptr()) };
    if rc != 0 {
        return Err(anyhow!(
            "failed to read terminal attributes: {}",
            io::Error::last_os_error()
        ));
    }

    let original = unsafe { termios.assume_init() };
    let mut raw = original;
    unsafe {
        libc::cfmakeraw(&mut raw);
    }
    let rc = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) };
    if rc != 0 {
        return Err(anyhow!(
            "failed to set terminal raw mode: {}",
            io::Error::last_os_error()
        ));
    }

    Ok(Some(original))
}

pub(crate) fn read_ui_key() -> Result<UiKey> {
    if !stdin_has_input(LIVE_REFRESH_POLL_MS)? {
        return Ok(UiKey::None);
    }
    let byte = read_stdin_byte()?;
    let key = match byte {
        b'q' | b'Q' => UiKey::Quit,
        b'r' | b'R' => UiKey::Refresh,
        b'm' | b'M' => UiKey::MarkRead,
        b'd' | b'D' => UiKey::Dismiss,
        b'?' => UiKey::Help,
        b'\n' | b'\r' => UiKey::Enter,
        b'\t' | b'l' | b'L' => UiKey::Right,
        b'h' | b'H' => UiKey::Left,
        b'j' | b'J' => UiKey::Down,
        b'k' | b'K' => UiKey::Up,
        b'1'..=b'9' => UiKey::Digit((byte - b'0') as usize),
        27 => read_escape_sequence()?,
        _ => UiKey::None,
    };

    Ok(key)
}

fn read_escape_sequence() -> Result<UiKey> {
    std::thread::sleep(ESCAPE_SEQUENCE_SETTLE_WINDOW);
    let mut seq = Vec::with_capacity(ESCAPE_SEQUENCE_MAX_BYTES);
    while seq.len() < ESCAPE_SEQUENCE_MAX_BYTES {
        if !stdin_has_input(ESCAPE_SEQUENCE_BYTE_TIMEOUT_MS)? {
            break;
        }
        let byte = read_stdin_byte()?;
        seq.push(byte);
        if is_escape_sequence_complete(&seq) {
            break;
        }
    }

    Ok(escape_sequence_key(&seq))
}

pub(crate) fn parse_escape_sequence_bytes(seq: &[u8]) -> UiKey {
    if let Some(key) = parse_sgr_mouse_sequence(seq) {
        return key;
    }
    if let Some(key) = parse_legacy_mouse_sequence(seq) {
        return key;
    }

    let Some((&prefix, rest)) = seq.split_first() else {
        return UiKey::None;
    };
    let Some(&last) = rest.last() else {
        return UiKey::None;
    };

    match (prefix, last) {
        (b'[', b'A') | (b'O', b'A') => UiKey::Up,
        (b'[', b'B') | (b'O', b'B') => UiKey::Down,
        (b'[', b'C') | (b'O', b'C') => UiKey::Right,
        (b'[', b'D') | (b'O', b'D') => UiKey::Left,
        (b'[', b'Z') => UiKey::Left,
        _ => UiKey::None,
    }
}

pub(crate) fn escape_sequence_key(seq: &[u8]) -> UiKey {
    if seq.is_empty() {
        UiKey::Quit
    } else {
        parse_escape_sequence_bytes(seq)
    }
}

fn parse_legacy_mouse_sequence(seq: &[u8]) -> Option<UiKey> {
    if seq.len() < 5 || !seq.starts_with(b"[M") {
        return None;
    }
    let button = seq[2].checked_sub(32)? as u16;
    let x = seq[3].checked_sub(32)? as u16;
    let y = seq[4].checked_sub(32)? as u16;
    Some(UiKey::Mouse(MouseEvent {
        button,
        x,
        y,
        released: button == 3,
    }))
}

pub(crate) fn is_escape_sequence_complete(seq: &[u8]) -> bool {
    if seq.starts_with(b"[M") {
        return seq.len() >= 5;
    }
    seq.last()
        .copied()
        .is_some_and(is_escape_sequence_terminator)
}

fn parse_sgr_mouse_sequence(seq: &[u8]) -> Option<UiKey> {
    if seq.len() < 6 || !seq.starts_with(b"[<") {
        return None;
    }
    let released = match seq.last().copied()? {
        b'M' => false,
        b'm' => true,
        _ => return None,
    };
    let payload = std::str::from_utf8(&seq[2..seq.len().saturating_sub(1)]).ok()?;
    let mut parts = payload.split(';');
    let button = parts.next()?.parse::<u16>().ok()?;
    let x = parts.next()?.parse::<u16>().ok()?;
    let y = parts.next()?.parse::<u16>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(UiKey::Mouse(MouseEvent {
        button,
        x,
        y,
        released,
    }))
}

fn is_escape_sequence_terminator(byte: u8) -> bool {
    matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'~')
}

pub(crate) fn term_cols() -> usize {
    std::env::var("ELASTOS_TERM_COLS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value >= 40)
        .unwrap_or(100)
}

pub(crate) fn term_rows() -> usize {
    std::env::var("ELASTOS_TERM_ROWS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value >= 20)
        .unwrap_or(32)
}

pub(crate) fn stdin_has_input(timeout_ms: i32) -> Result<bool> {
    let mut pollfd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };

    loop {
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err.into());
        }

        return Ok(ready != 0 && (pollfd.revents & libc::POLLIN) != 0);
    }
}

pub(crate) fn read_stdin_byte() -> Result<u8> {
    let mut byte = [0u8; 1];

    loop {
        let read = unsafe { libc::read(libc::STDIN_FILENO, byte.as_mut_ptr().cast(), 1) };
        if read == 1 {
            return Ok(byte[0]);
        }
        if read == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stdin closed").into());
        }

        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(err.into());
    }
}

pub(crate) fn wait_for_enter() -> Result<()> {
    print!("Press Enter to continue...");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(())
}
