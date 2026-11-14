# 0097. 流式重写的 mid-stream 取消：给 `ContentRewriter` 传 cancel token＋E3010 区分「取消」与「失败」

- **状态**：已采纳
- **日期**：2026-11-13
- **决策者**：开发代理（工程实现决策；取消语义引方案 §7.7 与设计 01 §2.2 的既定口径）
- **关联**：方案 v1.22 §7.7（取消＝停止派发＋在途安全收尾）；设计 01 §2.2（取消粒度）、§5（`ContentRewriter`／`ApplyEntryReport.detail`）；设计 06 §2（错误码表，新增 E3010）；TC-C02-EXE-04

---

## 1. 问题背景

TC-C02-EXE-04 长期挂 ◐，缺口原文：

> 取消在 kernel 里按**条目／分块边界**生效（`generic_apply` 派发循环内的 `is_cancelled_token` 检查＋`ContentRewriter` trait 不接收 cancel token），因此**不存在**"单条内部 mid-stream 中止"这条路径；已实测确认：⑤ 臂取消后按备份逐字节还原原文件。真要支持流式中途中止需先改 trait（设计变更，非本批射程）。

即：一个大文件（C02 换行转换、C03 替换、C01 转码）在流式重写过程中，用户点取消不会生效，要等整条重写跑完。设计 01 §2.2 写的"每个条目边界检查一次"是**当时的能力边界**，不是产品要求——方案 §7.7 的取消语义是"停止派发＋在途条目安全收尾"，对"在途"的理解应当包含**正在重写的那一条**。

## 2. 候选与否决理由

| 议题 | 候选 | 结论 |
|---|---|---|
| 怎么让重写器知道取消 | a) 给 `ContentRewriter::rewrite` 加 `cancel: &dyn CancelToken`；b) 把 token 塞进 rewriter 结构体；c) 在 `rewrite_atomic_with` 的回调里包一层 | **a**：b 要在构造期绑定 token（壳层每次派发都要重建 rewriter，且并发下同一 rewriter 被多条目共享时不成立）；c 的回调是**一次调用**，包不住读块循环 |
| 取消怎么传回执行器 | a) 复用某个现有错误码；b) **新增 E3010「操作已被用户取消」**；c) 在 `KernelError` 上加 `cancelled` 字段 | **b**：c 要给 `KernelError` 加 serde 字段（线协议变更，波及所有调用方）；a 会让"用户取消"与"真的写回失败"混为一谈 |
| 为什么必须是 `Err` | — | **硬约束**：重写跑在 `rewrite_atomic_with` 的回调里，回调返回 `Ok` 就会把临时文件**原子改名盖到源文件**。半截内容落盘比不取消更糟，所以中止必须让回调返回 `Err` |
| 取消的条目记什么状态 | a) `Ok`（沿用 C10 归档臂"口径不对称"的写法）；b) **`Skipped`＋E3010** | **b**：`Ok` 会让用户以为这一条已经转换完成，而实际源文件一个字节都没动。`Skipped` 如实表达"未改动"，且不进 failed 计数 |
| 哪些臂接线 | a) C01/C02/C03 全接；b) **只接 C02** | **b**：C02 是单一均匀的 64 KiB 读块循环，一处检查即可；C03 有四条重写路径（cell／plain／窗口化正则／整段）、C01 有两条，逐块接线另属切片（§5） |
| 最后一条取消后终态 | a) 靠循环顶部的边界检查；b) **循环后再确认一次 token** | **b**：mid-stream 取消若落在**最后一条**，循环自然结束、顶部检查没机会跑，整单会被误判成 `Completed` |

## 3. 决策

1. **端口**：`ContentRewriter::rewrite(entry, input, output, cancel: &dyn CancelToken)`；无取消端口的调用方（CLI 测试、重写器单测）传 `progress::NoCancel`（新增的空实现）。
2. **检查点**：C02 在读块循环的每轮开头问一次；命中即 `return Err(KernelError::cancelled())`。回调因此以 `Err` 收场 → `rewrite_atomic_with` 丢弃临时文件 → **源文件保持原样**。
3. **错误码**：新增 **E3010「操作已被用户取消」**（设计 06 §2，v1.8），配 `KernelError::cancelled()`／`is_cancelled()`。
4. **执行器**：`settle_entry` 把 E3010 记成 `Skipped`（释放授权、不进 failed）；串行与并发两条循环在**循环结束后**再确认一次 token，保证"最后一条被取消"也能得到 `Cancelled` 终态。
5. **取消不是失败**：E3010 永不进 failed 计数，也不参与 `PartialFailed`／`Failed` 判定。

## 4. 后果

- **内核**：`error.rs` 加 `cancelled()`／`is_cancelled()`；`progress.rs` 加 `NoCancel`；`generic_apply.rs` 的 `ContentRewriter` trait 与 `WorkerView` 加 cancel 端口、`settle_entry` 与两条循环接线；`line_endings.rs` 逐块检查。
- **调用点**：`encoding.rs`（C01）、`replace_content.rs`（C03）、两处测试桩、C03 单测与 CLI 单测同步签名。
- **错误码**：设计 06 升 v1.8（＋E3010 行）；两语言包各加 `summary`／`action`；`web/src/errors.ts` 加映射。
- **不变**：`ApplyStatus` 仍无 cancelled 取值（取消由 task state 表达）；C01/C03 的取消行为不变（仍只在条目边界生效）。

## 5. 残余

1. **C01／C03 未接逐块检查**：两者共用 `ContentRewriter` 端口（签名已带 cancel），但实现里是 `_cancel`。C03 有四条路径、C01 有两条，逐块接线各自属独立切片。
2. **检查粒度是 64 KiB 一块**：取消延迟上界＝一块的处理时间，对换行转换是微秒级，可接受；若将来出现单块极慢的变换，再谈更细的检查点。
3. **`Skipped` 与 C10 归档臂的 `Ok` 口径不对称**：C10 那条是"已写出的部分保留、报 ok"（设计 01 §2.2 的已知口径），C02 这条是"一个字节没动、报 skipped"。两者语义不同，不强行统一。
