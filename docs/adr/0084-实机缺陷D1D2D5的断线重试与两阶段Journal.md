# 0084. 实机缺陷 D1/D2/D5：断线有限重试与自动暂停、Windows 网络错误码映射、隔离移动与 Journal 两阶段

- **状态**：已采纳（代码、测试与文档同步落地；D3/D4 见 ADR-0083）
- **日期**：2026-11-08
- **决策者**：开发代理（工程实现决策；不改产品行为、错误码语义、数值阈值与线协议）
- **关联**：方案 §4.9（UNC 断线：最多 3 次重试、退避 2/4/8 秒、之后暂停任务）、§7.7（暂停/取消语义）、§7.8（Journal 与撤销）；[设计 01](../design/01-内核接口与IPC契约.md) §8（Journal 与撤销）、[设计 04](../design/04-文件系统适配层契约.md) §4.9/§5（错误码）、[设计 12](../design/12-前端与桌面壳架构.md) §S03/S01；`crates/kernel/src/retry.rs`（新增端口）、`crates/kernel/src/apply.rs`、`crates/kernel/src/generic_apply.rs`、`crates/kernel/src/journal.rs`、`crates/platform/src/fs.rs`、`crates/platform/src/journal.rs`、`src-tauri/src/tasks.rs`、`crates/cli/src/main.rs`；实机证据 `E:/batchdesk-shijiri/fix-d1d5/r05/`、`E:/batchdesk-shijiri/fix-d1d5/e04/`

---

## 1. 问题（同批三条实机缺陷）

### D1：UNC 断线没有有限重试、指数退避与自动暂停

2026-11-08 的 R05 实机（C07 复制 3000 条到 SMB 共享，t≈2.9s 停止 LanmanServer）显示：断线后 2263 条**逐条一次即败**，任务终态 `partial-failed`。方案 §4.9 要求「有限重试＋指数退避，超过阈值则任务自动暂停并提示（不直接失败）」，但执行器没有任何网络重试；D2 又把断线错误映射成不可重试的 E2008，重试判据根本等不到。

### D2：Windows 网络断线错误码落 E2008

同一 R05 断线实测 `os error 64`（`ERROR_NETNAME_DELETED`），`io_error` 的 `error.kind()` 落到 `Other`，默认分支把它判成 E2008（不可重试）。设计 06 的 E2005 语义是「网络路径断开（R05 语料），retryable=true」；错误码落错导致 D1 无法触发。

### D5：c09 隔离移动与 Journal 追加非原子，崩溃留孤儿

2026-11-08 的 S03 崩溃演练（368 条清理中强杀）暴露：c09 先 `remove_to_quarantine` 移动文件，再 `journal.append_with_kind`。崩溃落在两步之间时，隔离目录里有文件、Journal 里没有条目，撤销按 Journal 驱动**看不见**该文件（回滚 368 条全 reverted 但留 1 条孤儿）。

## 2. 决策

### D1：kernel 注入 `NetworkRetry` 端口，3 次/2-4-8 秒后退避，耗尽后由壳层暂停

1. 新增 `kernel::retry::NetworkRetry` 端口（`wait(delay) -> bool`、`pause(error) -> PauseAction`）与常量 `NETWORK_RETRY_ATTEMPTS = 3`、`NETWORK_RETRY_BACKOFF_SECS = [2, 4, 8]`（数值只引用方案 §4.9，不新增阈值）。
2. 通用执行器 `perform_with_retry` 在 `E2005` 时重试；c09 的隔离移动走 `retry::with_network_retry`。只对**触及 UNC/网络路径**的条目生效（`is_network_path`），且排除 `edit-content`/`transcode`（重试会重复落备份，见 §4）。
3. 次数耗尽后的三种宿主行为：
   - 桌面壳 `TaskNetworkRetry::pause`：`auto_pause` 把任务置 `paused` 并写原因，阻塞到用户继续；恢复后重试计数清零。
   - CLI `CliNetworkRetry::pause`：CLI 没有暂停交互面，返回 `Abort`，kernel 以 E2005 **中止整个任务**，不再逐条各等 14 秒（否则 2000+ 条会拖成小时级）。
   - 测试替身：`Resumed` 立即继续，用于证明重试次数/退避序列。
4. kernel 不 sleep、不读时钟；等待与暂停阻塞全部在注入实现里（不变量 1）。

### D2：Windows 网络错误码显式映射为 E2005

`io_error` 在默认分支前增加 Windows 网络错误识别：`raw_os_error` 为 **64 / 59 / 121 / 1236**（`ERROR_NETNAME_DELETED` / `ERROR_UNEXP_NET_ERR` / `ERROR_SEM_TIMEOUT` / `ERROR_CONNECTION_ABORTED`）时返回 `E2005`、`retryable=true`。R05 实测的 64 是主证据，其余三个同为 Windows 网络断线族；不改 E2008 的「改名失败」语义。

