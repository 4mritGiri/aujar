use crate::Capability;

pub const MODULE_API_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleMetadata {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub api_version: u16,
    pub description: &'static str,
    pub capabilities: &'static [Capability],
}

impl ModuleMetadata {
    pub const fn new(
        id: &'static str,
        name: &'static str,
        version: &'static str,
        description: &'static str,
        capabilities: &'static [Capability],
    ) -> Self {
        Self {
            id,
            name,
            version,
            api_version: MODULE_API_VERSION,
            description,
            capabilities,
        }
    }
}
