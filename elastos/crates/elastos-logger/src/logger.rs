use std::sync::{Arc, OnceLock};

use crate::{now_rfc3339, Level, LogRecord, LogSink, StderrSink};

/// The routing facade (SRP): owns the threshold + the sink list and fans one record out to every
/// sink at once. Constructed once and installed as the process-global via [`init`].
pub struct Logger {
    threshold: Level,
    component: String,
    sinks: Vec<Arc<dyn LogSink>>,
}

impl Logger {
    pub fn new(
        threshold: Level,
        component: impl Into<String>,
        sinks: Vec<Arc<dyn LogSink>>,
    ) -> Logger {
        Logger {
            threshold,
            component: component.into(),
            sinks,
        }
    }

    pub fn threshold(&self) -> Level {
        self.threshold
    }

    /// Build a record for an enabled level and fan it out. Callers gate on [`enabled`] first (the
    /// macros do), so this re-checks cheaply and returns early if below threshold.
    pub fn emit(&self, level: Level, target: &str, message: &str) {
        self.emit_as(level, self.component.as_str(), target, message);
    }

    /// Like [`Logger::emit`], but stamps the record with `component` instead of the logger's own —
    /// lets one process-global logger carry per-surface component names.
    pub fn emit_as(&self, level: Level, component: &str, target: &str, message: &str) {
        if !level.enabled_under(self.threshold) {
            return;
        }
        let rec = LogRecord {
            level,
            component,
            target,
            message,
            ts: now_rfc3339(),
        };
        for sink in &self.sinks {
            sink.write(&rec);
        }
    }
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Install the process logger. Call once at startup, before other threads log. A second call is a
/// no-op (returns the already-installed logger's threshold via [`level`]).
pub fn init(logger: Logger) {
    let _ = LOGGER.set(logger);
}

/// The active threshold — the installed logger's, or `Info` before `init`.
pub fn level() -> Level {
    LOGGER.get().map(|l| l.threshold()).unwrap_or(Level::Info)
}

/// Whether a record at `level` would be emitted now. The macros call this to skip formatting work.
pub fn enabled(level: Level) -> bool {
    level.enabled_under(self::level())
}

/// Emit through the process logger. Before `init`, falls back to a one-shot stderr sink at `Info`
/// so early/library logging never panics and is never silently lost.
pub fn emit(level: Level, target: &str, message: &str) {
    match LOGGER.get() {
        Some(logger) => logger.emit(level, target, message),
        None => fallback_emit(level, "elastos", target, message),
    }
}

/// Emit through the process logger with a per-record component override (see [`Logger::emit_as`]).
pub fn emit_as(level: Level, component: &str, target: &str, message: &str) {
    match LOGGER.get() {
        Some(logger) => logger.emit_as(level, component, target, message),
        None => fallback_emit(level, component, target, message),
    }
}

/// Pre-`init` path: one-shot stderr at `Info` so early logging is neither lost nor panicking.
fn fallback_emit(level: Level, component: &str, target: &str, message: &str) {
    if level.enabled_under(Level::Info) {
        let rec = LogRecord {
            level,
            component,
            target,
            message,
            ts: now_rfc3339(),
        };
        StderrSink.write(&rec);
    }
}

#[cfg(test)]
mod tests {
    use super::Logger;
    use crate::{Level, LogSink, VecSink};
    use std::sync::Arc;

    #[test]
    fn fans_one_record_out_to_all_sinks() {
        let a = Arc::new(VecSink::default());
        let b = Arc::new(VecSink::default());
        let logger = Logger::new(
            Level::Info,
            "test",
            vec![a.clone() as Arc<dyn LogSink>, b.clone()],
        );
        logger.emit(Level::Warn, "t", "boom");
        assert_eq!(a.lines().len(), 1);
        assert_eq!(b.lines().len(), 1);
        assert!(a.lines()[0].contains("WARN test: boom"));
    }

    #[test]
    fn emit_as_overrides_the_component_per_record() {
        let sink = Arc::new(VecSink::default());
        let logger = Logger::new(
            Level::Info,
            "elastos",
            vec![sink.clone() as Arc<dyn LogSink>],
        );
        logger.emit_as(Level::Warn, "auth", "t", "token rejected");
        logger.emit(Level::Warn, "t", "fallback component");
        let lines = sink.lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("WARN auth: token rejected"));
        assert!(lines[1].contains("WARN elastos: fallback component"));
    }

    #[test]
    fn threshold_drops_less_severe_records() {
        let sink = Arc::new(VecSink::default());
        let logger = Logger::new(Level::Warn, "test", vec![sink.clone() as Arc<dyn LogSink>]);
        logger.emit(Level::Info, "t", "kept?"); // Info < Warn → dropped
        logger.emit(Level::Error, "t", "kept"); // Error >= Warn → kept
        let lines = sink.lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("ERROR"));
    }
}
