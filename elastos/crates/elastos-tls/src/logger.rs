//! Crate-wide logger: `use crate::logger;` then `logger::warn!("...")` — the component
//! (`tls`) and call-site module path are stamped automatically.

elastos_logger::component!("tls");
