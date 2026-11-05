use layrs_core::scm::{
    Layer, LayerHeadSet, LayerId, LayerKind, LayerSettings, LayerStatus, Marker, MarkerId,
    MarkerKind, Metadata, Release, ReleaseId, RepositoryView, SpaceId, Step, StepId, Timestamp,
    TreeId, WorktreeId, WorktreeStatus, WorktreeViewState,
};
use layrs_graph::scm::{
    ReferenceKind, ReferenceSelector as Ref, ScmGraphError, SelectorError as Error,
    SelectorSnapshot, StepDag, StepSelector as Select,
};

fn space() -> SpaceId {
    SpaceId::parse("space:selectors").unwrap()
}
fn time() -> Timestamp {
    Timestamp::parse("2026-09-19T00:00:00Z").unwrap()
}
fn step(seed: u64, parents: &[&Step]) -> Step {
    Step::new(
        parents.iter().map(|step| step.id().clone()).collect(),
        TreeId::parse(format!("blake3:{seed:064x}")).unwrap(),
    )
    .unwrap()
}
fn layer(id: &str, name: &str) -> Layer {
    Layer::new(
        LayerId::parse(format!("layer:{id}")).unwrap(),
        space(),
        name,
        LayerKind::Base,
        LayerStatus::Active,
        vec![],
        LayerSettings::new(true, 500).unwrap(),
        time(),
        Metadata::empty(),
    )
    .unwrap()
}
fn marker(id: &str, name: &str, target: &Step) -> Marker {
    Marker::new(
        MarkerId::parse(format!("marker:{id}")).unwrap(),
        space(),
        target.id().clone(),
        MarkerKind::Checkpoint,
        name,
        "presentation title",
        "",
        None,
        time(),
        vec![],
        Metadata::empty(),
    )
    .unwrap()
}
fn release(id: &str, version: &str, target: &Step) -> Release {
    Release::new(
        ReleaseId::parse(format!("release:{id}")).unwrap(),
        space(),
        target.id().clone(),
        version,
        "presentation title",
        "",
        None,
        time(),
        vec![],
        Metadata::empty(),
    )
    .unwrap()
}

struct Fixture {
    root: Step,
    left: Step,
    right: Step,
    outside: Step,
    graph: StepDag,
    layers: Vec<Layer>,
    markers: Vec<Marker>,
    releases: Vec<Release>,
    view: RepositoryView,
}

impl Fixture {
    fn new() -> Self {
        let root = step(1, &[]);
        let left = step(2, &[&root]);
        let right = step(3, &[&root]);
        let outside = step(4, &[]);
        let graph = StepDag::from_steps([&root, &left, &right, &outside]).unwrap();
        let layers = vec![
            layer("main", "main"),
            layer("empty", "empty"),
            layer("fork", "fork"),
        ];
        let markers = vec![marker("one", "saved", &root)];
        let releases = vec![release("one", "1.0", &left)];
        let view = RepositoryView::new(
            space(),
            vec![
                LayerHeadSet::new(layers[0].id().clone(), vec![left.id().clone()]).unwrap(),
                LayerHeadSet::new(layers[1].id().clone(), vec![]).unwrap(),
                LayerHeadSet::new(
                    layers[2].id().clone(),
                    vec![right.id().clone(), left.id().clone()],
                )
                .unwrap(),
            ],
            vec![markers[0].id().clone()],
            vec![releases[0].id().clone()],
            vec![],
        )
        .unwrap();
        Self {
            root,
            left,
            right,
            outside,
            graph,
            layers,
            markers,
            releases,
            view,
        }
    }
    fn membership(&self) -> Vec<StepId> {
        vec![
            self.root.id().clone(),
            self.left.id().clone(),
            self.right.id().clone(),
        ]
    }
    fn snapshot(&self) -> Result<SelectorSnapshot<'_>, Error> {
        SelectorSnapshot::new(
            &space(),
            &self.graph,
            &self.view,
            self.membership(),
            &self.layers,
            &self.markers,
            &self.releases,
        )
    }
    fn named_view(&mut self) {
        self.view = RepositoryView::new(
            space(),
            self.view.layer_heads().to_vec(),
            self.markers
                .iter()
                .map(|object| object.id().clone())
                .collect(),
            self.releases
                .iter()
                .map(|object| object.id().clone())
                .collect(),
            vec![],
        )
        .unwrap();
    }
}

