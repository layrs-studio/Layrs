#![allow(async_fn_in_trait)]

use layrs_core::*;
use layrs_graph::GraphEdge;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    NotFound { entity: &'static str, id: String },
    Conflict { message: String },
    Validation { message: String },
    Transaction { message: String },
    Corruption { message: String },
    Unsupported { operation: String },
    Io { message: String },
    Serialization { message: String },
    Backend { message: String },
}

impl StoreError {
    pub fn not_found(entity: &'static str, id: impl fmt::Display) -> Self {
        Self::NotFound {
            entity,
            id: id.to_string(),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
            message: message.into(),
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { entity, id } => write!(f, "{entity} `{id}` was not found"),
            Self::Conflict { message } => write!(f, "store conflict: {message}"),
            Self::Validation { message } => write!(f, "store validation failed: {message}"),
            Self::Transaction { message } => write!(f, "store transaction failed: {message}"),
            Self::Corruption { message } => write!(f, "store corruption detected: {message}"),
            Self::Unsupported { operation } => {
                write!(f, "store operation is unsupported: {operation}")
            }
            Self::Io { message } => write!(f, "store io failed: {message}"),
            Self::Serialization { message } => write!(f, "store serialization failed: {message}"),
            Self::Backend { message } => write!(f, "store backend failed: {message}"),
        }
    }
}

impl Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransactionId(String);

impl TransactionId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(StoreError::validation("transaction id cannot be empty"));
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TransactionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait StoreTransaction {
    fn transaction_id(&self) -> &TransactionId;

    fn is_active(&self) -> bool;

    async fn commit(self) -> Result<()>
    where
        Self: Sized;

    async fn rollback(self) -> Result<()>
    where
        Self: Sized;
}

pub trait TransactionalStore {
    type Transaction: StoreTransaction;

    async fn begin_transaction(&mut self) -> Result<Self::Transaction>;
}

pub trait WorkspaceStore {
    async fn get_workspace(&self, id: &WorkspaceId) -> Result<Option<Workspace>>;

    async fn list_workspaces(&self) -> Result<Vec<Workspace>>;

    async fn put_workspace(&mut self, workspace: Workspace) -> Result<()>;

    async fn delete_workspace(&mut self, id: &WorkspaceId) -> Result<()>;
}

pub trait TeamStore {
    async fn get_team(&self, id: &TeamId) -> Result<Option<Team>>;

    async fn list_teams(&self, workspace_id: &WorkspaceId) -> Result<Vec<Team>>;

    async fn put_team(&mut self, team: Team) -> Result<()>;

    async fn delete_team(&mut self, id: &TeamId) -> Result<()>;
}

pub trait SpaceStore {
    async fn get_space(&self, id: &SpaceId) -> Result<Option<Space>>;

    async fn list_spaces(&self, workspace_id: &WorkspaceId) -> Result<Vec<Space>>;

    async fn put_space(&mut self, space: Space) -> Result<()>;

    async fn delete_space(&mut self, id: &SpaceId) -> Result<()>;
}

pub trait LayerStore {
    async fn get_layer(&self, id: &LayerId) -> Result<Option<Layer>>;

    async fn list_layers(&self, space_id: &SpaceId) -> Result<Vec<Layer>>;

    async fn put_layer(&mut self, layer: Layer) -> Result<()>;

    async fn delete_layer(&mut self, id: &LayerId) -> Result<()>;
}

pub trait ArtifactStore {
    async fn get_artifact(&self, id: &ArtifactId) -> Result<Option<Artifact>>;

    async fn list_artifacts(&self, space_id: &SpaceId) -> Result<Vec<Artifact>>;

    async fn put_artifact(&mut self, artifact: Artifact) -> Result<()>;

    async fn delete_artifact(&mut self, id: &ArtifactId) -> Result<()>;

    async fn get_artifact_version(&self, id: &ArtifactVersionId)
    -> Result<Option<ArtifactVersion>>;

    async fn list_artifact_versions(
        &self,
        artifact_id: &ArtifactId,
    ) -> Result<Vec<ArtifactVersion>>;

    async fn put_artifact_version(&mut self, version: ArtifactVersion) -> Result<()>;
}

pub trait StepStore {
    async fn get_step(&self, id: &StepId) -> Result<Option<Step>>;

    async fn list_steps(&self, space_id: &SpaceId) -> Result<Vec<Step>>;

    async fn put_step(&mut self, step: Step) -> Result<()>;

    async fn delete_step(&mut self, id: &StepId) -> Result<()>;

    async fn get_flow(&self, id: &FlowId) -> Result<Option<Flow>>;

    async fn list_flows(&self, space_id: &SpaceId) -> Result<Vec<Flow>>;

    async fn put_flow(&mut self, flow: Flow) -> Result<()>;

    async fn delete_flow(&mut self, id: &FlowId) -> Result<()>;
}

pub trait GraphStore {
    async fn get_edge(&self, id: &GraphEdgeId) -> Result<Option<GraphEdge>>;

    async fn list_edges_for(&self, reference: &ObjectRef) -> Result<Vec<GraphEdge>>;

    async fn put_edge(&mut self, edge: GraphEdge) -> Result<()>;

    async fn delete_edge(&mut self, id: &GraphEdgeId) -> Result<()>;
}

pub trait ProofStore {
    async fn get_proof(&self, id: &ProofId) -> Result<Option<Proof>>;

    async fn list_proofs(&self, space_id: &SpaceId) -> Result<Vec<Proof>>;

    async fn list_proofs_for(&self, target: &ObjectRef) -> Result<Vec<Proof>>;

    async fn put_proof(&mut self, proof: Proof) -> Result<()>;

    async fn delete_proof(&mut self, id: &ProofId) -> Result<()>;
}

pub trait PolicyStore {
    async fn get_policy(&self, id: &PolicyId) -> Result<Option<Policy>>;

    async fn list_policies(&self, scope: &PolicyScope) -> Result<Vec<Policy>>;

    async fn list_policies_for_action(
        &self,
        scope: &PolicyScope,
        action: &PolicyAction,
    ) -> Result<Vec<Policy>>;

    async fn put_policy(&mut self, policy: Policy) -> Result<()>;

    async fn delete_policy(&mut self, id: &PolicyId) -> Result<()>;
}

pub trait TimelineStore {
    async fn append_timeline_event(&mut self, event: TimelineEvent) -> Result<()>;

    async fn list_timeline_events(
        &self,
        workspace_id: &WorkspaceId,
        space_id: Option<&SpaceId>,
    ) -> Result<Vec<TimelineEvent>>;
}

pub trait ChunkStore {
    async fn get_chunk(&self, id: &ChunkId) -> Result<Option<Chunk>>;

    async fn get_chunk_bytes(&self, id: &ChunkId) -> Result<Option<Vec<u8>>>;

    async fn put_chunk(&mut self, chunk: Chunk, bytes: Vec<u8>) -> Result<()>;

    async fn delete_chunk(&mut self, id: &ChunkId) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_error_formats_not_found() {
        let err = StoreError::not_found("workspace", "ws.alpha");

        assert_eq!(err.to_string(), "workspace `ws.alpha` was not found");
    }
}
