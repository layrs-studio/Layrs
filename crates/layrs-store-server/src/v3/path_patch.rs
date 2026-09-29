//! Pure Tree reconstruction for already authorized exact leaf patches.
//! Does not authorize paths, admit contents, choose parents, or move a View.
use super::{V3StoreError, V3StoreResult};
use layrs_core::scm::{RepoPathComponent, Tree, TreeEntry, TreeEntryTarget, TreeId};
use layrs_sync::v3::{PathPatchChange, PathPatchEntry, validate_path_patch_changes};
use std::collections::BTreeMap;

const MAX_LOADED_TREES: usize = 16_384;
const MAX_VISITED_ENTRIES: usize = 200_000;
const MAX_METADATA_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct PatchedTree {
    pub root_tree_id: TreeId,
    /// Deduplicated generated Trees, ordered by ID. Existing opaque siblings are
    /// referenced unchanged and do not need to be loaded by this function.
    pub regenerated_trees: Vec<Tree>,
}

#[derive(Default)]
struct PatchNode<'a> {
    change: Option<&'a PathPatchChange>,
    children: BTreeMap<&'a str, PatchNode<'a>>,
}
#[derive(Default)]
struct Budget {
    trees: usize,
    entries: usize,
    bytes: usize,
}
fn invalid(message: &str) -> V3StoreError {
    V3StoreError::InvalidDescriptor(message.into())
}
impl Budget {
    fn charge(&mut self, tree: &Tree) -> V3StoreResult<()> {
        self.charge_entries(tree.entries())
    }
    fn charge_entries(&mut self, entries: &[TreeEntry]) -> V3StoreResult<()> {
        self.trees += 1;
        self.entries += entries.len();
        // A conservative bound before canonical allocation: names can expand to
        // six JSON bytes per UTF-8 byte; all other entry fields fit in 512 bytes.
        self.bytes = self.bytes.saturating_add(256);
        for entry in entries {
            self.bytes = self
                .bytes
                .saturating_add(512 + entry.name().as_str().len() * 6);
        }
        if self.trees > MAX_LOADED_TREES
            || self.entries > MAX_VISITED_ENTRIES
            || self.bytes > MAX_METADATA_BYTES
        {
            return Err(V3StoreError::LimitExceeded {
                resource: "path patch Tree metadata",
                limit: MAX_METADATA_BYTES as u64,
            });
        }
        Ok(())
    }
}

/// Loader must return a bounded canonical Tree; its ID is checked here. New file
/// contents still require independent CAS verification and scoped authorization.
pub fn rebuild_path_patch(
    root: &TreeId,
    changes: &[PathPatchChange],
    load: &mut impl FnMut(&TreeId) -> V3StoreResult<Tree>,
) -> V3StoreResult<PatchedTree> {
    validate_path_patch_changes(changes).map_err(|e| invalid(&e.message))?;
    let mut trie = PatchNode::default();
    for change in changes {
        let mut node = &mut trie;
        for part in change.path.as_str().split('/') {
            node = node.children.entry(part).or_default();
        }
        node.change = Some(change);
    }
    let mut generated = BTreeMap::new();
    let result = rebuild(
        Some(root),
        &trie,
        load,
        &mut Budget::default(),
        &mut generated,
    )?;
    Ok(PatchedTree {
        root_tree_id: result,
        regenerated_trees: generated.into_values().collect(),
    })
}

fn rebuild(
    id: Option<&TreeId>,
    node: &PatchNode<'_>,
    load: &mut impl FnMut(&TreeId) -> V3StoreResult<Tree>,
    budget: &mut Budget,
    generated: &mut BTreeMap<TreeId, Tree>,
) -> V3StoreResult<TreeId> {
    let original = match id {
        Some(id) => {
            let tree = load(id)?;
            if tree.id() != id {
                return Err(V3StoreError::IdentityMismatch);
            }
            budget.charge(&tree)?;
            tree
        }
        None => Tree::new(vec![]).map_err(|e| invalid(&e.to_string()))?,
    };
    let mut entries: BTreeMap<_, _> = original
        .entries()
        .iter()
        .map(|e| (e.name().as_str().to_owned(), e.clone()))
        .collect();
    for (name, next) in &node.children {
        let before = entries.get(*name).cloned();
        let component = RepoPathComponent::parse(*name).map_err(|e| invalid(&e.to_string()))?;
        if let Some(change) = next.change {
            let actual = before.as_ref().map(|e| PathPatchEntry {
                target: e.target().clone(),
                executable: e.executable(),
            });
            if actual != change.expected.0 {
                return Err(invalid(
                    "path patch expected entry differs from canonical base",
                ));
            }
            match &change.replacement.0 {
                Some(entry) => {
                    entries.insert(
                        (*name).into(),
                        TreeEntry::new(component, entry.target.clone(), entry.executable),
                    );
                }
                None => {
                    entries.remove(*name);
                }
            }
        } else {
            let child = match before.as_ref().map(|e| e.target()) {
                Some(TreeEntryTarget::Tree { tree_id }) => Some(tree_id),
                None => None,
                Some(_) => return Err(invalid("path patch traverses a non-directory ancestor")),
            };
            let new_child = rebuild(child, next, load, budget, generated)?;
            // Preserve an existing directory's mode too, even when its child changes.
            let executable = before.as_ref().is_some_and(|e| e.executable());
            entries.insert(
                (*name).into(),
                TreeEntry::new(component, TreeEntryTarget::tree(new_child), executable),
            );
        }
    }
    // Charge output separately; simultaneously retained input and output stay bounded.
    let entries: Vec<_> = entries.into_values().collect();
    if entries.len() > MAX_VISITED_ENTRIES {
        return Err(invalid("patched Tree exceeds entry bound"));
    }
    budget.charge_entries(&entries)?;
    let result = Tree::new(entries).map_err(|e| invalid(&e.to_string()))?;
    let id = result.id().clone();
    if result != original {
        generated.insert(id.clone(), result);
    }
    Ok(id)
}

#[cfg(test)]
#[path = "path_patch_tests.rs"]
mod tests;
