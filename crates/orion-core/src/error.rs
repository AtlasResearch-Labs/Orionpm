use thiserror::Error;

/// All errors that can originate from orion-core.
#[derive(Debug, Error)]
pub enum OrionError {
    #[error("Config error: {0}")]
    Config(#[from] toml::de::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Process not found: {name}")]
    ProcessNotFound { name: String },

    #[error("Invalid state for operation '{op}': process '{name}' is currently {state}")]
    InvalidState {
        op: String,
        name: String,
        state: String,
    },

    #[error("Config validation failed:\n{}", errors.join("\n"))]
    ValidationFailed { errors: Vec<String> },
}
