#[derive(Debug, Clone)]
pub struct ModuleContext {
    pub runtime_version: &'static str,
}

impl ModuleContext {
    pub const fn new(runtime_version: &'static str) -> Self {
        Self { runtime_version }
    }
}
