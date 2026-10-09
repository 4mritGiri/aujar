mod error;
mod executor;
mod model;
mod planner;

pub use error::RenameError;
pub use model::{RenameItem, RenameItemStatus, RenameOptions, RenamePlan};
pub use planner::RenamePlanner;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_dir() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("aujar-rename-test-{stamp}"))
    }

    #[test]
    fn literal_preview() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("old_report.txt");
        fs::write(&source, "data").unwrap();

        let planner = RenamePlanner::new(RenameOptions {
            pattern: "old".into(),
            replacement: "new".into(),
            regex: false,
            apply: false,
        });

        let plan = planner
            .plan(&[source.to_string_lossy().into_owned()])
            .unwrap();
        assert_eq!(plan.items[0].target, dir.join("new_report.txt"));
        assert!(!plan.has_errors());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn regex_preview() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("photo.jpeg");
        fs::write(&source, "data").unwrap();

        let planner = RenamePlanner::new(RenameOptions {
            pattern: r"^(.+)\.jpeg$".into(),
            replacement: "${1}.jpg".into(),
            regex: true,
            apply: false,
        });

        let plan = planner
            .plan(&[source.to_string_lossy().into_owned()])
            .unwrap();
        assert_eq!(plan.items[0].target, dir.join("photo.jpg"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn detects_missing_source() {
        let planner = RenamePlanner::new(RenameOptions {
            pattern: "old".into(),
            replacement: "new".into(),
            regex: false,
            apply: false,
        });

        let plan = planner
            .plan(&["/definitely/not/a/real/aujar-file.txt".into()])
            .unwrap();

        assert!(plan.has_errors());
        assert_eq!(plan.items[0].status, RenameItemStatus::MissingSource);
    }

    #[test]
    fn detects_collision_between_inputs() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let planner = RenamePlanner::new(RenameOptions {
            pattern: "[ab]".into(),
            replacement: "x".into(),
            regex: true,
            apply: false,
        });

        let plan = planner
            .plan(&[
                a.to_string_lossy().into_owned(),
                b.to_string_lossy().into_owned(),
            ])
            .unwrap();

        assert!(plan.has_errors());
        assert!(
            plan.items
                .iter()
                .all(|item| item.status == RenameItemStatus::DuplicateTarget)
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn executes_successfully() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("old.txt");
        let target = dir.join("new.txt");
        fs::write(&source, "data").unwrap();

        let planner = RenamePlanner::new(RenameOptions {
            pattern: "old".into(),
            replacement: "new".into(),
            regex: false,
            apply: true,
        });

        let plan = planner
            .plan(&[source.to_string_lossy().into_owned()])
            .unwrap();
        plan.execute().unwrap();

        assert!(!source.exists());
        assert_eq!(fs::read_to_string(target).unwrap(), "data");

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_path_traversal_in_replacement() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("old.txt");
        fs::write(&source, "data").unwrap();

        let planner = RenamePlanner::new(RenameOptions {
            pattern: "old".into(),
            replacement: "../escaped".into(),
            regex: false,
            apply: false,
        });

        let plan = planner
            .plan(&[source.to_string_lossy().into_owned()])
            .unwrap();

        assert_eq!(plan.items[0].status, RenameItemStatus::InvalidTarget);
        assert!(plan.has_errors());

        fs::remove_dir_all(dir).unwrap();
    }
}
