//! Lazy File content: unchanged closure Files are proven present without being
//! read, content is loaded only for multi-side leaves, verified, cached once
//! per plan, and only resident bytes count toward `max_input_bytes`.
use super::*;
use std::cell::RefCell;

#[derive(Default)]
struct Counting {
    files: BTreeMap<FileId, Vec<u8>>,
    loads: RefCell<BTreeMap<FileId, usize>>,
    corrupt: BTreeSet<FileId>,
}
impl MergeFileSource for Counting {
    fn contains(&self, id: &FileId) -> bool {
        self.files.contains_key(id)
    }
    fn load(&self, id: &FileId) -> std::result::Result<Vec<u8>, String> {
        *self.loads.borrow_mut().entry(id.clone()).or_default() += 1;
        let mut bytes = self.files.get(id).cloned().ok_or("absent")?;
        if self.corrupt.contains(id) {
            bytes.push(b'!');
        }
        Ok(bytes)
    }
}
impl Counting {
    fn total_loads(&self) -> usize {
        self.loads.borrow().values().sum()
    }
}

/// Moves every File out of the in-memory map into a counting lazy source.
fn lazy(objects: &MergeObjects) -> (MergeObjects, Counting) {
    let mut without = objects.clone();
    let files = std::mem::take(&mut without.files);
    (
        without,
        Counting {
            files,
            ..Counting::default()
        },
    )
}

fn tree(objects: &mut MergeObjects, files: &BTreeMap<String, Vec<u8>>) -> TreeId {
    let mut entries = vec![];
    let mut dirs = BTreeMap::<String, BTreeMap<String, Vec<u8>>>::new();
    for (path, bytes) in files {
        match path.split_once('/') {
            Some((dir, rest)) => {
                dirs.entry(dir.to_owned())
                    .or_default()
                    .insert(rest.to_owned(), bytes.clone());
            }
            None => entries.push(file(objects, path, bytes)),
        }
    }
    for (dir, children) in dirs {
        let id = tree(objects, &children);
        entries.push(TreeEntry::new(
            RepoPathComponent::parse(&dir).unwrap(),
            TreeEntryTarget::tree(id),
            false,
        ));
    }
    let tree = Tree::new(entries).unwrap();
    let id = tree.id().clone();
    objects.trees.insert(id.clone(), tree);
    id
}

fn snapshot(
    objects: &mut MergeObjects,
    files: &BTreeMap<String, Vec<u8>>,
    parents: &[StepId],
) -> StepId {
    let root = tree(objects, files);
    let step = Step::new(parents.to_vec(), root).unwrap();
    let id = step.id().clone();
    objects.steps.insert(id.clone(), step);
    id
}

fn repository(count: usize) -> BTreeMap<String, Vec<u8>> {
    (0..count)
        .map(|i| {
            (
                format!("dir{}/file{i}.txt", i % 7),
                format!("unchanged content {i}\n").into_bytes(),
            )
        })
        .collect()
}

#[test]
fn disjoint_changes_never_load_unchanged_file_content() {
    let mut objects = MergeObjects::default();
    let original = repository(200);
    let base = snapshot(&mut objects, &original, &[]);
    let mut left = original.clone();
    left.insert("dir1/file1.txt".into(), b"left edit\n".to_vec());
    left.insert("added/left.txt".into(), b"new left\n".to_vec());
    let mut right = original.clone();
    right.insert("dir2/file2.txt".into(), b"right edit\n".to_vec());
    right.remove("dir3/file3.txt");
    let a = snapshot(&mut objects, &left, std::slice::from_ref(&base));
    let b = snapshot(&mut objects, &right, std::slice::from_ref(&base));

    let eager = plan(&objects, std::slice::from_ref(&a), std::slice::from_ref(&b));
    let (without, source) = lazy(&objects);
    let merged = plan_graph_merge_with_source(
        &without,
        &source,
        std::slice::from_ref(&a),
        std::slice::from_ref(&b),
        policy(),
        GraphMergeLimits::default(),
    )
    .unwrap();
    assert_eq!(source.total_loads(), 0, "no File content may be read");
    assert!(merged.conflicts().is_empty());
    assert_eq!(merged.step(), eager.step());
    assert_eq!(merged.trees(), eager.trees());
    assert_eq!(
        merged.installation_bundle().unwrap().canonical_bytes(),
        eager.installation_bundle().unwrap().canonical_bytes()
    );

    let structural = plan_structural_merge_with_source(
        &without,
        &source,
        &[a],
        &[b],
        policy(),
        GraphMergeLimits::default(),
    )
    .unwrap();
    assert_eq!(structural.step(), eager.step());
    assert_eq!(source.total_loads(), 0);
}

