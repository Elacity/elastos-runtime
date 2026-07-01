//! Compute provider implementations

mod wasm;

pub use wasm::{BridgeHostcall, BridgePipes, BridgeSpawner, WasmProvider};
