use std::sync::Arc;

use crate::{Level, LogSink, Logger, StderrSink};

/// Resolve the threshold with precedence **CLI > env > default**. `env_vars` are tried in order;
/// the first present-and-parseable one wins (lets a component honor its legacy var first, e.g.
/// `DKMS_AUTHORITY_LOG_LEVEL`, then a shared `ELASTOS_LOG`).
pub fn resolve_level(cli: Option<Level>, env_vars: &[&str], default: Level) -> Level {
    if let Some(level) = cli {
        return level;
    }
    for var in env_vars {
        if let Some(level) = Level::from_env(var) {
            return level;
        }
    }
    default
}

/// Small builder for the common cases; a component can always call `Logger::new` directly for
/// bespoke sink sets. Defaults to a single stderr sink.
pub struct LoggerConfig {
    threshold: Level,
    component: String,
    sinks: Vec<Arc<dyn LogSink>>,
}

impl LoggerConfig {
    pub fn new(component: impl Into<String>, threshold: Level) -> LoggerConfig {
        LoggerConfig {
            threshold,
            component: component.into(),
            sinks: Vec::new(),
        }
    }

    /// Add a sink (chainable). With none added, [`build`] installs a stderr sink.
    pub fn with_sink(mut self, sink: Arc<dyn LogSink>) -> LoggerConfig {
        self.sinks.push(sink);
        self
    }

    pub fn build(mut self) -> Logger {
        if self.sinks.is_empty() {
            self.sinks.push(Arc::new(StderrSink));
        }
        Logger::new(self.threshold, self.component, self.sinks)
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_level;
    use crate::Level;

    #[test]
    fn cli_wins_over_env_and_default() {
        // env var set to warn, but CLI says trace → trace
        std::env::set_var("ELASTOS_LOG_TEST_A", "warn");
        let got = resolve_level(Some(Level::Trace), &["ELASTOS_LOG_TEST_A"], Level::Info);
        assert_eq!(got, Level::Trace);
        std::env::remove_var("ELASTOS_LOG_TEST_A");
    }

    #[test]
    fn env_wins_over_default_when_no_cli() {
        std::env::set_var("ELASTOS_LOG_TEST_B", "error");
        let got = resolve_level(None, &["ELASTOS_LOG_TEST_B"], Level::Info);
        assert_eq!(got, Level::Error);
        std::env::remove_var("ELASTOS_LOG_TEST_B");
    }

    #[test]
    fn first_present_env_var_wins_then_default() {
        std::env::remove_var("ELASTOS_LOG_TEST_C1");
        std::env::set_var("ELASTOS_LOG_TEST_C2", "trace");
        let got = resolve_level(
            None,
            &["ELASTOS_LOG_TEST_C1", "ELASTOS_LOG_TEST_C2"],
            Level::Info,
        );
        assert_eq!(got, Level::Trace);
        std::env::remove_var("ELASTOS_LOG_TEST_C2");
        // nothing set → default
        assert_eq!(
            resolve_level(None, &["ELASTOS_LOG_TEST_NONE"], Level::Warn),
            Level::Warn
        );
    }
}
