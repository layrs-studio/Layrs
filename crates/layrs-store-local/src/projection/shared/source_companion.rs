//! B owns a fixed metadata reservation and an exact retention root, never an
//! installation journal. C alone installs/debits U. All operations are SQL DATA
//! in caller-owned transactions; physical CAS and close scopes live above.
use crate::projection::{
    provider::{
        cancellation,
        records::{
            import::ImportSource,
            transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
            AllocationRecord, Record,
        },
        recovery::error::{invalid, Result},
    },
    shared::{exact_content::ExactBinding, exact_protection::ProtectionEffect},
};
use layrs_core::scm::{canonical_json, FileManifest, WorktreeId};
use layrs_vfs::Cancellation;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Write};

pub(in crate::projection) const NAMESPACE: &str = "provider-companion:";
const SCHEMA: &str = "layrs.source-companion-protection.v1";
const MAX_ROOT: u64 = 16 * 1024 * 1024;
const MAX_METADATA: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct SourceCompanionBinding {
    pub(in crate::projection) source_owner: WorktreeId,
    pub(in crate::projection) source_database: String,
    source_schema: u32,
    pub(in crate::projection) companion_id: String,
    pub(in crate::projection) destination: ExactBinding,
}
impl SourceCompanionBinding {
    pub(in crate::projection) fn new(
        a: &AllocationRecord,
        intent: &ProviderTransitionIntentRecord,
        destination: &ExactBinding,
    ) -> Result<Self> {
        intent.validate_fields()?;
        destination.validate()?;
        let ImportSource::Provider {
            identity,
            private_schema,
            ..
        } = &intent.plan.source_prefix.source
        else {
            return Err(invalid("source companion requires provider prefix"));
        };
        let private_database_identity = &a.private.database_file_identity;
        let c = intent
            .destination
            .as_ref()
            .ok_or_else(|| invalid("source companion C identity absent"))?;
        if intent.role != TransitionRole::Source
            || *private_schema != 8
            || identity != &a.format.identity
            || destination.owner != c.format.identity.worktree_id
            || destination.database_identity != c.private_database_identity
            || destination.owner == identity.worktree_id
            || destination.database_identity == *private_database_identity
            || destination.family_incarnation != a.family.incarnation
            || destination.family_root_identity != a.family.root_identity
            || destination.family_catalog_identity != a.family.catalog_file_identity
            || destination.family_marker_digest != a.family.marker_digest
            || destination.transaction_id != intent.transaction_id
            || destination.plan_digest != intent.plan.digest()?
            || destination.prefix_id != intent.plan.source_prefix_id
            || destination.destination_proof_digest != c.digest(&intent.plan)?
            || destination.quota_bytes != intent.plan.quota_bytes
            || destination.operation != intent.reservation_operation()?
        {
            return Err(invalid("source companion independent B/C binding differs"));
        }
        Ok(Self {
            source_owner: identity.worktree_id.clone(),
            source_database: private_database_identity.clone(),
            source_schema: 8,
            companion_id: intent
                .plan
                .companion_id
                .clone()
                .ok_or_else(|| invalid("source companion ID absent"))?,
            destination: destination.clone(),
        })
    }
    fn validate(&self) -> Result<()> {
        self.destination.validate()?;
        if self.source_schema != 8
            || self.source_owner == self.destination.owner
            || self.source_owner.as_str().is_empty()
            || self.source_owner.as_str().len() > 512
            || self.source_database.is_empty()
            || self.source_database.len() > 512
            || self.source_database.contains('\0')
            || self.source_database == self.destination.database_identity
            || !token(&self.companion_id)
        {
            return Err(invalid("source companion binding shape differs"));
        }
        Ok(())
    }
    pub(in crate::projection) fn operation(&self) -> Result<String> {
        self.validate()?;
        let bytes = canonical_json(self).map_err(|e| invalid(e.to_string()))?;
        Ok(format!(
            "{NAMESPACE}{}",
            hash(b"layrs-source-companion-operation-v1\0", &bytes)
        ))
    }
}
fn token(s: &str) -> bool {
    s.len() == 71
        && s.starts_with("blake3:")
        && s[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn hash(domain: &[u8], payload: &[u8]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(domain);
    h.update(payload);
    h.finalize().to_hex().to_string()
}
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| invalid("source companion size overflow"))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stored {
    schema: String,
    binding: SourceCompanionBinding,
    private_bytes: u64,
    reservation_bytes: u64,
    manifests: Vec<FileManifest>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RootFrame<'a> {
    schema: &'static str,
    binding: &'a SourceCompanionBinding,
    private_bytes: u64,
    manifests: Vec<&'a FileManifest>,
}
struct Count(u64);
impl Write for Count {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(b.len() as u64)
            .filter(|n| *n <= MAX_ROOT)
            .ok_or_else(|| std::io::Error::other("source root size exceeds maximum"))?;
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::projection) struct SourceCompanionProtection {
    pub(in crate::projection) binding: SourceCompanionBinding,
    id: String,
    payload: Vec<u8>,
    operation: String,
    manifests: Vec<String>,
    private_bytes: u64,
    reservation_bytes: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum SourceCompanionPresence {
    Absent,
    Reserved,
    Protected,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum SourceCompanionRecovery {
    Absent,
    ReservationOnly,
    RootOnly,
    DestinationReadySourceStaging,
    SourceReady,
    Published,
}
impl SourceCompanionProtection {
    /// DATA constructor. `private_bytes` is supplied only by the closed source
    /// SQL derivation; writers rederive it and compare the exact fixed amount.
    /// Count the encoding BEFORE allocating the root. The only omitted numeric
    /// field is charged at 19 decimal digits by a SIZING frame, never by a fake
    /// valid protocol record or a guessed quota value.
    pub(in crate::projection) fn new(
        binding: SourceCompanionBinding,
        required: &[FileManifest],
        private_bytes: u64,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        binding.validate()?;
        cancellation::check(cancel)?;
        if required.len() > 100000 || private_bytes > MAX_METADATA {
            return Err(invalid("source companion envelope exceeds bound"));
        }
        let mut sorted = BTreeMap::new();
        let mut lengths = BTreeMap::new();
        for m in required {
            cancellation::check(cancel)?;
            if m.chunks().len() > 1024
                || m.byte_length() > 1024 * 1024 * 1024
                || m.chunks()
                    .iter()
                    .any(|c| c.byte_length() == 0 || c.byte_length() > 1048576)
                || sorted.insert(m.id().as_str(), m).is_some()
                || lengths
                    .insert(m.file_id(), m.byte_length())
                    .is_some_and(|old| old != m.byte_length())
            {
                return Err(invalid("source companion U repeats or exceeds bound"));
            }
            super::exact_content::canonical_manifest_bytes(m)?;
        }
        // Canonical order is ManifestId; the plan's separate commitment is
        // FileId+ManifestId and is authenticated by the closed source caller.
        let ordered: Vec<&FileManifest> = sorted.into_values().collect();
        let frame = RootFrame {
            schema: SCHEMA,
            binding: &binding,
            private_bytes,
            manifests: ordered.clone(),
        };
        let mut count = Count(0);
        serde_json::to_writer(&mut count, &frame).map_err(|e| invalid(e.to_string()))?;
        // RootFrame's braces are retained; Stored adds exactly this one key,
        // comma and integer. camelCase spelling is part of funding algorithm 1.
        let root_bound = add(count.0, ",\"reservationBytes\":".len() as u64 + 19)?;
        if root_bound > MAX_ROOT {
            return Err(invalid("source companion root encoding exceeds 16MiB"));
        }
        let operation = binding.operation()?;
        // Normalized SQL values: INTEGER=8; TEXT/BLOB=UTF8/encoded length.
        // Root six columns, index pairs, fixed reservation four columns.
        let root_scalars = 64
            + binding.source_owner.as_str().len() as u64
            + binding.source_database.len() as u64
            + operation.len() as u64
            + 64;
        let index = (ordered.len() as u64)
            .checked_mul(64 + 71)
            .ok_or_else(|| invalid("source root index overflow"))?;
        let reservation_scalars =
            operation.len() as u64 * 2 + binding.source_owner.as_str().len() as u64 + 8;
        let reservation_bytes = add(
            add(add(add(private_bytes, root_bound)?, root_scalars)?, index)?,
            reservation_scalars,
        )?;
        let value = Stored {
            schema: SCHEMA.into(),
            binding: binding.clone(),
            private_bytes,
            reservation_bytes,
            manifests: ordered.into_iter().cloned().collect(),
        };
        let payload = canonical_json(&value).map_err(|e| invalid(e.to_string()))?;
        if payload.len() as u64 > root_bound {
            return Err(invalid("source actual root exceeds sizing envelope"));
        }
        let id = hash(b"layrs-source-companion-protection-v1\0", &payload);
        Ok(Self {
            binding,
            id,
            payload,
            operation,
            manifests: value.manifests.iter().map(|m| m.id().to_string()).collect(),
            private_bytes,
            reservation_bytes,
        })
    }
    pub(in crate::projection) fn require_union(
        &self,
        required: &[FileManifest],
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        cancellation::check(cancel)?;
        let stored: Stored =
            serde_json::from_slice(&self.payload).map_err(|e| invalid(e.to_string()))?;
        let mut expected = required.iter().collect::<Vec<_>>();
        expected.sort_by(|a, b| a.id().cmp(b.id()));
        if stored.manifests.len() != expected.len()
            || !stored.manifests.iter().zip(expected).all(|(a, b)| a == b)
        {
            return Err(invalid(
                "source retention U differs from actual protected C union",
            ));
        }
        Ok(())
    }
    pub(in crate::projection) fn require_plan(
        &self,
        a: &AllocationRecord,
        intent: &ProviderTransitionIntentRecord,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        if Self::binding_for(a, intent, &self.binding.destination)? != self.binding {
            return Err(invalid("source root belongs to another immutable plan"));
        }
        let stored: Stored =
            serde_json::from_slice(&self.payload).map_err(|e| invalid(e.to_string()))?;
        let expected = &intent.plan.protected_manifests;
        let mut ordered = BTreeMap::new();
        for m in &stored.manifests {
            ordered.insert((m.file_id(), m.id()), m);
        }
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-provider-managed-protected-manifests-v1\0");
        hash.update(&(ordered.len() as u64).to_be_bytes());
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Row<'a> {
            file_id: &'a layrs_core::scm::FileId,
            manifest_id: &'a layrs_core::scm::FileManifestId,
            bytes: u64,
        }
        for m in ordered.into_values() {
            cancellation::check(cancel)?;
            let b = canonical_json(&Row {
                file_id: m.file_id(),
                manifest_id: m.id(),
                bytes: m.byte_length(),
            })
            .map_err(|e| invalid(e.to_string()))?;
            hash.update(&(b.len() as u64).to_be_bytes());
            hash.update(&b);
        }
        if expected.count != stored.manifests.len() as u64
            || expected.digest != format!("blake3:{}", hash.finalize())
        {
            return Err(invalid("source root does not retain entire planned U"));
        }
        Ok(())
    }
    fn binding_for(
        a: &AllocationRecord,
        intent: &ProviderTransitionIntentRecord,
        c: &ExactBinding,
    ) -> Result<SourceCompanionBinding> {
        SourceCompanionBinding::new(a, intent, c)
    }
    pub(in crate::projection) fn id(&self) -> &str {
        &self.id
    }
    pub(in crate::projection) fn operation(&self) -> &str {
        &self.operation
    }
    pub(in crate::projection) fn reservation_bytes(&self) -> u64 {
        self.reservation_bytes
    }
    pub(in crate::projection) fn require_envelope(&self, actual: u64) -> Result<()> {
        if actual > self.private_bytes {
            return Err(invalid("source actual SQL exceeds fixed B envelope"));
        }
        Ok(())
    }
    fn transaction(c: &Connection, write: bool) -> Result<()> {
        if c.is_autocommit()
            || (write && c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))? != 1)
        {
            return Err(invalid(
                "source companion requires caller transaction and writer FK ON",
            ));
        }
        Ok(())
    }
    pub(in crate::projection) fn inspect(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<SourceCompanionPresence> {
        Self::transaction(c, false)?;
        cancellation::check(cancel)?;
        // ID and operation collisions are global, never hidden by owner filters.
        let count: u64 = c.query_row(
            "SELECT count(*) FROM reservations WHERE id=?1 OR operation=?1",
            [&self.operation],
            |r| r.get(0),
        )?;
        let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1 AND owner=?2 AND operation=?1 AND remaining=?3)",params![self.operation,self.binding.source_owner.as_str(),self.reservation_bytes],|r|r.get(0))?;
        if count > 1 || (count == 1 && !valid) {
            return Err(invalid(
                "source companion fixed reservation changed or foreign",
            ));
        }
        if c.query_row("SELECT EXISTS(SELECT 1 FROM exact_installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",[self.binding.source_owner.as_str()],|r|r.get::<_,bool>(0))? {
            return Err(invalid("source companion owner cannot own exact installation journals"));
        }
        let roots: u64 = c.query_row(
            "SELECT count(*) FROM roots WHERE id=?1 OR operation=?2",
            params![self.id, self.operation],
            |r| r.get(0),
        )?;
        if roots == 0 {
            if c.query_row(
                "SELECT EXISTS(SELECT 1 FROM root_manifests WHERE root_id=?1)",
                [&self.id],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(invalid(
                    "source companion orphan membership is not an absent root",
                ));
            }
            return Ok(if count == 0 {
                SourceCompanionPresence::Absent
            } else {
                SourceCompanionPresence::Reserved
            });
        }
        if roots != 1 || !valid {
            return Err(invalid("source companion partial root/reservation"));
        }
        let exact:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM roots WHERE id=?1 AND owner=?2 AND database_identity=?3 AND operation=?4 AND digest=?1 AND payload=?5)",params![self.id,self.binding.source_owner.as_str(),self.binding.source_database,self.operation,self.payload],|r|r.get(0))?;
        if !exact {
            return Err(invalid("source companion root differs"));
        }
        let mut q=c.prepare("SELECT manifest_id FROM root_manifests WHERE root_id=?1 ORDER BY manifest_id LIMIT 100001")?;
        let mut rows = q.query([&self.id])?;
        let mut n = 0;
        while let Some(row) = rows.next()? {
            cancellation::check(cancel)?;
            let id = row
                .get_ref(0)?
                .as_str()
                .map_err(|e| invalid(e.to_string()))?;
            if id.len() != 71 || self.manifests.get(n).map(String::as_str) != Some(id) {
                return Err(invalid("source root members differ"));
            }
            n += 1;
        }
        if n != self.manifests.len() {
            return Err(invalid("source root members incomplete"));
        }
        Ok(SourceCompanionPresence::Protected)
    }
    /// Before the FIRST B or C reservation, include BOTH absent amounts and all
    /// global durable journal charges. This does not grow quota or borrow any
    /// capture reservation. Replay requires C's independent exact validator.
    pub(in crate::projection) fn require_capacity(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        let presence = self.inspect(c, cancel)?;
        let c_reserved: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1)",
            [&self.binding.destination.reservation],
            |r| r.get(0),
        )?;
        if c_reserved && !c.query_row("SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1 AND owner=?2 AND operation=?1 AND remaining<=?3)",params![self.binding.destination.reservation,self.binding.destination.owner.as_str(),self.binding.destination.quota_bytes],|r|r.get::<_,bool>(0))? {
            return Err(invalid("source companion C reservation is foreign or increased"));
        }
        let usage = super::usage(c).map_err(|e| invalid(e.to_string()))?;
        let mut needed = add(usage.content_bytes, usage.reserved_bytes)?;
        if presence == SourceCompanionPresence::Absent {
            needed = add(needed, self.reservation_bytes)?;
        }
        if !c_reserved {
            needed = add(needed, self.binding.destination.quota_bytes)?;
        }
        if needed > usage.limit_bytes {
            return Err(invalid(
                "partial_store_quota: fixed B plus C envelopes insufficient",
            ));
        }
        Ok(())
    }
    pub(in crate::projection) fn reserve(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<ProtectionEffect> {
        Self::transaction(c, true)?;
        self.require_capacity(c, cancel)?;
        if self.inspect(c, cancel)? != SourceCompanionPresence::Absent {
            return Ok(ProtectionEffect::AlreadyPresent);
        }
        c.execute(
            "INSERT INTO reservations(id,owner,operation,remaining) VALUES(?1,?2,?1,?3)",
            params![
                self.operation,
                self.binding.source_owner.as_str(),
                self.reservation_bytes
            ],
        )?;
        if self.inspect(c, cancel)? != SourceCompanionPresence::Reserved {
            return Err(invalid("source reservation prospective readback differs"));
        }
        Ok(ProtectionEffect::Inserted)
    }
    /// Called only under the retained adapter's actual Published C scope and
    /// physical CAS witness. No B exact_installations are inserted here.
    pub(in crate::projection) fn protect(
        &self,
        c: &Connection,
        cancel: Option<&Cancellation>,
    ) -> Result<ProtectionEffect> {
        Self::transaction(c, true)?;
        match self.inspect(c, cancel)? {
            SourceCompanionPresence::Protected => return Ok(ProtectionEffect::AlreadyPresent),
            SourceCompanionPresence::Reserved => {}
            SourceCompanionPresence::Absent => {
                return Err(invalid("source root requires fixed reservation first"))
            }
        }
        self.require_capacity(c, cancel)?;
        c.execute("INSERT INTO roots(id,owner,database_identity,operation,digest,payload) VALUES(?1,?2,?3,?4,?1,?5)",params![self.id,self.binding.source_owner.as_str(),self.binding.source_database,self.operation,self.payload])?;
        for id in &self.manifests {
            cancellation::check(cancel)?;
            c.execute(
                "INSERT INTO root_manifests(root_id,manifest_id) VALUES(?1,?2)",
                params![self.id, id],
            )?;
        }
        if self.inspect(c, cancel)? != SourceCompanionPresence::Protected {
            return Err(invalid("source protection prospective readback differs"));
        }
        Ok(ProtectionEffect::Inserted)
    }
    /// Self-contained retained DATA interpretation. No B opener or content is
    /// consulted, and this root never contributes authority to C admission.
    pub(in crate::projection) fn read_retained(
        c: &Connection,
        id: &str,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        Self::transaction(c, false)?;
        let (len,owner_len,db_len,op_len):(u64,u64,u64,u64)=c.query_row("SELECT length(payload),length(CAST(owner AS BLOB)),length(CAST(database_identity AS BLOB)),length(CAST(operation AS BLOB)) FROM roots WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        if len > MAX_ROOT || owner_len > 512 || db_len > 512 || op_len > 128 {
            return Err(invalid("retained source root exceeds bound"));
        }
        let payload: Vec<u8> =
            c.query_row("SELECT payload FROM roots WHERE id=?1", [id], |r| r.get(0))?;
        // serde collection sizes are bounded by the 16MiB payload; constructor
        // also caps manifest/chunk cardinality before its canonical allocation.
        let stored: Stored =
            serde_json::from_slice(&payload).map_err(|e| invalid(e.to_string()))?;
        if stored.schema != SCHEMA {
            return Err(invalid("source root schema differs"));
        }
        if !c.query_row("SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND database_identity=?2 AND state='ready')",params![stored.binding.source_owner.as_str(),stored.binding.source_database],|r|r.get::<_,bool>(0))? {
            return Err(invalid("retained source group B registration identity differs"));
        }
        let expected = Self::new(
            stored.binding,
            &stored.manifests,
            stored.private_bytes,
            cancel,
        )?;
        if expected.id != id
            || expected.payload != payload
            || expected.reservation_bytes != stored.reservation_bytes
            || expected.inspect(c, cancel)? != SourceCompanionPresence::Protected
        {
            return Err(invalid("retained source root canonical funding differs"));
        }
        Ok(expected)
    }
    pub(in crate::projection) fn recovery(
        &self,
        c: &Connection,
        source: TransitionState,
        c_ready: bool,
        cancel: Option<&Cancellation>,
    ) -> Result<SourceCompanionRecovery> {
        let state = self.inspect(c, cancel)?;
        Ok(match (state, source, c_ready) {
            (SourceCompanionPresence::Absent, TransitionState::Staging, false) => {
                SourceCompanionRecovery::Absent
            }
            (SourceCompanionPresence::Reserved, TransitionState::Staging, false) => {
                SourceCompanionRecovery::ReservationOnly
            }
            (SourceCompanionPresence::Protected, TransitionState::Staging, false) => {
                SourceCompanionRecovery::RootOnly
            }
            (SourceCompanionPresence::Protected, TransitionState::Staging, true) => {
                SourceCompanionRecovery::DestinationReadySourceStaging
            }
            (SourceCompanionPresence::Protected, TransitionState::Ready, true) => {
                SourceCompanionRecovery::SourceReady
            }
            (SourceCompanionPresence::Protected, TransitionState::Published, true) => {
                SourceCompanionRecovery::Published
            }
            _ => {
                return Err(invalid(
                    "source companion recovery order or protection differs",
                ))
            }
        })
    }
}

