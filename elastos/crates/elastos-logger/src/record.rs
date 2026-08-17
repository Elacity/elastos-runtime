use crate::Level;

/// One log event — pure data, no behavior (SRP). Borrows its string fields so emitting is
/// allocation-light; `ts` is pre-rendered by the caller.
pub struct LogRecord<'a> {
    pub level: Level,
    pub component: &'a str,
    pub target: &'a str,
    pub message: &'a str,
    pub ts: String,
}

impl<'a> LogRecord<'a> {
    /// Canonical one line: `<ts> <LEVEL> <component>: <message>`. At TRACE the module `target` is
    /// appended in parentheses to aid workflow tracing; higher levels omit it for operator clarity.
    pub fn format_line(&self) -> String {
        if self.level == Level::Trace {
            format!(
                "{} {} {}: {} ({})",
                self.ts,
                self.level.as_str(),
                self.component,
                self.message,
                self.target
            )
        } else {
            format!(
                "{} {} {}: {}",
                self.ts,
                self.level.as_str(),
                self.component,
                self.message
            )
        }
    }
}

/// Current UTC time as an RFC3339 string (seconds precision). Falls back to the Unix epoch string
/// if the system clock is before 1970 (never happens in practice) so logging can't panic.
pub fn now_rfc3339() -> String {
    use time::{format_description::well_known::Rfc3339, OffsetDateTime};
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::LogRecord;
    use crate::Level;

    fn rec(level: Level) -> LogRecord<'static> {
        LogRecord {
            level,
            component: "key-provider",
            target: "key_provider::dkms",
            message: "dkms session established",
            ts: "2026-08-17T20:34:17Z".to_string(),
        }
    }

    #[test]
    fn non_trace_line_is_ts_level_component_message() {
        let line = rec(Level::Info).format_line();
        assert_eq!(
            line,
            "2026-08-17T20:34:17Z INFO key-provider: dkms session established"
        );
    }

    #[test]
    fn trace_line_appends_target_for_workflow_tracing() {
        let line = rec(Level::Trace).format_line();
        assert_eq!(line, "2026-08-17T20:34:17Z TRACE key-provider: dkms session established (key_provider::dkms)");
    }
}
