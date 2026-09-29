//! Pure, single-target selection against one immutable repository View.
use super::{ScmGraphError, StepDag, validate_repository_view};
use layrs_core::scm::{
    Layer, LayerId, Marker, MarkerId, Release, ReleaseId, RepositoryView, SpaceId, StepId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Names are exact, case-sensitive labels within one reference kind and Space.
/// No prefix matching, Unicode folding, or precedence between IDs and names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceSelector<I> {
    Id(I),
    Name(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepSelector {
    Step(StepId),
    Layer {
        reference: ReferenceSelector<LayerId>,
        /// Explicit choice must be a current head, never merely an ancestor.
        head: Option<StepId>,
    },
    Marker(ReferenceSelector<MarkerId>),
    /// A Release label is its version, not its presentation title.
    Release(ReferenceSelector<ReleaseId>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    Step,
    Layer,
    Marker,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorError {
    Graph(ScmGraphError),
    ViewSpaceMismatch {
        expected: SpaceId,
        actual: SpaceId,
    },
    Missing {
        kind: ReferenceKind,
        value: String,
    },
    Duplicate {
        kind: ReferenceKind,
        id: String,
    },
    OutsideSpace {
        kind: ReferenceKind,
        id: String,
    },
    NotVisible {
        kind: ReferenceKind,
        id: String,
    },
    AmbiguousName {
        kind: ReferenceKind,
        name: String,
        candidates: Vec<String>,
    },
    LayerWithoutHead(LayerId),
    MultipleHeads {
        layer_id: LayerId,
        heads: Vec<StepId>,
    },
    NotLayerHead {
        layer_id: LayerId,
        step_id: StepId,
    },
}

impl fmt::Display for SelectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph(error) => error.fmt(f),
            Self::ViewSpaceMismatch { expected, actual } => {
                write!(f, "View belongs to {actual}, expected {expected}")
            }
            Self::Missing { kind, value } => write!(f, "{kind:?} not found: {value}"),
            Self::Duplicate { kind, id } => write!(f, "duplicate {kind:?}: {id}"),
            Self::OutsideSpace { kind, id } => write!(f, "{kind:?} outside selected Space: {id}"),
            Self::NotVisible { kind, id } => {
                write!(f, "{kind:?} not visible in selected View: {id}")
            }
            Self::AmbiguousName {
                kind,
                name,
                candidates,
            } => write!(
                f,
                "ambiguous {kind:?} name {name:?}: {}",
                candidates.join(", ")
            ),
            Self::LayerWithoutHead(id) => write!(f, "Layer has no captured snapshot: {id}"),
            Self::MultipleHeads { layer_id, heads } => write!(
                f,
                "Layer {layer_id} has {} heads; select an explicit head",
                heads.len()
            ),
            Self::NotLayerHead { layer_id, step_id } => {
                write!(f, "Step {step_id} is not a head of Layer {layer_id}")
            }
        }
    }
}

impl std::error::Error for SelectorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Graph(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ScmGraphError> for SelectorError {
    fn from(value: ScmGraphError) -> Self {
        Self::Graph(value)
    }
}

/// Read-only selector context. Construction checks reference closure, Space
/// membership, duplicate identities, and Layer antichains without normalizing
/// or changing the View. It does not validate CAS bytes or replace admission.
///
/// Steps have no intrinsic Space. `space_steps` MUST come from an authorized
/// store snapshot for `space_id`; graph presence alone grants no membership.
/// The caller must read all inputs from the same snapshot and enforce access
/// control before exposing errors or metadata to an untrusted caller.
///
/// Hidden references cannot be selected, even by ID; use their historical View
/// to resolve them. Exact Step IDs remain selectable within `space_steps`.
#[derive(Debug)]
pub struct SelectorSnapshot<'a> {
    graph: &'a StepDag,
    view: &'a RepositoryView,
    space_steps: BTreeSet<StepId>,
    layers: BTreeMap<LayerId, &'a Layer>,
    markers: BTreeMap<MarkerId, &'a Marker>,
    releases: BTreeMap<ReleaseId, &'a Release>,
}

impl<'a> SelectorSnapshot<'a> {
    pub fn new(
        space_id: &SpaceId,
        graph: &'a StepDag,
        view: &'a RepositoryView,
        space_steps: impl IntoIterator<Item = StepId>,
        layers: impl IntoIterator<Item = &'a Layer>,
        markers: impl IntoIterator<Item = &'a Marker>,
        releases: impl IntoIterator<Item = &'a Release>,
    ) -> Result<Self, SelectorError> {
        if view.space_id() != space_id {
            return Err(SelectorError::ViewSpaceMismatch {
                expected: space_id.clone(),
                actual: view.space_id().clone(),
            });
        }
        let snapshot = Self {
            graph,
            view,
            space_steps: space_steps.into_iter().collect(),
            layers: index(
                layers,
                ReferenceKind::Layer,
                Layer::id,
                Layer::space_id,
                space_id,
            )?,
            markers: index(
                markers,
                ReferenceKind::Marker,
                Marker::id,
                Marker::space_id,
                space_id,
            )?,
            releases: index(
                releases,
                ReferenceKind::Release,
                Release::id,
                Release::space_id,
                space_id,
            )?,
        };
        for id in &snapshot.space_steps {
            snapshot.check_step(id)?;
            for parent in graph.parents(id)? {
                snapshot.check_step(&parent)?;
            }
        }
        validate_repository_view(graph, view)?;
        for heads in view.layer_heads() {
            require(&snapshot.layers, heads.layer_id(), ReferenceKind::Layer)?;
            for step in heads.step_ids() {
                snapshot.check_step(step)?;
            }
        }
        for id in view.marker_ids() {
            require(&snapshot.markers, id, ReferenceKind::Marker)?;
        }
        for id in view.release_ids() {
            require(&snapshot.releases, id, ReferenceKind::Release)?;
        }
        for marker in snapshot.markers.values() {
            snapshot.check_step(marker.target_step_id())?;
        }
        for release in snapshot.releases.values() {
            snapshot.check_step(release.target_step_id())?;
        }
        for worktree in view.worktrees() {
            if let Some(id) = worktree.active_layer_id() {
                snapshot.visible_layer(id)?;
            }
            if let Some(id) = worktree.base_step_id() {
                snapshot.check_step(id)?;
            }
        }
        Ok(snapshot)
    }

    pub fn view(&self) -> &RepositoryView {
        self.view
    }

    pub fn resolve(&self, selector: &StepSelector) -> Result<StepId, SelectorError> {
        match selector {
            StepSelector::Step(id) => {
                self.check_step(id)?;
                Ok(id.clone())
            }
            StepSelector::Layer { reference, head } => {
                let layer = select(
                    &self.layers,
                    reference,
                    ReferenceKind::Layer,
                    Layer::name,
                    |id| {
                        self.view
                            .layer_heads()
                            .iter()
                            .any(|heads| heads.layer_id() == id)
                    },
                )?;
                let heads = self.visible_layer(layer.id())?;
                if let Some(id) = head {
                    self.check_step(id)?;
                    return if heads.contains(id) {
                        Ok(id.clone())
                    } else {
                        Err(SelectorError::NotLayerHead {
                            layer_id: layer.id().clone(),
                            step_id: id.clone(),
                        })
                    };
                }
                match heads {
                    [] => Err(SelectorError::LayerWithoutHead(layer.id().clone())),
                    [id] => Ok(id.clone()),
                    _ => Err(SelectorError::MultipleHeads {
                        layer_id: layer.id().clone(),
                        heads: heads.to_vec(),
                    }),
                }
            }
            StepSelector::Marker(reference) => Ok(select(
                &self.markers,
                reference,
                ReferenceKind::Marker,
                Marker::name,
                |id| self.view.marker_ids().contains(id),
            )?
            .target_step_id()
            .clone()),
            StepSelector::Release(reference) => Ok(select(
                &self.releases,
                reference,
                ReferenceKind::Release,
                Release::version,
                |id| self.view.release_ids().contains(id),
            )?
            .target_step_id()
            .clone()),
        }
    }

    fn check_step(&self, id: &StepId) -> Result<(), SelectorError> {
        if !self.graph.contains(id) {
            return Err(SelectorError::Missing {
                kind: ReferenceKind::Step,
                value: id.to_string(),
            });
        }
        if !self.space_steps.contains(id) {
            return Err(SelectorError::OutsideSpace {
                kind: ReferenceKind::Step,
                id: id.to_string(),
            });
        }
        Ok(())
    }

    fn visible_layer(&self, id: &LayerId) -> Result<&[StepId], SelectorError> {
        require(&self.layers, id, ReferenceKind::Layer)?;
        self.view
            .layer_heads()
            .iter()
            .find(|heads| heads.layer_id() == id)
            .map(|heads| heads.step_ids())
            .ok_or_else(|| SelectorError::NotVisible {
                kind: ReferenceKind::Layer,
                id: id.to_string(),
            })
    }
}

fn index<'a, I: Ord + Clone + fmt::Display, T>(
    objects: impl IntoIterator<Item = &'a T>,
    kind: ReferenceKind,
    id: impl Fn(&T) -> &I,
    space: impl Fn(&T) -> &SpaceId,
    expected: &SpaceId,
) -> Result<BTreeMap<I, &'a T>, SelectorError> {
    let mut result = BTreeMap::new();
    for object in objects {
        if space(object) != expected {
            return Err(SelectorError::OutsideSpace {
                kind,
                id: id(object).to_string(),
            });
        }
        if result.insert(id(object).clone(), object).is_some() {
            return Err(SelectorError::Duplicate {
                kind,
                id: id(object).to_string(),
            });
        }
    }
    Ok(result)
}

