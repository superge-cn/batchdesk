# 契约到实现/测试追踪

> 状态快照：2026-10-29 ｜ 用途：一眼看出“契约已冻结但代码/测试还没落地”的差距；实现推进时同步更新。
>
> 这不是新的权威：契约以 `docs/design/` 为准，测试以 T v1.12 与 `docs/testing/` 为准。
> 移植来源与移植部件见 [`docs/porting/feature-source-map.md`](../porting/feature-source-map.md)；**L2a 未批准前不得写代码**。

## 1. 追踪表

| 契约 | 目标代码落点 | 当前实现 | 测试/语料锚点 | 主要缺口 |
|---|---|---|---|---|
| 01 内核接口/IPC | `crates/kernel` traits/DTO；`crates/platform`；`src-tauri/src/lib.rs`、`tasks.rs` | kernel + C09 plan/apply/undo；`ProgressSink`/`CancelToken` 已建；**通用执行器 `generic_apply`（rename/move/delete，含冲突二次裁决、目标路径保护复核与 Journal）与 `conflict` 冲突枚举（§5.13 三类冲突、保留名/非法字符警告、sequence 命名）已建**；Tauri 任务队列与 `task://progress`/`task://state`/`task://done`，`applyPlan`/`undoPlan` 返回 taskId；`cancelTask`/`pauseTask`/`resumeTask`/`listTasks`/`getTask`/`preflightPlan`/`recoverTask` 已注册；S01 `preflight`（磁盘余量/只读占用/占位/长路径/UNC/保护）与 S03 快照持久化/`interrupted` 恢复已建 | kernel 102 + platform 35 + CLI 6 单测 + 3 集成 + shell 8；web build；WebView E2E；kernel preflight 9 项、恢复 4 项测试 | `copy` 的撤销需 Journal 删除型逆向（契约评审后接）；Clock/Rng 端口、`retryTask`、事件合帧；S01/S02 的 TC UI 用例逐条执行 |
| 02 Web 传输 | `web/`、WASM 导出面、云端（后置） | 均未建 | TW-A/B/C/D/F；TW-E mock 待建 | WASM 构建、Worker、OPFS 适配、生成绑定 |
| 03 配置/模板/UiState | `crates/kernel` 类型；`src-tauri` 存储；`web` 设置区 | kernel `config`/`recipe`/`ui_state` 类型、安全默认登记、`SettingValue`/`ScopeView` 来源追踪、`resolveEffective`、v1→v2→v3 模板迁移与导入校验已完成；`platform::config_store` 原子写、损坏备份 + E5009、高版本只读 E1007、UiState 损坏只重置自身、模板 CRUD/导入导出已完成；`src-tauri` 注册 12 条命令（getConfig…importRecipe）；设置 UI 未建 | `cargo test --workspace`（kernel 102 + platform 35，含 config_store 12 项）；`fixtures/migrations/v1` + `check_schema.py` | 迁移链（v1 为空，随 schema 演进注册）、UiState 离屏/clamp 恢复规则、`loadRecipe`（单步直出计划）、列表分页入参语义、E5009/E1007 的用户提示（Toast/日志）、Web 端 OPFS 适配 |
| C09 重复文件查找（PDR-001） | `crates/kernel`、`crates/platform`、`crates/cli`、`src-tauri`、`web/src` | **CLI scan→plan→apply→undo+S02+保留期，以及桌面任务队列/IPC/UI 切片已建立**；apply→undo 与执行中取消回滚通过真实 WebView E2E；plan→apply 内容指纹复核（`expected`+E2010，设计 01 v1.3）；S01 `preflightPlan` 与 apply 前置 E2004 阻断；S03 中断恢复提示条 | T v1.12 C09；C-6；`apply_flow.rs`（含 E2010 集成场景）；shell 队列/恢复测试；`scripts/e2e-tauri.mjs` | 长路径、i18n/虚拟滚动/shadcn、任务列表 UI、T 全量 UI 验收 |
| 04 文件系统适配 | `crates/platform/src/fs.rs`；WASM 适配；`src-tauri` | Native std::fs 适配器已建（路径操作/隔离/stat/list/canonicalize/case）；scan 仍直接 std::fs；**Windows 文件身份已实现**（`GetFileInformationByHandle` → volume serial + file index，ADR-0012；不可得时回退 `needs_manual_review`）；S02 执行期补 `rename/move` 目标路径保护复核（literal+canonical）；S01 采集使用 statvfs/GetDiskFreeSpaceExW、写探测、2 秒 UNC 超时 | `apply_flow.rs`；CLI fs 单测；platform Windows 交叉检查（`cargo check --target x86_64-pc-windows-msvc`）；`windows_identity_uses_volume_and_file_index`；`protected_target_is_skipped_before_any_change`；platform preflight 8 项 | scanDirectory 接入、完整占位探测、长路径矩阵、跨卷元数据保留、UNC 超时线程残留（已知取舍） |
| 05 Web 强制力 | `web/`、WASM 导出面 | Web 仅有 C09 桌面 UI 切片；WASM/导出面未建 | TW-F3；WB-1…WB-4 | 编译期 feature gate、导出面断言、CI diff |
| 06 错误码/文案 | kernel 错误类型、各端映射 | `KernelError` + Tauri `IpcError` 已建；文案资源未建 | `check_docs.py` 白名单；apply 单测 | 用户文案资源、其余码位覆盖 |
| 07 云端服务端 | 服务端仓库（未建） | 未开始 | TW-E1…E13（mock 待建） | 身份/配额、会话/任务、限流、保留/清理 |
| 08 授权签发 | kernel 判定、src-tauri 注入、签发工具 | `LicenseService` + `AlwaysAllowLicense` 已建；商业授权/签发未开始 | apply 授权拒绝单测；C-7 阻塞 | 载荷/签名/序列化、设备指纹、激活流程、QuotaBased |
| 09 i18n | `web/src/locales/`、设置区 | 未建 | 设计 09 §6 的 I18N-01…08；T v1.12 B8 | 资源 key、语言切换、英文排版 |
| 10 日志/诊断 | `src-tauri/src/logging/`、kernel 记录 | Journal JSONL + CLI/桌面 undo 已建；操作/诊断日志未建 | `apply_flow.rs`；journal 单测（含 planKind 兼容）；shell 队列测试 | 双轨落库、脱敏、轮转、支持包 |
| 11 发布/签名/更新 | CI release、`src-tauri/src/update/` | CI 仅骨架；SBOM/bench 对比占位 | 发布清单（设计 11 §6） | 签名、更新清单、防降级、回滚 |
| 12 前端/壳架构 | `web/`、`src-tauri/` | C09 React/Vite 事件驱动 UI + Tauri v2 任务队列/事件切片；`src-tauri` 独立 workspace；WebView E2E 脚本可运行；S03 任务快照持久化 + `interrupted` + `recoverTask`；S01 预检报告与恢复提示条已接入 web | `pnpm build`；`src-tauri` 队列/恢复测试 8 项；`scripts/e2e-tauri.mjs`；浏览器 mock 流 | `retryTask`/事件合帧、Playwright/Vitest、CSP/capabilities、i18n/虚拟滚动、任务列表 UI（`src-tauri` 独立 lock 已由 CI deny job 覆盖） |
| 13 CLI（P1） | `crates/cli` + `crates/platform` | C09 scan/plan/apply/undo + S02 + cleanup 切片（OQ-30 已追认为实验命令，设计 13 v0.2 §2.1）；通用命令面未建；S01 预检/S03 恢复走桌面壳，CLI 未接入 | platform 35 + CLI 6 单测 + 3 集成；C-6 | 命令面、退出码矩阵、真实授权注入、进度/取消、CLI preflight/recover |
| 14 AI 助手（F15 Phase 1/2/3） | `src-tauri/ai`、`web/src/features/ai` | 未开始 | `fixtures/ai/`、AI-01…AI-26 | FunctionCatalog、IntentGraph/PlanBundle、模型 Provider、工具 allowlist、同意/隐私、E6xxx、多计划冲突 |