#[test]
fn exact_ids_resolve_all_four_kinds() {
    let f = Fixture::new();
    let snapshot = f.snapshot().unwrap();
    for (selector, expected) in [
        (Select::Step(f.root.id().clone()), f.root.id()),
        (
            Select::Layer {
                reference: Ref::Id(f.layers[0].id().clone()),
                head: None,
            },
            f.left.id(),
        ),
        (
            Select::Marker(Ref::Id(f.markers[0].id().clone())),
            f.root.id(),
        ),
        (
            Select::Release(Ref::Id(f.releases[0].id().clone())),
            f.left.id(),
        ),
    ] {
        assert_eq!(&snapshot.resolve(&selector).unwrap(), expected);
    }
    assert_eq!(snapshot.view().id(), f.view.id());
}

#[test]
fn names_are_exact_explicit_and_separate_from_ids_and_titles() {
    let mut f = Fixture::new();
    f.markers.push(marker("two", "marker:one", &f.right));
    f.named_view();
    let snapshot = f.snapshot().unwrap();
    assert_eq!(
        snapshot
            .resolve(&Select::Marker(Ref::Id(f.markers[0].id().clone())))
            .unwrap(),
        *f.root.id()
    );
    assert_eq!(
        snapshot
            .resolve(&Select::Marker(Ref::Name("marker:one".into())))
            .unwrap(),
        *f.right.id()
    );
    assert_eq!(
        snapshot
            .resolve(&Select::Release(Ref::Name("1.0".into())))
            .unwrap(),
        *f.left.id()
    );
    assert_eq!(
        snapshot
            .resolve(&Select::Layer {
                reference: Ref::Name("main".into()),
                head: None
            })
            .unwrap(),
        *f.left.id()
    );
    for query in ["MAIN", "mai", " main", "presentation title"] {
        assert!(matches!(
            snapshot.resolve(&Select::Layer {
                reference: Ref::Name(query.into()),
                head: None
            }),
            Err(Error::Missing { .. })
        ));
    }
    assert!(matches!(
        snapshot.resolve(&Select::Release(Ref::Name("presentation title".into()))),
        Err(Error::Missing { .. })
    ));
}

#[test]
fn empty_and_divergent_layers_never_choose_an_implicit_target() {
    let f = Fixture::new();
    let snapshot = f.snapshot().unwrap();
    assert_eq!(
        snapshot.resolve(&Select::Layer {
            reference: Ref::Id(f.layers[1].id().clone()),
            head: None
        }),
        Err(Error::LayerWithoutHead(f.layers[1].id().clone()))
    );
    let mut expected = vec![f.left.id().clone(), f.right.id().clone()];
    expected.sort();
    assert_eq!(
        snapshot.resolve(&Select::Layer {
            reference: Ref::Id(f.layers[2].id().clone()),
            head: None
        }),
        Err(Error::MultipleHeads {
            layer_id: f.layers[2].id().clone(),
            heads: expected
        })
    );
    for head in [&f.left, &f.right] {
        assert_eq!(
            snapshot
                .resolve(&Select::Layer {
                    reference: Ref::Id(f.layers[2].id().clone()),
                    head: Some(head.id().clone())
                })
                .unwrap(),
            *head.id()
        );
    }
    assert!(matches!(
        snapshot.resolve(&Select::Layer {
            reference: Ref::Id(f.layers[2].id().clone()),
            head: Some(f.root.id().clone())
        }),
        Err(Error::NotLayerHead { .. })
    ));
}

