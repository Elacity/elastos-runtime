use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use crate::LogRecord;

/// The writer interface (DIP/OCP seam). A new output — a file, or *later* a logging provider — is a
/// new `LogSink` impl; `Logger` never changes. Impls MUST be cheap and non-panicking: a failed write
/// is dropped, never propagated (logging must not be able to fail the path it observes).
pub trait LogSink: Send + Sync {
    fn write(&self, rec: &LogRecord);
    fn flush(&self) {}
}

/// Writes rendered lines to stderr (the default; the gateway captures child stderr into one sink).
pub struct StderrSink;
impl LogSink for StderrSink {
    fn write(&self, rec: &LogRecord) {
        let _ = writeln!(std::io::stderr(), "{}", rec.format_line());
    }
}

/// Writes rendered lines to stdout.
pub struct StdoutSink;
impl LogSink for StdoutSink {
    fn write(&self, rec: &LogRecord) {
        let _ = writeln!(std::io::stdout(), "{}", rec.format_line());
    }
}

/// Appends rendered lines to a file (opened in append mode; created if absent).
pub struct FileSink {
    file: Mutex<std::fs::File>,
}
impl FileSink {
    pub fn new(path: impl AsRef<Path>) -> std::io::Result<FileSink> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(FileSink {
            file: Mutex::new(file),
        })
    }
}
impl LogSink for FileSink {
    fn write(&self, rec: &LogRecord) {
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "{}", rec.format_line());
        }
    }
    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

/// In-memory sink for tests — captures rendered lines so fan-out / threshold behavior can be
/// asserted without touching stdout/stderr.
#[derive(Default)]
pub struct VecSink {
    lines: Mutex<Vec<String>>,
}
impl VecSink {
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().map(|v| v.clone()).unwrap_or_default()
    }
}
impl LogSink for VecSink {
    fn write(&self, rec: &LogRecord) {
        if let Ok(mut v) = self.lines.lock() {
            v.push(rec.format_line());
        }
    }
}

// NOTE (future provider sink): a `ProviderSink` that forwards to the logging provider is simply
// another `impl LogSink` added here later — no change to `Logger`. Not built in this PoC (YAGNI).

#[cfg(test)]
mod tests {
    use super::{FileSink, LogSink, VecSink};
    use crate::{now_rfc3339, Level, LogRecord};

    fn rec() -> LogRecord<'static> {
        LogRecord {
            level: Level::Info,
            component: "test",
            target: "t",
            message: "hello",
            ts: now_rfc3339(),
        }
    }

    #[test]
    fn vec_sink_captures_rendered_lines() {
        let sink = VecSink::default();
        sink.write(&rec());
        let lines = sink.lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("INFO test: hello"));
    }

    #[test]
    fn file_sink_appends_a_line() {
        let dir = std::env::temp_dir().join(format!("elog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.log");
        let sink = FileSink::new(&path).unwrap();
        sink.write(&rec());
        sink.flush();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("INFO test: hello"));
        assert!(body.ends_with('\n'));
    }
}
