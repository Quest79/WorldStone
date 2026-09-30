//! Shared, serializable models. Field names reach the frontend in camelCase.

use serde::Serialize;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Unknown,
    Ready,
    Missing,
    Warning,
    Error,
}

/// One row of the environment or dependency check.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: String,
    pub label: String,
    pub status: Status,
    pub detail: String,
}

impl Check {
    pub fn new(id: &str, label: &str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status,
            detail: detail.into(),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub root: String,
}
