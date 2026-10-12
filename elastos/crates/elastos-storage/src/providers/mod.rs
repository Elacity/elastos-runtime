//! Storage provider implementations

mod ipfs;
mod local;

pub use ipfs::IpfsProvider;
pub use local::LocalFSProvider;