fn require<'a, I: Ord + fmt::Display, T>(
    objects: &BTreeMap<I, &'a T>,
    id: &I,
    kind: ReferenceKind,
) -> Result<&'a T, SelectorError> {
    objects
        .get(id)
        .copied()
        .ok_or_else(|| SelectorError::Missing {
            kind,
            value: id.to_string(),
        })
}

fn select<'a, I: Ord + fmt::Display, T>(
    objects: &BTreeMap<I, &'a T>,
    selector: &ReferenceSelector<I>,
    kind: ReferenceKind,
    name: impl Fn(&T) -> &str,
    visible: impl Fn(&I) -> bool,
) -> Result<&'a T, SelectorError> {
    match selector {
        ReferenceSelector::Id(id) => {
            let object = require(objects, id, kind)?;
            if !visible(id) {
                return Err(SelectorError::NotVisible {
                    kind,
                    id: id.to_string(),
                });
            }
            Ok(object)
        }
        ReferenceSelector::Name(query) => {
            let matches: Vec<_> = objects
                .iter()
                .filter(|(id, value)| visible(id) && name(value) == query)
                .collect();
            match matches.as_slice() {
                [] => Err(SelectorError::Missing {
                    kind,
                    value: query.clone(),
                }),
                [(_, object)] => Ok(*object),
                _ => Err(SelectorError::AmbiguousName {
                    kind,
                    name: query.clone(),
                    candidates: matches.iter().map(|(id, _)| id.to_string()).collect(),
                }),
            }
        }
    }
}
