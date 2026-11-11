//! Shared metadata-list vocabulary; not a second session store.
use serde::{Deserialize, Serialize};

pub const DEFAULT_SESSION_PAGE_SIZE: usize = 25;
pub const MAX_SESSION_PAGE_SIZE: usize = 100;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSortKey {
    CreatedAt,
    #[default]
    UpdatedAt,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSortDirection {
    Asc,
    #[default]
    Desc,
}
