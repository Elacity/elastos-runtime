//! `ddrm-media` — the local dDRM media-prep building blocks shared by the
//! viewer-seam demo (`scripts/dev/ddrm-viewer-demo`) and the gateway.
//!
//! It does NOT speak HTTP and holds NO long-lived global state. It provides three
//! things a local key-authority needs to make an owned video playable through the
//! real decrypt boundary:
//!
//!   * [`mp4`]  — minimal fMP4 box surgery: split a fragmented MP4 into an init
//!                segment + media fragments, CENC-encrypt a fragment under a CEK
//!                (insert `senc`, fix `trun.data_offset`/sizes), strip `senc` from a
//!                decrypted fragment, and read the AVC codec string for the MSE mime.
//!   * [`rail`] — a thin client for a long-lived `decrypt-provider` subprocess
//!                (`rail-stream` + `rail-mint`) speaking its stdio JSON protocol.
//!   * [`seal`] — the local key-authority: mint a CEK, CENC-pack the asset, launch
//!                the provider, and seal the CEK to the provider's published session
//!                key bound to the full decrypt transcript (`ddrm-envelope`). The raw
//!                CEK is zeroized once sealed; the sealed material carries no key.
//!
//! The containment invariant is the boundary's: the CEK/IV never leave the
//! decrypt-provider sandbox, and only one segment's plaintext is ever in flight.

pub mod mp4;
pub mod rail;
pub mod seal;

pub use rail::DecryptProviderProc;
pub use seal::{prepare, PreparedSession, SessionParams};
