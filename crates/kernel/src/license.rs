//! LicenseService port (design 01 §3, baseline §12.4/§13.8).
//!
//! Single-user builds inject `AlwaysAllowLicense`; business builds inject the
//! full implementation; Web injects a quota implementation. Kernel execution
//! code must only use this trait, never inspect license data itself.

use crate::error::KernelError;

pub type OperationKind = &'static str;
pub const C09: OperationKind = "c09";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capability {
    Allow,
    Deny { reason: String },
    Quota { used: u32, limit: u32 },
}

impl Capability {
    /// 是否已超额（供界面置灰等**显示**用途；判定仍在服务端）。
    pub fn is_quota_exhausted(&self) -> bool {
        matches!(self, Capability::Quota { .. })
    }
}

pub trait LicenseService: Send + Sync {
    /// `op` 取 `PlanDocument.kind`（功能编号，如 `"c05"`）；执行层逐条复核（§8.5）。
    fn capability(&self, op: &str) -> Capability;
    fn consume(&self, op: &str, n: u32) -> Result<(), KernelError>;
    fn release(&self, op: &str, n: u32);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AlwaysAllowLicense;

impl LicenseService for AlwaysAllowLicense {
    fn capability(&self, _op: &str) -> Capability {
        Capability::Allow
    }

    fn consume(&self, _op: &str, _n: u32) -> Result<(), KernelError> {
        Ok(())
    }

    fn release(&self, _op: &str, _n: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_allow_never_blocks_and_never_counts() {
        let license = AlwaysAllowLicense;
        assert_eq!(license.capability(C09), Capability::Allow);
        assert!(license.consume(C09, 1).is_ok());
        license.release(C09, 1);
    }
}

/// **Web 形态的配额实现**（ADR-0101，2026-11-18 产品裁决）。
///
/// 口径（人类裁决，非 AI 自定）：
/// - **计量单位**：按操作次数（每条 1 次）；
/// - **免费额度**：每个设备 **10 次**；
/// - **超额**：返回 `Capability::Quota`，由**服务端拦截**（设计 02 §4 的 403/429）；
/// - **归属**：按**设备**——**登录不重置**（设备级计数，账号只用于识别，不改写配额）；
/// - 订阅（商业授权）走 `Deny` 之外的另一条路：本实现只表达**免费配额**，
///   订阅状态由服务端在更大系统中裁决，不在此结构内。
///
/// 记账：本结构是**服务端/测试的进程内镜像**（`Mutex<HashMap>`）；
/// 真实服务端需自行持久化（设计 07），但**同一 trait、同一口径**。
pub const QUOTA_FREE_LIMIT: u32 = 10;

pub struct QuotaBasedLicense {
    /// 设备标识 → 已用次数。设备级＝登录不重置。
    used: std::sync::Mutex<std::collections::HashMap<String, u32>>,
    /// 单设备额度（默认 [`QUOTA_FREE_LIMIT`]）。
    limit: u32,
}

impl Default for QuotaBasedLicense {
    fn default() -> Self {
        Self::with_limit(QUOTA_FREE_LIMIT)
    }
}

impl QuotaBasedLicense {
    pub fn with_limit(limit: u32) -> Self {
        Self {
            used: std::sync::Mutex::new(std::collections::HashMap::new()),
            limit,
        }
    }

    /// 指定设备已用次数（只读，供服务端下发 `quota:{used,limit}` 显示数据）。
    pub fn used_of(&self, device: &str) -> u32 {
        self.used
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(device)
            .copied()
            .unwrap_or(0)
    }
}

impl LicenseService for QuotaBasedLicense {
    fn capability(&self, _op: &str) -> Capability {
        // 逐条复核时由调用方传设备（见 `capability_for`）；此处默认设备 `"default"`。
        self.capability_for("default")
    }

    fn consume(&self, _op: &str, _n: u32) -> Result<(), KernelError> {
        self.consume_for("default", _n)
    }

