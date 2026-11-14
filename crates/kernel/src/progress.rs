//! Progress and cancellation ports (design 01 §2.2/§3).
//!
//! The kernel reports phase progress through `ProgressSink` and checks a
//! `CancelToken` at entry boundaries. Neither port is allowed to perform
//! domain work; the shell owns throttling, queue pause and event delivery.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProgressPhase {
    Queued,
    Scanning,
    Planning,
    Executing,
    Journaling,
    Finalizing,
}

impl ProgressPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Scanning => "scanning",
            Self::Planning => "planning",
            Self::Executing => "executing",
            Self::Journaling => "journaling",
            Self::Finalizing => "finalizing",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub task_id: String,
    pub phase: ProgressPhase,
    /// 任务级进度：**条目数**（已完成的条目／总条目）。
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// 条目内进度（字节）：当前条目已处理／当前条目总字节（ADR-0100）。
    ///
    /// 方案 §7.5 的「双层进度」要求"当前条目进度（大文件可见）"——N:1 的合并
    /// （F04）整条任务只有一条 entry，不报字节进度的话界面会长时间停在 0/1。
    /// `None` 表示这是**条目边界**事件，壳层据此只更新条目计数并清掉上一段的字节进度；
    /// `Some` 表示这是条目内事件，壳层只更新字节字段、不动 `done/total`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_done: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_total: Option<u64>,
}

pub trait ProgressSink: Send + Sync {
    fn report(&self, event: ProgressEvent);
}

pub trait CancelToken: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

/// 永不取消的空实现：调用方没有取消端口时使用（如 CLI 的 `apply`）。
#[derive(Clone, Copy, Debug, Default)]
pub struct NoCancel;

impl CancelToken for NoCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}
