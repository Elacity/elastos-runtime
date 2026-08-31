use std::str::FromStr;

/// Severity. `Ord` yields `Trace < Info < Warn < Error < Critical`; a record is emitted iff
/// `record.level >= threshold` (see [`Level::enabled_under`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Trace,
    Info,
    Warn,
    Error,
    Critical,
}

impl Level {
    /// Fixed-width-free upper-case label used in the rendered line.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Trace => "TRACE",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
            Level::Critical => "CRITICAL",
        }
    }

    /// Parse a level name — case-insensitive, surrounding whitespace ignored, common aliases
    /// accepted. Unknown → `None` (callers fall back rather than fail closed on a typo).
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_ascii_lowercase().as_str() {
            "trace" | "debug" => Some(Level::Trace),
            "info" => Some(Level::Info),
            "warn" | "warning" => Some(Level::Warn),
            "error" | "err" => Some(Level::Error),
            "critical" | "crit" | "fatal" => Some(Level::Critical),
            _ => None,
        }
    }

    /// Resolve a level from an environment variable; missing/empty/invalid → `None`.
    pub fn from_env(var: &str) -> Option<Level> {
        std::env::var(var).ok().and_then(|v| Level::parse(&v))
    }

    /// True if a record at `self` should be emitted under `threshold` (the filtering rule).
    pub fn enabled_under(self, threshold: Level) -> bool {
        self >= threshold
    }
}

impl FromStr for Level {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Level::parse(s).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::Level;

    #[test]
    fn ordering_is_trace_lowest_critical_highest() {
        assert!(Level::Trace < Level::Info);
        assert!(Level::Info < Level::Warn);
        assert!(Level::Warn < Level::Error);
        assert!(Level::Error < Level::Critical);
    }

    #[test]
    fn parse_is_case_insensitive_with_aliases() {
        assert_eq!(Level::parse(" TRACE "), Some(Level::Trace));
        assert_eq!(Level::parse("info"), Some(Level::Info));
        assert_eq!(Level::parse("Warning"), Some(Level::Warn));
        assert_eq!(Level::parse("error"), Some(Level::Error));
        assert_eq!(Level::parse("fatal"), Some(Level::Critical));
        assert_eq!(Level::parse("nonsense"), None);
    }

    #[test]
    fn parse_accepts_debug_as_alias_for_trace() {
        assert_eq!(Level::parse("debug"), Some(Level::Trace));
        assert_eq!(Level::parse("DEBUG"), Some(Level::Trace));
    }

    #[test]
    fn enabled_under_matches_the_filtering_rule() {
        // threshold=Critical → only Critical
        assert!(Level::Critical.enabled_under(Level::Critical));
        assert!(!Level::Error.enabled_under(Level::Critical));
        // threshold=Info → Info..Critical, not Trace
        assert!(Level::Info.enabled_under(Level::Info));
        assert!(Level::Critical.enabled_under(Level::Info));
        assert!(!Level::Trace.enabled_under(Level::Info));
        // threshold=Trace → everything
        assert!(Level::Trace.enabled_under(Level::Trace));
        assert!(Level::Critical.enabled_under(Level::Trace));
    }
}