#[derive(Clone, Copy)]
pub(in crate::projection) struct SourceCompanionFamilyContext<'a> {
    pub(in crate::projection) publication:
        &'a crate::projection::provider::validation::PrivateActivePublicationValidation,
    pub(in crate::projection) pending: Option<&'a SourceCompanionProtection>,
}
pub(in crate::projection) struct ClassifiedSourceGroups {
    pub(super) reservations: std::collections::BTreeSet<String>,
    pub(super) roots: std::collections::BTreeSet<String>,
}
pub(in crate::projection) fn classify_source_groups(
    c: &Connection,
    a: &AllocationRecord,
    context: Option<&SourceCompanionFamilyContext<'_>>,
    cancel: Option<&Cancellation>,
) -> Result<ClassifiedSourceGroups> {
    let mut result = ClassifiedSourceGroups {
        reservations: Default::default(),
        roots: Default::default(),
    };
    let Some(context) = context else {
        return Ok(result);
    };
    for group in context.publication.companions.groups() {
        cancellation::check(cancel)?;
        let intent = context
            .publication
            .source_transitions()
            .iter()
            .find(|i| i.transaction_id == group.transaction)
            .ok_or_else(|| invalid("source family companion intent absent"))?;
        let root = SourceCompanionProtection::read_retained(c, &group.root, cancel)?;
        root.require_plan(a, intent, cancel)?;
        if root.private_bytes != group.metadata_bytes
            || root.binding.companion_id != group.companion_id
            || root.binding.destination.plan_digest != group.plan_digest
            || root.binding.destination.prefix_id != group.prefix_id
        {
            return Err(invalid("source family private companion binding differs"));
        }
        // Exact sticky and alternate representations are ALL members of U.
        let stored: Stored =
            serde_json::from_slice(&root.payload).map_err(|e| invalid(e.to_string()))?;
        for (file, (manifest, bytes)) in &group.content {
            if !stored.manifests.iter().any(|m| {
                m.file_id().as_str() == file
                    && m.id().as_str() == manifest
                    && m.byte_length() == *bytes
            }) {
                return Err(invalid(
                    "source companion private mapping outside retained U",
                ));
            }
        }
        let header =
            crate::projection::provider::records::companion_v2::CompanionHeaderRecord::decode(
                &group.header,
            )?;
        if let crate::projection::provider::records::companion_v2::CompanionHeaderRecord::V2(h) =
            header
        {
            for r in h.preserved_representations {
                if !stored.manifests.iter().any(|m| {
                    m.file_id() == &r.file_id
                        && m.id() == &r.manifest_id
                        && m.byte_length() == r.bytes
                }) {
                    return Err(invalid(
                        "source companion exact alternate outside retained U",
                    ));
                }
            }
        }
        if !result.reservations.insert(root.operation.clone())
            || !result.roots.insert(root.id.clone())
        {
            return Err(invalid("source family companion group repeats"));
        }
    }
    if let Some(root) = context.pending {
        let intent = context
            .publication
            .source_transitions()
            .iter()
            .find(|i| i.transaction_id == root.binding.destination.transaction_id)
            .ok_or_else(|| invalid("pending source protection has no actual Staging intent"))?;
        root.require_plan(a, intent, cancel)?;
        if intent.state == TransitionState::Staging {
            if context
                .publication
                .companions
                .groups()
                .iter()
                .any(|g| g.transaction == intent.transaction_id)
            {
                return Err(invalid("source Staging has premature companion SQL"));
            }
            let state = root.inspect(c, cancel)?;
            if state != SourceCompanionPresence::Absent {
                result.reservations.insert(root.operation.clone());
            }
            if state == SourceCompanionPresence::Protected {
                result.roots.insert(root.id.clone());
            }
        } else if !result.roots.contains(root.id()) {
            return Err(invalid(
                "pending source group is neither Staging nor exact terminal",
            ));
        }
    }
    Ok(result)
}

