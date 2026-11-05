//! SQL-memory DATA only. The physical caller separately owns identities,
//! schema9 provenance, locks and CAS pins. No test here grants byte authority.
use super::*;
use crate::projection::provider::{database_io::with_reference, owned_test as child};
use layrs_core::scm::{Chunk, ChunkingDescriptor};
use serde_json::json;

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED admitted-family SQL DATA; ROOT review/release child120 required"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                120,
                || {
                    child::declare();
                    $body
                },
            );
        }
    };
}

fn family() -> Vec<u8> {
    canonical_json(&json!({"schemaVersion":"layrs.partial-store-family.v1",
        "magic":"LAYRS_NATIVE_STORE","formatVersion":4,"minReaderVersion":4,
        "incarnation":"family","identity":{"storeId":"store:family","spaceId":"space:family",
        "origin":"https://example.invalid","principalId":"principal:test"},
        "rootIdentity":"fs:v1:0000000000000001:0000000000000002",
        "metadataIdentity":"fs:v1:0000000000000001:0000000000000003",
        "databaseIdentity":"1:0:4"}))
    .unwrap()
}

pub(super) fn manifests(bytes: &[u8]) -> Vec<FileManifest> {
    vec![
        FileManifest::from_chunks(
            ChunkingDescriptor::single(),
            &[Chunk::new(bytes.to_vec()).unwrap()],
        )
        .unwrap(),
        FileManifest::from_chunks(
            ChunkingDescriptor::deterministic_fixed(3).unwrap(),
            &bytes
                .chunks(3)
                .map(|b| Chunk::new(b.to_vec()).unwrap())
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    ]
}

pub(super) fn context(family: &[u8], n: u8, manifests: &[FileManifest]) -> AdmittedFamilyContext {
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-projection-recovery-family-v1\0");
    hash.update(family);
    let digest = format!("blake3:{}", hash.finalize());
    let token = format!("blake3:{}", format!("{n:x}").repeat(64));
    let operation = format!("provider-transition:{}", format!("{n:x}").repeat(64));
    let marker: format::Marker = serde_json::from_value(json!({
        "schemaVersion":"layrs.projection-store-format.v1","magic":"LAYRS_NATIVE_STORE",
        "formatVersion":4,"minReaderVersion":4,
        "identity":{"storeId":"store:family","spaceId":"space:family",
            "worktreeId":format!("worktree:member{n}"),"origin":"https://example.invalid",
            "principalId":"principal:test"},
        "checkoutRoot":format!("C:\\checkout{n}"),"checkoutIdentity":format!("checkout:{n}"),
        "storeIdentity":format!("store:{n}"),"metadataIdentity":format!("metadata:{n}"),
        "initialSnapshotDigest":"0".repeat(64),"importDigest":token,
        "shared":{"root":"C:\\family","incarnation":"family",
            "rootIdentity":"fs:v1:0000000000000001:0000000000000002",
            "metadataPath":format!("C:\\family\\worktrees\\member{n}"),
            "databaseIdentity":format!("1:0:{}", 100+n)}
    }))
    .unwrap();
    let binding = ExactBinding {
        family_incarnation: "family".into(),
        family_root_identity: "fs:v1:0000000000000001:0000000000000002".into(),
        family_marker_digest: digest,
        family_catalog_identity: "1:0:4".into(),
        plan_digest: token.clone(),
        prefix_id: token.clone(),
        destination_proof_digest: token.clone(),
        transaction_id: format!("{n:x}").repeat(32),
        owner: marker.identity.worktree_id.clone(),
        database_identity: marker.shared.as_ref().unwrap().database_identity.clone(),
        reservation: operation.clone(),
        operation,
        quota_bytes: 1024 * 1024,
    };
    let protection = ExactProtection::new(
        &binding.family_incarnation,
        &binding.owner,
        &binding.database_identity,
        &binding.transaction_id,
        manifests,
        None,
    )
    .unwrap();
    AdmittedFamilyContext {
        primary: ImportContext {
            marker,
            binding,
            root_id: protection.id().into(),
            required: manifests
                .iter()
                .map(|m| (m.file_id().to_string(), m.id().to_string(), m.byte_length()))
                .collect(),
            admission_digest: token,
        },
        additional: Vec::new(),
    }
}

pub(super) fn install(c: &Connection, context: &AdmittedFamilyContext, manifests: &[FileManifest]) {
    let g = &context.primary;
    let b = &g.binding;
    c.execute(
        "INSERT INTO worktrees VALUES(?1,?2,?3,?4,'ready')",
        params![
            b.owner.as_str(),
            g.marker
                .shared
                .as_ref()
                .unwrap()
                .metadata_path
                .to_string_lossy(),
            b.database_identity,
            g.marker.bytes().unwrap()
        ],
    )
    .unwrap();
    c.execute(
        "INSERT INTO reservations VALUES(?1,?2,?3,?4)",
        params![b.reservation, b.owner.as_str(), b.operation, b.quota_bytes],
    )
    .unwrap();
    let mut debit = 0;
    for (ordinal, m) in manifests.iter().enumerate() {
        let payload = canonical_json(m).unwrap();
        c.execute(
            "INSERT INTO files VALUES(?1,?2) ON CONFLICT DO NOTHING",
            params![m.file_id().as_str(), m.byte_length()],
        )
        .unwrap();
        let added = c
            .execute(
                "INSERT INTO manifests VALUES(?1,?2,?3,?4) ON CONFLICT DO NOTHING",
                params![
                    m.id().as_str(),
                    m.file_id().as_str(),
                    m.byte_length(),
                    payload
                ],
            )
            .unwrap()
            == 1;
        let mut charge = 8192 + payload.len() as u64 + if added { payload.len() as u64 } else { 0 };
        let mut bits = vec![b'0'; 256];
        for (n, chunk) in m.chunks().iter().enumerate() {
            if c.execute(
                "INSERT INTO chunks VALUES(?1,?2) ON CONFLICT DO NOTHING",
                params![chunk.chunk_id().as_str(), chunk.byte_length()],
            )
            .unwrap()
                == 1
            {
                charge += chunk.byte_length();
                let before = (bits[n / 4] as char).to_digit(16).unwrap() as u8;
                bits[n / 4] = b"0123456789abcdef"[(before | (1 << (n % 4))) as usize];
            }
            c.execute(
                "INSERT INTO manifest_chunks VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",
                params![m.id().as_str(), n, chunk.chunk_id().as_str()],
            )
            .unwrap();
        }
        let record = json!({"schemaVersion":"layrs.shared-exact-installation.v1","binding":b,
            "manifestId":m.id(),"fileId":m.file_id(),"bytes":m.byte_length(),
            "directory":format!("1-2-{ordinal}"),"directoryIdentity":"fs:v1:1:2",
            "phase":"published","spoolCursor":m.chunks().len(),"publishCursor":m.chunks().len(),
            "active":null,"chargedChunks":String::from_utf8(bits).unwrap(),
            "residentChunks":"0".repeat(256),"chargedManifest":added,"debitBytes":charge});
        c.execute(
            "INSERT INTO exact_installations VALUES(?1,?2,?3,?4,?5,?6,1)",
            params![
                b.reservation,
                m.id().as_str(),
                m.file_id().as_str(),
                m.byte_length(),
                canonical_json(&record).unwrap(),
                payload
            ],
        )
        .unwrap();
        debit += charge;
    }
    c.execute(
        "UPDATE reservations SET remaining=remaining-?1 WHERE id=?2",
        params![debit, b.reservation],
    )
    .unwrap();
    let p = ExactProtection::new(
        &b.family_incarnation,
        &b.owner,
        &b.database_identity,
        &b.transaction_id,
        manifests,
        None,
    )
    .unwrap();
    c.execute(
        "INSERT INTO roots VALUES(?1,?2,?3,?4,?1,?5)",
        params![
            p.id(),
            b.owner.as_str(),
            b.database_identity,
            p.operation(),
            p.payload()
        ],
    )
    .unwrap();
    for m in manifests {
        c.execute(
            "INSERT INTO root_manifests VALUES(?1,?2)",
            params![p.id(), m.id().as_str()],
        )
        .unwrap();
    }
}

pub(super) fn fixture(
    body: impl FnOnce(&Connection, &[u8], &AdmittedFamilyContext, &[FileManifest]),
) {
    with_reference("", |c| {
        super::super::schema_v2::initialize_test_reference(c)?;
        c.execute_batch("BEGIN IMMEDIATE")?;
        let family = family();
        c.execute("INSERT INTO family VALUES(1,?1)", [&family])?;
        c.execute("INSERT INTO quota VALUES(1,100000000)", [])?;
        let manifests = manifests(b"abcabc");
        let context = context(&family, 1, &manifests);
        install(c, &context, &manifests);
        body(c, &family, &context, &manifests);
        c.execute_batch("ROLLBACK")?;
        Ok(())
    })
    .unwrap();
}

fn unchanged(c: &Connection, f: &[u8], context: &AdmittedFamilyContext, succeeds: bool) {
    let before = c.total_changes();
    assert_eq!(context.validate_catalog(c, f, None).is_ok(), succeeds);
    assert_eq!(before, c.total_changes(), "reader never writes or repairs");
}

owned!(
    admitted_import_coexists_with_ordinary_roots_and_reservations,
    {
        fixture(|c, f, context, manifests| {
            let owned = context
                .validate_catalog(c, f, None)
                .unwrap()
                .into_manifests();
            assert_eq!(
                owned.iter().map(|m| m.id()).collect::<BTreeSet<_>>(),
                manifests.iter().map(|m| m.id()).collect::<BTreeSet<_>>()
            );
            assert_eq!(owned[0].file_id(), owned[1].file_id());
            assert_ne!(owned[0].id(), owned[1].id());
            c.execute(
                "INSERT INTO roots VALUES('ordinary',?1,?2,'capture','ordinary',?3)",
                params![
                    context.binding().owner.as_str(),
                    context.binding().database_identity,
                    b"ordinary".as_slice()
                ],
            )
            .unwrap();
            c.execute(
                "INSERT INTO reservations VALUES('ordinary',?1,'capture',1024)",
                [context.binding().owner.as_str()],
            )
            .unwrap();
            c.execute(
                "INSERT INTO installations VALUES('ordinary',?1,?2,NULL,0)",
                params![manifests[0].file_id().as_str(), b"ordinary".as_slice()],
            )
            .unwrap();
            unchanged(c, f, context, true);
            let b = context.binding();
            let p = ExactProtection::new(
                &b.family_incarnation,
                &b.owner,
                &b.database_identity,
                &b.transaction_id,
                manifests,
                None,
            )
            .unwrap();
            assert!(
                p.inspect(c, None).is_err(),
                "legacy reader preserves owner-root uniqueness"
            );
            assert!(
                super::super::exact_content::validate_protected_installations(
                    c, b, manifests, &p, None
                )
                .is_err()
            );
        });
    }
);

owned!(
    two_published_import_groups_coexist_and_foreign_corruption_refuses,
    {
        fixture(|c, f, first, first_manifests| {
            let manifests = manifests(b"xyzxyz");
            let second = context(f, 2, &manifests);
            install(c, &second, &manifests);
            unchanged(c, f, first, true);
            unchanged(c, f, &second, true);
            // A further import reuses indexed representations/chunks without
            // charging either physical object a second time.
            let third = context(f, 3, first_manifests);
            install(c, &third, first_manifests);
            unchanged(c, f, first, true);
            unchanged(c, f, &third, true);
            let b = first.binding();
            let p = ExactProtection::new(
                &b.family_incarnation,
                &b.owner,
                &b.database_identity,
                &b.transaction_id,
                first_manifests,
                None,
            )
            .unwrap();
            let state = b
                .validate_retained_installations(
                    c,
                    first_manifests,
                    &p,
                    &mut BTreeSet::new(),
                    &mut BTreeSet::new(),
                    None,
                )
                .unwrap();
            let expected = super::super::usage(c).unwrap();
            assert_eq!(
                state.content_bytes, expected.content_bytes,
                "all retained journals contribute to family usage"
            );
            assert_eq!(state.reserved_bytes, expected.reserved_bytes);
            c.execute(
                "UPDATE exact_installations SET published=0 WHERE reservation=?1",
                [&second.binding().reservation],
            )
            .unwrap();
            unchanged(c, f, first, false);
        });
    }
);

owned!(
    retained_budget_root_and_journal_mapping_are_exact_without_repairs,
    {
        fixture(|c, f, context, _| {
            for sql in [
            "UPDATE reservations SET remaining=remaining+1",
            "DELETE FROM root_manifests",
            "UPDATE exact_installations SET bytes=bytes+1",
            "UPDATE roots SET database_identity='replacement'",
            "UPDATE worktrees SET state='initializing'",
            "INSERT INTO reservations SELECT 'provider-transition:unknown',owner,'provider-transition:unknown',0 FROM reservations LIMIT 1",
            "UPDATE quota SET bytes=1",
        ] {
            c.execute_batch("SAVEPOINT corrupt").unwrap();
            c.execute_batch(sql).unwrap();
            unchanged(c,f,context,false);
            c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt").unwrap();
            unchanged(c,f,context,true);
        }
            let changes = c.total_changes();
            let cancelled = Cancellation::default();
            cancelled.cancel();
            assert!(context.validate_catalog(c, f, Some(&cancelled)).is_err());
            assert_eq!(changes, c.total_changes());
        });
    }
);

owned!(empty_foreign_import_requires_prevalidated_context, {
    fixture(|c, f, first, _| {
        let second = context(f, 2, &[]);
        install(c, &second, &[]);
        unchanged(c, f, first, false);
        let mut supplied = first.clone();
        supplied.include_retained(&second).unwrap();
        unchanged(c, f, &supplied, true);
        unchanged(c, f, &second, true);
    });
});

owned!(
    ordinary_staging_refuses_retained_directory_name_or_identity,
    {
        fixture(|c, f, context, _| {
            use super::super::admitted_access::require_unretained_directory;
            context.validate_catalog(c, f, None).unwrap();
            let payload: Vec<u8> = c
                .query_row("SELECT payload FROM exact_installations LIMIT 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            let record: ExactRecord = serde_json::from_slice(&payload).unwrap();
            let identity = record.directory_identity.as_deref().unwrap();
            let before = c.total_changes();
            require_unretained_directory(c, "ordinary-new-directory", "ordinary-new-inode")
                .unwrap();
            assert!(
                require_unretained_directory(c, &record.directory, "ordinary-new-inode").is_err()
            );
            assert!(require_unretained_directory(c, "ordinary-new-directory", identity).is_err());
            assert!(require_unretained_directory(c, &record.directory, identity).is_err());
            assert_eq!(before, c.total_changes());
            context.validate_catalog(c, f, None).unwrap();
        });
    }
);
