//! Errors shared by the Lua mod store and its callers.

use std::fmt;

/// Failure while reading/writing the `mods/` directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModError {
    /// Filesystem failure (message already contains the path).
    Io(String),
    /// The mod/configuration does not exist.
    NotFound(String),
    /// Validation failure (bad manifest, path, name, ...).
    Invalid(String),
    /// Optimistic-concurrency mismatch.
    Conflict(String),
    /// Script or body above the configured limit.
    TooLarge(String),
}

impl ModError {
    pub fn io(message: impl Into<String>) -> ModError {
        ModError::Io(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> ModError {
        ModError::NotFound(message.into())
    }

    pub fn invalid(message: impl Into<String>) -> ModError {
        ModError::Invalid(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> ModError {
        ModError::Conflict(message.into())
    }

    pub fn too_large(message: impl Into<String>) -> ModError {
        ModError::TooLarge(message.into())
    }

    pub fn message(&self) -> &str {
        match self {
            ModError::Io(message)
            | ModError::NotFound(message)
            | ModError::Invalid(message)
            | ModError::Conflict(message)
            | ModError::TooLarge(message) => message,
        }
    }
}

impl fmt::Display for ModError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ModError {}
