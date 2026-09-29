//! Server navigation contracts: the authorized Space catalog and per-Space UI
//! capabilities. Both are hints for presentation; the server rechecks every
//! action and a cached answer never proves access.
use super::*;

/// Bounds mirrored from `layrs_store_server::v3` (catalog limit and sealed
/// cursor length); the server enforces them as well.
const CATALOG_MAX_LIMIT: u16 = 100;
const CATALOG_CURSOR_MAX_CHARS: usize = 2048;

impl NativeRemote {
    /// One page of the Spaces the signed-in principal may open.
    pub fn list_spaces(
        &self,
        limit: Option<u16>,
        cursor: Option<&str>,
    ) -> RemoteResult<serde_json::Value> {
        let mut query = Vec::new();
        if let Some(limit) = limit {
            if !(1..=CATALOG_MAX_LIMIT).contains(&limit) {
                return Err(NativeRemoteError::Invalid("invalid catalog page limit"));
            }
            query.push(format!("limit={limit}"));
        }
        if let Some(cursor) = cursor {
            if cursor.is_empty()
                || cursor.len() > CATALOG_CURSOR_MAX_CHARS
                || !cursor
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            {
                return Err(NativeRemoteError::Invalid("invalid catalog cursor"));
            }
            query.push(format!("cursor={cursor}"));
        }
        if query.is_empty() {
            self.get("v3/spaces")
        } else {
            self.get(&format!("v3/spaces?{}", query.join("&")))
        }
    }

    /// Capability hint for one Space. A Space the principal cannot open fails
    /// with service code `not_found`, exactly like a missing Space.
    pub fn space_capabilities(&self, space: &SpaceId) -> RemoteResult<serde_json::Value> {
        self.get(&format!("v3/spaces/{}/capabilities", space.as_str()))
    }
}
