//! Generated Rust bindings for the `elastos:bus@v1` Component Model world.
//!
//! Enable the `component-bindings` feature to share the canonical constants
//! and generated import-side types. Capsule crates that export a component
//! still generate local bindings so `wit-bindgen`'s private `export!` macro is
//! available in the exporting module.

pub const ABI: &str = "elastos.component/v1";
pub const WIT_PACKAGE: &str = "elastos:bus@1.0.0";
pub const WIT_WORLD: &str = "product-capsule-v1";

wit_bindgen::generate!({
    path: "../../wit",
    world: "product-capsule-v1",
});