#[test]
fn multi_side_content_leaf_loads_each_original_exactly_once() {
    let mut objects = MergeObjects::default();
    let mut original = repository(50);
    original.insert("code.ts".into(), b"a\nb\nc\n".to_vec());
    let base = snapshot(&mut objects, &original, &[]);
    let mut left = original.clone();
    left.insert("code.ts".into(), b"A\nb\nc\n".to_vec());
    let mut right = original.clone();
    right.insert("code.ts".into(), b"a\nb\nC\n".to_vec());
    let a = snapshot(&mut objects, &left, std::slice::from_ref(&base));
    let b = snapshot(&mut objects, &right, std::slice::from_ref(&base));

    let eager = plan(&objects, std::slice::from_ref(&a), std::slice::from_ref(&b));
    let (without, source) = lazy(&objects);
    let merged = plan_graph_merge_with_source(
        &without,
        &source,
        &[a],
        &[b],
        policy(),
        GraphMergeLimits::default(),
    )
    .unwrap();
    assert!(merged.conflicts().is_empty());
    assert_eq!(merged.step(), eager.step());
    assert_eq!(merged.files(), eager.files());
    let loads = source.loads.borrow();
    let expected: BTreeSet<_> = [&b"a\nb\nc\n"[..], b"A\nb\nc\n", b"a\nb\nC\n"]
        .into_iter()
        .map(FileId::from_bytes)
        .collect();
    assert_eq!(loads.keys().cloned().collect::<BTreeSet<_>>(), expected);
    assert!(loads.values().all(|count| *count == 1));
}

#[test]
fn input_byte_limit_counts_resident_bytes_not_unread_closure_files() {
    let mut objects = MergeObjects::default();
    let mut original = BTreeMap::new();
    original.insert("big/blob.bin".to_owned(), vec![7u8; 2 * 1024 * 1024]);
    original.insert("left.txt".to_owned(), b"left\n".to_vec());
    original.insert("right.txt".to_owned(), b"right\n".to_vec());
    let base = snapshot(&mut objects, &original, &[]);
    let mut left = original.clone();
    left.insert("left.txt".into(), b"left edit\n".to_vec());
    let mut right = original.clone();
    right.insert("right.txt".into(), b"right edit\n".to_vec());
    let a = snapshot(&mut objects, &left, std::slice::from_ref(&base));
    let b = snapshot(&mut objects, &right, std::slice::from_ref(&base));
    let limits = GraphMergeLimits {
        max_input_bytes: 64 * 1024,
        ..GraphMergeLimits::default()
    };
    // In-memory callers keep the historical accounting: the closure is resident.
    assert_eq!(
        plan_graph_merge(
            &objects,
            std::slice::from_ref(&a),
            std::slice::from_ref(&b),
            policy(),
            limits
        )
        .unwrap_err(),
        GraphMergeError::Limit("input bytes")
    );
    let (without, source) = lazy(&objects);
    let merged = plan_graph_merge_with_source(
        &without,
        &source,
        std::slice::from_ref(&a),
        std::slice::from_ref(&b),
        policy(),
        limits,
    )
    .unwrap();
    assert!(merged.conflicts().is_empty());
    assert_eq!(source.total_loads(), 0);

    // Content that must actually be loaded is still charged to the budget.
    let mut big_left = original.clone();
    big_left.insert("big/blob.bin".into(), vec![8u8; 2 * 1024 * 1024]);
    let mut big_right = original.clone();
    big_right.insert("big/blob.bin".into(), vec![9u8; 2 * 1024 * 1024]);
    let c = snapshot(&mut objects, &big_left, std::slice::from_ref(&base));
    let d = snapshot(&mut objects, &big_right, std::slice::from_ref(&base));
    let (without, source) = lazy(&objects);
    // A binary leaf has no Lens, so it becomes a structural conflict unread.
    let conflict = plan_graph_merge_with_source(
        &without,
        &source,
        std::slice::from_ref(&c),
        std::slice::from_ref(&d),
        policy(),
        limits,
    )
    .unwrap();
    assert_eq!(conflict.conflicts().len(), 1);
    assert_eq!(source.total_loads(), 0);
    let mut text_left = original.clone();
    text_left.insert("big/notes.txt".into(), vec![b'l'; 1024 * 1024]);
    let mut text_base = original.clone();
    text_base.insert("big/notes.txt".into(), vec![b'b'; 1024 * 1024]);
    let mut text_right = original;
    text_right.insert("big/notes.txt".into(), vec![b'r'; 1024 * 1024]);
    let base2 = snapshot(&mut objects, &text_base, &[]);
    let e = snapshot(&mut objects, &text_left, std::slice::from_ref(&base2));
    let f = snapshot(&mut objects, &text_right, std::slice::from_ref(&base2));
    let (without, source) = lazy(&objects);
    assert_eq!(
        plan_graph_merge_with_source(&without, &source, &[e], &[f], policy(), limits).unwrap_err(),
        GraphMergeError::Limit("input bytes")
    );
    assert_eq!(
        source.total_loads(),
        1,
        "rejected at the first oversized load"
    );
}

