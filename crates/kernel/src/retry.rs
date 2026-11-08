//! 网络断线的有限重试与自动暂停端口（D1）。
//!
//! 基线《办公辅助工具产品方案》§4.9：断线采用「有限重试＋指数退避」，最多
//! 3 次、退避 2/4/8 秒，超过阈值则任务自动暂停并提示（不直接失败）。
//! kernel 不读时钟、不 sleep；等待与暂停由注入的 `NetworkRetry` 实现（桌面壳
//! 用它切换任务状态并阻塞到用户继续，CLI 只等待后退回失败）。

use std::time::Duration;

use crate::error::KernelError;
use crate::plan::PlanAction;

/// §4.9 的重试次数（不含首次尝试）。
pub const NETWORK_RETRY_ATTEMPTS: u32 = 3;
/// §4.9 的退避序列：第 1/2/3 次重试前分别等待 2/4/8 秒。
pub const NETWORK_RETRY_BACKOFF_SECS: [u64; NETWORK_RETRY_ATTEMPTS as usize] = [2, 4, 8];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseAction {
    /// 已恢复（网络回来/用户点继续），继续重试。
    Resumed,
    /// 用户取消：把本次错误作为条目失败上抛，由取消流程收尾。
    Cancel,
    /// 宿主没有暂停交互面（CLI）：直接中止整个任务，不再逐条重试。
    Abort,
}

pub trait NetworkRetry: Send + Sync {
    /// 本次退避等待；返回 `false` 表示已取消，调用方应立即把错误上抛。
    fn wait(&self, delay: Duration) -> bool;
    /// 重试次数耗尽：切到暂停并阻塞到用户继续。返回 `false` 表示已取消。
    fn pause(&self, error: &KernelError) -> PauseAction;
}

/// 重试失败：`abort` 为真时调用方应中止整个任务（CLI 无暂停交互面），不是条目失败。
pub struct RetryFailure {
    pub error: KernelError,
    pub abort: bool,
}

/// 对单个动作应用 D1 重试：E2005 最多重 3 次（2/4/8 秒），耗尽后暂停等待恢复；
/// 恢复后重试计数清零（网络再次断开会再次暂停，而不是静默失败）。
pub fn with_network_retry<T>(
    retry: Option<&dyn NetworkRetry>,
    network: bool,
    mut op: impl FnMut() -> Result<T, KernelError>,
) -> Result<T, RetryFailure> {
    let Some(retry) = retry else {
        return op().map_err(|error| RetryFailure {
            error,
            abort: false,
        });
    };
    if !network {
        return op().map_err(|error| RetryFailure {
            error,
            abort: false,
        });
    }
    let mut attempts = 0u32;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(error) if error.code == "E2005" => {
                if attempts < NETWORK_RETRY_ATTEMPTS {
                    let delay = Duration::from_secs(NETWORK_RETRY_BACKOFF_SECS[attempts as usize]);
                    if retry.wait(delay) {
                        attempts += 1;
                        continue;
                    }
                } else {
                    match retry.pause(&error) {
                        PauseAction::Resumed => {
                            attempts = 0;
                            continue;
                        }
                        PauseAction::Cancel => {
                            return Err(RetryFailure {
                                error,
                                abort: false,
                            })
                        }
                        PauseAction::Abort => return Err(RetryFailure { error, abort: true }),
                    }
                }
                return Err(RetryFailure {
                    error,
                    abort: false,
                });
            }
            Err(error) => {
                return Err(RetryFailure {
                    error,
                    abort: false,
                })
            }
        }
    }
}

/// UNC/网络路径判定（纯字符串，不做 I/O）。`\\server\share`、`//server/share`
/// 与 `\\?\UNC\server\share` 都算；盘符映射的网络盘无法从路径字符串识别，
/// 只按 UNC 收口（与 R05 的实测场景一致）。
pub fn is_network_path(path: &str) -> bool {
    let trimmed = path.trim_start();
    trimmed.starts_with(r"\\") || trimmed.starts_with("//")
}

/// 条目是否触及网络路径（源或目标任意一侧）。
pub fn entry_touches_network(entry: &crate::plan::PlanEntry) -> bool {
    is_network_path(&entry.source)
        || entry
            .target
            .as_deref()
            .map(is_network_path)
            .unwrap_or(false)
}

/// 可重试动作：内容是原子写/改名/移动/删除，失败不产生半成品；`edit-content`
/// 与 `transcode` 先备份再重写，重试会重复落备份，暂不纳入（记在设计 04）。
pub fn action_can_retry_network(action: PlanAction) -> bool {
    !matches!(action, PlanAction::EditContent | PlanAction::Transcode)
}