#[test]
fn ambiguous_names_return_sorted_identity_candidates_even_for_same_target() {
    let mut f = Fixture::new();
    f.markers.insert(0, marker("zero", "saved", &f.root));
    f.releases.push(release("two", "1.0", &f.root));
    f.layers.push(layer("duplicate", "main"));
    let mut heads = f.view.layer_heads().to_vec();
    heads.push(LayerHeadSet::new(f.layers[3].id().clone(), vec![f.root.id().clone()]).unwrap());
    f.view = RepositoryView::new(space(), heads, vec![], vec![], vec![]).unwrap();
    f.named_view();
    let snapshot = f.snapshot().unwrap();
    for (selector, kind, expected) in [
        (
            Select::Marker(Ref::Name("saved".into())),
            ReferenceKind::Marker,
            vec!["marker:one", "marker:zero"],
        ),
        (
            Select::Release(Ref::Name("1.0".into())),
            ReferenceKind::Release,
            vec!["release:one", "release:two"],
        ),
        (
            Select::Layer {
                reference: Ref::Name("main".into()),
                head: None,
            },
            ReferenceKind::Layer,
            vec!["layer:duplicate", "layer:main"],
        ),
    ] {
        match snapshot.resolve(&selector) {
            Err(Error::AmbiguousName {
                kind: actual,
                candidates,
                ..
            }) => {
                assert_eq!(actual, kind);
                assert_eq!(candidates, expected);
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
    }
}

#[test]
fn hidden_references_do_not_shadow_names_and_historical_view_restores_visibility() {
    let mut f = Fixture::new();
    let historical_view = f.view.clone();
    f.markers.push(marker("two", "saved", &f.right));
    f.view = RepositoryView::new(
        space(),
        vec![],
        vec![f.markers[1].id().clone()],
        vec![],
        vec![],
    )
    .unwrap();
    let snapshot = f.snapshot().unwrap();
    assert_eq!(
        snapshot
            .resolve(&Select::Marker(Ref::Name("saved".into())))
            .unwrap(),
        *f.right.id()
    );
    for selector in [
        Select::Marker(Ref::Id(f.markers[0].id().clone())),
        Select::Release(Ref::Id(f.releases[0].id().clone())),
        Select::Layer {
            reference: Ref::Id(f.layers[0].id().clone()),
            head: None,
        },
    ] {
        assert!(matches!(
            snapshot.resolve(&selector),
            Err(Error::NotVisible { .. })
        ));
    }
    assert_eq!(
        snapshot
            .resolve(&Select::Step(f.root.id().clone()))
            .unwrap(),
        *f.root.id()
    );
    f.view = historical_view;
    assert_eq!(
        f.snapshot()
            .unwrap()
            .resolve(&Select::Marker(Ref::Id(f.markers[0].id().clone())))
            .unwrap(),
        *f.root.id()
    );
}

#[test]
fn missing_targets_and_known_steps_outside_space_are_distinct() {
    let f = Fixture::new();
    let snapshot = f.snapshot().unwrap();
    let unknown = step(999, &[]);
    assert!(matches!(
        snapshot.resolve(&Select::Step(unknown.id().clone())),
        Err(Error::Missing {
            kind: ReferenceKind::Step,
            ..
        })
    ));
    assert!(matches!(
        snapshot.resolve(&Select::Step(f.outside.id().clone())),
        Err(Error::OutsideSpace {
            kind: ReferenceKind::Step,
            ..
        })
    ));
    for selector in [
        Select::Marker(Ref::Id(MarkerId::parse("marker:unknown").unwrap())),
        Select::Release(Ref::Id(ReleaseId::parse("release:unknown").unwrap())),
        Select::Layer {
            reference: Ref::Id(LayerId::parse("layer:unknown").unwrap()),
            head: None,
        },
    ] {
        assert!(matches!(
            snapshot.resolve(&selector),
            Err(Error::Missing { .. })
        ));
    }
}

#[test]
fn duplicate_objects_and_incomplete_visible_reference_sets_are_rejected() {
    let mut f = Fixture::new();
    f.markers.push(f.markers[0].clone());
    assert!(matches!(
        f.snapshot(),
        Err(Error::Duplicate {
            kind: ReferenceKind::Marker,
            ..
        })
    ));
    f.markers.clear();
    assert!(matches!(
        f.snapshot(),
        Err(Error::Missing {
            kind: ReferenceKind::Marker,
            ..
        })
    ));
}

#[test]
fn invalid_head_antichain_is_rejected_without_silent_normalization() {
    let mut f = Fixture::new();
    f.view = RepositoryView::new(
        space(),
        vec![
            LayerHeadSet::new(
                f.layers[0].id().clone(),
                vec![f.root.id().clone(), f.left.id().clone()],
            )
            .unwrap(),
        ],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    assert!(matches!(
        f.snapshot(),
        Err(Error::Graph(ScmGraphError::LayerHeadsNotAntichain { .. }))
    ));
}

#[test]
fn foreign_views_and_objects_are_rejected() {
    let mut f = Fixture::new();
    let foreign = SpaceId::parse("space:foreign").unwrap();
    assert!(matches!(
        SelectorSnapshot::new(
            &foreign,
            &f.graph,
            &f.view,
            f.membership(),
            &f.layers,
            &f.markers,
            &f.releases
        ),
        Err(Error::ViewSpaceMismatch { .. })
    ));
    f.markers.push(
        Marker::new(
            MarkerId::parse("marker:foreign").unwrap(),
            foreign,
            f.root.id().clone(),
            MarkerKind::Checkpoint,
            "saved",
            "",
            "",
            None,
            time(),
            vec![],
            Metadata::empty(),
        )
        .unwrap(),
    );
    assert!(matches!(
        f.snapshot(),
        Err(Error::OutsideSpace {
            kind: ReferenceKind::Marker,
            ..
        })
    ));
}

#[test]
fn dangling_or_out_of_space_named_targets_fail_snapshot_construction() {
    let mut f = Fixture::new();
    f.markers = vec![marker("one", "saved", &step(999, &[]))];
    assert!(matches!(
        f.snapshot(),
        Err(Error::Missing {
            kind: ReferenceKind::Step,
            ..
        })
    ));
    f.markers = vec![marker("one", "saved", &f.outside)];
    assert!(matches!(
        f.snapshot(),
        Err(Error::OutsideSpace {
            kind: ReferenceKind::Step,
            ..
        })
    ));
}

#[test]
fn space_membership_must_include_ancestry_even_if_graph_is_global() {
    let f = Fixture::new();
    assert!(matches!(
        SelectorSnapshot::new(
            &space(),
            &f.graph,
            &f.view,
            [f.left.id().clone()],
            &f.layers,
            &f.markers,
            &f.releases
        ),
        Err(Error::OutsideSpace {
            kind: ReferenceKind::Step,
            ..
        })
    ));
}

#[test]
fn worktree_references_cannot_escape_selected_space() {
    let mut f = Fixture::new();
    let worktree = WorktreeViewState::new(
        WorktreeId::parse("worktree:one").unwrap(),
        Some(f.layers[0].id().clone()),
        Some(f.outside.id().clone()),
        None,
        WorktreeStatus::Ready,
    );
    f.view = RepositoryView::new(
        space(),
        f.view.layer_heads().to_vec(),
        vec![],
        vec![],
        vec![worktree],
    )
    .unwrap();
    assert!(matches!(
        f.snapshot(),
        Err(Error::OutsideSpace {
            kind: ReferenceKind::Step,
            ..
        })
    ));
}

#[test]
fn input_order_does_not_change_resolution_or_ambiguity() {
    let mut f = Fixture::new();
    f.markers.push(marker("two", "saved", &f.root));
    f.named_view();
    let selector = Select::Marker(Ref::Name("saved".into()));
    let expected = f.snapshot().unwrap().resolve(&selector);
    f.layers.reverse();
    f.markers.reverse();
    f.releases.reverse();
    assert_eq!(f.snapshot().unwrap().resolve(&selector), expected);
}
