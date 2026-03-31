//! Runtime bootstrap and initialization
//!
//! Handles the complete startup sequence:
//! 1. Initialize primitives (time, audit, metrics)
//! 2. Initialize capability system
//! 3. Initialize capsule manager
//! 4. Initialize messaging
//! 5. Initialize content resolver
//! 6. Initialize request handler
//! 7. Launch shell capsule
//!
//! Also handles graceful shutdown.

mod runtime_builder;
mod shell;

#[allow(unused_imports)]
pub use runtime_builder::{
    BuildError, ConfigFile, ElastosRuntime, RuntimeConfig, StartError, StopError,
};
#[allow(unused_imports)]
pub use shell::{ShellConfig, ShellError, ShellManager};
