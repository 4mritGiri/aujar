use aujar_core::Capability;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
}

pub trait Module {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
}
