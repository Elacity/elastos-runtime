//! Storage abstraction layer for ElastOS

mod cache;
mod content_id;
mod large_cache;
pub mod mutable;
mod traits;

pub mod providers;

pub use cache::ContentCache;
pub use content_id::ContentId;
pub use large_cache::{LargeCacheConfig, LargeFileCache};
pub use mutable::{
    DirEntry, EntryType, LocalMutableStorage, Metadata, MutableStorage, StorageError,
};
pub use traits::StorageProvider;
