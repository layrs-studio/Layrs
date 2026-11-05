//! Conservative offline CAS collection. No catalog/history/lease deletion.
#[cfg(target_os = "linux")]
mod engine;
#[cfg(target_os = "linux")]
mod files;
#[cfg(target_os = "linux")]
mod roots;
mod types;
#[cfg(target_os = "linux")]
pub use engine::*;
pub use types::*;

#[cfg(not(target_os = "linux"))]
pub async fn command(
    _: &sqlx::PgPool,
    _: &crate::config::ServerConfig,
    _: &[String],
) -> GcResult<serde_json::Value> {
    Err("offline GC mutations currently require Linux pinned filesystem primitives".into())
}
