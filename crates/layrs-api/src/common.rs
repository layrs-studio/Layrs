use crate::ids::PrincipalId;
use crate::validation::{ApiResult, Validate, optional_positive};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestContext {
    pub principal_id: PrincipalId,
    pub request_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageRequest {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

impl Validate for PageRequest {
    fn validate(&self) -> ApiResult<()> {
        optional_positive("limit", self.limit)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageResponse<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PermissionMode {
    Read,
    Write,
    Admin,
    Owner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SortOrder {
    Asc,
    Desc,
}
