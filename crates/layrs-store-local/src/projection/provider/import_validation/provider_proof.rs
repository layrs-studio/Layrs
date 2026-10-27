//! Private borrow-only dispatch. No publication proof is converted to a
//! capture permission, and no outgoing receipt becomes incoming ancestry.
use super::*;
use crate::projection::{
    provider::{
        records::{
            activation::{ProviderActivationRecord, ProviderActiveMountRecord},
            ProviderBootstrapSource, Record,
        },
        validation::PrivateCaptureProof,
    },
    VersionedProjection,
};

type Parts<'a> = (
    &'a ProviderActiveMountRecord,
    &'a ProviderActivationRecord,
    &'a ProviderBootstrapSource,
    &'a VersionedProjection,
    &'a PrivateCaptureProof,
);

#[derive(Clone, Copy)]
pub(super) enum ProviderProof<'a> {
    Capture(&'a PrivateActiveCaptureValidation),
    Publication(&'a PrivateActivePublicationValidation),
}

#[derive(PartialEq, Eq)]
pub(super) enum ProviderModeBinding {
    Capture,
    Publication(String),
}

impl ProviderProof<'_> {
    pub(super) fn parts(&self) -> Parts<'_> {
        match self {
            Self::Capture(proof) => proof.import_parts(),
            Self::Publication(proof) => proof.import_parts(),
        }
    }

    pub(super) fn mode_binding(
        self,
        cancel: Option<&Cancellation>,
    ) -> CauseResult<ProviderModeBinding> {
        let Self::Publication(proof) = self else {
            return Ok(ProviderModeBinding::Capture);
        };
        if proof.transitions.len() > 4096 || proof.receipts.len() > 4096 {
            return Err(invalid("import publication proof row count exceeds bound"));
        }
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-provider-import-publication-proof-v1\0");
        hash_rows(&proof.transitions, &mut hash, cancel)?;
        hash_rows(&proof.receipts, &mut hash, cancel)?;
        Ok(ProviderModeBinding::Publication(format!(
            "blake3:{}",
            hash.finalize()
        )))
    }

    /// Re-read under the same retained transaction, including the bounded
    /// terminal relation fences BEFORE any import/global FK traversal. A
    /// complete proof has no connection pointer of its own, so borrowing an
    /// older or different connection's proof cannot bypass those fences.
    pub(super) fn require_current_publication(
        self,
        c: &Connection,
        references: &ImportSourceReferences<'_>,
        cancel: Option<&Cancellation>,
    ) -> CauseResult<()> {
        if c.is_autocommit() {
            return Err(invalid(
                "import publication requires a retained SQL snapshot",
            ));
        }
        let Self::Publication(proof) = self else {
            return Err(invalid("import publication proof kind differs"));
        };
        let ImportSourceReferences::Provider {
            allocation,
            prepared,
            activation,
        } = references
        else {
            return Err(invalid("import publication source references differ"));
        };
        let current = super::super::validation::private_active_publication_with_source(
            c,
            allocation,
            prepared,
            &activation.binding,
            None,
            cancel,
        )?;
        let (_, old_activation, old_source, old_activated, old_captures) = proof.import_parts();
        let (_, current_activation, current_source, current_activated, current_captures) =
            current.import_parts();
        if self.mode_binding(cancel)? != ProviderProof::Publication(&current).mode_binding(cancel)?
            || old_activation.digest()? != current_activation.digest()?
            || source_commitment_digest(old_source)? != source_commitment_digest(current_source)?
            || old_activated.generation != current_activated.generation
            || old_activated.digest != current_activated.digest
            || old_captures.patches.len() != current_captures.patches.len()
            || old_captures.patches.last().map(|p| p.digest.as_str())
                != current_captures.patches.last().map(|p| p.digest.as_str())
        {
            return Err(invalid("import publication proof differs from current SQL"));
        }
        Ok(())
    }
}

fn hash_rows<T: Record>(
    rows: &[T],
    hash: &mut blake3::Hasher,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    hash.update(&(rows.len() as u64).to_be_bytes());
    let mut total = 0u64;
    for row in rows {
        super::super::cancellation::check(cancel)?;
        // At most one individually bounded Record payload is materialized.
        let payload = row.encode()?;
        total = total
            .checked_add(payload.len() as u64)
            .filter(|n| *n <= 16 * 1024 * 1024)
            .ok_or_else(|| invalid("import publication proof payload exceeds bound"))?;
        hash.update(&(payload.len() as u64).to_be_bytes());
        hash.update(&payload);
    }
    Ok(())
}
