use async_trait::async_trait;

use crate::{context::ModuleContext, error::RuntimeError, metadata::ModuleMetadata};

#[async_trait]
pub trait Module: Send + Sync {
    fn metadata(&self) -> &'static ModuleMetadata;

    async fn start(&self, context: ModuleContext) -> Result<(), RuntimeError>;

    async fn stop(&self) -> Result<(), RuntimeError>;
}
