//! batchdesk 领域引擎骨架。
//!
//! 本 crate 只承载纯函数变换器与执行编排类型；禁止磁盘 I/O、时钟与环境读取
//! （AGENTS.md 不变量 1）。文件系统、授权与进度上报一律经 trait 端口注入。

#![forbid(unsafe_code)]

pub mod apply;
pub mod config;
pub mod conflict;
pub mod csv;
pub mod duplicate;
pub mod encoding;
pub mod error;
pub mod fs;
pub mod generic_apply;
pub mod journal;
pub mod license;
pub mod merge;
pub mod path;
pub mod plan;
pub mod preflight;
pub mod progress;
pub mod recipe;
pub mod split;
pub mod ui_state;

/// 引擎版本，供 IPC 握手与诊断使用（契约见 docs/design/01-内核接口与IPC契约.md §9 第 1 条）。
pub const KERNEL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 骨架自检：确认 kernel 可在无宿主环境下独立编译与测试。
pub fn is_ready() -> bool {
    !KERNEL_VERSION.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_version_is_not_empty() {
        assert!(is_ready());
    }
}
