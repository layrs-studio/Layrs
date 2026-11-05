//! Declared SQL DATA recipes. No source owner/private admission, physical CAS
//! or Ready claim is authenticated here. Runtime release belongs to ROOT.
use super::{installing_tests::state, tests as retained, *};
use crate::projection::provider::owned_test as child;

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "ROOT source release and audited terminal Ready SQL child120/outer180 required"]
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

fn inspect(
    c: &Connection,
    family: &[u8],
    context: &AdmittedFamilyContext,
    manifests: &[FileManifest],
    succeeds: bool,
) -> Option<ExactInstallState> {
    let binding = context.binding();
    let protection = ExactProtection::new(
        &binding.family_incarnation,
        &binding.owner,
        &binding.database_identity,
        &binding.transaction_id,
        manifests,
        None,
    )
    .unwrap();
    let before = state(c);
    let changes = c.total_changes();
    // Only marker/binding/union/root DATA crosses this production interface;
    // the declared test context is not an argument to the terminal reader.
    let actual = validate_terminal_ready_catalog(
        c,
        family,
        context.marker(),
        binding,
        manifests,
        &protection,
        None,
    );
    assert_eq!(actual.is_ok(), succeeds, "{actual:?}");
    assert_eq!(changes, c.total_changes(), "terminal reader never repairs");
    assert_eq!(before, state(c));
    actual.ok()
}

owned!(
    terminal_ready_accepts_complete_family_without_source_context,
    {
        retained::fixture(|c, family, terminal, manifests| {
            // Another group shares physical chunks/representations without a
            // second physical debit. No foreign private context reaches the API.
            let foreign = retained::context(family, 2, manifests);
            retained::install(c, &foreign, manifests);
            c.execute(
                "INSERT INTO reservations VALUES('ordinary',?1,'capture',1024)",
                [foreign.binding().owner.as_str()],
            )
            .unwrap();
            let actual = inspect(c, family, terminal, manifests, true).unwrap();
            assert_eq!(actual.records, manifests.len() as u64);
            assert_eq!(actual.published, actual.records);
            let expected = super::super::usage(c).unwrap();
            assert_eq!(actual.content_bytes, expected.content_bytes);
            assert_eq!(actual.reserved_bytes, expected.reserved_bytes);
            assert_eq!(actual.limit_bytes, expected.limit_bytes);
            // The old single-target Provider protection gate remains strict.
            let binding = terminal.binding();
            let protection = ExactProtection::new(
                &binding.family_incarnation,
                &binding.owner,
                &binding.database_identity,
                &binding.transaction_id,
                manifests,
                None,
            )
            .unwrap();
            assert!(
                super::super::exact_content::validate_protected_installations(
                    c,
                    binding,
                    manifests,
                    &protection,
                    None
                )
                .is_err()
            );
        });
    }
);

owned!(
    terminal_ready_rejects_foreign_corruption_and_unclassified_groups,
    {
        retained::fixture(|c, family, terminal, manifests| {
            let foreign = retained::context(family, 2, manifests);
            retained::install(c, &foreign, manifests);
            inspect(c, family, terminal, manifests, true);
            for sql in [
            "UPDATE exact_installations SET published=0 WHERE reservation=(SELECT id FROM reservations WHERE owner='worktree:member2')",
            "UPDATE reservations SET remaining=remaining+1 WHERE owner='worktree:member2'",
            "UPDATE worktrees SET state='initializing' WHERE id='worktree:member2'",
            "DELETE FROM root_manifests WHERE root_id IN (SELECT id FROM roots WHERE owner='worktree:member2')",
            "UPDATE roots SET digest='wrong' WHERE owner='worktree:member2'",
            "DELETE FROM roots WHERE owner='worktree:member2'",
            "INSERT INTO reservations SELECT 'provider-transition:unknown',owner,'provider-transition:unknown',0 FROM reservations LIMIT 1",
            "UPDATE quota SET bytes=1",
        ] {
            c.execute_batch("SAVEPOINT corrupt").unwrap();
            c.execute_batch(sql).unwrap();
            inspect(c, family, terminal, manifests, false);
            c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt").unwrap();
            inspect(c, family, terminal, manifests, true);
        }
        });
    }
);

owned!(terminal_ready_exact_member_root_and_union_are_mandatory, {
    retained::fixture(|c, family, terminal, manifests| {
        inspect(c, family, terminal, manifests, true);
        inspect(c, family, terminal, &manifests[..1], false);
        for sql in [
            "UPDATE worktrees SET state='initializing'",
            "UPDATE worktrees SET marker=CAST(CAST(marker AS TEXT)||' ' AS BLOB)",
            "UPDATE worktrees SET metadata_path='C:\\replacement'",
            "DELETE FROM root_manifests",
            "UPDATE roots SET operation='capture'",
            "INSERT INTO reservations SELECT 'ordinary',owner,'capture',0 FROM reservations LIMIT 1",
            "INSERT INTO roots SELECT 'ordinary',owner,database_identity,'capture','ordinary',payload FROM roots LIMIT 1",
        ] {
            c.execute_batch("SAVEPOINT corrupt").unwrap();
            c.execute_batch(sql).unwrap();
            inspect(c, family, terminal, manifests, false);
            c.execute_batch("ROLLBACK TO corrupt; RELEASE corrupt").unwrap();
            inspect(c, family, terminal, manifests, true);
        }
        let binding = terminal.binding();
        let protection = ExactProtection::new(
            &binding.family_incarnation,
            &binding.owner,
            &binding.database_identity,
            &binding.transaction_id,
            manifests,
            None,
        )
        .unwrap();
        let cancelled = Cancellation::default();
        cancelled.cancel();
        let before = state(c);
        let changes = c.total_changes();
        assert!(validate_terminal_ready_catalog(
            c,
            family,
            terminal.marker(),
            binding,
            manifests,
            &protection,
            Some(&cancelled)
        )
        .is_err());
        assert_eq!(changes, c.total_changes());
        assert_eq!(before, state(c));
    });
});

owned!(terminal_ready_known_empty_union_and_unknown_empty_policy, {
    retained::fixture(|c, family, first, manifests| {
        let empty = retained::context(family, 2, &[]);
        retained::install(c, &empty, &[]);
        let actual = inspect(c, family, &empty, &[], true).unwrap();
        assert_eq!(actual.records, 0);
        assert_eq!(actual.published, 0);
        assert_eq!(actual.debit_bytes, 0);
        assert_eq!(actual.remaining, empty.binding().quota_bytes);
        // The same empty group is unknown from another terminal member and
        // cannot identify quota/binding through an absent Published journal.
        inspect(c, family, first, manifests, false);
    });
});
