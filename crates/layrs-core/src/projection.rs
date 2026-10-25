//! Local partial-checkout records. These are neither complete SCM admission nor
//! cached authority to access remote content.
mod availability;
mod model;
mod paths;
mod selection;
pub use availability::{
    AvailabilityBlocker, AvailabilityRequest, OfflineOperation, ProjectionAvailability,
    ProjectionVersion, VersionPathAvailability,
};
pub use model::{EntryState, ProjectionEntry, ProjectionSnapshot};
pub use paths::{PathSemantics, ReservedPaths};
pub use selection::{ProjectionSelection, SelectionTraversal};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionError(pub String);
impl std::fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ProjectionError {}
pub type Result<T> = std::result::Result<T, ProjectionError>;
pub(super) fn invalid(message: impl Into<String>) -> ProjectionError {
    ProjectionError(message.into())
}
