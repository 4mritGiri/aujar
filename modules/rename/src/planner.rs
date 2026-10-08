use crate::{
    error::RenameError,
    model::{RenameItem, RenameItemStatus, RenameOptions, RenamePlan},
};
use regex::Regex;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

pub struct RenamePlanner {
    options: RenameOptions,
}

impl RenamePlanner {
    pub fn new(options: RenameOptions) -> Self {
        Self { options }
    }

    pub fn plan(&self, paths: &[String]) -> Result<RenamePlan, RenameError> {
        let regex = if self.options.regex {
            Some(Regex::new(&self.options.pattern)?)
        } else {
            None
        };

        let mut items = Vec::with_capacity(paths.len());

        for raw in paths {
            let source = PathBuf::from(raw);

            if !source.exists() {
                items.push(RenameItem {
                    source: source.clone(),
                    target: source,
                    status: RenameItemStatus::MissingSource,
                });

                continue;
            }

            let file_name = source
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();

            let new_name = match regex.as_ref() {
                Some(regex) => regex
                    .replace(file_name, self.options.replacement.as_str())
                    .into_owned(),

                None => file_name.replace(&self.options.pattern, &self.options.replacement),
            };

            let target = source
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(new_name);

            let status = if target == source {
                RenameItemStatus::Unchanged
            } else if target.exists() {
                RenameItemStatus::Collision
            } else {
                RenameItemStatus::Ready
            };

            items.push(RenameItem {
                source,
                target,
                status,
            });
        }

        let mut counts: HashMap<&PathBuf, usize> = HashMap::new();

        for item in &items {
            if item.status != RenameItemStatus::MissingSource {
                *counts.entry(&item.target).or_default() += 1;
            }
        }

        let duplicates: HashSet<PathBuf> = counts
            .into_iter()
            .filter_map(|(path, count)| (count > 1).then_some(path.clone()))
            .collect();

        for item in &mut items {
            if duplicates.contains(&item.target) && item.status == RenameItemStatus::Ready {
                item.status = RenameItemStatus::DuplicateTarget;
            }
        }

        Ok(RenamePlan { items })
    }
}
