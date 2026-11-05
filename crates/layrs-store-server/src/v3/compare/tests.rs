use super::fixture::{Counts, Fixture, TestSource, entry, space};
use super::*;
use layrs_core::scm::{ConflictId, Tombstone};
use serde_json::Value;

pub(super) fn reader<'a>(source: &'a TestSource, space_id: &'a SpaceId) -> Reader<'a, TestSource> {
    Reader::new(source, space_id)
}

pub(super) async fn differential(
    fixture: &Fixture,
    before: &StepId,
    after: &StepId,
) -> (Result<Value, String>, Counts, Counts) {
    let space_id = space();
    let eager_source = fixture.source();
    let source = fixture.source();
    let mut eager = legacy::LegacyReader {
        source: &eager_source,
        space_id: &space_id,
        bytes: 0,
        objects: 0,
    };
    let mut actual = reader(&source, &space_id);
    let expected = async {
        let before = eager.step(before).await?;
        let after = eager.step(after).await?;
        Ok::<_, V3StoreError>(
            serde_json::to_value(diff_entries(&entries(&before), &entries(&after))).unwrap(),
        )
    }
    .await
    .map_err(|error| error.to_string());
    let result = async {
        let before = actual.step(before).await?;
        let after = actual.step(after).await?;
        Ok::<_, V3StoreError>(
            serde_json::to_value(diff_entries(&entries(&before), &entries(&after))).unwrap(),
        )
    }
    .await
    .map_err(|error| error.to_string());
    assert_eq!(result, expected);
    assert_eq!((actual.bytes, actual.objects), (eager.bytes, eager.objects));
    let eager_counts = eager_source.counts.lock().unwrap().clone();
    let counts = source.counts.lock().unwrap().clone();
    assert_eq!(counts.descriptors, eager_counts.descriptors);
    assert_eq!(counts.authorizations, eager_counts.authorizations);
    (result, eager_counts, counts)
}

#[tokio::test]
async fn differential_types_modes_presence_and_content() {
    let mut fixture = Fixture::default();
    let mut alternatives = vec![None];
    for bytes in [
        b"".as_slice(),
        b"old\r\ntext",
        b"new\ntext\n",
        b"\0binary",
        b"\xff",
    ] {
        let file = fixture.file(bytes);
        for executable in [false, true] {
            alternatives.push(Some(entry(
                "item",
                TreeEntryTarget::file(file.clone()),
                executable,
            )));
        }
    }
    alternatives.push(Some(entry("item", fixture.link("target"), false)));
    alternatives.push(Some(entry("item", fixture.link("target"), true)));
    let tombstone = Tombstone::new(None).unwrap();
    fixture.object(
        tombstone.id().as_str(),
        ObjectKind::Tombstone,
        serde_json::to_vec(&tombstone).unwrap(),
    );
    alternatives.push(Some(entry(
        "item",
        TreeEntryTarget::tombstone(tombstone.id().clone()),
        false,
    )));
    let child_file = fixture.file(b"nested");
    let child = fixture.tree(vec![entry(
        "child",
        TreeEntryTarget::file(child_file),
        false,
    )]);
    alternatives.push(Some(entry("item", TreeEntryTarget::tree(child), false)));
    let steps: Vec<_> = alternatives
        .into_iter()
        .map(|value| {
            let root = fixture.tree(value.into_iter().collect());
            fixture.step(root)
        })
        .collect();
    for before in &steps {
        for after in &steps {
            differential(&fixture, before, after).await.0.unwrap();
        }
    }
}

fn aliased_fixture() -> (Fixture, StepId, StepId) {
    let mut fixture = Fixture::default();
    let common = fixture.file(b"common\r\ntext\n");
    let nested = fixture.tree(vec![entry(
        "file",
        TreeEntryTarget::file(common.clone()),
        true,
    )]);
    let child = fixture.tree(vec![entry("nested", TreeEntryTarget::tree(nested), false)]);
    let before_tree = fixture.tree(vec![
        entry("left", TreeEntryTarget::tree(child.clone()), false),
        entry("right", TreeEntryTarget::tree(child.clone()), true),
        entry("mode", TreeEntryTarget::file(common.clone()), false),
    ]);
    let after_tree = fixture.tree(vec![
        entry("left", TreeEntryTarget::tree(child.clone()), true),
        entry("right", TreeEntryTarget::tree(child), false),
        entry("mode", TreeEntryTarget::file(common), true),
    ]);
    let before = fixture.step(before_tree);
    let after = fixture.step(after_tree);
    (fixture, before, after)
}

#[tokio::test]
async fn differential_aliases_keep_each_path_and_mode() {
    let (fixture, before, after) = aliased_fixture();
    let (result, eager, actual) = differential(&fixture, &before, &after).await;
    let changes = result.unwrap();
    assert_eq!(changes.as_array().unwrap().len(), 1);
    assert_eq!(changes[0]["path"], "mode");
    assert_eq!(changes[0]["executableChanged"], true);
    println!(
        "alias reads before={:?}, after={:?}",
        eager.reads, actual.reads
    );
    assert_eq!(eager.reads(ObjectKind::Tree), 10);
    assert_eq!(actual.reads(ObjectKind::Tree), 4);
    assert_eq!(eager.reads(ObjectKind::FileManifest), 6);
    assert_eq!(actual.reads(ObjectKind::FileManifest), 1);
    assert_eq!(eager.reads(ObjectKind::Chunk), 6);
    assert_eq!(actual.reads(ObjectKind::Chunk), 1);
}

#[tokio::test]
async fn differential_conflicts_denied_descendants_and_malformed_metadata() {
    let (mut fixture, before, after) = aliased_fixture();
    fixture.denied.insert("left/nested/file".into());
    assert!(
        differential(&fixture, &before, &after)
            .await
            .0
            .unwrap_err()
            .contains("capability")
    );
    fixture.denied.clear();
    let conflict = ConflictId::parse(format!("blake3:{}", "f".repeat(64))).unwrap();
    let child = fixture.tree(vec![entry(
        "conflict",
        TreeEntryTarget::conflict(conflict),
        false,
    )]);
    let root = fixture.tree(vec![entry("child", TreeEntryTarget::tree(child), false)]);
    let step = fixture.step(root.clone());
    assert!(
        differential(&fixture, &step, &step)
            .await
            .0
            .unwrap_err()
            .contains("unresolved conflicts")
    );
    fixture.object(root.as_str(), ObjectKind::Tree, b"{}".to_vec());
    assert!(
        differential(&fixture, &step, &step)
            .await
            .0
            .unwrap_err()
            .contains("corrupt CAS")
    );
}

#[tokio::test]
async fn differential_text_output_budget_is_unchanged() {
    let mut fixture = Fixture::default();
    let old = fixture.file(&vec![b'a'; 400_000]);
    let new = fixture.file(&vec![b'b'; 400_000]);
    let before = fixture.tree(
        (0..6)
            .map(|index| {
                entry(
                    &format!("file{index}"),
                    TreeEntryTarget::file(old.clone()),
                    false,
                )
            })
            .collect(),
    );
    let after = fixture.tree(
        (0..6)
            .map(|index| {
                entry(
                    &format!("file{index}"),
                    TreeEntryTarget::file(new.clone()),
                    false,
                )
            })
            .collect(),
    );
    let before = fixture.step(before);
    let after = fixture.step(after);
    let changes = differential(&fixture, &before, &after).await.0.unwrap();
    assert_eq!(changes[5]["textOmitted"], true);
    assert_eq!(changes[0]["textOmitted"], false);
}
