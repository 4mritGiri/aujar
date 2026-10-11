//! `> command` runs a shell command (PowerToys Run's shell plugin). Opt-in.

use crate::model::{LaunchSpec, Provider, ResultKind, SearchResult};

pub struct ShellProvider;

impl Provider for ShellProvider {
    fn id(&self) -> &'static str {
        "shell"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let Some(command) = query.trim().strip_prefix('>') else {
            return Vec::new();
        };

        let command = command.trim();

        if command.is_empty() {
            return Vec::new();
        }

        vec![SearchResult {
            id: format!("shell:{command}"),
            title: format!("Run: {command}"),
            subtitle: Some("Runs in a shell (sh -c); output is not shown".to_owned()),
            kind: ResultKind::Command,
            score: 1800,
            icon: None,
        }]
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let command = id.strip_prefix("shell:")?;

        Some(Ok(LaunchSpec {
            title: command.to_owned(),
            program: "sh".to_owned(),
            args: vec!["-c".to_owned(), command.to_owned()],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_the_prefix_and_a_command() {
        assert!(ShellProvider.search("ls").is_empty());
        assert!(ShellProvider.search(">").is_empty());
        assert_eq!(ShellProvider.search("> ls -la")[0].id, "shell:ls -la");
    }

    #[test]
    fn resolves_to_sh_dash_c() {
        let spec = ShellProvider.resolve("shell:echo hi").unwrap().unwrap();

        assert_eq!(spec.program, "sh");
        assert_eq!(spec.args, ["-c", "echo hi"]);
    }
}
