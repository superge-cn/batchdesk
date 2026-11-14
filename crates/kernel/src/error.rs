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

    /// 用户取消（E3010）。**取消不是失败**——执行器据此把整单标成 `Cancelled`
    /// 并停止派发，而不是给条目记一条 failed。
    ///
    /// 存在的理由：流式内容重写（C02/C03/C01）跑在 `rewrite_atomic_with` 的回调里，
    /// 要中途放弃必须让回调返回 `Err` 才能丢掉临时文件（否则半截内容会被原子改名
    /// 盖到源文件上）。取消与真正的写回失败因此必须可区分。
    pub fn cancelled() -> Self {
        Self::new("E3010", "cancelled by user", None, false)
    }

    pub fn is_cancelled(&self) -> bool {
        self.code == "E3010"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_use_registered_codes() {
        assert_eq!(KernelError::missing_param("x").code, "E1001");
        assert_eq!(KernelError::invalid_params("x").code, "E1002");
        assert!(KernelError::cancelled().is_cancelled());
        assert!(!KernelError::invalid_params("x").is_cancelled());
        let error = KernelError::internal("x", "detail");
        assert_eq!(error.code, "E9001");
        assert_eq!(error.detail.as_deref(), Some("detail"));
        assert!(!error.retryable);
    }
}
