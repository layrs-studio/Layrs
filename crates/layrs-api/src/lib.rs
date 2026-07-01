//! Transport-neutral API contract for Layrs V1.
//!
//! These types are deliberately plain Rust structs. Serialization, Axum
//! extractors, and generated clients can be layered on later without changing
//! the logical contract.

pub mod accounts;
pub mod artifacts;
pub mod auth;
pub mod chunks;
pub mod common;
pub mod ids;
pub mod layer_access;
pub mod layers;
pub mod policies;
pub mod proofs;
pub mod spaces;
pub mod sync;
pub mod teams;
pub mod timeline;
pub mod validation;
pub mod weaves;
pub mod workspaces;

pub use accounts::*;
pub use artifacts::*;
pub use auth::*;
pub use chunks::*;
pub use common::*;
pub use ids::*;
pub use layer_access::*;
pub use layers::*;
pub use policies::*;
pub use proofs::*;
pub use spaces::*;
pub use sync::*;
pub use teams::*;
pub use timeline::*;
pub use validation::*;
pub use weaves::*;
pub use workspaces::*;
