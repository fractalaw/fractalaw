//! Sync layer: Arrow Flight RPC transport, Lance delta sync, Loro CRDTs for conflict resolution.

#[cfg(feature = "http")]
pub mod http;

#[cfg(feature = "http")]
pub use http::{SyncClient, SyncError};
