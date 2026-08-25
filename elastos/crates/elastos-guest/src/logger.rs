//! Crate-wide logger: `use crate::logger;` then `logger::warn!("...")` — the component
//! (`guest`) and call-site module path are stamped automatically.

elastos_logger::component!("guest");
