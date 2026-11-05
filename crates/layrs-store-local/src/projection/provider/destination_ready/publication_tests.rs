//! Reuse the complete Ready fixture without changing any older recipe. This
//! tests SQL DATA after the family flag changes, not physical publication.
use super::*;

#[test]
#[ignore = "owned audited SQLite child; dedicated family-Ready checkout reader retains all strict suffix checks"]
fn published_member_ready_reader_is_separate_strict_and_readonly() {
    child::run(
        concat!(
            module_path!(),
            "::published_member_ready_reader_is_separate_strict_and_readonly"
        ),
        120,
        || {
            child::declare();
            with_fixture(|s, c, family, f, closure, expected| {
                complete(c, expected);
                let data = f.data();
                let inputs = f.inputs(closure, &data);
                finalize_checkout_or_replay(s, c, family, &inputs, expected, None).unwrap();
                let journal = witness(c);
                read_checkout_ready(s, c, family, &inputs, expected, None).unwrap();
                let changes = (c.total_changes(), family.total_changes());
                refuses(
                    read_checkout_family_ready(s, c, family, &inputs, expected, None),
                    "destination Ready family member differs",
                );
                assert_eq!((c.total_changes(), family.total_changes()), changes);
                assert_eq!(family.execute("UPDATE worktrees SET state='ready' WHERE id=?1 AND state='initializing'",[f.marker.identity.worktree_id.as_str()]).unwrap(),1);
                read_checkout_family_ready(s, c, family, &inputs, expected, None).unwrap();
                refuses(
                    read_checkout_ready(s, c, family, &inputs, expected, None),
                    "destination Ready family member differs",
                );
                refuses(
                    finalize_checkout_or_replay(s, c, family, &inputs, expected, None),
                    "destination Ready family member differs",
                );
                assert_eq!(witness(c), journal);
                let forged_claim = [ManagedReadyClaim {
                    root_id: f.claims[0].root_id.clone(),
                    digest: f.claims[0].digest.clone(),
                    kind: "generation".into(),
                    owner: f.claims[0].owner.clone(),
                }];
                let mut wrong = f.data();
                wrong.claims = &forged_claim;
                let changes = (c.total_changes(), family.total_changes());
                refuses(
                    read_checkout_family_ready(
                        s,
                        c,
                        family,
                        &f.inputs(closure, &wrong),
                        expected,
                        None,
                    ),
                    "destination Ready durable import claim differs",
                );
                assert_eq!((c.total_changes(), family.total_changes()), changes);
                c.execute_batch(
                    "COMMIT; PRAGMA foreign_keys=OFF; PRAGMA query_only=ON; BEGIN DEFERRED",
                )
                .unwrap();
                family
                    .execute_batch(
                        "COMMIT; PRAGMA foreign_keys=OFF; PRAGMA query_only=ON; BEGIN DEFERRED",
                    )
                    .unwrap();
                let changes = (c.total_changes(), family.total_changes());
                read_checkout_family_ready(s, c, family, &inputs, expected, None).unwrap();
                assert_eq!((c.total_changes(), family.total_changes()), changes);
                assert_eq!(witness(c), journal);
                let cancel = Cancellation::default();
                cancel.cancel();
                assert!(
                    read_checkout_family_ready(s, c, family, &inputs, expected, Some(&cancel))
                        .is_err()
                );
                assert_eq!((c.total_changes(), family.total_changes()), changes);
            });
        },
    );
}