### D5：先写 Pending 意图，移动成功后再确认 Applied

1. `JournalStatus` 追加 `Pending`；`JournalSink` 追加 `append_pending_with_kind`、`mark_applied`、`discard_pending`（默认实现保持旧替身行为）。
2. c09 顺序改为：
   - `quarantine_target_path` 预留隔离目标；
   - `append_pending_with_kind` 落 Pending（失败则不移动）；
   - 移动（D1 重试）；
   - 成功 → `mark_applied`；失败 → `discard_pending` 并记条目失败。
3. `FileJournal::open` 打开时收敛 Pending：隔离路径存在 → 补成 Applied（撤销可见）；不存在 → 丢弃（不留假记录）。撤销只回滚 `Applied`，Pending 不参与。
4. 这一步不改变 Journal JSONL 的既有字段；`Pending` 只是 status 的新枚举值，旧日志的 `applied/reverted` 照常解析。

## 3. 为什么不选另外几条路

- **D1 在 `CliFsAdapter` 层逐方法包裹重试**：`FsAdapter` 有 30+ 方法，流式回调不可安全重放；且「任务暂停」是队列状态，适配层不该反向操作壳层状态机。
- **D1 每条失败都无限重试**：断线期间没有用户交互面，逐条 14 秒重试 2000 条等于把任务挂死；方案 §4.9 明确「之后暂停任务」，不是无限重试。
- **D1 让 CLI 也暂停**：CLI 没有任务队列与「继续」按钮；假装暂停只会阻塞。CLI 选择中止并响亮报 E2005，把暂停语义留给桌面壳。
- **D5 先写 Journal 再移动（无 Pending）**：崩溃后会留下指向不存在隔离文件的 Applied 假记录，撤销时反而报 missing；必须区分「意图」与「已确认」。
- **D5 用独立 intent 文件**：多一个文件与清理协议，且仍要启动恢复；直接把 Pending 放进同一 Journal 更简单，且撤销读取端只需一条过滤。

## 4. 已接受的坑、边界与仍开放的缺陷

- **D1 的网络路径判定**：UNC 由纯字符串识别；Windows 盘符映射网络盘由适配层 `GetDriveTypeW == DRIVE_REMOTE` 识别（`FsAdapter::is_network_path` 能力方法，kernel 不自拼盘符规则）。**未覆盖**：没有 UNC 形态的非 Windows 网络挂载点（如 Linux CIFS 挂载的 `/mnt/share`）仍按本地路径处理，留待后续平台需求。
- **D1 不覆盖 `edit-content`/`transcode`**：这两个动作会先 `backup_to_quarantine`；重试会累积备份。若后续要覆盖，需先做备份去重/复用，不在本次范围。
- **D1 覆盖范围**：已接通用执行器、c09 隔离移动、**C10 归档执行器**（`ArchiveApplyContext.network_retry`：inspect/compress/write_atomic/write_atomic_with/stat/create_dir 均走重试；`platform::archive` 把 ZIP 读与源文件读的网络错误保持 E2005）与 **undo**（`UndoPorts.network_retry`：逆向改名/隔离/写回/建删目录均走重试，Abort 中止整任务）。**未覆盖**：`edit-content`/`transcode` 不在重试族。
- **D1 暂停是「在途条目内」暂停**：与手动暂停（停止派发、在途条目完成）不同；断线时条目本来就阻塞在网络调用上，语义可接受。恢复后重试计数清零，若网络仍断会再次暂停。
- **D2 的 59/121/1236 未在实机逐码验证**：只有 64 是 R05 实测；其余按 Windows 错误常量归族，错误码语义不变。
- **D5 的 Pending 收敛依赖隔离路径存在性**：跨卷复制后删源失败可能同时存在源与隔离副本，此时收敛为 Applied（撤销会报冲突，但不会丢数据）；这是可接受的保守选择。
- **D5 只在 c09 使用两阶段**：generic_apply 的 Journal 追加仍在动作后；其复制/改名已由 D3 原子化保证「失败不留半成品」，但崩溃窗口仍可能留未记账产物，留待后续按 corpus 另批处理。

## 5. 验证

