//! Exact path classification. Observations never become standalone write capabilities.
use super::*;

fn validate_access(
    response: &ProjectionAccess,
    request: &ProjectionAccessRequest,
    principal: &PrincipalId,
    space: &SpaceId,
) -> RemoteResult<()> {
    request
        .validate()
        .map_err(|_| NativeRemoteError::Invalid("invalid projection access paths"))?;
    if &response.principal_id != principal
        || &response.space_id != space
        || response.step_id != request.step_id
        || response.policy_set_id != request.expected_policy_set_id
        || response.authorization_epoch != request.expected_authorization_epoch
        || response.entries.len() != request.paths.len()
        || response
            .entries
            .iter()
            .zip(&request.paths)
            .any(|(entry, path)| {
                &entry.path != path
                    || (entry.writable && !entry.content_readable)
                    || (entry.executable
                        && !matches!(entry.target.as_ref(), Some(TreeEntryTarget::File { .. })))
            })
    {
        return Err(NativeRemoteError::Invalid(
            "projection access correlation mismatch",
        ));
    }
    Ok(())
}
impl NativeRemote {
    /// Classifies 1..200 normalized paths for the authenticated account, including absence.
    /// The caller must compare root_tree_id to its exact Step before using a local observation.
    pub fn projection_access(
        &self,
        space: &SpaceId,
        request: &ProjectionAccessRequest,
    ) -> RemoteResult<ProjectionAccess> {
        request
            .validate()
            .map_err(|_| NativeRemoteError::Invalid("invalid projection access paths"))?;
        let principal: PrincipalId =
            serde_json::from_value(self.whoami()?["principal_id"].clone())?;
        let response: ProjectionAccess =
            self.post(&format!("v3/spaces/{space}/projection/access"), request)?;
        validate_access(&response, request, &principal, space)?;
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(n: usize) -> ObjectId {
        ObjectId::parse(format!("blake3:{n:064x}")).unwrap()
    }
    fn sample() -> (ProjectionAccessRequest, ProjectionAccess) {
        let request = ProjectionAccessRequest {
            step_id: StepId::parse(id(1).as_str()).unwrap(),
            expected_policy_set_id: id(2),
            expected_authorization_epoch: 7,
            paths: vec![
                RepoPath::parse("private/missing").unwrap(),
                RepoPath::parse("public/file").unwrap(),
            ],
        };
        let response:ProjectionAccess=serde_json::from_value(serde_json::json!({"schemaVersion":"layrs.projection-access.v1","principalId":"principal:member","spaceId":"space:test","stepId":request.step_id,"rootTreeId":id(3),"policySetId":id(2),"authorizationEpoch":7,"entries":[{"path":"private/missing","target":null,"executable":false,"contentReadable":false,"writable":false},{"path":"public/file","target":{"kind":"file","fileId":id(4)},"executable":false,"contentReadable":true,"writable":true}]})).unwrap();
        (request, response)
    }
    fn check(request: &ProjectionAccessRequest, response: &ProjectionAccess) -> RemoteResult<()> {
        validate_access(
            response,
            request,
            &PrincipalId::parse("principal:member").unwrap(),
            &SpaceId::parse("space:test").unwrap(),
        )
    }
    #[test]
    fn presence_and_denied_absence_are_distinct_exact_observations() {
        let (r, a) = sample();
        check(&r, &a).unwrap();
        assert!(a.entries[0].target.is_none());
        assert!(!a.entries[0].writable);
    }
    #[test]
    fn path_missing_reordered_or_duplicate_never_succeeds() {
        let (r, a) = sample();
        let mut bad = a.clone();
        bad.entries.pop();
        assert!(check(&r, &bad).is_err());
        let mut bad = a.clone();
        bad.entries.reverse();
        assert!(check(&r, &bad).is_err());
        let mut bad = a;
        bad.entries[1] = bad.entries[0].clone();
        assert!(check(&r, &bad).is_err());
    }
    #[test]
    fn identity_space_step_policy_and_epoch_are_bound() {
        let (r, a) = sample();
        let mut changes = vec![];
        let mut b = a.clone();
        b.principal_id = PrincipalId::parse("principal:other").unwrap();
        changes.push(b);
        let mut b = a.clone();
        b.space_id = SpaceId::parse("space:other").unwrap();
        changes.push(b);
        let mut b = a.clone();
        b.step_id = StepId::parse(id(9).as_str()).unwrap();
        changes.push(b);
        let mut b = a.clone();
        b.policy_set_id = id(9);
        changes.push(b);
        let mut b = a;
        b.authorization_epoch += 1;
        changes.push(b);
        for b in changes {
            assert!(check(&r, &b).is_err());
        }
    }
    #[test]
    fn absent_executable_and_write_without_read_are_refused() {
        let (r, mut a) = sample();
        a.entries[0].executable = true;
        assert!(check(&r, &a).is_err());
        a.entries[0].executable = false;
        a.entries[0].writable = true;
        assert!(check(&r, &a).is_err());
    }
    #[test]
    fn request_limits_and_duplicates_validate_before_network() {
        let (mut r, a) = sample();
        r.paths.clear();
        assert!(check(&r, &a).is_err());
        let (mut r, a) = sample();
        r.paths = vec![r.paths[0].clone(); 201];
        assert!(check(&r, &a).is_err());
        let (mut r, a) = sample();
        r.paths.reverse();
        assert!(check(&r, &a).is_err());
    }
}
