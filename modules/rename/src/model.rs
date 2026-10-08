use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameOptions {
    pub pattern: String,
    pub replacement: String,
    pub regex: bool,
    pub apply: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenameItemStatus {
    Ready,
    MissingSource,
    Unchanged,
    Collision,
    DuplicateTarget,
    InvalidTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameItem {
    pub source: PathBuf,
    pub target: PathBuf,
    pub status: RenameItemStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenamePlan {
    pub items: Vec<RenameItem>,
}

impl RenamePlan {
    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|item| {
            matches!(
                item.status,
                RenameItemStatus::MissingSource
                    | RenameItemStatus::Collision
                    | RenameItemStatus::DuplicateTarget
                    | RenameItemStatus::InvalidTarget
            )
        })
    }

    pub fn summary(&self) -> String {
        let total = self.items.len();

        let ready = self
            .items
            .iter()
            .filter(|item| item.status == RenameItemStatus::Ready)
            .count();

        let unchanged = self
            .items
            .iter()
            .filter(|item| item.status == RenameItemStatus::Unchanged)
            .count();

        let errors = self
            .items
            .iter()
            .filter(|item| {
                matches!(
                    item.status,
                    RenameItemStatus::MissingSource
                        | RenameItemStatus::Collision
                        | RenameItemStatus::DuplicateTarget
                        | RenameItemStatus::InvalidTarget
                )
            })
            .count();

        format!("{total} item(s): {ready} ready, {unchanged} unchanged, {errors} blocked")
    }

    pub fn execute(&self) -> Result<(), crate::RenameError> {
        crate::executor::execute(self)
    }
}
