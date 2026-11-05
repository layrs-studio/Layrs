use super::*;
#[path = "projection_access.rs"]
mod access;

impl NativeRemote {
    /// Read structural entries and effective path rights. No bytes are installed
    /// and no complete native Step is admitted by this method.
    pub fn projection_page(
        &self,
        space: &SpaceId,
        request: &ProjectionPageRequest,
    ) -> RemoteResult<ProjectionPage> {
        if !(1..=200).contains(&request.limit)
            || request
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.len() > 4096)
        {
            return Err(NativeRemoteError::Invalid(
                "invalid projection page request",
            ));
        }
        let page: ProjectionPage = self.post(&format!("v3/spaces/{space}/projection"), request)?;
        if &page.space_id != space
            || page.step_id != request.step_id
            || page.policy_set_id != request.expected_policy_set_id
            || page.authorization_epoch != request.expected_authorization_epoch
            || page.entries.len() > usize::from(request.limit)
            || (request.cursor.is_none() && page.offset != 0)
            || page
                .next_cursor
                .as_ref()
                .is_some_and(|cursor| cursor.len() > 4096)
            || (page.next_cursor.is_some() && page.entries.is_empty())
            || page
                .entries
                .iter()
                .any(|entry| entry.writable && !entry.content_readable)
        {
            return Err(NativeRemoteError::Invalid(
                "projection page correlation mismatch",
            ));
        }
        let mut paths = BTreeSet::new();
        for entry in &page.entries {
            if entry.path.as_str().len() > 4096 || !paths.insert(entry.path.clone()) {
                return Err(NativeRemoteError::Invalid("invalid projection entries"));
            }
        }
        Ok(page)
    }
}