#[test]
fn lazy_content_is_verified_and_presence_is_required_before_planning() {
    let (objects, _, a, b) = branches();
    let (without, mut source) = lazy(&objects);
    source.corrupt = source.files.keys().cloned().collect();
    assert_eq!(
        plan_graph_merge_with_source(
            &without,
            &source,
            std::slice::from_ref(&a),
            std::slice::from_ref(&b),
            policy(),
            GraphMergeLimits::default()
        )
        .unwrap_err(),
        GraphMergeError::Invalid("File content ID mismatch".into())
    );
    let (without, mut source) = lazy(&objects);
    let base_file = FileId::from_bytes(b"a\nb\nc\n");
    source.files.remove(&base_file);
    assert_eq!(
        plan_graph_merge_with_source(
            &without,
            &source,
            &[a],
            &[b],
            policy(),
            GraphMergeLimits::default()
        )
        .unwrap_err(),
        GraphMergeError::Missing(base_file.to_string())
    );
    assert_eq!(source.total_loads(), 0, "closure is proven before any load");
}

#[test]
fn replay_and_resolution_with_source_read_no_unchanged_content() {
    let mut objects = MergeObjects::default();
    let original = repository(100);
    let base = snapshot(&mut objects, &original, &[]);
    let mut left = original.clone();
    left.insert("dir1/file1.txt".into(), b"left edit\n".to_vec());
    let mut right = original.clone();
    right.insert("dir2/file2.txt".into(), b"right edit\n".to_vec());
    let a = snapshot(&mut objects, &left, std::slice::from_ref(&base));
    let b = snapshot(&mut objects, &right, std::slice::from_ref(&base));
    let eager = plan_replay(
        &objects,
        &a,
        None,
        &b,
        false,
        policy(),
        GraphMergeLimits::default(),
    )
    .unwrap();
    let (without, source) = lazy(&objects);
    let replayed = plan_replay_with_source(
        &without,
        &source,
        &a,
        None,
        &b,
        false,
        policy(),
        GraphMergeLimits::default(),
    )
    .unwrap();
    assert_eq!(replayed.step, eager.step);
    assert_eq!(source.total_loads(), 0);

    // A structural (binary) conflict among many unchanged Files.
    let mut conflicted_left = original.clone();
    conflicted_left.insert("blob.bin".into(), vec![0, 1, 2]);
    let mut conflicted_right = original;
    conflicted_right.insert("blob.bin".into(), vec![0, 3, 4]);
    let c = snapshot(&mut objects, &conflicted_left, std::slice::from_ref(&base));
    let d = snapshot(&mut objects, &conflicted_right, std::slice::from_ref(&base));
    let merged = plan(&objects, &[c], &[d]);
    assert_eq!(merged.conflicts().len(), 1);
    install(&mut objects, &merged);
    let conflict = merged.conflicts().values().next().unwrap();
    let term =
        ObjectId::parse(conflict.document()["sides"][0]["termId"].as_str().unwrap()).unwrap();
    let choices = BTreeMap::from([(conflict.id(), term)]);
    let eager = resolve_conflict_terms(
        &objects,
        merged.step().id(),
        &choices,
        GraphMergeLimits::default(),
    )
    .unwrap();
    let (without, source) = lazy(&objects);
    let resolved = resolve_conflict_terms_with_source(
        &without,
        &source,
        merged.step().id(),
        &choices,
        GraphMergeLimits::default(),
    )
    .unwrap();
    assert!(resolved.conflicts.is_empty());
    assert_eq!(resolved.step, eager.step);
    assert_eq!(source.total_loads(), 0);
}
