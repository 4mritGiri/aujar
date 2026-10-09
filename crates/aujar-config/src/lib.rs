use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_socket_path")]
    pub socket_path: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_socket_path() -> String {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());

    format!("{base}/aujar.sock")
}

fn default_log_level() -> String {
    "info".into()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            socket_path: default_socket_path(),
            log_level: default_log_level(),
        }
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let content = fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }
}

/// Administrator policy, normally `/etc/aujar/policy.toml`.
///
/// ```toml
/// denied_capabilities = ["execute_command"]
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    pub denied_capabilities: Vec<String>,
}

impl Policy {
    /// Load a policy file. A missing file means "no restrictions"; an
    /// unreadable or invalid file is an error (fail closed).
    pub fn load_optional(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        match fs::read_to_string(path.as_ref()) {
            Ok(content) => Ok(toml::from_str(&content)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Policy;

    #[test]
    fn parses_policy() {
        let policy: Policy = toml::from_str("denied_capabilities = [\"execute_command\"]").unwrap();

        assert_eq!(policy.denied_capabilities, ["execute_command"]);
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Policy>("denyed_capabilities = []").is_err());
    }

    #[test]
    fn missing_file_means_no_restrictions() {
        let policy = Policy::load_optional("/definitely/not/a/real/aujar-policy.toml").unwrap();

        assert_eq!(policy, Policy::default());
    }
}
