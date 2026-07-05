//! Hand-rolled core error type. House style (AGENTS.md): `thiserror` is in the
//! workspace manifest but every existing crate uses hand-rolled errors; core
//! stays consistent rather than split styles mid-push.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    pub code: String,
    pub message: String,
}

impl CoreError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CoreError {}

pub type Result<T> = std::result::Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_carries_code_and_message() {
        let error = CoreError::new("invalid_node_id", "node value was empty");
        assert_eq!(error.code, "invalid_node_id");
        assert_eq!(error.message, "node value was empty");
        assert!(format!("{error}").contains("invalid_node_id"));
    }
}
