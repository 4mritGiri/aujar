use async_trait::async_trait;
use nexora_runtime::{Capability, Module, ModuleContext, ModuleMetadata, RuntimeError};

const CORE_CAPABILITIES: &[Capability] = &[];

static CORE_METADATA: ModuleMetadata = ModuleMetadata::new(
    "core",
    "Nexora Core",
    env!("CARGO_PKG_VERSION"),
    "Core Nexora runtime services.",
    CORE_CAPABILITIES,
);

pub struct CoreModule;

#[async_trait]
impl Module for CoreModule {
    fn metadata(&self) -> &'static ModuleMetadata {
        &CORE_METADATA
    }

    async fn start(&self, context: ModuleContext) -> Result<(), RuntimeError> {
        tracing::info!(
            runtime_version = context.runtime_version,
            module = CORE_METADATA.id,
            "Nexora core module started"
        );

        Ok(())
    }

    async fn stop(&self) -> Result<(), RuntimeError> {
        tracing::info!(module = CORE_METADATA.id, "Nexora core module stopped");

        Ok(())
    }
}