- **自动化**（2026-11-08 本机）：
  - D1：`generic_apply::tests::network_disconnect_is_retried_three_times_then_pauses_and_resumes`（前 4 枪 E2005、第 5 枪成功；断言 copy 调用 5 次、退避 `[2,4,8]`、pause 1 次）；`apply::tests::c09_network_disconnect_is_retried_three_times_then_pauses`（隔离移动同序列）；`apply::tests::undo_network_disconnect_is_retried_three_times_then_pauses`（撤销逆向改名同序列）；`apply::tests::undo_network_disconnect_aborts_the_whole_task_without_a_pause_host`（Abort 宿主整任务返回 E2005）；`apply::tests::mapped_network_drive_apply_and_undo_are_retried`（适配层把 `Z:` 标成网络盘后，apply 与 undo 都进重试族）；`archive_network::c10_extract_retries_network_inspect_three_times_then_pauses`（platform 集成：inspect 前 4 枪 E2005、第 5 枪读真实 ZIP 夹具，断言 inspect 5 次、退避 `[2,4,8]`、pause 1 次，目标 UNC 不可达时终态非完成）；`tasks::tests::auto_pause_sets_reason_and_resume_clears_it`（暂停状态、原因与恢复清理）。
  - D2：`platform::fs::tests::windows_network_disconnect_maps_to_e2005`（64/59/121/1236 → E2005、retryable）；`platform::fs::tests::network_path_detection_keeps_unc_and_rejects_local_drive`（UNC 为真、本机盘与相对路径为假）。
  - D5：`apply::tests::c09_crash_after_quarantine_move_leaves_a_journal_entry`（移动后 panic 仍留下 Journal 意图）；`platform::journal::tests::pending_quarantine_entry_recovers_applied_when_the_file_exists`、`pending_entry_is_dropped_when_the_move_never_happened`。
  - 全量：`cargo test --workspace` 678 项（kernel 443＋platform 119）、桌面壳 66＋6 项全绿（2026-11-08 升 Rust 1.99.1 后复跑）；`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、桌面壳 clippy、`check_docs.py`、`check_scope.py` 全绿。
- **R05 实机复跑**（2026-11-08，CLI，`E:/batchdesk-shijiri/fix-d1d5/r05/`）：3000 条 copy 至 SMB，t≈2.058s 停止 LanmanServer；stderr 依次出现 `2 秒/4 秒/8 秒后重试`，t≈16.188s 以 E2005（os error 64）中止；已复制 220 条、0 字节 0 条、SHA256 不符 0 条、Journal 220 条；未再出现「任务报 completed 但文件 0 字节」。
- **E04 实机复跑**（2026-11-08，桌面壳预检路径，`E:/batchdesk-shijiri/fix-d1d5/e04/`）：F: 填至剩 1 MiB，f11 计划在预检阶段产生 `disk-space` 项：`Block`、`available 1048576 < required 0 + margin 209715200`、path `F:\batchdesk-e04-target`，整任务阻断；未产生 31 条副作用（临时测试代码已移除，日志留档）。
- **C10 归档执行器实机复跑**（2026-11-08，CLI，`E:/batchdesk-shijiri/fix-d1d5/c10/`）：3000 条 ZIP 放在 SMB 共享、解压到本地；t≈1.087s 停 LanmanServer（已解 23 条），stderr 依次 `2/4/8 秒后重试`，t≈15.152s 以 **E2005（os error 64）** 中止；已解 23 条、0 字节 0 条、Journal 23 条。证明 C10 的 inspect/extract 网络错误已进入 D1 重试族。
- **映射盘 undo 实机复验**（2026-11-08，CLI，`E:/batchdesk-shijiri/fix-d1-undo/records.md`）：10 个同内容文件放在临时 SMB 共享、客户端映射 `Z:`；c09 计划删 9 条、apply 后 9 条进隔离；`Stop-Service LanmanServer` 后跑 undo——stderr 依次 `2/4/8 秒后重试`、t≈14.421s 以 **E2005（os error 64）** 中止（`detail: \\?\Z:\data\dup9.txt`，路径不含 UNC 前缀，证明映射盘由适配层识别）；隔离 9 条与源 1 条原样、零半成品。`Start-Service` 后复跑 undo：reverted 9、failed 0、t≈0.095s；源文件 10、隔离 0、Journal reverted 9；10 个文件 SHA256 唯一值 1、0 空文件。测后共享与 `Z:` 映射已删除、LanmanServer 已恢复 Running。
- **CI**：run 42475364987（2026-11-08，head `2692392`＝C10 接入批收尾，覆盖 `86899b7` 的 C10 代码与 `2692392` 的 1.99 适配）七 job 全 success、95 步零 skipped 零失败（bench 11／desktop 15／deny 12／lint 10／test·ubuntu 16／web 15／test·windows 16，逐 job／逐 step 核对，匿名 REST）。**判红留档**：run 42471860342（head `86899b7`）仅 lint job 红——CI 用 Rust 1.99.1，`AtomicU64::fetch_update` 在该版本被弃用（`::error::` 注解指向 `crates/platform/tests/archive_network.rs:58`），其余六 job 已 success；改 `try_update`（`2692392`）后本轮转绿，本机工具链同步升到 1.99.1 复跑全部门禁。上一轮 run 42449989630（head `2a145bf`＝D1/D2/D5 修复批）七 job 全 success、95 步零 skipped 零失败。
