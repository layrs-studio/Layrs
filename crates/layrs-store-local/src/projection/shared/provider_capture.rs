//! Exact target-owned family state for a fully validated private capture proof.
//! Call only inside the caller's read-only catalog transaction and held gates.
use super::super::{content::Protection, *};
use super::FamilyAccess;
use crate::{
    projection::provider::{
        records::{capture::ProviderCaptureState, AllocationRecord},
        recovery::error::{invalid, Result},
        validation::PrivateCaptureProof,
    },
    secure_fs::PinnedDirectory,
};
use layrs_core::scm::{canonical_json, FileManifest};
use layrs_vfs::Cancellation;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ffi::OsStr, io::Read};

const MAX_ROOTS: usize = 4096;
const MAX_INSTALLATIONS: usize = 16384;
const MAX_MANIFEST: u64 = 262144;
const MAX_CHUNKS: usize = 1024;

pub(in crate::projection) fn operation(transaction_id: &str) -> String {
    format!(
        "provider-capture:{}",
        blake3::hash(transaction_id.as_bytes()).to_hex()
    )
}

fn cancel(cancel: Option<&Cancellation>) -> Result<()> {
    crate::projection::provider::cancellation::check(cancel)
}

fn converted(
    error: impl std::fmt::Display,
) -> crate::projection::provider::recovery::error::ProviderBootstrapCause {
    invalid(error.to_string())
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Intent {
    incarnation: String,
    reservation: String,
    owner: WorktreeId,
    database_identity: String,
    operation: String,
    file_id: layrs_core::scm::FileId,
    bytes: u64,
    directory: String,
    directory_identity: String,
}

struct Attempt<'a> {
    reservation: &'a str,
    operation: String,
    files: BTreeMap<&'a str, u64>,
    state: &'a ProviderCaptureState,
}

pub(super) fn validate(
    family: &FamilyAccess,
    c: &Connection,
    allocation: &AllocationRecord,
    proof: &PrivateCaptureProof,
    cancellation: Option<&Cancellation>,
) -> Result<()> {
    cancel(cancellation)?;
    family.verify()?;
    let owner = allocation.format.identity.worktree_id.as_str();
    let mut attempts = BTreeMap::<&str, Attempt<'_>>::new();
    let mut reservations = BTreeMap::<&str, &str>::new();
    let mut all_files = BTreeMap::<&str, u64>::new();
    for transaction in &proof.transactions {
        cancel(cancellation)?;
        let stage = match &transaction.state {
            ProviderCaptureState::Hashing | ProviderCaptureState::Cancelled { stage: None } => {
                continue;
            }
            ProviderCaptureState::Staging { stage }
            | ProviderCaptureState::Accepted { stage, .. }
            | ProviderCaptureState::Cancelled { stage: Some(stage) } => stage,
        };
        let op = operation(&transaction.transaction_id);
        if reservations
            .insert(&stage.reservation, transaction.transaction_id.as_str())
            .is_some()
        {
            return Err(invalid(
                "provider capture reservation reused by another attempt",
            ));
        }
        let mut files = BTreeMap::new();
        for change in &stage.changes {
            if let Some(file) = &change.replacement {
                let id = file.file_id.as_str();
                if files
                    .insert(id, file.bytes)
                    .is_some_and(|previous| previous != file.bytes)
                    || all_files
                        .insert(id, file.bytes)
                        .is_some_and(|previous| previous != file.bytes)
                {
                    return Err(invalid(
                        "provider capture FileId length differs between stages",
                    ));
                }
            }
        }
        attempts.insert(
            transaction.transaction_id.as_str(),
            Attempt {
                reservation: &stage.reservation,
                operation: op,
                files,
                state: &transaction.state,
            },
        );
    }
    let content = proof
        .content
        .iter()
        .map(|row| (row.file_id.as_str(), (row.manifest_id.as_str(), row.bytes)))
        .collect::<BTreeMap<_, _>>();
    if content.len() != proof.content.len() {
        return Err(invalid("provider capture private content repeats FileId"));
    }
    for row in &proof.content {
        if all_files.get(row.file_id.as_str()) != Some(&row.bytes) {
            return Err(invalid(
                "provider capture content lacks an exact staged file",
            ));
        }
        // The private row is durable only after family admission. This is a
        // metadata and pinned-path proof; the installer and each resident read
        // authenticate file bytes incrementally.
        catalog_manifest(
            family,
            c,
            &row.file_id,
            &row.manifest_id,
            row.bytes,
            cancellation,
        )?;
    }
    validate_reservations(c, owner, &attempts, &reservations, cancellation)?;
    validate_installations(
        family,
        c,
        allocation,
        &attempts,
        &reservations,
        cancellation,
    )?;
    validate_roots(
        family,
        c,
        allocation,
        proof,
        &attempts,
        &content,
        cancellation,
    )?;
    family.verify()?;
    cancel(cancellation)
}

