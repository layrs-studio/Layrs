use super::*;

struct DropSource {
    inner: Arc<Source>,
    dropped: Arc<AtomicBool>,
}
impl Drop for DropSource {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}
impl SnapshotSource for DropSource {
    fn catalog(&self) -> &Catalog {
        self.inner.catalog()
    }
    fn read_exact_at(
        &self,
        id: &FileId,
        offset: u64,
        buffer: &mut [u8],
        cancel: &Cancellation,
    ) -> Result<()> {
        self.inner.read_exact_at(id, offset, buffer, cancel)
    }
}

#[test]
#[ignore = "successful disconnect then source destruction; sole retained complete proof isolates native sharing"]
fn stage2_root_proof_outlives_source() {
    let run = fixture::run("independent-proof");
    let (f, control) = fixture::Fixture::create(&run, "sole-proof");
    let catalog = catalog(false);
    let proof = create(&f, &control, catalog.clone());
    let ticket = proof.registration().clone();
    let expected = control.identity().clone();
    let dropped = Arc::new(AtomicBool::new(false));
    let source = Arc::new(DropSource {
        inner: Source::new(catalog.clone()),
        dropped: dropped.clone(),
    });
    let weak = Arc::downgrade(&source);
    let weak_inner = Arc::downgrade(&source.inner);
    let mut session =
        Session::reconnect_disconnected_with_control_directory(proof, source.clone()).unwrap();
    session.connect().unwrap();
    let retained = session.root_guard().unwrap();
    session.disconnect().unwrap();
    assert!(!session.is_connected());
    let checkpoint = session.checkpoint().unwrap();
    checkpoint.root_guard().unwrap().revalidate().unwrap();
    checkpoint.revalidate_namespace().unwrap();
    drop(checkpoint); // Required by its borrow of Session.
    assert_eq!(source.inner.bytes.load(Ordering::Acquire), 0);
    drop(session);
    drop(source);
    drop(control);
    assert!(
        weak.upgrade().is_none(),
        "source retained after successful disconnect/Session drop"
    );
    assert!(weak_inner.upgrade().is_none());
    assert!(dropped.load(Ordering::Acquire));
    // No connection or other proof/control/checkpoint owner remains. These
    // denials isolate the actual independent guard's native pins.
    retained.revalidate().unwrap();
    for path in [
        &f.root,
        &f.root.join(".layrs"),
        &f.root.join(".layrs/FORMAT"),
    ] {
        fixture::denied_delete(path);
    }
    fixture::raw32(
        fs::open(
            &f.root.join(".layrs/FORMAT"),
            io::ALL_SHARING,
            FILE_WRITE_DATA,
        )
        .unwrap_err(),
    );
    retained.revalidate().unwrap();
    assert_eq!(calls().1, 0);
    drop(retained);
    let reopened = ControlDirectoryGuard::open(&f.root, &expected).unwrap();
    ticket
        .guard_with_control_directory(catalog, &reopened)
        .unwrap()
        .revalidate()
        .unwrap();
    eprintln!(
        "PASS independent actual proof survives successful disconnect and destruction of Session/checkpoint/source/original control; Weak+Drop witnesses confirm source destruction; sole proof root/.layrs/FORMAT DELETE32 + FORMAT WRITE32; fresh owner opens only after proof drop."
    );
}

#[test]
#[ignore = "V2 normal-drive root beyond260 UTF16 units; keep old connect conversion for discriminating qualification"]
fn stage2_long_path_root() {
    let run = fixture::run("long-path");
    let mut parent = run;
    for _ in 0..2 {
        parent = parent.join("long-component-".to_owned() + &"x".repeat(66));
        std::fs::create_dir(&parent).unwrap();
    }
    let (f, control) = fixture::Fixture::create(&parent, "v2-root");
    let units = f.root.to_str().unwrap().encode_utf16().count();
    assert!(units > 260 && f.root.to_str().unwrap().len() < CONTROL_DIRECTORY_MAX_PATH_UTF8_BYTES);
    eprintln!(
        "V2 long-path exact root units={units}, root={}",
        f.root.display()
    );
    let catalog = catalog(false);
    let proof = create(&f, &control, catalog.clone());
    let ticket = proof.registration().clone();
    let expected = control.identity().clone();
    let source = Source::new(catalog.clone());
    let mut session =
        Session::reconnect_disconnected_with_control_directory(proof, source.clone()).unwrap();
    assert_eq!(
        io::id(session.root.file()).unwrap(),
        (ticket.volume, ticket.file_index)
    );
    session.connect().unwrap(); // No normalization/fallback path on failure.
    owned_child(&f.base, "read", "", 0);
    assert!(source.bytes.load(Ordering::Acquire) > 0);
    session.disconnect().unwrap();
    drop(session);
    drop(source);
    drop(control);
    let reopened = ControlDirectoryGuard::open(&f.root, &expected).unwrap();
    ticket
        .guard_with_control_directory(catalog, &reopened)
        .unwrap()
        .revalidate()
        .unwrap();
    eprintln!(
        "PASS V2 long path {units} UTF16 units: source-free creation, explicit connection/native read/disconnect, fresh expected proof reacquisition; unchanged normal-drive restrictions."
    );
}