    fn release(&self, _op: &str, _n: u32) {
        self.release_for("default", _n)
    }
}

impl QuotaBasedLicense {
    /// 指定设备的配额判定：未超额 `Allow`；已超额 `Quota`（服务端据此拦截）。
    pub fn capability_for(&self, device: &str) -> Capability {
        let used = self.used_of(device);
        if used >= self.limit {
            Capability::Quota {
                used,
                limit: self.limit,
            }
        } else {
            Capability::Allow
        }
    }

    /// 记账一次操作；超额返回 E4004（与桌面同一码位）。
    pub fn consume_for(&self, device: &str, n: u32) -> Result<(), KernelError> {
        let mut used = self
            .used
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = used.entry(device.to_string()).or_insert(0);
        if *entry + n > self.limit {
            return Err(KernelError::new(
                "E4004",
                format!("quota exhausted ({}/{})", *entry, self.limit),
                None,
                false,
            ));
        }
        *entry += n;
        Ok(())
    }

    /// 退还（撤销时）；**不因登录/登出重置**。
    pub fn release_for(&self, device: &str, n: u32) {
        let mut used = self
            .used
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = used.get_mut(device) {
            *entry = entry.saturating_sub(n);
        }
    }
}

#[cfg(test)]
mod quota_tests {
    use super::*;

    /// 口径一：按操作次数，免费额度 10 次。
    #[test]
    fn free_limit_is_ten_operations() {
        let license = QuotaBasedLicense::default();
        for _ in 0..QUOTA_FREE_LIMIT {
            assert_eq!(license.capability_for("dev-a"), Capability::Allow);
            license.consume_for("dev-a", 1).expect("额度内可消费");
        }
        assert_eq!(license.used_of("dev-a"), 10);
        assert_eq!(
            license.capability_for("dev-a"),
            Capability::Quota {
                used: 10,
                limit: 10
            }
        );
        assert_eq!(license.capability_for("dev-a").is_quota_exhausted(), true);
    }

    /// 口径二：超额＝服务端拦截（E4004），且不产生记账副作用。
    #[test]
    fn exceeding_returns_e4004_without_consuming() {
        let license = QuotaBasedLicense::default();
        for _ in 0..10 {
            license.consume_for("dev-b", 1).unwrap();
        }
        let error = license.consume_for("dev-b", 1).unwrap_err();
        assert_eq!(error.code, "E4004");
        assert_eq!(license.used_of("dev-b"), 10, "超额不得继续累加");
    }

    /// 口径三：按设备归属——不同设备互不侵占；登录/登出不重置。
    #[test]
    fn quota_is_per_device_and_login_does_not_reset() {
        let license = QuotaBasedLicense::default();
        for _ in 0..10 {
            license.consume_for("device-1", 1).unwrap();
        }
        // 设备 2 未受影响
        assert_eq!(license.capability_for("device-2"), Capability::Allow);
        // 「重新登录」＝同一设备再来一次，计数仍在（登录不重置）
        assert_eq!(
            license.capability_for("device-1"),
            Capability::Quota {
                used: 10,
                limit: 10
            }
        );
    }

    /// 口径四：撤销退还次数（但不低于 0）。
    #[test]
    fn release_returns_quota() {
        let license = QuotaBasedLicense::default();
        license.consume_for("dev-c", 3).unwrap();
        license.release_for("dev-c", 2);
        assert_eq!(license.used_of("dev-c"), 1);
        license.release_for("dev-c", 99);
        assert_eq!(license.used_of("dev-c"), 0, "不得下溢");
    }

    /// 实现走 `LicenseService` trait（同一接口，只换实现——不变量 5）。
    #[test]
    fn works_through_the_trait_object() {
        let license: std::sync::Arc<dyn LicenseService> =
            std::sync::Arc::new(QuotaBasedLicense::default());
        assert_eq!(license.capability("c09"), Capability::Allow);
        license.consume("c09", 1).unwrap();
        license.release("c09", 1);
    }
}
