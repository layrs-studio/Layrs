//! Deterministic graph algorithms for the native Layrs SCM model.

mod error;
mod heads;
mod operation_dag;
mod selectors;
mod step_dag;

pub use error::{GraphObjectKind, ScmGraphError};
pub use heads::{
    NormalizedHeads, integrate_heads, normalize_heads, validate_heads, validate_repository_view,
};
pub use operation_dag::OperationDag;
pub use selectors::{
    ReferenceKind, ReferenceSelector, SelectorError, SelectorSnapshot, StepSelector,
};
pub use step_dag::StepDag;
