use std::{collections::BTreeMap, sync::Arc};

use crate::{
    context::ModuleContext, error::RuntimeError, metadata::MODULE_API_VERSION, module::Module,
};

pub struct ModuleRegistry {
    modules: BTreeMap<&'static str, Arc<dyn Module>>,
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleRegistry {
    pub fn new() -> Self {
        Self {
            modules: BTreeMap::new(),
        }
    }

    pub fn register<M>(&mut self, module: M) -> Result<(), RuntimeError>
    where
        M: Module + 'static,
    {
        let metadata = module.metadata();

        if metadata.api_version != MODULE_API_VERSION {
            return Err(RuntimeError::IncompatibleModuleApi(
                metadata.id.to_owned(),
                metadata.api_version,
            ));
        }

        if self.modules.contains_key(metadata.id) {
            return Err(RuntimeError::ModuleAlreadyRegistered(
                metadata.id.to_owned(),
            ));
        }

        self.modules.insert(metadata.id, Arc::new(module));

        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Module>> {
        self.modules.get(id).cloned()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.modules.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn metadata(&self) -> Vec<&'static crate::ModuleMetadata> {
        self.modules
            .values()
            .map(|module| module.metadata())
            .collect()
    }

    pub async fn start_all(&self, context: ModuleContext) -> Result<(), RuntimeError> {
        for module in self.modules.values() {
            let metadata = module.metadata();

            module.start(context.clone()).await.map_err(|error| {
                RuntimeError::ModuleStartFailed(metadata.id.to_owned(), error.to_string())
            })?;
        }

        Ok(())
    }

    pub async fn stop_all(&self) -> Result<(), RuntimeError> {
        for module in self.modules.values().rev() {
            let metadata = module.metadata();

            module.stop().await.map_err(|error| {
                RuntimeError::ModuleStopFailed(metadata.id.to_owned(), error.to_string())
            })?;
        }

        Ok(())
    }
}
