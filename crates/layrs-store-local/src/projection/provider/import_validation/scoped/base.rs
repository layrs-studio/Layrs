//! Resolve companion preimages at their source version, by exact path.
use super::*;
use crate::projection::{capture, offline};
use layrs_core::{projection::ProjectionSnapshot, scm::TreeEntryTarget};

pub(super) struct EffectiveBase {
    snapshot: ProjectionSnapshot,
    patch: Option<capture::CapturedPatch>,
}

impl EffectiveBase {
    pub fn at(
        c: &Connection,
        version: &super::super::super::records::import::ImportVersion,
        lineage: &ProvenanceIndex,
    ) -> CauseResult<Self> {
        let snapshot =
            offline::snapshot_at(c, version.generation, Some(&version.snapshot_digest))?.snapshot;
        let patch = if version.capture_count == 0 {
            None
        } else {
            let (origin, first, maximum) =
                lineage.capture_source(version.capture_count, version.generation)?;
            let patch = capture::storage::load_metadata_for_origin(
                c,
                version.capture_count,
                origin,
                maximum,
            )?;
            if patch.projection_generation < first
                || patch.digest != version.capture_head.as_deref().unwrap_or("")
            {
                return Err(invalid("import companion source capture head differs"));
            }
            (patch.projection_generation == version.generation).then_some(patch)
        };
        Ok(Self { snapshot, patch })
    }

    pub fn require(
        &self,
        path: &RepoPath,
        expected_file: Option<&FileId>,
        expected_executable: Option<bool>,
    ) -> CauseResult<()> {
        let entry = self
            .snapshot
            .entries
            .binary_search_by(|candidate| candidate.path.cmp(path))
            .ok()
            .map(|index| &self.snapshot.entries[index]);
        let snapshot_base = entry.and_then(|entry| match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some((file_id, entry.executable)),
            _ => None,
        });
        let changed = self.patch.as_ref().and_then(|patch| {
            patch
                .changes
                .binary_search_by(|change| change.path.cmp(path))
                .ok()
                .map(|index| &patch.changes[index])
        });
        let actual = match changed {
            Some(change) => change
                .replacement
                .as_ref()
                .map(|file| (&file.file_id, file.executable)),
            None => snapshot_base,
        };
        if actual.map(|(file, _)| file) != expected_file
            || actual.map(|(_, executable)| executable) != expected_executable
        {
            return Err(invalid("import companion base differs at source path"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::provider::validation::tests::fixture::Fixture;

    #[test]
    fn companion_base_is_resolved_at_its_own_path() {
        let fixture = Fixture::new("G1", false);
        let mut snapshot = fixture.source.initial().clone();
        let readme = snapshot.entries[0].path.clone();
        let original = match snapshot.entries[0].target.as_ref().unwrap() {
            TreeEntryTarget::File { file_id } => file_id.clone(),
            _ => panic!("fixture must be a file"),
        };
        let other = RepoPath::parse("second.txt").unwrap();
        let other_id = FileId::from_bytes(b"different-file");
        let mut second = snapshot.entries[0].clone();
        second.path = other.clone();
        second.target = Some(TreeEntryTarget::file(other_id.clone()));
        snapshot.entries.push(second);
        snapshot
            .entries
            .sort_by(|left, right| left.path.cmp(&right.path));
        snapshot.validate().unwrap();
        let effective = EffectiveBase {
            snapshot,
            patch: None,
        };
        effective
            .require(&readme, Some(&original), Some(false))
            .unwrap();
        effective
            .require(&other, Some(&other_id), Some(false))
            .unwrap();
        assert!(
            effective
                .require(&readme, Some(&other_id), Some(false))
                .is_err()
        );
        assert!(
            effective
                .require(&other, Some(&original), Some(false))
                .is_err()
        );
        assert!(
            effective
                .require(&other, Some(&other_id), Some(true))
                .is_err()
        );
    }
}
