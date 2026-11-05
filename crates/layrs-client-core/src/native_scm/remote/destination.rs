//! Exact-root, read-only inspection before cloning into a chosen directory.
use super::*;

type LocalInspection = (NativeWorkspace, WorkspaceState, Option<Binding>);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneDestination {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_space_id: Option<SpaceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

impl CloneDestination {
    fn empty() -> Self {
        Self {
            kind: "empty",
            local_space_id: None,
            name: None,
            context_id: None,
            reason: None,
        }
    }
    fn blocked(reason: &'static str) -> Self {
        Self {
            kind: "blocked",
            local_space_id: None,
            name: None,
            context_id: None,
            reason: Some(reason),
        }
    }
    fn project(kind: &'static str, space: SpaceId, name: String, context: Option<String>) -> Self {
        Self {
            kind,
            local_space_id: Some(space),
            name: Some(name),
            context_id: context,
            reason: None,
        }
    }
}

/// A blocked inspection is deliberately an ordinary result. The caller may
/// present it without turning an authorization or filesystem error into a
/// suggestion to publish over another project.
impl NativeRemote {
    /// Provision an exact local checkout. A repeated request may find that its
    /// first response was lost: the server's exact replay check remains the
    /// authority for matching name, slug, context, Layer, and Space identities.
    pub fn provision_existing(
        &self,
        root: &Path,
        expected: &SpaceId,
        context_id: &str,
        workspace_slug: &str,
        space_slug: &str,
    ) -> RemoteResult<RemoteProvisionedSpace> {
        let (workspace, before, binding) = inspect_local(root)
            .map_err(NativeRemoteError::Invalid)?
            .ok_or(NativeRemoteError::Invalid("local project is missing"))?;
        if before.space.id() != expected || binding.is_some() {
            return Err(NativeRemoteError::Invalid(
                "local project changed or is already bound",
            ));
        }
        match self.bootstrap(expected) {
            Ok(bootstrap) => {
                receive::require_complete_access(&bootstrap)?;
                if !matching_base(&before, &bootstrap) {
                    return Err(NativeRemoteError::Invalid(
                        "local project differs from server",
                    ));
                }
                let capabilities = self.space_capabilities(expected)?;
                if capabilities["contextId"].as_str() != Some(context_id) {
                    return Err(NativeRemoteError::Invalid("project context changed"));
                }
            }
            Err(NativeRemoteError::Service { status: 403, code }) if code == "forbidden" => {
                self.check_local_project(expected, Some(context_id))?;
            }
            Err(error) => return Err(error),
        }
        let (verified, current, current_binding) = inspect_local(root)
            .map_err(NativeRemoteError::Invalid)?
            .ok_or(NativeRemoteError::Invalid("local project changed"))?;
        if current.space.id() != expected
            || current.worktree != before.worktree
            || current.operation.id() != before.operation.id()
            || current_binding.is_some()
            || verified.checkout_root()? != workspace.checkout_root()?
        {
            return Err(NativeRemoteError::Invalid("local project changed"));
        }
        let bases: Vec<_> = current
            .layers
            .iter()
            .filter(|layer| layer.kind() == LayerKind::Base && layer.parent_layer_ids().is_empty())
            .collect();
        if bases.len() != 1 {
            return Err(NativeRemoteError::Invalid(
                "initial publication requires one base Layer",
            ));
        }
        let response: RemoteProvisionedSpace = self.post(
            "v3/workspaces/provision",
            &serde_json::json!({
                "workspaceSlug":workspace_slug,
                "spaceSlug":space_slug,
                "name":current.space.name(),
                "initialLayer":bases[0],
                "contextId":context_id,
            }),
        )?;
        if response.space_id != *expected || response.layer_id != *bases[0].id() {
            return Err(NativeRemoteError::Invalid(
                "provisioned identities differ from native identities",
            ));
        }
        Ok(response)
    }

    pub fn inspect_clone_destination(
        &self,
        root: &Path,
        selected: Option<&SpaceId>,
        context_id: Option<&str>,
    ) -> CloneDestination {
        let (_workspace, state, binding) = match inspect_local(root) {
            Ok(None) => return CloneDestination::empty(),
            Ok(Some(local)) => local,
            Err(reason) => return CloneDestination::blocked(reason),
        };
        let local_space = state.space.id().clone();
        let name = state.space.name().to_owned();
        if let Some(binding) = &binding {
            if !binding_matches(self, &local_space, binding) {
                return CloneDestination::blocked("foreignOrPendingBinding");
            }
        }
        match self.bootstrap(&local_space) {
            Ok(bootstrap) => {
                if receive::require_complete_access(&bootstrap).is_err()
                    || !matching_base(&state, &bootstrap)
                {
                    return CloneDestination::blocked("projectStateMismatch");
                }
                let capabilities = match self.space_capabilities(&local_space) {
                    Ok(value) => value,
                    Err(_) => return CloneDestination::blocked("projectAccessUnavailable"),
                };
                let Some(context) = capabilities["contextId"].as_str() else {
                    return CloneDestination::blocked("projectContextUnavailable");
                };
                let kind = if selected == Some(&local_space) {
                    "sameProject"
                } else {
                    "otherProject"
                };
                CloneDestination::project(kind, local_space, name, Some(context.to_owned()))
            }
            Err(_) if binding.is_some() => CloneDestination::blocked("boundProjectUnavailable"),
            Err(NativeRemoteError::Service { status: 403, code }) if code == "forbidden" => {
                match self.check_local_project(&local_space, context_id) {
                    Ok(context) => CloneDestination::project(
                        "unpublishedProject",
                        local_space,
                        name,
                        Some(context),
                    ),
                    Err(_) => CloneDestination::blocked("projectAccessUnavailable"),
                }
            }
            Err(_) => CloneDestination::blocked("projectAccessUnavailable"),
        }
    }

    /// Link an existing checkout on this origin. No capture, receive, or file
    /// materialization occurs. The local Space ID is the compare-and-swap key.
    pub fn link_existing(&self, root: &Path, expected: &SpaceId) -> RemoteResult<WorkspaceState> {
        let (workspace, state, initial_binding) = inspect_local(root)
            .map_err(NativeRemoteError::Invalid)?
            .ok_or(NativeRemoteError::Invalid("clone destination is empty"))?;
        if state.space.id() != expected {
            return Err(NativeRemoteError::Invalid("local project changed"));
        }
        let _lock = remote_lock(&workspace)?;
        let (workspace, state, existing) = inspect_local(root)
            .map_err(NativeRemoteError::Invalid)?
            .ok_or(NativeRemoteError::Invalid("local project changed"))?;
        if state.space.id() != expected {
            return Err(NativeRemoteError::Invalid("local project changed"));
        }
        if existing
            .as_ref()
            .is_some_and(|binding| !binding_matches(self, expected, binding))
        {
            return Err(NativeRemoteError::Invalid(
                "foreign or pending remote binding",
            ));
        }
        let bootstrap = self.bootstrap(expected)?;
        receive::require_complete_access(&bootstrap)?;
        if !matching_base(&state, &bootstrap) {
            return Err(NativeRemoteError::Invalid(
                "local project differs from server",
            ));
        }
        let capabilities = self.space_capabilities(expected)?;
        if capabilities["contextId"].as_str().is_none() {
            return Err(NativeRemoteError::Invalid("project context unavailable"));
        }
        let (verified, current, current_binding) = inspect_local(root)
            .map_err(NativeRemoteError::Invalid)?
            .ok_or(NativeRemoteError::Invalid("local project changed"))?;
        if current.space.id() != expected
            || current.worktree != state.worktree
            || current.operation.id() != state.operation.id()
            || verified.checkout_root()? != workspace.checkout_root()?
            || serde_json::to_vec(&initial_binding)? != serde_json::to_vec(&current_binding)?
        {
            return Err(NativeRemoteError::Invalid(
                "local project changed during access check",
            ));
        }
        if current_binding.is_none() {
            let binding = Binding {
                origin: self.origin.to_string(),
                space_id: expected.clone(),
                client_id: ClientId::parse(format!("client:{}", uuid::Uuid::new_v4()))
                    .map_err(|_| NativeRemoteError::Invalid("invalid client identity"))?,
                clone_pending: false,
                observed: None,
                pending: None,
                last_receipt: None,
            };
            write_new_binding(&verified, &binding)?;
        }
        Ok(current)
    }

    /// Rechecks an absent native Space and the selected creation context. The
    /// actual provision action remains explicit and performs its own checks.
    pub fn prepare_existing_publication(
        &self,
        root: &Path,
        expected: &SpaceId,
        context_id: &str,
    ) -> RemoteResult<(WorkspaceState, String)> {
        let (workspace, state, binding) = inspect_local(root)
            .map_err(NativeRemoteError::Invalid)?
            .ok_or(NativeRemoteError::Invalid("clone destination is empty"))?;
        if state.space.id() != expected || binding.is_some() {
            return Err(NativeRemoteError::Invalid(
                "local project changed or is already bound",
            ));
        }
        match self.bootstrap(expected) {
            Ok(_) => {
                return Err(NativeRemoteError::Invalid(
                    "project already exists on server",
                ));
            }
            Err(NativeRemoteError::Service { status: 403, code }) if code == "forbidden" => {}
            Err(error) => return Err(error),
        }
        let context = self.check_local_project(expected, Some(context_id))?;
        let (rechecked, current, current_binding) = inspect_local(root)
            .map_err(NativeRemoteError::Invalid)?
            .ok_or(NativeRemoteError::Invalid("local project changed"))?;
        if current.space.id() != expected
            || current.worktree != state.worktree
            || current.operation.id() != state.operation.id()
            || current_binding.is_some()
            || rechecked.checkout_root()? != workspace.checkout_root()?
        {
            return Err(NativeRemoteError::Invalid("local project changed"));
        }
        Ok((current, context))
    }

    fn check_local_project(&self, space: &SpaceId, context: Option<&str>) -> RemoteResult<String> {
        let value: serde_json::Value = self.post(
            "v3/projects/check-local",
            &serde_json::json!({"spaceId":space,"contextId":context}),
        )?;
        if value["canPublish"] != true {
            return Err(NativeRemoteError::Invalid(
                "project cannot be published here",
            ));
        }
        let resolved = value["contextId"]
            .as_str()
            .filter(|id| id.starts_with("workspace:") && id.len() <= 128)
            .ok_or(NativeRemoteError::Invalid("invalid project context"))?;
        if context.is_some_and(|requested| requested != resolved) {
            return Err(NativeRemoteError::Invalid("project context changed"));
        }
        Ok(resolved.to_owned())
    }
}

fn binding_matches(remote: &NativeRemote, space: &SpaceId, binding: &Binding) -> bool {
    binding.origin == remote.origin.as_str()
        && &binding.space_id == space
        && !binding.clone_pending
        && binding.pending.is_none()
}

fn matching_base(state: &WorkspaceState, remote: &SpaceBootstrap) -> bool {
    let local: Vec<_> = state
        .layers
        .iter()
        .filter(|layer| layer.kind() == LayerKind::Base && layer.parent_layer_ids().is_empty())
        .collect();
    let server: Vec<_> = remote
        .layers
        .iter()
        .filter(|layer| {
            layer.record.kind() == LayerKind::Base && layer.record.parent_layer_ids().is_empty()
        })
        .collect();
    local.len() == 1 && server.len() == 1 && local[0] == &server[0].record
}

fn inspect_local(root: &Path) -> std::result::Result<Option<LocalInspection>, &'static str> {
    let root = std::path::absolute(root).map_err(|_| "unreadableDestination")?;
    for component in root.ancestors() {
        let metadata = std::fs::symlink_metadata(component).map_err(|_| "unreadableDestination")?;
        if reparse(&metadata) {
            return Err("linkedDestination");
        }
    }
    let metadata = std::fs::symlink_metadata(&root).map_err(|_| "unreadableDestination")?;
    if !metadata.is_dir() {
        return Err("notDirectory");
    }
    let store = root.join(".layrs");
    match std::fs::symlink_metadata(&store) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if std::fs::read_dir(&root)
                .map_err(|_| "unreadableDestination")?
                .next()
                .is_none()
            {
                return Ok(None);
            }
            return Err("notLayrsProject");
        }
        Err(_) => return Err("unreadableDestination"),
        Ok(metadata) if reparse(&metadata) || !(metadata.is_dir() || metadata.is_file()) => {
            return Err("invalidLayrsStore");
        }
        Ok(_) => {}
    }
    let workspace = NativeWorkspace::open(&root).map_err(|_| "invalidLayrsStore")?;
    let state = workspace.state().map_err(|_| "invalidLayrsStore")?;
    if state.view.space_id() != state.space.id()
        || state
            .layers
            .iter()
            .any(|layer| layer.space_id() != state.space.id())
    {
        return Err("projectIdentityMismatch");
    }
    if Path::new(state.worktree.checkout_root()) != root
        || state.worktree.space_id() != state.space.id()
    {
        return Err("projectRootMismatch");
    }
    if workspace.needs_repair().map_err(|_| "invalidLayrsStore")? {
        return Err("repairRequired");
    }
    let binding_path = workspace.store_root().join("native-remote.json");
    let binding = match std::fs::symlink_metadata(&binding_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("invalidRemoteBinding"),
        Ok(metadata) => {
            if reparse(&metadata) || !metadata.is_file() {
                return Err("invalidRemoteBinding");
            }
            Some(
                serde_json::from_slice(
                    &bounded_file(&binding_path, MAX_OBJECT).map_err(|_| "invalidRemoteBinding")?,
                )
                .map_err(|_| "invalidRemoteBinding")?,
            )
        }
    };
    Ok(Some((workspace, state, binding)))
}

