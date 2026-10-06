//! Per-component progress lines for `elastos setup`.
//!
//! A terminal gets one line per component, redrawn while its bytes arrive.
//! Pipes, CI, NO_COLOR and dumb terminals get one plain line when each
//! component finishes. Progress observes bytes that are already being read;
//! what is fetched, verified and written stays the same.

use std::future::Future;
use std::io::{IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// A rich line fits 80 columns: 24-character names and a 12-cell bar.
const BAR_CELLS: u64 = 12;
const REDRAW_INTERVAL: Duration = Duration::from_millis(100);
pub(super) const NAME_WIDTH_LIMIT: usize = 24;

tokio::task_local! {
    static REPORTING: ();
}

/// True inside [`ComponentProgress::track`]: the component line reports the
/// result, so download helpers print no per-step lines of their own.
pub(super) fn reporting() -> bool {
    REPORTING.try_with(|_| ()).is_ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OutputMode {
    Rich,
    Plain,
}

impl OutputMode {
    pub(super) fn detect() -> Self {
        let set = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        Self::from_environment(
            std::io::stdout().is_terminal(),
            set("NO_COLOR"),
            set("CI"),
            std::env::var("TERM").ok().as_deref(),
        )
    }

    fn from_environment(terminal: bool, no_color: bool, ci: bool, term: Option<&str>) -> Self {
        let dumb = matches!(term, None | Some("") | Some("dumb"));
        if terminal && !no_color && !ci && !dumb {
            Self::Rich
        } else {
            Self::Plain
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Running,
    Done,
    Failed,
}

struct Line<'a> {
    mode: OutputMode,
    index: usize,
    total: usize,
    name: &'a str,
    name_width: usize,
    size: Option<u64>,
    received: u64,
    elapsed: Duration,
    status: Status,
    detail: Option<&'a str>,
}

pub(super) fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 10 {
        format!("{:.1} s", elapsed.as_secs_f64())
    } else if seconds < 60 {
        format!("{seconds} s")
    } else {
        format!("{} min {} s", seconds / 60, seconds % 60)
    }
}

pub(super) fn format_size(bytes: u64) -> String {
    crate::update::format_bytes(usize::try_from(bytes).unwrap_or(usize::MAX))
}

fn fit(name: &str, width: usize) -> String {
    if name.chars().count() <= width || width == 0 {
        return name.to_string();
    }
    let mut short: String = name.chars().take(width - 1).collect();
    short.push('…');
    short
}

fn prefix(mode: OutputMode, index: usize, total: usize) -> String {
    match mode {
        OutputMode::Rich => {
            let digits = total.to_string().len();
            format!("[{index:>digits$}/{total}]")
        }
        OutputMode::Plain => format!("[{index}/{total}]"),
    }
}

fn render(line: &Line) -> String {
    let elapsed = format_elapsed(line.elapsed);
    let size = line.size.filter(|size| *size > 0);
    match line.mode {
        OutputMode::Rich => {
            let mut out = format!(
                "  {} {:<width$}",
                prefix(line.mode, line.index, line.total),
                fit(line.name, line.name_width),
                width = line.name_width
            );
            if let Some(size) = size {
                let received = match line.status {
                    Status::Done => size,
                    _ => line.received.min(size),
                };
                let filled =
                    (u128::from(received) * u128::from(BAR_CELLS) / u128::from(size)) as u64;
                let percent = u128::from(received) * 100 / u128::from(size);
                out.push_str(&format!(
                    "  {:>8}  {}{}  {:>3}%",
                    format_size(size),
                    "━".repeat(filled as usize),
                    "─".repeat((BAR_CELLS - filled) as usize),
                    percent
                ));
            } else if line.received > 0 {
                out.push_str(&format!("  {:>8}", format_size(line.received)));
            }
            out.push_str(&format!("  {elapsed}"));
            match line.status {
                Status::Running => {}
                Status::Done => out.push_str("  \x1b[32m✓\x1b[0m"),
                Status::Failed => out.push_str("  \x1b[31m✗\x1b[0m"),
            }
            if let (Some(detail), Status::Done | Status::Failed) = (line.detail, line.status) {
                out.push_str(&format!("  {detail}"));
            }
            out
        }
        OutputMode::Plain => {
            let size = size
                .map(|size| format!(" {}", format_size(size)))
                .unwrap_or_default();
            let result = match line.status {
                Status::Running => "...",
                Status::Done => "ok",
                Status::Failed => "failed",
            };
            let detail = line
                .detail
                .map(|detail| format!("; {detail}"))
                .unwrap_or_default();
            format!(
                "{} {}{} ... {} ({}){}",
                prefix(line.mode, line.index, line.total),
                line.name,
                size,
                result,
                elapsed,
                detail
            )
        }
    }
}

fn render_note(
    mode: OutputMode,
    index: usize,
    total: usize,
    name: &str,
    name_width: usize,
    note: &str,
) -> String {
    match mode {
        OutputMode::Rich => format!(
            "  {} {:<name_width$}  {}",
            prefix(mode, index, total),
            fit(name, name_width),
            note
        ),
        OutputMode::Plain => format!("{} {}: {}", prefix(mode, index, total), name, note),
    }
}

#[derive(Default)]
struct Transfer {
    completed: u64,
    current: u64,
    declared: u64,
    drawn_at: Option<Instant>,
}

/// One component's line: its position, its signed size and the bytes read
/// for it so far.
pub(super) struct ComponentProgress {
    mode: OutputMode,
    index: usize,
    total: usize,
    name: String,
    name_width: usize,
    size: Option<u64>,
    started: Instant,
    transfer: Mutex<Transfer>,
    detail: Mutex<Option<String>>,
}

impl ComponentProgress {
    pub(super) fn new(
        mode: OutputMode,
        index: usize,
        total: usize,
        name: &str,
        name_width: usize,
        size: Option<u64>,
    ) -> Arc<Self> {
        Arc::new(Self {
            mode,
            index,
            total,
            name: name.to_string(),
            name_width,
            size,
            started: Instant::now(),
            transfer: Mutex::new(Transfer::default()),
            detail: Mutex::new(None),
        })
    }

    /// Keeps the refresh reason for the line's final result.
    pub(super) fn refreshing(&self, reason: &str) {
        *self
            .detail
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(format!("refresh: {reason}"));
    }

    /// Runs one fetch with this line as its progress sink. A failure ends the
    /// line as failed and returns the same error.
    pub(super) async fn track<T>(
        self: &Arc<Self>,
        future: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        let line = Arc::clone(self);
        let sink: crate::carrier::ReplyProgress =
            Arc::new(move |read, declared| line.received(read, declared));
        // Download futures are large; the heap keeps setup's own future small.
        let result = Box::pin(REPORTING.scope(
            (),
            crate::carrier::with_reply_progress(sink, Box::pin(future)),
        ))
        .await;
        if result.is_err() {
            self.finish(Status::Failed);
        }
        result
    }

    pub(super) fn done(&self) {
        self.finish(Status::Done);
    }

    pub(super) fn failed(&self) {
        self.finish(Status::Failed);
    }

    pub(super) fn note(&self, note: &str) {
        let text = render_note(
            self.mode,
            self.index,
            self.total,
            &self.name,
            self.name_width,
            note,
        );
        self.write(&text, true);
    }

    fn received(&self, read: u64, declared: u64) {
        let mut transfer = self
            .transfer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if read == 0 {
            // A new reply starts. A finished reply counts toward this
            // component; an unfinished one was a failed attempt.
            if transfer.declared > 0 && transfer.current == transfer.declared {
                transfer.completed += transfer.current;
            }
            transfer.current = 0;
            transfer.declared = declared;
        } else {
            transfer.current = read;
        }
        let due = transfer
            .drawn_at
            .is_none_or(|drawn| drawn.elapsed() >= REDRAW_INTERVAL);
        if self.mode == OutputMode::Rich && due {
            transfer.drawn_at = Some(Instant::now());
            let received = transfer.completed + transfer.current;
            drop(transfer);
            self.draw(Status::Running, received);
        }
    }

    fn finish(&self, status: Status) {
        let received = {
            let transfer = self
                .transfer
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            transfer.completed + transfer.current
        };
        self.draw(status, received);
    }

    fn draw(&self, status: Status, received: u64) {
        let detail = self
            .detail
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let text = render(&Line {
            mode: self.mode,
            index: self.index,
            total: self.total,
            name: &self.name,
            name_width: self.name_width,
            size: self.size,
            received,
            elapsed: self.started.elapsed(),
            status,
            detail: detail.as_deref(),
        });
        match (self.mode, status) {
            (OutputMode::Rich, _) => self.write(&text, status != Status::Running),
            (OutputMode::Plain, Status::Running) => {}
            (OutputMode::Plain, _) => self.write(&text, true),
        }
    }

    fn write(&self, text: &str, end_line: bool) {
        let mut out = std::io::stdout().lock();
        let clear = if self.mode == OutputMode::Rich {
            "\r\x1b[2K"
        } else {
            ""
        };
        let _ = write!(out, "{clear}{text}");
        if end_line {
            let _ = writeln!(out);
        }
        let _ = out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(mode: OutputMode, size: Option<u64>, received: u64, status: Status) -> String {
        render(&Line {
            mode,
            index: 3,
            total: 41,
            name: "chain-provider",
            name_width: 16,
            size,
            received,
            elapsed: Duration::from_millis(400),
            status,
            detail: None,
        })
    }

    #[test]
    fn refresh_reason_rides_on_the_result_line_only() {
        let render_with = |mode, status| {
            render(&Line {
                mode,
                index: 3,
                total: 41,
                name: "chain-provider",
                name_width: 16,
                size: Some(1_048_576),
                received: 524_288,
                elapsed: Duration::from_millis(400),
                status,
                detail: Some("refresh: checksum changed"),
            })
        };
        assert_eq!(
            render_with(OutputMode::Plain, Status::Done),
            "[3/41] chain-provider 1.0 MB ... ok (0.4 s); refresh: checksum changed"
        );
        assert!(render_with(OutputMode::Rich, Status::Failed)
            .ends_with("✗\x1b[0m  refresh: checksum changed"));
        assert!(!render_with(OutputMode::Rich, Status::Running).contains("refresh"));
    }

    #[test]
    fn output_mode_is_plain_off_a_terminal_and_for_no_color_ci_and_dumb() {
        use OutputMode::{Plain, Rich};
        assert_eq!(
            OutputMode::from_environment(true, false, false, Some("xterm")),
            Rich
        );
        assert_eq!(
            OutputMode::from_environment(false, false, false, Some("xterm")),
            Plain
        );
        assert_eq!(
            OutputMode::from_environment(true, true, false, Some("xterm")),
            Plain
        );
        assert_eq!(
            OutputMode::from_environment(true, false, true, Some("xterm")),
            Plain
        );
        assert_eq!(
            OutputMode::from_environment(true, false, false, Some("dumb")),
            Plain
        );
        assert_eq!(
            OutputMode::from_environment(true, false, false, None),
            Plain
        );
    }

    #[test]
    fn plain_lines_name_position_size_result_and_time_without_escapes() {
        let size = Some(3_670_016);
        assert_eq!(
            line(OutputMode::Plain, size, 3_670_016, Status::Done),
            "[3/41] chain-provider 3.5 MB ... ok (0.4 s)"
        );
        assert_eq!(
            line(OutputMode::Plain, size, 10, Status::Failed),
            "[3/41] chain-provider 3.5 MB ... failed (0.4 s)"
        );
        assert_eq!(
            line(OutputMode::Plain, None, 0, Status::Done),
            "[3/41] chain-provider ... ok (0.4 s)"
        );
        assert_eq!(
            render_note(OutputMode::Plain, 1, 41, "shell", 16, "already installed"),
            "[1/41] shell: already installed"
        );
        for status in [Status::Running, Status::Done, Status::Failed] {
            assert!(!line(OutputMode::Plain, size, 5, status).contains('\x1b'));
        }
    }

    #[test]
    fn rich_line_shows_signed_size_bar_percent_and_result() {
        let size = Some(1_048_576);
        let half = line(OutputMode::Rich, size, 524_288, Status::Running);
        assert!(half.starts_with("  [ 3/41] chain-provider  "), "{half}");
        assert!(half.contains("1.0 MB"), "{half}");
        assert!(
            half.contains(&format!("{}{}", "━".repeat(6), "─".repeat(6))),
            "{half}"
        );
        assert!(half.contains(" 50%"), "{half}");
        assert!(!half.contains('✓'));
        let done = line(OutputMode::Rich, size, 0, Status::Done);
        assert!(
            done.contains(&"━".repeat(12)) && done.contains("100%") && done.contains('✓'),
            "{done}"
        );
        let over = line(OutputMode::Rich, size, 9_999_999, Status::Running);
        assert!(over.contains("100%"), "{over}");
        let no_size = line(OutputMode::Rich, None, 2048, Status::Running);
        assert!(
            no_size.contains("2.0 KB") && !no_size.contains('%'),
            "{no_size}"
        );
        assert!(line(OutputMode::Rich, size, 1, Status::Failed).contains('✗'));
    }

    #[test]
    fn rich_line_fits_eighty_columns_with_the_longest_name() {
        let longest = render(&Line {
            mode: OutputMode::Rich,
            index: 31,
            total: 41,
            name: "protected-content-decrypt-provider",
            name_width: NAME_WIDTH_LIMIT,
            size: Some(1_168_192),
            received: 1_000,
            elapsed: Duration::from_secs(75),
            status: Status::Done,
            detail: None,
        });
        let visible = longest.replace("\x1b[32m", "").replace("\x1b[0m", "");
        assert!(
            visible.chars().count() <= 80,
            "{} columns: {visible}",
            visible.chars().count()
        );
        assert!(visible.contains("protected-content-decry…"), "{visible}");
        assert_eq!(
            render_note(
                OutputMode::Plain,
                1,
                1,
                "protected-content-decrypt-provider",
                24,
                "ok"
            ),
            "[1/1] protected-content-decrypt-provider: ok"
        );
    }

    #[test]
    fn elapsed_reads_in_tenths_then_seconds_then_minutes() {
        assert_eq!(format_elapsed(Duration::from_millis(400)), "0.4 s");
        assert_eq!(format_elapsed(Duration::from_secs(12)), "12 s");
        assert_eq!(format_elapsed(Duration::from_secs(64)), "1 min 4 s");
    }

    #[test]
    fn finished_replies_add_up_and_a_failed_attempt_does_not_count() {
        let progress = ComponentProgress::new(OutputMode::Plain, 1, 1, "fixture", 7, Some(30));
        let received = |read, declared| progress.received(read, declared);
        // A failed first attempt, then a full retry of the binary, then metadata.
        received(0, 20);
        received(8, 20);
        received(0, 20);
        received(20, 20);
        received(0, 10);
        received(10, 10);
        let transfer = progress.transfer.lock().unwrap();
        assert_eq!(transfer.completed + transfer.current, 30);
    }

    #[tokio::test]
    async fn reporting_holds_only_inside_a_tracked_fetch() {
        assert!(!reporting());
        let progress = ComponentProgress::new(OutputMode::Plain, 1, 1, "fixture", 7, None);
        let inside = progress.track(async { Ok(reporting()) }).await.unwrap();
        assert!(inside);
        assert!(!reporting());
        let error = progress
            .track(async { Err::<(), _>(anyhow::anyhow!("refused")) })
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "refused");
    }
}
