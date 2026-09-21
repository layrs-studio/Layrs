//! Layrs Sync V3 wire contract.
//!
//! V3 deliberately reuses SCM V1 records and content identifiers. Local
//! Operations and Worktrees are not synchronization records.

mod authorization;
mod bootstrap;
mod common;
mod events;
mod evidence;
mod native_policy;
mod observations;
pub use native_policy::*;
mod plans;
mod path_patch;
mod path_patch_submission;
mod products;
mod projection;
mod receipt;
mod transaction;
mod transaction_error;
mod validation;

pub use authorization::*;
pub use bootstrap::*;
pub use common::*;
pub use events::*;
pub use evidence::*;
pub use observations::*;
pub use plans::*;
pub use path_patch::*;
pub use path_patch_submission::*;
pub use products::*;
pub use projection::*;
pub use receipt::*;
pub use transaction::*;
pub use transaction_error::*;
pub use validation::{V3Result, V3ValidationError, ValidateV3};

pub const PROTOCOL: &str = "layrs.sync.v3";
