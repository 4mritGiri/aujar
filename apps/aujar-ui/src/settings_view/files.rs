//! Files: the folders the launcher indexes and how deep it looks.

use super::SettingsView;
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px, rgb};

impl SettingsView {
    pub(super) fn files_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut roots: Vec<AnyElement> = self
            .settings
            .files
            .roots
            .iter()
            .map(|root| self.row(root, "File names under this folder are searchable", div()))
            .collect();

        roots.push(self.row(
            "Add or remove folders",
            "Edit `roots` under [files] in the config file (General page).",
            div(),
        ));

        vec![
            self.section("Indexed folders", roots),
            self.section(
                "Index",
                vec![
                    toggle_row!(
                        self,
                        cx,
                        "files-on",
                        "Files & folders",
                        "Search file names in the folders above",
                        plugins.files
                    ),
                    stepper_row!(
                        self,
                        cx,
                        "depth",
                        "Folder depth",
                        "How many levels below each folder to look. Deeper finds more but indexes slower",
                        1,
                        "",
                        files.max_depth
                    ),
                ],
            ),
        ]
    }
}
