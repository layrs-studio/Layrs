//! Native Layrs server: identity, collaboration, storage maintenance and the
//! v3 HTTP surface served by `main.rs`.

pub mod backup;
pub mod collaboration;
pub mod config;
pub mod identity;
pub mod installation;
pub mod lenses;
pub mod retention;
pub mod storage_budget;
pub mod telemetry;
pub mod v3;
pub mod webhooks;