fn validate_reservations(
    c: &Connection,
    owner: &str,
    attempts: &BTreeMap<&str, Attempt<'_>>,
    reservations: &BTreeMap<&str, &str>,
    cancellation: Option<&Cancellation>,
) -> Result<()> {
    // Reservation IDs are family-global. An expected ID owned by somebody
    // else is not the legitimate gap before this capture reserves its budget.
    for attempt in attempts.values() {
        cancel(cancellation)?;
        let foreign: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1 AND (owner!=?2 OR operation!=?3))",
            rusqlite::params![attempt.reservation, owner, attempt.operation],
            |row| row.get(0),
        )?;
        if foreign {
            return Err(invalid(
                "provider capture reservation belongs to another owner or operation",
            ));
        }
    }
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM reservations WHERE owner=?1 LIMIT 4097)",
        [owner],
        |r| r.get(0),
    )?;
    if count > MAX_ROOTS as u64 {
        return Err(invalid("provider capture reservation count exceeds bound"));
    }
    let oversized: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1 AND (length(CAST(id AS BLOB))>128 OR length(CAST(operation AS BLOB))>81))", [owner], |r| r.get(0))?;
    if oversized {
        return Err(invalid("provider capture reservation text exceeds bound"));
    }
    let mut q = c.prepare(
        "SELECT id,operation,remaining FROM reservations WHERE owner=?1 ORDER BY id LIMIT 4097",
    )?;
    let mut rows = q.query([owner])?;
    while let Some(row) = rows.next()? {
        cancel(cancellation)?;
        let id: String = row.get(0)?;
        let op: String = row.get(1)?;
        let remaining: u64 = row.get(2)?;
        let transaction = reservations
            .get(id.as_str())
            .ok_or_else(|| invalid("provider capture has unproven family reservation"))?;
        let attempt = attempts
            .get(transaction)
            .ok_or_else(|| invalid("provider capture reservation attempt absent"))?;
        if op != attempt.operation
            || remaining > crate::projection::capture::MAX_CAPTURE_BYTES + 8 * 1024 * 1024
        {
            return Err(invalid(
                "provider capture reservation operation or charge differs",
            ));
        }
    }
    // A private Staging intent commits before the family reservation; that
    // no-effect gap is a valid resumable predecessor.
    Ok(())
}

