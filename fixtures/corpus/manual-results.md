# 人工语料跟踪台账

> **本台账只记 CI 显式排除的 4 条语料**：R05、E01、E02、E04（原因见 `manifest.md` 的「人工跟踪」节与 `manual-only.txt`）。
>
> **为什么不进 CI**：其构造方式依赖人工操作或特定环境（任务中断线、Excel 占用、OneDrive 释放空间、配额受限卷），CI runner 无法复现。
>
> **与验收的关系**：本台账与自动化语料两栏都过才满足 T v1.12 §4.2 的"回归通过"；自动化栏已由 `crates/kernel/tests/corpus.rs` 逐条断言（现状见 `crates/kernel/tests/assertions.tsv`，0 条 partial），但**本台账四行仍无执行记录与签署**。**CI 绿灯 ≠ 语料回归零失败**。

---

## 执行时机

| 时机 | 范围 | 说明 |
|---|---|---|
| 阶段末全量回归 | 4 条全部 | 与 T v1.12 §6.3 的"阶段末"同步 |
| 涉及以下功能的改动 | 按功能关联选择 | R05→C07/C61；E01→F12/C10/C07；E02→F10/C09/C07；E04→全写操作（S01） |
| 闭门验收前 | 4 条全部 | 承接 T v1.12 §4.3；同时满足"连续两周无 P0"的稳定性观察 |

## 记录字段（每次执行一行）

| 字段 | 填写规则 |
|---|---|
| 语料 | 编号（R05／E01／E02／E04） |
| 环境组合 | T v1.12 §6.1 的 M1–M8；按 `manifest.md` 的建议组合取 |
| 日期 | YYYY-MM-DD |
| 构建 | 版本号＋通道（如 `1.0.0-B3`，口径见方案 §12.5） |
| 结论 | 通过／失败／阻塞（阻塞须写原因，如环境未就绪） |
| 实测行为 | 一句话：实际发生了什么（与 T v1.12 §3.2 对应行的"预期行为"比对） |
| 缺陷 | 失败时填缺陷编号（T v1.12 §5.3 的流转）；通过则填 `—` |

## 台账

| 语料 | 环境组合 | 日期 | 构建 | 结论 | 实测行为 | 缺陷 |
|---|---|---|---|---|---|---|
| R05 | M3 | | | | | |
| E01 | M1 | | | | | |
| E02 | M2 | | | | | |
| E04 | M1 | | | | | |
| E02 | M2 | 2026-11-08 | 开发构建（提交 4dcd22a，二进制 sha256 4b279811…b40ec；尚无正式版本号） | 通过 | D:\OneDrive 实机：C09 full 扫描对 7 个占位文件（Offline/RecallOnDataAccess 属性位）全部跳过并逐条点名（skipped 列表 reason=placeholder file skipped），F10 计划出 placeholders-skipped 警告（计数 7）、apply 落盘 CSV 不含任何占位行（5 文件＋4 目录与摘要一致）；扫描后复查属性位未变＝未触发按需下载。证据 E:\batchdesk-shijiri\b1\e02\（f10-plan/apply.json、c09-scan.json、listing.csv） | — |
| E01 | M1 | 2026-11-08 | 开发构建（提交 4dcd22a，二进制 sha256 4b279811…b40ec） | 通过 | Excel 2016（Office16 COM 自动化）真实打开 locked.csv 并保持（独占打开验证被拒）：C07 move 占用条目**响亮失败** E2008（os error 32 共享冲突，detail 带路径）、被锁文件 sha256 原样、对照条目正常移动——与 OQ-37 裁决「C07 不在跳过族、维持 Failed」一致；关闭 Excel 后补完移动、undo 两条全部还原逐字节一致。证据 E:\batchdesk-shijiri\b1\e01\（excel-hold.ps1、c07-plan/apply/retry/undo.json）。注：CLI 重放对已应用条目报 E2001 属重放语义（桌面 retryTask 仅重试失败条目）；预检标记半边待 B6 桌面走查 | — |
| E04 | M8 | 2026-11-08 | 开发构建（提交 4dcd22a，二进制 sha256 4b279811…b40ec） | 失败 | F: 小容量可移动盘（14.8 GB exFAT）以压舱文件填至剩 1 MiB：F11 40 条建档 apply 前 31 条成功后磁盘耗尽，9 条失败——E2004（os error 112，detail 带路径，retryable=true）＋父目录失败的子目录级联 E2001；释放空间后重放补齐 9 条、undo 40 条全还原、目标目录归零。**执行期报告与恢复闭环通过，但 T §3.2 E04 判定点「S01 预检阻断、任务零副作用」不成立**：壳层预检磁盘余量判定只接 c09（src-tauri/src/lib.rs preflight_gate 的 `if plan.kind == "c09"`），f11 计划无 disk-space 项，满盘未被预检阻断、产生 31 条目录副作用。证据 E:\batchdesk-shijiri\b1\e04\（f11-plan/apply/retry/undo.json） | 待登记（预检磁盘判定覆盖面缺口，修复走五步闭环） |
| R05 | M3 | 2026-11-08 | 开发构建（提交 4dcd22a，二进制 sha256 4b279811…b40ec） | 失败 | 本机 SMB 共享（临时 share＋临时防火墙规则，测后已删）：C07 copy 3000 条至 UNC，t=2.9s 停 LanmanServer（语料定义的断线手段），已拷 331 条；其余 2263 条**逐条一次即败**、任务终态 partial-failed——方案 §4.9「最多 3 次重试、退避 2/4/8 秒、之后暂停任务」**未实现**（kernel/壳层均无该逻辑）。另两条实测发现：① os error 64（网络名删除）落 E2008 而非 E2005（io_error 映射表缺 Windows 网络错误码分支）；② **静默数据损伤**：断线瞬间 f0342.txt 的 copy 留下 0 字节目标（FsAdapter::copy 走 std::fs::copy 直写、非原子、失败不清理），恢复后重放该条被 E2007「目标已存在」跳过，任务报 completed 而共享上该文件 0 字节（全量 SHA256 对账 3000 条中唯一不符）。证据 E:\batchdesk-shijiri\b1\r05\（disconnect.ps1、c07-plan/apply/retry.json、journal.jsonl、verify2.ps1 输出） | 待登记（三条：断线重试未实现／E2005 映射缺口／copy 非原子致静默损伤，第三条按 P0/P1 缺陷走五步闭环＋挂 manifest） |

---

<!--
维护约定：
- 每次带结论的执行**追加一行**，不覆盖历史（本台账即回归历史）；
- "结论＝阻塞"必须写清缺什么环境，并在下一次全量回归前补齐；
- 若某条语料经改造后变得可脚本化：在 manifest.md 改标「就绪」、从 manual-only.txt 移除，
  check_scope.py 会强制这两处同步；随后在本台账末尾加一行说明该条已转为自动化。
-->