use std::fmt;

use kai_domain::container::ActionNotAllowed;
use kai_domain::{ContainerAction, ContainerId, RuntimeError};

/// Why a use case did not complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppError {
    /// A destructive action was asked for without explicit confirmation.
    ConfirmationRequired {
        action: ContainerAction,
        name: String,
    },
    /// The container does not exist (any more).
    NotFound(ContainerId),
    /// The container's state does not allow the action.
    NotAllowed(ActionNotAllowed),
    /// The engine failed or refused.
    Runtime(RuntimeError),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfirmationRequired { action, name } => {
                write!(f, "{action} {name} needs confirmation")
            }
            Self::NotFound(id) => write!(f, "no container {id}"),
            Self::NotAllowed(e) => e.fmt(f),
            Self::Runtime(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for AppError {}

impl From<RuntimeError> for AppError {
    fn from(e: RuntimeError) -> Self {
        Self::Runtime(e)
    }
}
