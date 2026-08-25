//! `elastos-logger` — a small, dependency-light logging library with five severity levels and a
//! pluggable multi-sink writer interface. Not `log`/`tracing`: a purpose-built logger.
//!
//! Levels (least→most verbose): `Critical < Error < Warn < Info < Trace` in verbosity, which is the
//! INVERSE of the `Ord` ordering `Trace < Info < Warn < Error < Critical`. A record is emitted iff
//! `record.level >= threshold`.
//!
//! TRACE MUST be secret-free — log `fp(value)` fingerprints, never raw keys/seeds/grants.

mod level;
mod logger;
mod record;
mod sink;

pub use level::Level;
pub use logger::{emit, emit_as, enabled, init, level, Logger};
pub use record::{now_rfc3339, LogRecord};
pub use sink::{FileSink, LogSink, StderrSink, StdoutSink, VecSink};

mod config;
mod json;

pub use config::{resolve_level, LoggerConfig};
pub use json::JsonRingSink;

/// Privacy fingerprint: a short, non-reversible tag (`fp:` + first 8 hex of SHA-256) for a sensitive
/// identifier (wallet, content id, grant, seed). Use this in TRACE/INFO instead of the raw value so
/// a single workflow correlates across lines without persisting the secret.
pub fn fp(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let v = value.trim();
    if v.is_empty() {
        return "fp:<none>".to_string();
    }
    let digest = Sha256::digest(v.as_bytes());
    format!("fp:{}", hex::encode(&digest[..4]))
}

/// Internal: emit a formatted message at `$level` iff enabled. Formatting happens only when enabled
/// (lazy), so a suppressed TRACE costs one integer compare.
#[macro_export]
macro_rules! log_at {
    ($level:expr, component: $component:expr, $($arg:tt)*) => {{
        let __lvl = $level;
        if $crate::enabled(__lvl) {
            $crate::emit_as(__lvl, $component, module_path!(), &format!($($arg)*));
        }
    }};
    ($level:expr, $($arg:tt)*) => {{
        let __lvl = $level;
        if $crate::enabled(__lvl) {
            $crate::emit(__lvl, module_path!(), &format!($($arg)*));
        }
    }};
}

#[macro_export]
macro_rules! log_critical { ($($arg:tt)*) => { $crate::log_at!($crate::Level::Critical, $($arg)*) }; }
#[macro_export]
macro_rules! log_error { ($($arg:tt)*) => { $crate::log_at!($crate::Level::Error, $($arg)*) }; }
#[macro_export]
macro_rules! log_warn { ($($arg:tt)*) => { $crate::log_at!($crate::Level::Warn, $($arg)*) }; }
#[macro_export]
macro_rules! log_info { ($($arg:tt)*) => { $crate::log_at!($crate::Level::Info, $($arg)*) }; }
#[macro_export]
macro_rules! log_trace { ($($arg:tt)*) => { $crate::log_at!($crate::Level::Trace, $($arg)*) }; }

#[cfg(test)]
mod macro_tests {
    use crate::{Level, LogSink, VecSink};
    use std::sync::Arc;

    #[test]
    fn fp_is_stable_and_non_reversible() {
        let a = crate::fp("0xdeadbeef");
        assert_eq!(a, crate::fp("0xdeadbeef"));
        assert!(a.starts_with("fp:"));
        assert!(!a.contains("deadbeef"));
    }

    #[test]
    fn macros_emit_at_their_level_through_the_global() {
        let sink = Arc::new(VecSink::default());
        crate::init(crate::Logger::new(
            Level::Info,
            "test",
            vec![sink.clone() as Arc<dyn LogSink>],
        ));
        crate::log_trace!("hidden {}", 1); // Trace < Info → dropped
        crate::log_warn!("shown {}", 2); // Warn >= Info → kept
        crate::log_warn!(component: "auth", "override {}", 3); // per-surface component
        let lines = sink.lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("WARN test: shown 2"));
        assert!(lines[1].contains("WARN auth: override 3"));
    }
}
