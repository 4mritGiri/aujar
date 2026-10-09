mod context;
mod error;
mod metadata;
mod module;
mod registry;

pub use aujar_core::Capability;
pub use context::ModuleContext;
pub use error::RuntimeError;
pub use metadata::{MODULE_API_VERSION, ModuleMetadata};
pub use module::Module;
pub use registry::ModuleRegistry;
