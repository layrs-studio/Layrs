//! Native Layrs SCM V1 domain model.
//!
//! This module is intentionally independent from the legacy automation model
//! still exported at the crate root during the development transition.

mod admission;
mod canonical;
mod content;
mod diff;
mod error;
mod ids;
mod path;
mod release_events;
mod repository;
mod serde_support;
mod step;
mod tree;

pub use admission::{
    AdmissionResolver, validate_file_manifest_admission, validate_layer_admission,
    validate_marker_admission, validate_operation_admission, validate_release_admission,
    validate_release_publication_admission, validate_repository_view_admission,
    validate_step_admission, validate_step_observation_admission, validate_tree_admission,
    validate_worktree_admission,
};
pub use canonical::{canonical_json, domain_separated_bytes};
pub use content::{
    Chunk, ChunkingDescriptor, FileChunk, FileManifest, FileManifestBuilder, FileManifestVerifier,
    FileState,
};
pub use error::{Result, ScmError};
pub use ids::{
    ActorId, ChunkId, ConflictId, DeviceId, FileId, FileManifestId, LayerId, MarkerId, ObjectId,
    ObservationId, OperationId, ProofId, ReleaseId, ReleasePublicationId, RepositoryViewId,
    SpaceId, StepId, StoreId, SymlinkId, TombstoneId, TreeId, WorktreeId,
};
pub use path::{RepoPath, RepoPathComponent};
pub use release_events::{ReleaseLifecycleAction, ReleaseLifecycleEvent};
pub use repository::{
    CaptureReason, Layer, LayerHeadSet, LayerKind, LayerSettings, LayerStatus, Marker, MarkerKind,
    Metadata, MetadataEntry, Operation, OperationKind, Release, ReleasePublication, RepositoryView,
    RewriteTarget, Space, Timestamp, Worktree, WorktreeStatus, WorktreeViewState,
};
pub use step::{Step, StepObservation};
pub use tree::{Symlink, Tombstone, Tree, TreeEntry, TreeEntryTarget};

pub use diff::{DiffContent, DiffEntry, NativeDiffLine, NativeFileChange, diff_entries};