fn validate_installations(
    family: &FamilyAccess,
    c: &Connection,
    allocation: &AllocationRecord,
    attempts: &BTreeMap<&str, Attempt<'_>>,
    reservations: &BTreeMap<&str, &str>,
    cancellation: Option<&Cancellation>,
) -> Result<()> {
    let owner = allocation.format.identity.worktree_id.as_str();
    let count: u64 = c.query_row("SELECT count(*) FROM (SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1 LIMIT 16385)", [owner], |r| r.get(0))?;
    if count > MAX_INSTALLATIONS as u64 {
        return Err(invalid("provider capture installation count exceeds bound"));
    }
    let oversized: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1 AND (length(CAST(i.file_id AS BLOB))!=71 OR length(i.payload)>8192 OR length(i.manifest)>262144))", [owner], |r| r.get(0))?;
    if oversized {
        return Err(invalid("provider capture installation row exceeds bound"));
    }
    let mut q = c.prepare("SELECT i.reservation,i.file_id,i.payload,i.manifest,i.published FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1 ORDER BY i.reservation,i.file_id LIMIT 16385")?;
    let mut rows = q.query([owner])?;
    while let Some(row) = rows.next()? {
        cancel(cancellation)?;
        let reservation: String = row.get(0)?;
        let file_id: String = row.get(1)?;
        let payload: Vec<u8> = row.get(2)?;
        let manifest: Option<Vec<u8>> = row.get(3)?;
        let published: bool = row.get(4)?;
        let transaction = reservations
            .get(reservation.as_str())
            .ok_or_else(|| invalid("provider capture installation reservation unproven"))?;
        let attempt = attempts
            .get(transaction)
            .ok_or_else(|| invalid("provider capture installation attempt absent"))?;
        let bytes = attempt
            .files
            .get(file_id.as_str())
            .ok_or_else(|| invalid("provider capture installation FileId unproven"))?;
        let intent: Intent = serde_json::from_slice(&payload).map_err(converted)?;
        if canonical_json(&intent).map_err(converted)? != payload
            || intent.incarnation != allocation.family.incarnation
            || intent.reservation != reservation
            || intent.owner.as_str() != owner
            || intent.database_identity != allocation.private.database_file_identity
            || intent.operation != attempt.operation
            || intent.file_id.as_str() != file_id
            || intent.bytes != *bytes
        {
            return Err(invalid("provider capture installation intent differs"));
        }
        if intent.directory.is_empty()
            || intent.directory.len() > 128
            || !intent
                .directory
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'-')
        {
            return Err(invalid(
                "provider capture installation directory name differs",
            ));
        }
        if let Some(payload) = &manifest {
            let m: FileManifest = serde_json::from_slice(payload).map_err(converted)?;
            if canonical_json(&m).map_err(converted)?.as_slice() != payload.as_slice()
                || m.file_id().as_str() != file_id
                || m.byte_length() != *bytes
            {
                return Err(invalid("provider capture installation manifest differs"));
            }
            // Published installations may be between cleanup and row deletion;
            // the catalog manifest must already carry their exact bytes.
            if published {
                let actual =
                    catalog_manifest(family, c, &file_id, m.id().as_str(), *bytes, cancellation)?;
                if actual != m {
                    return Err(invalid(
                        "published capture installation representation differs",
                    ));
                }
            }
        } else if published {
            return Err(invalid(
                "provider capture published installation lacks manifest",
            ));
        }
        let staging = PinnedDirectory::open(&family.root().join("staging"))?;
        match staging.open_child(OsStr::new(&intent.directory)) {
            Ok(directory) => {
                if directory.identity_token() != intent.directory_identity {
                    return Err(invalid(
                        "provider capture installation directory identity differs",
                    ));
                }
                if !published {
                    if let Some(expected) = &manifest {
                        match directory.open_file(OsStr::new("manifest")) {
                            Ok(staged) => verify_manifest_pin(
                                &staged,
                                &directory,
                                OsStr::new("manifest"),
                                expected,
                                cancellation,
                            )?,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                // publish_installation moves the manifest before
                                // committing the catalog. A rollback leaves the
                                // exact durable intent/manifest row, staging
                                // directory and immutable object in this shape.
                                let objects = PinnedDirectory::open(family.root())?
                                    .open_child_for_traversal(OsStr::new("objects"))?;
                                let manifests =
                                    objects.open_child_for_traversal(OsStr::new("manifests"))?;
                                let decoded: FileManifest =
                                    serde_json::from_slice(expected).map_err(converted)?;
                                let name = OsStr::new(object_name(decoded.id().as_str())?);
                                let moved = manifests.open_file(name)?;
                                verify_manifest_pin(
                                    &moved,
                                    &manifests,
                                    name,
                                    expected,
                                    cancellation,
                                )?;
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }
                }
                directory.verify_path_identity()?;
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && (published || manifest.is_none()) =>
            {
                // Stage::finish removes an owned empty directory before its
                // exact intent row. An unverified intent admits no bytes;
                // a verified unpublished intent must retain its directory.
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn verify_manifest_pin(
    pin: &crate::secure_fs::PinnedFile,
    parent: &PinnedDirectory,
    name: &OsStr,
    expected: &[u8],
    cancellation: Option<&Cancellation>,
) -> Result<()> {
    if pin.file().metadata()?.len() != expected.len() as u64
        || read_bounded(pin.file(), MAX_MANIFEST as usize, cancellation)?.as_slice() != expected
    {
        return Err(invalid(
            "provider capture installation manifest file differs",
        ));
    }
    pin.verify_parent()?;
    if !pin.same_file(&parent.open_file(name)?) {
        return Err(invalid(
            "provider capture installation manifest path changed",
        ));
    }
    parent.verify_path_identity()?;
    cancel(cancellation)
}

fn validate_roots(
    family: &FamilyAccess,
    c: &Connection,
    allocation: &AllocationRecord,
    proof: &PrivateCaptureProof,
    attempts: &BTreeMap<&str, Attempt<'_>>,
    content: &BTreeMap<&str, (&str, u64)>,
    cancellation: Option<&Cancellation>,
) -> Result<()> {
    let owner = allocation.format.identity.worktree_id.as_str();
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM roots WHERE owner=?1 LIMIT 4097)",
        [owner],
        |r| r.get(0),
    )?;
    if count > MAX_ROOTS as u64 {
        return Err(invalid("provider capture root count exceeds bound"));
    }
    let (oversized, total): (bool, u64) = c.query_row("SELECT coalesce(max(length(CAST(id AS BLOB))!=64 OR length(CAST(digest AS BLOB))!=64 OR length(CAST(operation AS BLOB))!=81 OR length(CAST(database_identity AS BLOB))>512 OR length(payload)>16777216),0),coalesce(sum(length(payload)),0) FROM (SELECT id,digest,operation,database_identity,payload FROM roots WHERE owner=?1 LIMIT 4097)", [owner], |r| Ok((r.get(0)?,r.get(1)?)))?;
    if oversized || total > 128 * 1024 * 1024 {
        return Err(invalid("provider capture root row exceeds bound"));
    }
    let mut observed = BTreeMap::<&str, (String, String)>::new();
    let mut q = c.prepare("SELECT id,digest,operation,database_identity,payload FROM roots WHERE owner=?1 ORDER BY id LIMIT 4097")?;
    let mut rows = q.query([owner])?;
    while let Some(row) = rows.next()? {
        cancel(cancellation)?;
        let id: String = row.get(0)?;
        let digest: String = row.get(1)?;
        let op: String = row.get(2)?;
        let db: String = row.get(3)?;
        let payload: Vec<u8> = row.get(4)?;
        let matches = attempts
            .iter()
            .filter(|(_, attempt)| attempt.operation == op)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(invalid("provider capture root operation is unproven"));
        }
        let (transaction, attempt) = matches[0];
        if matches!(attempt.state, ProviderCaptureState::Staging { .. }) {
            let present: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1 AND owner=?2 AND operation=?3)",
                rusqlite::params![attempt.reservation, owner, attempt.operation],
                |r| r.get(0),
            )?;
            if !present {
                return Err(invalid("staging protection lacks its family reservation"));
            }
        }
        let mut manifests = Vec::new();
        for (file_id, bytes) in &attempt.files {
            let (manifest_id, content_bytes) = content.get(file_id).ok_or_else(|| {
                invalid("provider capture protection lacks private content binding")
            })?;
            if content_bytes != bytes {
                return Err(invalid(
                    "provider capture protection content length differs",
                ));
            }
            manifests.push(catalog_manifest(
                family,
                c,
                file_id,
                manifest_id,
                *bytes,
                cancellation,
            )?);
        }
        manifests.sort_by(|a, b| a.id().cmp(b.id()));
        manifests.dedup_by(|a, b| a.id() == b.id());
        let expected = Protection {
            incarnation: allocation.family.incarnation.clone(),
            owner: allocation.format.identity.worktree_id.clone(),
            database_identity: allocation.private.database_file_identity.clone(),
            operation: attempt.operation.clone(),
            manifests,
        };
        let expected_payload = canonical_json(&expected).map_err(converted)?;
        let expected_digest = super::super::digest(&expected).map_err(converted)?;
        if id != expected_digest
            || digest != expected_digest
            || db != expected.database_identity
            || payload != expected_payload
        {
            return Err(invalid(
                "provider capture root protection differs from staged files",
            ));
        }
        let count: u64 = c.query_row(
            "SELECT count(*) FROM (SELECT 1 FROM root_manifests WHERE root_id=?1 LIMIT 1025)",
            [&id],
            |row| row.get(0),
        )?;
        if count != expected.manifests.len() as u64 {
            return Err(invalid("provider capture root manifest count differs"));
        }
        let oversized: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM root_manifests WHERE root_id=?1 AND length(CAST(manifest_id AS BLOB))!=71)",
            [&id], |row| row.get(0),
        )?;
        if oversized {
            return Err(invalid("provider capture root manifest key exceeds bound"));
        }
        let mut index = c.prepare("SELECT manifest_id FROM root_manifests WHERE root_id=?1 ORDER BY manifest_id LIMIT 1025")?;
        let actual = index
            .query_map([&id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if actual
            != expected
                .manifests
                .iter()
                .map(|m| m.id().as_str().to_owned())
                .collect::<Vec<_>>()
        {
            return Err(invalid("provider capture root manifest membership differs"));
        }
        if observed.insert(transaction, (id, digest)).is_some() {
            return Err(invalid("provider capture attempt has multiple roots"));
        }
    }
    let claims = proof
        .claims
        .iter()
        .map(|claim| (claim.owner.as_str(), claim))
        .collect::<BTreeMap<_, _>>();
    if claims.len() != proof.claims.len() {
        return Err(invalid(
            "provider capture attempt has multiple private claims",
        ));
    }
    if claims
        .keys()
        .any(|transaction| !attempts.contains_key(transaction))
    {
        return Err(invalid(
            "provider capture private claim has no durable stage",
        ));
    }
    for (transaction, attempt) in attempts {
        let root = observed.get(transaction);
        let claim = claims.get(transaction);
        if matches!(attempt.state, ProviderCaptureState::Accepted { .. })
            && (root.is_none() || claim.is_none())
        {
            return Err(invalid(
                "accepted capture lacks family protection and private claim",
            ));
        }
        if claim.is_some() && root.is_none() {
            return Err(invalid(
                "provider capture private claim lacks family protection",
            ));
        }
        if let Some(claim) = claim {
            if claim.kind != "provider-capture"
                || root != Some(&(claim.root_id.clone(), claim.digest.clone()))
            {
                return Err(invalid(
                    "provider capture private claim differs from family protection",
                ));
            }
        }
    }
    Ok(())
}

