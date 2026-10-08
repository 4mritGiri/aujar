use crate::{
    error::RenameError,
    model::{RenameItemStatus, RenamePlan},
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn execute(plan: &RenamePlan) -> Result<(), RenameError> {
    if plan.has_errors() {
        return Err(RenameError::ExecutionBlocked);
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    let mut staged = Vec::new();

    /*
     * Stage every rename into a temporary file first.
     *
     * This allows operations such as:
     *
     *     a.txt -> b.txt
     *     b.txt -> a.txt
     *
     * without overwriting either source during the first phase.
     */
    for (index, item) in plan.items.iter().enumerate() {
        if item.status != RenameItemStatus::Ready {
            continue;
        }

        let temporary = temporary_path(&item.source, nonce, index);

        fs::rename(&item.source, &temporary).map_err(|source| RenameError::Rename {
            from: item.source.clone(),
            to: temporary.clone(),
            source,
        })?;

        staged.push((temporary, item.source.clone(), item.target.clone()));
    }

    /*
     * Commit the staged renames.
     */
    for (temporary, source, target) in staged {
        if let Err(error) = fs::rename(&temporary, &target) {
            /*
             * Best-effort rollback to the original source.
             *
             * We deliberately do not hide the original failure if rollback
             * also fails.
             */
            let _ = fs::rename(&temporary, restore_path(&source));

            return Err(RenameError::Rename {
                from: source,
                to: target,
                source: error,
            });
        }
    }

    Ok(())
}

fn temporary_path(source: &Path, nonce: u128, index: usize) -> PathBuf {
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("item");

    source
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!(".nexora-rename-{nonce}-{index}-{file_name}"))
}

fn restore_path(source: &Path) -> PathBuf {
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("item");

    source
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!(".nexora-restore-{file_name}"))
}
