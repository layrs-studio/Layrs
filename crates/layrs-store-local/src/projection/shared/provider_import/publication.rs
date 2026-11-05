//! Family member publication SQL DATA only. The driver authenticates the
//! destination private Ready snapshot, receipt and physical immutable proofs.
//! Reservations, protection and both sets of installation journals are retained.
use super::content::{ImportContentInputs, ImportProtectionState};
use super::*;
use crate::projection::{
    provider::records::import::ProviderTransitionReceiptRecord,
    shared::{
        exact_content::ExactBinding,
        exact_protection::{ExactProtection, ProtectionPresence},
    },
};

/// source_staging inside content is the immutable original anchor. The driver
/// separately interprets the actual source phase in its retained transaction.
pub(in crate::projection) struct ImportPublicationInputs<'a> {
    pub content: ImportContentInputs<'a>,
    pub destination_ready: &'a ProviderTransitionIntentRecord,
    pub receipt: &'a ProviderTransitionReceiptRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ImportPublicationMember {
    Initializing,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) struct ImportPublicationState {
    pub member: ImportPublicationMember,
    pub protection: ImportProtectionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ImportPublicationEffect {
    Updated,
    AlreadyReady,
}

fn require_ready(
    inputs: &ImportPublicationInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    cancellation::check(cancel)?;
    let ready = inputs.destination_ready;
    if ready.plan.companion_id.is_some() {
        let context = inputs
            .content
            .source_context
            .as_ref()
            .ok_or_else(|| invalid("companion C Ready requires full B source context"))?;
        let root = context
            .pending
            .ok_or_else(|| invalid("companion C Ready requires B fixed retention"))?;
        root.require_plan(
            inputs.content.allocation,
            inputs.content.source_staging,
            cancel,
        )?;
    }
    ready.require_transition(inputs.content.destination_staging)?;
    if ready.role != TransitionRole::Destination || ready.state != TransitionState::Ready {
        return Err(invalid(
            "family publication requires exact destination Ready",
        ));
    }
    ready.require_receipt(inputs.receipt)?;
    cancellation::check(cancel)
}

fn member_phase(
    c: &Connection,
    owner: &str,
    cancel: Option<&Cancellation>,
) -> Result<ImportMemberPhase> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid("family publication requires caller transaction"));
    }
    if crate::projection::shared::provider_access::require_provider_catalog_schema(c, cancel)?
        != crate::projection::shared::CatalogSchema::V2
    {
        return Err(invalid("family publication requires exact schema2"));
    }
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND length(CAST(state AS BLOB))>12)",
        [owner],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("family publication member phase exceeds bound"));
    }
    let phase: Option<String> = c
        .query_row("SELECT state FROM worktrees WHERE id=?1", [owner], |r| {
            r.get(0)
        })
        .optional()?;
    match phase.as_deref() {
        Some("initializing") => Ok(ImportMemberPhase::Initializing),
        Some("ready") => Ok(ImportMemberPhase::Ready),
        _ => Err(invalid(
            "family publication exact member absent or phase differs",
        )),
    }
}

fn compare_and_publish(
    c: &Connection,
    owner: &str,
    path: &str,
    database_identity: &str,
    marker: &[u8],
) -> Result<()> {
    if c.is_autocommit() || c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))? != 1 {
        return Err(invalid(
            "family publication writer requires FK ON caller transaction",
        ));
    }
    if c.execute(
        "UPDATE worktrees SET state='ready' WHERE id=?1 AND metadata_path=?2 AND database_identity=?3 AND marker=?4 AND state='initializing'",
        params![owner,path,database_identity,marker],
    )? != 1 {
        return Err(invalid("family publication exact member CAS differs"));
    }
    Ok(())
}

impl FamilyAccess {
    /// Complete publication interpretation, never ordinary admission. The
    /// Initializing and Ready member branches repeat the same exact source,
    /// union, reservation, debit, Published journals and protection checks.
    pub(in crate::projection) fn read_import_publication(
        &self,
        c: &Connection,
        inputs: &ImportPublicationInputs<'_>,
        binding: &ExactBinding,
        protection: &ExactProtection,
        private_metadata_floor: u64,
        cancel: Option<&Cancellation>,
    ) -> Result<ImportPublicationState> {
        require_ready(inputs, cancel)?;
        if inputs.destination_ready.plan.companion_id.is_some() {
            let root = inputs
                .content
                .source_context
                .as_ref()
                .and_then(|s| s.pending)
                .ok_or_else(|| invalid("C receipt B protection scope absent"))?;
            if root.inspect(c, cancel)?
                != crate::projection::shared::source_companion::SourceCompanionPresence::Protected
            {
                return Err(invalid(
                    "C receipt requires B protection before either receipt",
                ));
            }
        }
        let phase = member_phase(c, binding.owner.as_str(), cancel)?;
        let checked = self.validate_import_protection_at_member(
            c,
            &inputs.content,
            binding,
            protection,
            private_metadata_floor,
            phase,
            cancel,
        )?;
        if checked.root != ProtectionPresence::Present {
            return Err(invalid(
                "family publication requires exact durable protection",
            ));
        }
        let member = match phase {
            ImportMemberPhase::Initializing => ImportPublicationMember::Initializing,
            ImportMemberPhase::Ready => ImportPublicationMember::Ready,
        };
        self.verify()?;
        cancellation::check(cancel)?;
        Ok(ImportPublicationState {
            member,
            protection: checked,
        })
    }

    /// Only the worktrees state changes. Caller rolls back every failed
    /// post-write check, commits, closes positively and revalidates all private
    /// and physical proofs before acknowledging success. Ready replay is RO.
    pub(in crate::projection) fn publish_import_ready(
        &self,
        c: &Connection,
        inputs: &ImportPublicationInputs<'_>,
        binding: &ExactBinding,
        protection: &ExactProtection,
        private_metadata_floor: u64,
        cancel: Option<&Cancellation>,
    ) -> Result<ImportPublicationEffect> {
        let before = self.read_import_publication(
            c,
            inputs,
            binding,
            protection,
            private_metadata_floor,
            cancel,
        )?;
        if before.member == ImportPublicationMember::Ready {
            return Ok(ImportPublicationEffect::AlreadyReady);
        }
        cancellation::check(cancel)?;
        let destination = inputs
            .content
            .source_staging
            .destination
            .as_ref()
            .ok_or_else(|| invalid("family publication destination absent"))?;
        let marker = &destination.format;
        let shared = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("family publication destination binding absent"))?;
        compare_and_publish(
            c,
            marker.identity.worktree_id.as_str(),
            exact_path(&shared.metadata_path)?,
            &destination.private_database_identity,
            &marker.bytes()?,
        )?;
        let after = self.read_import_publication(
            c,
            inputs,
            binding,
            protection,
            private_metadata_floor,
            cancel,
        )?;
        if after.member != ImportPublicationMember::Ready || after.protection != before.protection {
            return Err(invalid(
                "family publication changed accounting or exact post-read differs",
            ));
        }
        cancellation::check(cancel)?;
        Ok(ImportPublicationEffect::Updated)
    }
}

#[cfg(test)]
#[path = "publication/tests.rs"]
mod tests;