fn catalog_manifest(
    family: &FamilyAccess,
    c: &Connection,
    file_id: &str,
    expected_id: &str,
    bytes: u64,
    cancellation: Option<&Cancellation>,
) -> Result<FileManifest> {
    cancel(cancellation)?;
    let bounds: Option<(u64, u64, u64, u64)> = c.query_row("SELECT length(CAST(id AS BLOB)),length(CAST(file_id AS BLOB)),length(payload),bytes FROM manifests WHERE id=?1 AND file_id=?2", [expected_id, file_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
    let Some((id_len, file_len, payload_len, declared)) = bounds else {
        return Err(invalid(
            "provider capture private content lacks family manifest",
        ));
    };
    if id_len != 71 || file_len != 71 || payload_len > MAX_MANIFEST || declared != bytes {
        return Err(invalid("provider capture family manifest bound differs"));
    }
    let (id, payload): (String, Vec<u8>) = c.query_row(
        "SELECT id,payload FROM manifests WHERE id=?1 AND file_id=?2",
        [expected_id, file_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let manifest: FileManifest = serde_json::from_slice(&payload).map_err(converted)?;
    if id != expected_id
        || id != manifest.id().as_str()
        || manifest.file_id().as_str() != file_id
        || manifest.byte_length() != bytes
        || canonical_json(&manifest).map_err(converted)? != payload
        || manifest.chunks().len() > MAX_CHUNKS
    {
        return Err(invalid("provider capture family manifest identity differs"));
    }
    let index_count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM manifest_chunks WHERE manifest_id=?1 LIMIT 1025)",
        [&id],
        |r| r.get(0),
    )?;
    if index_count != manifest.chunks().len() as u64 {
        return Err(invalid(
            "provider capture family manifest index count differs",
        ));
    }
    let oversized: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM manifest_chunks WHERE manifest_id=?1 AND length(CAST(chunk_id AS BLOB))!=71)",
        [&id], |row| row.get(0),
    )?;
    if oversized {
        return Err(invalid("provider capture chunk index key exceeds bound"));
    }
    let mut q = c.prepare("SELECT mc.ordinal,mc.chunk_id,ch.bytes FROM manifest_chunks mc LEFT JOIN chunks ch ON ch.id=mc.chunk_id WHERE mc.manifest_id=?1 ORDER BY mc.ordinal LIMIT 1025")?;
    let mut rows = q.query([&id])?;
    let objects =
        PinnedDirectory::open(family.root())?.open_child_for_traversal(OsStr::new("objects"))?;
    let manifests = objects.open_child_for_traversal(OsStr::new("manifests"))?;
    let chunks = objects.open_child_for_traversal(OsStr::new("chunks"))?;
    let name = object_name(&id)?;
    let pin = manifests.open_file(OsStr::new(name))?;
    if pin.file().metadata()?.len() != payload.len() as u64
        || read_bounded(pin.file(), MAX_MANIFEST as usize, cancellation)? != payload
    {
        return Err(invalid("provider capture family manifest file differs"));
    }
    pin.verify_parent()?;
    if !pin.same_file(&manifests.open_file(OsStr::new(name))?) {
        return Err(invalid("provider capture family manifest path changed"));
    }
    for (ordinal, part) in manifest.chunks().iter().enumerate() {
        cancel(cancellation)?;
        let Some(row) = rows.next()? else {
            return Err(invalid("provider capture family manifest index short"));
        };
        let n: u64 = row.get(0)?;
        let chunk_id: String = row.get(1)?;
        let chunk_bytes: Option<u64> = row.get(2)?;
        if n != ordinal as u64
            || chunk_id != part.chunk_id().as_str()
            || chunk_bytes != Some(part.byte_length())
            || part.byte_length() == 0
            || part.byte_length() > 1048576
        {
            return Err(invalid("provider capture family chunk index differs"));
        }
        let chunk = chunks.open_file(OsStr::new(object_name(&chunk_id)?))?;
        if chunk.file().metadata()?.len() != part.byte_length() {
            return Err(invalid("provider capture family chunk file length differs"));
        }
        chunk.verify_parent()?;
        if !chunk.same_file(&chunks.open_file(OsStr::new(object_name(&chunk_id)?))?) {
            return Err(invalid("provider capture family chunk path changed"));
        }
    }
    if rows.next()?.is_some() {
        return Err(invalid("provider capture family manifest index extra row"));
    }
    family.verify()?;
    Ok(manifest)
}

fn object_name(id: &str) -> Result<&str> {
    let name = id
        .strip_prefix("blake3:")
        .ok_or_else(|| invalid("provider capture content key differs"))?;
    if name.len() != 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("provider capture content key differs"));
    }
    Ok(name)
}

fn read_bounded(
    file: &std::fs::File,
    limit: usize,
    cancellation: Option<&Cancellation>,
) -> Result<Vec<u8>> {
    let mut reader = file;
    let mut output = Vec::new();
    let mut scratch = [0u8; 65536];
    loop {
        cancel(cancellation)?;
        let n = reader.read(&mut scratch)?;
        if n == 0 {
            break;
        }
        if output.len().checked_add(n).is_none_or(|len| len > limit) {
            return Err(invalid("provider capture manifest file exceeds bound"));
        }
        output.extend_from_slice(&scratch[..n]);
    }
    cancel(cancellation)?;
    Ok(output)
}