/// Autonomous family scan recognizes the new namespace explicitly, without
/// opening B or treating its retention as C's own admission proof. A reserve
/// without a root blocks this admission; it is resumable only by the B owner.
pub(in crate::projection) fn retained_namespace(
    c: &Connection,
    cancel: Option<&Cancellation>,
    mut validate: impl FnMut(&SourceCompanionProtection) -> Result<()>,
) -> Result<()> {
    // Only small operation IDs survive each iteration. The complete source
    // payload/union and the caller's C cross-check are released before the
    // next source root is materialized. A failed callback stops immediately.
    let mut operations = std::collections::BTreeSet::new();
    let mut q = c.prepare(
        "SELECT id FROM roots WHERE operation LIKE 'provider-companion%' ORDER BY id LIMIT 4097",
    )?;
    let mut rows = q.query([])?;
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        let id = row
            .get_ref(0)?
            .as_str()
            .map_err(|e| invalid(e.to_string()))?;
        if operations.len() >= 4096 || id.len() != 64 {
            return Err(invalid("source namespace root count/key exceeds bound"));
        }
        let root = SourceCompanionProtection::read_retained(c, id, cancel)?;
        if root.operation().len() > 128 || !operations.insert(root.operation().to_owned()) {
            return Err(invalid(
                "source namespace root operation repeats or exceeds bound",
            ));
        }
        validate(&root)?;
    }
    drop(rows);
    drop(q);
    let mut q=c.prepare("SELECT id FROM reservations WHERE operation LIKE 'provider-companion%' OR id LIKE 'provider-companion%' ORDER BY id LIMIT 4097")?;
    let mut rows = q.query([])?;
    let mut seen = std::collections::BTreeSet::new();
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        let id = row
            .get_ref(0)?
            .as_str()
            .map_err(|e| invalid(e.to_string()))?;
        if id.len() > 128
            || seen.len() >= 4096
            || !operations.contains(id)
            || !seen.insert(id.to_owned())
        {
            return Err(invalid(
                "unowned or reservation-only source companion family group",
            ));
        }
    }
    if seen != operations {
        return Err(invalid("source namespace root reservation absent"));
    }
    cancellation::check(cancel)?;
    Ok(())
}

#[cfg(test)]
#[path = "source_companion/tests.rs"]
mod tests;