## 2. 当前可走通性

- **可运行**：`cargo test --workspace`（kernel 102 + platform 35 + CLI 6 单测 + CLI 3 集成）；`cargo fmt/clippy`；`web` 的 `tsc --noEmit` + `vite build`；`cargo test --manifest-path src-tauri/Cargo.toml`（队列/恢复 8）；C-6 只读/计划预览断言（本地手动）；**CI 六 job 全绿（run 40872864062，2026-10-29；含 Windows 单元测试与 deny/SBOM；head 1420a3d 复验 run 40907291374 七 job 全 success）**。
- **可端到端走通的功能**：**C09 CLI 切片**（扫描→计划→隔离清理→Journal 撤销）与**桌面任务队列切片**（plan→apply→undo、执行中取消回滚）均走通；后者已通过真实 Tauri WebView E2E（`scripts/e2e-tauri.mjs`），浏览器 mock 流保留。
- **C09**：CLI scan/plan/apply/undo + S02 + cleanup，以及 Tauri 任务队列/事件/React UI；apply 只处理 C09 delete，移入隔离目录、逐条写 Journal、AlwaysAllow 授权；取消在条目边界回滚并终态 `cancelled`；快速窗口由调用方显式传入（OQ-28 待确认）；保留策略默认 `earliest`（OQ-29 已确认，待基线收编）。
- **C37 处理模板/持久化**：ConfigDocument/UiState/Recipe 类型、存储与 12 条壳层命令已建（见 03 行）；C37 Phase A/B 的 plan/apply 与 UI、设置 UI 未实现。
- **F15/授权/队列/日志/撤销**：AlwaysAllow 已建；任务队列已建（S03 持久化/恢复已建；retry/事件合帧未建）；商业授权、操作日志、F15 UI 未实现；Journal 为 CLI JSONL + 桌面 undo（含可选 planKind）。
- **测试**：无逐条语料断言 harness；TC v0.3 已展开 C37/B9 实例；C-6 覆盖只读/预览，`apply_flow.rs` 覆盖 apply/undo/保留期；shell 测试覆盖事件/暂停/取消状态与中断恢复三决策；`scripts/e2e-tauri.mjs` 覆盖真实 WebView apply→undo 与取消回滚；不能把 CI 绿灯表述为功能验收。

## 3. 状态口径

- **已实现**：代码存在且通过对应测试；当前**没有产品级完整功能**，C09 有 CLI scan/plan/apply/undo 切片（纯分组 + 三阶段哈希 + `PlanDocument` + 隔离 + Journal，见上表）。
- **部分**：只有脚本/样本/文档，没有执行路径；表中“当前实现”列会写明。
- **未开始**：目录/代码均不存在。

## 4. 更新规则

1. 每次合并涉及契约的 PR，更新对应行与“当前实现”列；
2. 新增测试/语料时，把锚点写到“测试/语料锚点”列；
3. 未实现项不得在 README/AGENTS 中写成已实现；
4. 本表不替代 `docs/OPEN-QUESTIONS.md`：需要人决策的仍登记在后者。
5. **完成即标注**（[AGENTS.md](../../AGENTS.md) 协作纪律第 4 条）：任务/功能/切片状态变化当日同步本表“当前实现/测试锚点/主要缺口”与 `docs/DEVELOPMENT-PLAN.md`（Wave 状态、§6.1 状态记录）；证据须可复核（测试命令与结果、门禁、日期、文件/章节回链）。
