# 0040. UI 接入：壳层 kind 路由与 F05/F11/F12 主流程页面

- **状态**：已采纳（2026-11-01，开发代理按台账 §3.4 既定方向实施）
- **日期**：2026-11-01
- **决策者**：开发代理（方案 §10.2 阶段 0 出口标准的既定硬前置）
- **关联**：方案 §10.2（阶段 0 出口三条主流程）、§7.1（plan/apply）；设计 01 §4.1（planOperation）、设计 12 §3.1（壳层与 UI 架构）；ADR-0036/0037/0039；硬性不变量 5/11

---

## 1. 问题背景

阶段 0 出口标准要求非技术用户无帮助完成"拆 CSV→下发（F05）""清单→建档（F11）""批量改名→撤销（F12）"三条主流程，但桌面壳 `planOperation` 硬编码只接受 c09、任务队列 `run_apply` 只走 c09 专用执行器——20 个非 c09 切片全部只有 CLI 入口。UI 接入是阶段 0 验收的硬前置。

## 2. 候选与结论

| 议题 | 候选 | 结论 |
|---|---|---|
| 首批接入功能 | a) 24 个功能全部一次接入；b) 三条主流程（F05/F11/F12）先行 | **b**：三条主流程是出口标准的验收对象；其覆盖了 move/copy/创建目录/split 动作与事件进度/撤销全链路，后续功能复用同一通道 |
| 队列执行路由 | a) run_apply 保持 c09 专用、新计划走 CLI；b) run_apply 按 kind 路由（c09 专用 / 通用执行器） | **b**：桌面队列是事件进度/暂停/取消/重试的唯一载体；通用执行器 `apply_generic_plan` 已覆盖非 c09 全族（除 c10 ZIP，见后果） |
| kind 事实采集位置 | a) kernel 读盘；b) 壳层采集（CSV 嗅探/目录遍历）后传 kernel 纯函数 | **b**：不变量 1；f05 用 `platform::csv::csv_split_layout`，f11/f12 用 `read_dir`，与 CLI 同源 |
| C05/C06 呈现 | a) 独立页面；b) F12 页内嵌预览面板 + 回滚预演 | **b**：C05 预览报告与 C06 回滚预演本来就是 F12 计划的派生视图（`renamePreview`/`undoDryRun` 壳层命令） |

## 3. 选择理由

- **队列路由**：`run_apply` 按 `plan.kind` 分流——c09 走既有 `apply_plan`（隔离清理专用），其余走 `apply_generic_plan`（GenericApplyContext 携带 `directory_lister`）；跨盘校验删源、子目录自动创建、Journal 语义全部继承既有 Move 臂，零新执行机制。c10 ZIP 归档需要 `ArchiveApplyContext`，桌面执行暂缓（登记台账），CLI 仍可用。
- **kind 路由（planOperation）**：c09 保留原路径；f05/f11/f12 各自的参数结构（serde camelCase）→ 壳层事实采集 → kernel 计划函数 → 存入 `AppState.plans`（`applyPlan` 既有通道直接可用，无需改动）；未知 kind 明确 E1002。C18 幂等：f05 的 `groupId` 经 `derived_ulid("f05:{planId}")` 派生。
- **C05/C06**：`renamePreview(planId)` 与 `undoDryRun(planId)` 两个新壳层命令直接暴露 kernel 的预览/回滚预演纯函数——web 端 F12 页内嵌 C05 对照表（状态标记＋四类计数＋高危汇总）与 C06 回滚预演面板。
- **web 架构**：`FeaturePages` 组件承载三个视图（导航切换），自带 task:// 事件订阅与进度面板；C09 主页面用 `display: contents` 容器条件渲染（不破坏既有 grid 布局）。

## 4. 后果与退路

- **后果**：桌面队列现在可执行所有非 c09 通用族计划（move/copy/delete/edit-content/split/merge/csv-*）——授权强制力仍由 LicenseService 端口与 kernel 执行层复核（不变量 5/11，web 零强制力不变）；web 端新增三页均为"壳层生成计划→apply→事件"同一模式。
- **退路**：新功能接入只需三处：kernel 计划函数已有 → 壳层 `plan_{kind}_shell` 参数结构 → web 表单；c10 ZIP 桌面执行待 `ArchiveApplyContext` 接入。

## 5. 关联

- 代码：`src-tauri/src/tasks.rs`（run_apply kind 路由＋generic 队列测试）、`src-tauri/src/lib.rs`（planOperation 路由＋f05/f11/f12 shell 计划＋renamePreview/undoDryRun）、`web/src/FeaturePages.tsx`、`web/src/lib/ipc`（renamePreview/undoDryRun 封装与 mock）；
- 测试：壳层 generic 队列测试 1 项（c07 move 计划经桌面队列 apply→undo）；
- 后续：剩余功能按同模式接入；Playwright/Vitest 矩阵；c10 桌面执行。
