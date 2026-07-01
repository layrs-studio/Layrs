use layrs_core::{GraphEdgeId, Metadata, ObjectKind, ObjectRef, RelationKind, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub source: ObjectRef,
    pub kind: RelationKind,
    pub target: ObjectRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub id: GraphEdgeId,
    pub relation: Relation,
    pub created_at: Timestamp,
    pub metadata: Metadata,
}

impl GraphEdge {
    pub fn new(
        id: GraphEdgeId,
        source: ObjectRef,
        kind: RelationKind,
        target: ObjectRef,
        created_at: Timestamp,
    ) -> Result<Self, GraphError> {
        if source == target {
            return Err(GraphError::SelfRelation { node: source });
        }

        Ok(Self {
            id,
            relation: Relation {
                source,
                kind,
                target,
            },
            created_at,
            metadata: BTreeMap::new(),
        })
    }

    pub fn source(&self) -> &ObjectRef {
        &self.relation.source
    }

    pub fn target(&self) -> &ObjectRef {
        &self.relation.target
    }

    pub fn kind(&self) -> &RelationKind {
        &self.relation.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    SelfRelation { node: ObjectRef },
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfRelation { node } => {
                write!(f, "graph relation cannot point `{}` to itself", node.id)
            }
        }
    }
}

impl Error for GraphError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedObjectRef {
    pub kind: ObjectKind,
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedRelation {
    pub source: RedactedObjectRef,
    pub kind: RelationKind,
    pub target: RedactedObjectRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedGraphEdge {
    pub id: Option<String>,
    pub relation: RedactedRelation,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionPolicy {
    pub reveal_ids: bool,
    pub metadata_allowlist: BTreeSet<String>,
}

impl RedactionPolicy {
    pub fn structure_only() -> Self {
        Self {
            reveal_ids: false,
            metadata_allowlist: BTreeSet::new(),
        }
    }

    pub fn reveal_ids() -> Self {
        Self {
            reveal_ids: true,
            metadata_allowlist: BTreeSet::new(),
        }
    }

    pub fn with_metadata_keys(mut self, keys: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.metadata_allowlist
            .extend(keys.into_iter().map(Into::into));
        self
    }
}

pub fn redact_edge(edge: &GraphEdge, policy: &RedactionPolicy) -> RedactedGraphEdge {
    RedactedGraphEdge {
        id: policy.reveal_ids.then(|| edge.id.to_string()),
        relation: RedactedRelation {
            source: redact_ref(edge.source(), policy.reveal_ids),
            kind: edge.kind().clone(),
            target: redact_ref(edge.target(), policy.reveal_ids),
        },
        metadata: edge
            .metadata
            .iter()
            .filter(|(key, _)| policy.metadata_allowlist.contains(*key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    }
}

pub fn redact_edges(edges: &[GraphEdge], policy: &RedactionPolicy) -> Vec<RedactedGraphEdge> {
    edges.iter().map(|edge| redact_edge(edge, policy)).collect()
}

fn redact_ref(reference: &ObjectRef, reveal_id: bool) -> RedactedObjectRef {
    RedactedObjectRef {
        kind: reference.kind,
        id: reveal_id.then(|| reference.id.clone()),
    }
}

pub fn edges_touching<'a>(edges: &'a [GraphEdge], reference: &ObjectRef) -> Vec<&'a GraphEdge> {
    edges
        .iter()
        .filter(|edge| edge.source() == reference || edge.target() == reference)
        .collect()
}

pub fn minimal_impact(edges: &[GraphEdge], roots: &[ObjectRef]) -> BTreeSet<ObjectRef> {
    let mut impacted = BTreeSet::new();
    let mut queue = VecDeque::new();

    for root in roots {
        if impacted.insert(root.clone()) {
            queue.push_back(root.clone());
        }
    }

    while let Some(current) = queue.pop_front() {
        for edge in edges {
            if let Some(next) = impacted_neighbor(edge, &current) {
                if impacted.insert(next.clone()) {
                    queue.push_back(next.clone());
                }
            }
        }
    }

    impacted
}

fn impacted_neighbor<'a>(edge: &'a GraphEdge, changed: &ObjectRef) -> Option<&'a ObjectRef> {
    use RelationKind::*;

    match edge.kind() {
        DependsOn | Requires | Verifies | References | Annotates => {
            (edge.target() == changed).then(|| edge.source())
        }
        _ => (edge.source() == changed).then(|| edge.target()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_core::{ArtifactId, LayerId};

    #[test]
    fn graph_edge_construction_sets_relation() {
        let source = ObjectRef::layer(LayerId::new("layer.base").unwrap());
        let target = ObjectRef::artifact(ArtifactId::new("artifact.report").unwrap());
        let edge = GraphEdge::new(
            GraphEdgeId::new("edge.1").unwrap(),
            source.clone(),
            RelationKind::Produces,
            target.clone(),
            Timestamp::from_unix_millis(42),
        )
        .expect("edge");

        assert_eq!(edge.source(), &source);
        assert_eq!(edge.target(), &target);
        assert_eq!(edge.kind(), &RelationKind::Produces);
    }

    #[test]
    fn minimal_impact_follows_relation_semantics() {
        let layer = ObjectRef::layer(LayerId::new("layer.base").unwrap());
        let artifact = ObjectRef::artifact(ArtifactId::new("artifact.report").unwrap());
        let edge = GraphEdge::new(
            GraphEdgeId::new("edge.impact").unwrap(),
            layer.clone(),
            RelationKind::Produces,
            artifact.clone(),
            Timestamp::from_unix_millis(1),
        )
        .expect("edge");

        let impact = minimal_impact(&[edge], &[layer]);

        assert!(impact.contains(&artifact));
    }
}
