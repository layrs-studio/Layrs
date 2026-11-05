//! Durable local metadata and content-addressed storage for Layrs.
//!
//! This crate owns the on-disk `.layrs` Store: the native SCM format in
//! [`scm_v1`], checkout projections and the bounded download cache.

pub mod anchored_export;
pub mod download_cache;
mod error;
mod layout;
mod lock;
pub mod perf;
// Partial projections are a Windows (CFAPI) feature; their portable core is
// only partly reachable elsewhere. Windows builds still lint it in full.
#[cfg_attr(not(windows), allow(dead_code))]
pub mod projection;
pub mod scm_v1;
pub use secure_fs::private_spool_file;
mod secure_fs;

pub use error::{StoreError, StoreResult};
pub use lock::SessionMode;
