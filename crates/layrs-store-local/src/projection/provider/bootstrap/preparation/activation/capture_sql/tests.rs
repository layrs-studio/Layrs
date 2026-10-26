use super::*;
use crate::projection::{
    capture::CapturedPatch,
    provider::{
        bootstrap::tests::capture_family::active_holder_with_private,
        process_profile::tests::child,
        records::{
            capture::{ProviderCaptureStage, ProviderCaptureTransactionRecord},
            Record,
        },
    },
    shared::{
        provider_access::{provider_capture::operation, FamilyAccess},
        provider_content_lease,
    },
};
use layrs_core::scm::{canonical_json, FileId};
use layrs_vfs::{Cancellation, Catalog, SnapshotSource};
use rusqlite::ffi;
use std::sync::Arc;

struct NoRead(Catalog);
impl SnapshotSource for NoRead {
    fn catalog(&self) -> &Catalog {
        &self.0
    }
    fn read_exact_at(
        &self,
        id: &FileId,
        offset: u64,
        output: &mut [u8],
        _: &Cancellation,
    ) -> layrs_vfs::Result<()> {
        Err(layrs_vfs::Error::MissingBytes {
            file_id: id.clone(),
            offset,
            length: output.len(),
        })
    }
}

struct ObserveClose;
impl WriterCloseObserver for ObserveClose {
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, _: bool) -> Option<Cause> {
        None
    }
}

fn patch_digest(patch: &mut CapturedPatch) {
    patch.digest.clear();
    let bytes = canonical_json(patch).unwrap();
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-projection-capture-v1\0");
    hash.update(&bytes);
    patch.digest = format!("blake3:{}", hash.finalize());
}

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this exact owned native capture SQL recipe"]
fn owned_empty_image_capture_sql_cycle() {
    child::run(
        concat!(module_path!(), "::owned_empty_image_capture_sql_cycle"),
        240,
        || {
            child::declare();
            let (f, holder, allocation, _, _) = active_holder_with_private();
            let active = observe(
                &holder.held,
                &holder.root,
                &holder.record,
                &holder.activation,
            )
            .unwrap();
            let source: Arc<dyn SnapshotSource> = Arc::new(NoRead(holder.root.catalog().clone()));
            let session = layrs_vfs::Session::reconnect_disconnected_with_control_directory(
                holder.root.try_clone().unwrap(),
                source,
            )
            .unwrap();
            let mut checkpoint = session.checkpoint().unwrap();
            let image = super::super::capture_image::plan(
                &mut checkpoint,
                &active.active.source,
                &active.active.activated.snapshot,
                &Cancellation::default(),
            )
            .unwrap();
            assert!(
                image.changes.is_empty(),
                "fresh active image should be unchanged"
            );
            let tx = ProviderCaptureTransactionRecord::begin(
                &allocation,
                &holder.record,
                &holder.activation,
                "owned-empty-capture".into(),
                0,
                None,
            )
            .unwrap();
            begin(
                &holder.held,
                &holder.root,
                &holder.record,
                &holder.activation,
                &tx,
                &mut ObserveClose,
            )
            .unwrap();
            let observed = observe(
                &holder.held,
                &holder.root,
                &holder.record,
                &holder.activation,
            )
            .unwrap();
            assert_eq!(observed.capture.transactions, vec![tx.clone()]);
            let stage = ProviderCaptureStage {
                image_digest: image.image_digest,
                image_path_count: image.image_path_count,
                changes: image.changes,
                reservation: "owned-empty-capture-reservation".into(),
            };
            let mut staged = tx.clone();
            staged.state = ProviderCaptureState::Staging {
                stage: stage.clone(),
            };
            transition(
                &holder.held,
                &holder.root,
                &holder.record,
                &holder.activation,
                &tx,
                &staged,
                &checkpoint,
                &mut ObserveClose,
            )
            .unwrap();
            let observed = observe(
                &holder.held,
                &holder.root,
                &holder.record,
                &holder.activation,
            )
            .unwrap();
            let family = FamilyAccess::open(f.family.root()).unwrap();
            let lease = provider_content_lease(
                family,
                crate::projection::provider::records::AllocationRecord::decode(
                    &allocation.encode().unwrap(),
                )
                .unwrap(),
                observed.capture,
            )
            .unwrap();
            let op = operation(&staged.transaction_id);
            lease
                .reserve_exact(
                    &stage.reservation,
                    &f.worktree,
                    &op,
                    crate::projection::retention::budget_for(&[], 0).unwrap(),
                )
                .unwrap();
            drop(lease);
            publish_content(
                &holder.held,
                &holder.root,
                &holder.record,
                &holder.activation,
                &staged,
                &[],
                &checkpoint,
                &mut ObserveClose,
            )
            .unwrap();
            let observed = observe(
                &holder.held,
                &holder.root,
                &holder.record,
                &holder.activation,
            )
            .unwrap();
            let family = FamilyAccess::open(f.family.root()).unwrap();
            let lease = provider_content_lease(
                family,
                crate::projection::provider::records::AllocationRecord::decode(
                    &allocation.encode().unwrap(),
                )
                .unwrap(),
                observed.capture,
            )
            .unwrap();
            let (root_id, digest) = lease.protect(&allocation.format, &op, &[]).unwrap();
            assert_eq!(root_id, digest);
            drop(lease);
            let mut patch = CapturedPatch {
                schema_version: "layrs.projection-capture.v1".into(),
                sequence: 1,
                previous_patch_digest: None,
                projection_generation: holder.activation.activated_generation,
                projection_digest: holder.activation.activated_digest.clone(),
                identity: allocation.format.identity.clone(),
                changes: stage.changes.clone(),
                digest: String::new(),
            };
            patch_digest(&mut patch);
            let mut accepted = staged.clone();
            accepted.state = ProviderCaptureState::Accepted {
                stage,
                patch: patch.clone(),
            };
            let receipt = ProviderCaptureReceiptRecord::derive(&accepted).unwrap();
            accept(
                &holder.held,
                &holder.root,
                &holder.record,
                &holder.activation,
                &staged,
                &accepted,
                &receipt,
                &root_id,
                &checkpoint,
                &mut ObserveClose,
            )
            .unwrap();
            let final_state = observe(
                &holder.held,
                &holder.root,
                &holder.record,
                &holder.activation,
            )
            .unwrap();
            assert_eq!(final_state.capture.transactions, vec![accepted]);
            assert_eq!(final_state.capture.receipts, vec![receipt]);
            assert_eq!(final_state.capture.patches, vec![patch]);
            assert!(final_state.capture.content.is_empty());
            assert_eq!(final_state.capture.claims.len(), 1);
        },
    );
}
