//! Process-wide cache of decoded, verified Trees.
//!
//! A Tree is content-addressed: once its canonical bytes were read and
//! verified against its id (and its SQL edges checked against them), its
//! value never changes. Merges, projections and admission checks read the
//! same Trees many times in one process (a 3-way Room merge decoded 12
//! distinct Trees about 74 times). A hit skips the CAS read and the
//! decode; the Tree's presence in the Store is still answered by its
//! verified `objects` row at every read (`get_tree`), like git's in-memory
//! object cache. Entries are keyed by Store root so that each Store's own
//! rows and edges were validated at least once.
use layrs_core::scm::{Tree, TreeId};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

/// Bound on the total number of entries of cached Trees (about 150 B each).
const MAX_ENTRIES: usize = 400_000;

type Key = (PathBuf, TreeId);

#[derive(Default)]
struct Cache {
    trees: HashMap<Key, Arc<Tree>>,
    order: VecDeque<Key>,
    entries: usize,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

pub(super) fn get(root: &Path, id: &TreeId) -> Option<Arc<Tree>> {
    let cache = cache().lock().ok()?;
    cache.trees.get(&(root.to_path_buf(), id.clone())).cloned()
}

pub(super) fn insert(root: &Path, tree: &Tree) {
    let size = tree.entries().len().max(1);
    if size > MAX_ENTRIES {
        return;
    }
    let Ok(mut cache) = cache().lock() else {
        return;
    };
    let key = (root.to_path_buf(), tree.id().clone());
    if cache.trees.contains_key(&key) {
        return;
    }
    while cache.entries + size > MAX_ENTRIES {
        let Some(oldest) = cache.order.pop_front() else {
            break;
        };
        if let Some(evicted) = cache.trees.remove(&oldest) {
            cache.entries -= evicted.entries().len().max(1);
        }
    }
    cache.entries += size;
    cache.order.push_back(key.clone());
    cache.trees.insert(key, Arc::new(tree.clone()));
}
