use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("module `{0}` is already registered")]
    ModuleAlreadyRegistered(String),

    #[error("module `{0}` is not registered")]
    ModuleNotFound(String),

    #[error("module `{0}` has incompatible API version {1}")]
    IncompatibleModuleApi(String, u16),

    #[error("module `{0}` failed to start: {1}")]
    ModuleStartFailed(String, String),

    #[error("module `{0}` failed to stop: {1}")]
    ModuleStopFailed(String, String),
}
