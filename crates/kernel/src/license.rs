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
