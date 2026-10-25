//! Selection history is local metadata. Schema6 makes every older mutation
//! reader fail closed, including readers holding an already-open store handle.
use super::*;
use layrs_core::projection::ProjectionSelection;
use rusqlite::{Connection, params};

pub(super) const SQL: &str = "CREATE TABLE projection_selections(generation INTEGER PRIMARY KEY REFERENCES projection_snapshots(generation) ON DELETE RESTRICT,digest TEXT NOT NULL CHECK(length(digest)=64),payload BLOB NOT NULL CHECK(length(payload)<=16777216)) STRICT;";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    schema_version: String,
    selection: ProjectionSelection,
}

pub(super) fn encode(selection: &ProjectionSelection) -> Result<Vec<u8>> {
    let bytes = layrs_core::scm::canonical_json(&Record {
        schema_version: "layrs.projection-selection.v1".into(),
        selection: selection.clone(),
    })
    .map_err(invalid)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(invalid("selection document exceeds16MiB"));
    }
    Ok(bytes)
}

pub(super) fn initial_digest(selection: &ProjectionSelection) -> Result<String> {
    Ok(blake3::hash(&encode(selection)?).to_hex().to_string())
}

pub(super) fn insert(
    c: &Connection,
    generation: u64,
    selection: &ProjectionSelection,
) -> Result<()> {
    let snapshot: String = c.query_row(
        "SELECT digest FROM projection_snapshots WHERE generation=?1",
        [generation],
        |r| r.get(0),
    )?;
    let payload = encode(selection)?;
    let digest = database::digest(generation, Some(&snapshot), &payload);
    c.execute(
        "INSERT INTO projection_selections VALUES(?1,?2,?3)",
        params![generation, digest, payload],
    )?;
    Ok(())
}

pub(super) fn current(c: &Connection) -> Result<ProjectionSelection> {
    let generation: u64 = c.query_row(
        "SELECT generation FROM projection_head WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    at(c, generation)
}
pub(super) fn at(c: &Connection, generation: u64) -> Result<ProjectionSelection> {
    let (generation, length): (u64,u64) = c.query_row("SELECT generation,length(payload) FROM projection_selections WHERE generation<=?1 ORDER BY generation DESC LIMIT 1", [generation], |r| Ok((r.get(0)?,r.get(1)?)))?;
    if length > 16 * 1024 * 1024 {
        return Err(invalid("selection document exceeds16MiB"));
    }
    let (digest, payload, snapshot): (String,Vec<u8>,String) = c.query_row("SELECT s.digest,s.payload,p.digest FROM projection_selections s JOIN projection_snapshots p USING(generation) WHERE s.generation=?1", [generation], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if database::digest(generation, Some(&snapshot), &payload) != digest {
        return Err(invalid("selection digest mismatch"));
    }
    let record: Record = serde_json::from_slice(&payload).map_err(invalid)?;
    if record.schema_version != "layrs.projection-selection.v1"
        || encode(&record.selection)? != payload
    {
        return Err(invalid("unsupported or noncanonical selection document"));
    }
    Ok(record.selection)
}

pub(super) fn validate(c: &Connection, snapshot: &ProjectionSnapshot) -> Result<()> {
    current(c)?.validate_snapshot(snapshot)?;
    Ok(())
}

/// Use the caller's connection so scans inside a write transaction observe its
/// head and never contend with their own transaction after SQLite spills pages.
pub(super) fn for_scan(
    c: &Connection,
    snapshot: &ProjectionSnapshot,
) -> Result<ProjectionSelection> {
    let selection = current(c)?;
    selection.validate_snapshot(snapshot)?;
    Ok(selection)
}

impl ProjectionStore {
    pub fn selection(&self) -> Result<ProjectionSelection> {
        Ok(self.state_with_selection()?.1)
    }
    /// One shared lock binds the selection and snapshot to the same generation.
    pub fn state_with_selection(&self) -> Result<(VersionedProjection, ProjectionSelection)> {
        let access = format::access(self, SessionMode::Shared)?;
        database::require_idle(&access.connection)?;
        let state = database::current(&access.connection)?;
        let selection = current(&access.connection)?;
        selection.validate_snapshot(&state.snapshot)?;
        access.verify()?;
        Ok((state, selection))
    }
    pub fn create_selected(
        root: impl AsRef<Path>,
        identity: ProjectionIdentity,
        initial: ProjectionSnapshot,
        selection: ProjectionSelection,
    ) -> Result<Self> {
        Self::create_selected_with_faults(root, identity, initial, selection, &NoCreationFaults)
    }
    pub fn create_selected_with_faults(
        root: impl AsRef<Path>,
        identity: ProjectionIdentity,
        initial: ProjectionSnapshot,
        selection: ProjectionSelection,
        faults: &dyn CreationFaults,
    ) -> Result<Self> {
        let selection = selection.normalized(initial.path_semantics)?;
        selection.validate_snapshot(&initial)?;
        format::create_selected(root.as_ref(), identity, initial, &selection, faults)
    }
    pub fn resume_create_selected(
        root: impl AsRef<Path>,
        identity: &ProjectionIdentity,
        initial: &ProjectionSnapshot,
        selection: &ProjectionSelection,
    ) -> Result<Self> {
        format::resume_selected(root.as_ref(), identity, initial, selection)
    }
    /// Only expands intent. Hydration and intent commit in the same journal/CAS;
    /// existing checkout files and captured bytes are never rewritten by expansion.
    pub fn expand_selection(
        &self,
        expected_generation: u64,
        selection: &ProjectionSelection,
        target: &ProjectionSnapshot,
        write_file: &mut dyn FnMut(&str, &mut dyn io::Write) -> io::Result<()>,
    ) -> Result<VersionedProjection> {
        self.expand_selection_with_faults(
            expected_generation,
            selection,
            target,
            write_file,
            &NoInstallFaults,
        )
    }
    pub fn expand_selection_with_faults(
        &self,
        expected_generation: u64,
        selection: &ProjectionSelection,
        target: &ProjectionSnapshot,
        write_file: &mut dyn FnMut(&str, &mut dyn io::Write) -> io::Result<()>,
        faults: &dyn InstallFaults,
    ) -> Result<VersionedProjection> {
        let selection = selection.normalized(target.path_semantics)?;
        install::install_selected(
            self,
            expected_generation,
            target,
            Some(&selection),
            write_file,
            faults,
        )
    }
}

pub(super) fn check_paths(c: &Connection, paths: &[layrs_core::scm::RepoPath]) -> Result<()> {
    let selection = current(c)?;
    if paths.iter().any(|path| !selection.includes(path)) {
        return Err(invalid(
            "path is outside the checkout selection; expand explicitly first",
        ));
    }
    Ok(())
}

pub(super) fn verify_target(c: &Connection, target: &ProjectionSnapshot) -> Result<()> {
    validate(c, target)
}
