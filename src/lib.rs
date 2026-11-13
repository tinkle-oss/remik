//! Memory-only storage and RESP2 networking for the Remik server.
//!
//! This pre-1.0 Rust API is experimental. See `docs/compatibility.md` for the
//! supported wire protocol; Redis compatibility does not imply persistence.

pub mod config;
pub mod db;
pub mod protocol;
pub mod server;
