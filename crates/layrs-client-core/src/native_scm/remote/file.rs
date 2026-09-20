//! Read one explicitly authorized file without constructing a partial Worktree.
use super::*;

impl NativeRemote {
    pub fn read_remote_file(
        &self,
        workspace: &NativeWorkspace,
        step: &StepId,
        path: &RepoPath,
    ) -> RemoteResult<(FileId, Vec<u8>)> {
        let (graph, file_id) = self.authorized_file(workspace, step, path)?;
        let bytes = graph.file_bytes(&file_id)?;
        Ok((file_id, bytes))
    }

    /// Verify the complete authorized file before creating a new output, then
    /// write one verified chunk at a time. Never replaces an existing path.
    pub fn download_remote_file(
        &self,
        workspace: &NativeWorkspace,
        step: &StepId,
        path: &RepoPath,
        output: &Path,
    ) -> RemoteResult<(FileId, u64)> {
        let (graph, file_id) = self.authorized_file(workspace, step, path)?;
        graph.verify_file(graph.manifest(&file_id)?)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        let length = graph.write_file(&file_id, &mut file)?;
        file.sync_all()?;
        Ok((file_id, length))
    }

    fn authorized_file(
        &self,
        workspace: &NativeWorkspace,
        step: &StepId,
        path: &RepoPath,
    ) -> RemoteResult<(GraphObjects, FileId)> {
        let binding = self.binding(workspace)?;
        let root = AuthorizationRoot::Step {
            step_id: step.clone(),
        };
        let mut graph = GraphObjects::default();
        self.fetch_visit(
            &binding,
            FetchClaim::Object {
                object: ObjectReference {
                    kind: ObjectKind::Step,
                    object_id: ObjectId::parse(step.as_str())?,
                },
                root: root.clone(),
                repo_path: Nullable::null(),
            },
            |object| graph.insert(object),
        )?;
        let mut tree_id = graph
            .steps
            .get(step)
            .ok_or(NativeRemoteError::Invalid("missing requested Step"))?
            .root_tree_id()
            .clone();
        let parts = path.as_str().split('/').collect::<Vec<_>>();
        if parts.is_empty() || parts.len() > 128 {
            return Err(NativeRemoteError::Invalid("file path exceeds depth bound"));
        }
        for (index, part) in parts.iter().enumerate() {
            self.fetch_visit(
                &binding,
                FetchClaim::Object {
                    object: ObjectReference {
                        kind: ObjectKind::Tree,
                        object_id: ObjectId::parse(tree_id.as_str())?,
                    },
                    root: root.clone(),
                    repo_path: Nullable::null(),
                },
                |object| graph.insert(object),
            )?;
            let Some(CanonicalObject::Tree(tree)) =
                graph.objects.get(&StoreObjectId::Tree(tree_id.clone()))
            else {
                return Err(NativeRemoteError::Invalid("missing requested Tree"));
            };
            let target = tree
                .entries()
                .iter()
                .find(|entry| entry.name().as_str() == *part)
                .ok_or(NativeRemoteError::Invalid(
                    "file path absent from requested Step",
                ))?
                .target()
                .clone();
            match target {
                TreeEntryTarget::Tree { tree_id: child } if index + 1 < parts.len() => {
                    tree_id = child
                }
                TreeEntryTarget::File { file_id } if index + 1 == parts.len() => {
                    // Always claim this exact path, even if a manifest for the
                    // same FileId happened to arrive under another claim.
                    self.fetch_visit(
                        &binding,
                        FetchClaim::File {
                            file_id: file_id.clone(),
                            preferred_manifest_ids: vec![],
                            root: root.clone(),
                            repo_path: path.clone(),
                        },
                        |object| graph.insert(object),
                    )?;
                    return Ok((graph, file_id));
                }
                _ => {
                    return Err(NativeRemoteError::Invalid(
                        "requested path is not an ordinary file",
                    ))
                }
            }
        }
        Err(NativeRemoteError::Invalid(
            "requested path is not an ordinary file",
        ))
    }
}
