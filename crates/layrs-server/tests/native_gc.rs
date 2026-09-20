#![cfg(target_os = "linux")]
#[path = "support/gc_fixture.rs"]
mod gc_fixture;
use gc_fixture::*;
use layrs_server::{
    backup::{self, LocalPgBackupTools},
    retention::gc::*,
};
use layrs_store_server::v3::{CasLimits, FsServerCas};
use serde_json::Value;
use std::{fs, sync::Arc};

struct StopAfter(usize);
impl GcFaults for StopAfter {
    fn after_mutation(&self, index: usize) -> GcResult<()> {
        if index == self.0 {
            Err("injected process-stop boundary".into())
        } else {
            Ok(())
        }
    }
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL 17 and pg_dump/pg_restore; scripts/test-linux-gc.mjs"]
async fn offline_gc_preserves_all_roots_and_recovers_quarantine_restore_purge() {
    let fixture = Fixture::new().await;
    let before = fixture.authorities().await;
    let backup = fixture.base.join("backup");
    let manifest = backup::create_backup(
        &fixture.pool,
        &fixture.config,
        &backup,
        Arc::new(LocalPgBackupTools),
    )
    .await
    .unwrap();
    assert!(!manifest.objects.is_empty());
    let old = fixture.old_orphans();
    let old_bytes: Vec<_> = old.iter().map(|p| fs::read(p).unwrap()).collect();
    let live = FsServerCas::open(&fixture.config.cas_root, CasLimits::default())
        .await
        .unwrap();
    assert!(
        create_plan(&fixture.pool, &fixture.config, &backup, &fixture.now)
            .await
            .is_err()
    );
    drop(live);
    let planned = create_plan(&fixture.pool, &fixture.config, &backup, &fixture.now)
        .await
        .unwrap();
    assert_eq!(planned["candidateFiles"], 3);
    let id = planned["planId"].as_str().unwrap();
    assert!(old.iter().all(|p| p.exists()), "planning never moves data");
    let apply = || {
        execute(
            &fixture.pool,
            &fixture.config,
            id,
            &backup,
            "quarantine",
            &fixture.now,
            &NoFaults,
        )
    };
    // Incorrect digest cannot select arbitrary paths or another plan.
    let plan_path = fixture
        .config
        .cas_root
        .join(format!(".layrs-retention/{id}/plan.json"));
    let canonical = fs::read(&plan_path).unwrap();
    fs::write(&plan_path, b"{}").unwrap();
    assert!(apply().await.is_err());
    fs::write(&plan_path, &canonical).unwrap();
    assert!(old.iter().all(|p| p.exists()));
    // A link substituted after the plan never escapes into a neighbour file.
    let moved = fixture.base.join("preserved-orphan");
    fs::rename(&old[0], &moved).unwrap();
    let sentinel = fixture.base.join("outside");
    fs::write(&sentinel, b"never touch").unwrap();
    std::os::unix::fs::symlink(&sentinel, &old[0]).unwrap();
    assert!(apply().await.is_err());
    assert_eq!(fs::read(&sentinel).unwrap(), b"never touch");
    fs::remove_file(&old[0]).unwrap();
    fs::rename(&moved, &old[0]).unwrap();
    assert!(
        execute(
            &fixture.pool,
            &fixture.config,
            "../outside",
            &backup,
            "quarantine",
            &fixture.now,
            &NoFaults
        )
        .await
        .is_err()
    );
    for checkpoint in [usize::MAX, 0, 1, 2, usize::MAX - 1] {
        assert!(
            execute(
                &fixture.pool,
                &fixture.config,
                id,
                &backup,
                "quarantine",
                &fixture.now,
                &StopAfter(checkpoint)
            )
            .await
            .is_err()
        );
    }
    assert!(fixture.config.cas_root.join(ACTIVE).exists());
    assert!(
        FsServerCas::open(&fixture.config.cas_root, CasLimits::default())
            .await
            .is_err()
    );
    assert_eq!(apply().await.unwrap()["affected"], 3);
    assert!(!fixture.config.cas_root.join(ACTIVE).exists());
    assert_eq!(apply().await.unwrap()["replayed"], true);
    assert!(old.iter().all(|p| !p.exists()));
    fixture.assert_retained().await;
    assert_eq!(fixture.authorities().await, before);
    // No immediate irreversible purge; retention is checked with OS time in CLI.
    assert!(
        execute(
            &fixture.pool,
            &fixture.config,
            id,
            &backup,
            "purge",
            &fixture.now,
            &NoFaults
        )
        .await
        .is_err()
    );
    for checkpoint in [usize::MAX, 0, 1, 2, usize::MAX - 1] {
        assert!(
            execute(
                &fixture.pool,
                &fixture.config,
                id,
                &backup,
                "restore",
                &fixture.now,
                &StopAfter(checkpoint)
            )
            .await
            .is_err()
        );
    }
    assert!(
        FsServerCas::open(&fixture.config.cas_root, CasLimits::default())
            .await
            .is_err()
    );
    execute(
        &fixture.pool,
        &fixture.config,
        id,
        &backup,
        "restore",
        &fixture.now,
        &NoFaults,
    )
    .await
    .unwrap();
    for (path, bytes) in old.iter().zip(&old_bytes) {
        assert_eq!(&fs::read(path).unwrap(), bytes)
    }
    assert!(apply().await.is_err(), "a restored plan is terminal");
    // Plan again with a distinct trusted clock tick; same bytes remain candidates.
    let later = Clock(fixture.now.0 + 1);
    let planned = create_plan(&fixture.pool, &fixture.config, &backup, &later)
        .await
        .unwrap();
    let second = planned["planId"].as_str().unwrap();
    execute(
        &fixture.pool,
        &fixture.config,
        second,
        &backup,
        "quarantine",
        &later,
        &NoFaults,
    )
    .await
    .unwrap();
    let purge_time = Clock(later.0 + MIN_RETENTION_SECONDS + 1);
    let plan: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .config
                .cas_root
                .join(format!(".layrs-retention/{second}/plan.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let acquired = plan["candidates"][2]["objectId"].as_str().unwrap();
    sqlx::query("INSERT INTO object_leases(lease_id,space_id,object_id,lease_purpose,owner_client_id,fencing_generation,expires_at,heartbeat_at) SELECT 'lease:gc-new-authority',space_id,$1,'upload','client:gc',1,clock_timestamp()-interval '1 day',clock_timestamp()-interval '2 days' FROM spaces ORDER BY space_id LIMIT 1").bind(acquired).execute(&fixture.pool).await.unwrap();
    let after_new_reservation = fixture.authorities().await;
    let purge_backup = fixture.base.join("backup-purge");
    backup::create_backup(
        &fixture.pool,
        &fixture.config,
        &purge_backup,
        Arc::new(LocalPgBackupTools),
    )
    .await
    .unwrap();
    for checkpoint in [usize::MAX, 0, 1, 2, usize::MAX - 1] {
        assert!(
            execute(
                &fixture.pool,
                &fixture.config,
                second,
                &purge_backup,
                "purge",
                &purge_time,
                &StopAfter(checkpoint)
            )
            .await
            .is_err()
        );
        if checkpoint == 0 {
            // A resumed purge must not report success if protected bytes have
            // disappeared from both their original and quarantine locations.
            let held = fixture.base.join("held-protected-candidate");
            let stored = fixture
                .config
                .cas_root
                .join(format!(".layrs-retention/{second}/quarantine/000002"));
            fs::rename(&stored, &held).unwrap();
            let error = execute(
                &fixture.pool,
                &fixture.config,
                second,
                &purge_backup,
                "purge",
                &purge_time,
                &NoFaults,
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("protected"));
            assert!(fixture.config.cas_root.join(ACTIVE).exists());
            fs::rename(&held, &stored).unwrap();
        }
    }
    assert!(
        FsServerCas::open(&fixture.config.cas_root, CasLimits::default())
            .await
            .is_err()
    );
    let purged = execute(
        &fixture.pool,
        &fixture.config,
        second,
        &purge_backup,
        "purge",
        &purge_time,
        &NoFaults,
    )
    .await
    .unwrap();
    assert_eq!(purged["affected"], 2);
    assert_eq!(purged["retained"], 1);
    assert_eq!(
        execute(
            &fixture.pool,
            &fixture.config,
            second,
            &purge_backup,
            "purge",
            &purge_time,
            &NoFaults
        )
        .await
        .unwrap()["replayed"],
        true
    );
    assert_eq!(
        old.iter().filter(|p| p.exists()).count(),
        1,
        "newly protected object restored to active CAS"
    );
    let protected = fixture
        .config
        .cas_root
        .join(plan["candidates"][2]["path"].as_str().unwrap());
    assert!(protected.is_file());
    let quarantine = fixture
        .config
        .cas_root
        .join(format!(".layrs-retention/{second}/quarantine"));
    assert_eq!(
        fs::read_dir(quarantine).unwrap().count(),
        0,
        "newly reserved bytes returned to active CAS"
    );
    fixture.assert_retained().await;
    assert_eq!(fixture.authorities().await, after_new_reservation);
    fixture
        .restore_and_compare(&purge_backup, &after_new_reservation)
        .await;
    // A real CLI dry-run, still using the read/write maintenance boundary.
    let cli = fixture.cli(&["gc-plan", backup.to_str().unwrap()]);
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let value: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(value["candidateFiles"], 0);
    println!(
        "GC real PostgreSQL: three candidates quarantined/restored, two purged, one restored to CAS after new reservation; interruption recovery and all catalogue/Marker/Release/View/reservations/staging bytes verified; real backup restored"
    );
    fixture.cleanup().await;
}
