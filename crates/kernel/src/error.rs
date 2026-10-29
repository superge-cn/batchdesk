//! Kernel error type aligned with the IPC contract (design 01 §2.1, design 06).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub retryable: bool,
}

impl KernelError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        detail: Option<String>,
        retryable: bool,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail,
            retryable,
        }
    }

    pub fn missing_param(message: impl Into<String>) -> Self {
        Self::new("E1001", message, None, false)
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new("E1002", message, None, false)
    }

    pub fn internal(message: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new("E9001", message, Some(detail.into()), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_use_registered_codes() {
        assert_eq!(KernelError::missing_param("x").code, "E1001");
        assert_eq!(KernelError::invalid_params("x").code, "E1002");
        let error = KernelError::internal("x", "detail");
        assert_eq!(error.code, "E9001");
        assert_eq!(error.detail.as_deref(), Some("detail"));
        assert!(!error.retryable);
    }
}