fn reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn write_new_binding(workspace: &NativeWorkspace, binding: &Binding) -> RemoteResult<()> {
    let path = workspace.store_root().join("native-remote.json");
    let temp = workspace
        .store_root()
        .join(format!("native-remote-{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(&serde_json::to_vec(binding)?)?;
    file.sync_all()?;
    drop(file);
    let result = std::fs::hard_link(&temp, &path);
    let _ = std::fs::remove_file(&temp);
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_secondary_worktree_uses_validated_locator_and_shared_binding() {
        let root = std::env::temp_dir().join(format!("layrs-destination-{}", uuid::Uuid::new_v4()));
        let primary_root = root.join("primary");
        let secondary_root = root.join("secondary");
        std::fs::create_dir_all(&primary_root).unwrap();
        std::fs::create_dir(&secondary_root).unwrap();
        std::fs::write(primary_root.join("keep.txt"), b"local work").unwrap();
        let primary = NativeWorkspace::initialize(&primary_root, "Destination").unwrap();
        let state = primary.state().unwrap();
        let selected = state
            .view
            .worktrees()
            .iter()
            .find(|value| value.worktree_id() == primary.worktree_id())
            .unwrap();
        let secondary = primary
            .create_worktree(
                &secondary_root,
                selected.active_layer_id().unwrap(),
                selected.base_step_id().unwrap(),
            )
            .unwrap();
        assert!(secondary_root.join(".layrs").is_file());
        let (inspected, current, binding) = inspect_local(&secondary_root).unwrap().unwrap();
        assert_eq!(current.worktree.id(), secondary.worktree_id());
        assert_eq!(inspected.store_root(), primary.store_root());
        assert!(binding.is_none());
        let binding = Binding {
            origin: "https://example.invalid/".into(),
            space_id: state.space.id().clone(),
            client_id: ClientId::parse("client:destination-test").unwrap(),
            clone_pending: false,
            observed: None,
            pending: None,
            last_receipt: None,
        };
        write_new_binding(&inspected, &binding).unwrap();
        for checkout in [&primary_root, &secondary_root] {
            let (_, _, current_binding) = inspect_local(checkout).unwrap().unwrap();
            assert_eq!(current_binding.unwrap().space_id, binding.space_id);
            assert_eq!(
                std::fs::read(checkout.join("keep.txt")).unwrap(),
                b"local work"
            );
        }
        assert!(!secondary_root.join("native-remote.json").exists());
        // Accepting locator files must not accept arbitrary or corrupt markers.
        std::fs::write(secondary_root.join(".layrs"), b"invalid locator").unwrap();
        assert!(matches!(
            inspect_local(&secondary_root),
            Err("invalidLayrsStore")
        ));
        drop((inspected, secondary, primary));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
