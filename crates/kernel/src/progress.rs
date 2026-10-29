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
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub trait ProgressSink: Send + Sync {
    fn report(&self, event: ProgressEvent);
}

pub trait CancelToken: Send + Sync {
    fn is_cancelled(&self) -> bool;
}
