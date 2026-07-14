//! Compute provider implementations

mod component;
mod wasm;

pub use component::ComponentProvider;
pub use wasm::{BridgeHostcall, BridgePipes, BridgeSpawner, WasmProvider};
