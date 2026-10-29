# 开发计划与状态台账

> 状态：v0.1 ｜ 日期：2026-10-29 ｜ 性质：实施路线与完成度台账
>
> 本文件不替代四份基线、设计契约、T 用例或 `TRACEABILITY`。它只回答三个问题：
> **先做什么、后做什么、现在完成到哪**。产品行为、优先级、阈值仍以产品方案与 T 为准；
> 契约到代码的逐项映射见 [`design/TRACEABILITY.md`](./design/TRACEABILITY.md)。
>
> 更新纪律：每当一个 Wave/任务状态变化时更新本文件；不得把“切片完成”写成“产品功能完成”。

## 1. 当前结论

需要一份开发计划，而且当前应继续采用 **“平台核心 + 纵向切片”** 的推进方式，而不是按功能编号平铺开发：

1. 先收口已有 C09 切片的剩余项（`src-tauri` 依赖审计、内容再校验、Windows 能力）；任务队列/事件/IPC 返回形状已完成；
2. 再补平台核心（配置/UiState、文件操作主链、日志/授权注入），让后续功能复用同一套 plan/apply/Journal；
3. 然后按 T v1.12 的阶段 0 功能组批量实现；
4. C37 处理模板与 F15 AI 属阶段 1，必须等平台核心与阶段 0 主链稳定后再进；
5. Web/云端/商业授权/发布硬化最后做。

**当前没有任何功能达到 T 的“产品完成”验收口径；C09 是目前唯一具备可运行纵向切片的功能。**

## 2. 状态口径

| 状态 | 含义 |
|---|---|
| **完成** | 代码存在、对应测试/门禁通过、文档同步；若为产品功能，还必须满足 T 的验收口径 |
| **切片** | 有一条可运行纵向路径（如 C09 CLI/UI），但缺少平台件或 T 全量验收，不得称为产品完成 |
| **部分** | 只有契约/样本/部分实现，没有完整执行路径 |
| **未开始** | 目录/代码/契约均未进入实现 |
| **阻塞** | 依赖未决事项、外部工具或人工裁决（见 `OPEN-QUESTIONS.md`） |

## 3. 当前完成度（2026-10-29）

### 3.1 已验证可用

- 基线/契约/测试文档体系：产品方案 v1.19、T v1.12、U v1.3、设计 01–14、ADR、PDR、TC v0.3。
- kernel：C09 纯分组、`PlanDocument`/`plan_c09`、`FsAdapter`/`LicenseService`/`Journal`/`apply_plan`/`undo_plan`、`ProgressSink`/`CancelToken`（逐条进度、取消回滚）、路径 normalize/is_under 与 S02 blocked、plan→apply 内容指纹复核（`expected`+E2010，设计 01 v1.3）。
- `crates/platform`：扫描/BLAKE3、原生 FsAdapter、FileJournal、保留期 cleanup，CLI 与桌面壳共用。
- CLI：C09 scan → plan → apply → undo/cleanup、S02、`--protected-dir`、`--retention-days`。
- 桌面：`src-tauri` Tauri v2 任务队列（`tasks.rs`）与 `task://progress`/`task://state`/`task://done`；`applyPlan`/`undoPlan` 返回 `{taskId}`；取消/暂停/恢复与 list/get 已注册；应用数据目录下 quarantine/journal。
- Web：React 19/Vite 7 C09 页面，U v1.3 token + 液态玻璃外壳；计划预览、确认执行、事件驱动进度/暂停/取消、撤销、保留期演练/永久清理；任务事件按 taskId 归档，避免 IPC 返回前完成导致丢报告；浏览器 mock IPC 流已验证。
- 验证：`cargo test --workspace` 70 项通过（kernel 46 + platform 15 + CLI 6 + CLI 集成 3）；`cargo test --manifest-path src-tauri/Cargo.toml` 2 项通过；`scripts/e2e-tauri.mjs` 真实 WebView apply→undo、cleanup 演练与执行中取消回滚通过；C-6 断言通过；`pnpm build` 通过；`src-tauri` cargo check 通过；**CI 六 job 全绿（run 40872864062，2026-10-29；lint/deny/test·ubuntu/test·windows/web/desktop/bench 全 success）**。
- 门禁：fmt/clippy、docs、语料范围/哈希、迁移 schema。

### 3.2 部分完成/有明确缺口

| 域 | 已完成 | 未完成/缺口 |
|---|---|---|
| C09 产品化 | 纯分组、计划、apply/undo、S02、保留期、CLI/UI 切片、任务队列/事件、取消回滚、WebView E2E、plan→apply 内容再校验（E2010）、Windows 文件身份（ADR-0012） | 长路径、C09 的 T 全量 UI 验收 |
| 平台核心 | Native FsAdapter、Journal、AlwaysAllow、路径/保留期、TaskQueue/TaskState/进度/取消/暂停、ConfigDocument + UiState + Recipe 存储与命令、通用执行器（rename/move/delete）+ 冲突枚举 | 重试（retryTask）、任务持久化、设置 UI、`loadRecipe`、日志/授权注入、Hashing 端口、`copy` 执行器 |
| IPC/UI | C09 任务队列命令、`{taskId}+事件`、cleanup/storage 合约化、事件驱动 UI | CSP/capabilities、i18n、虚拟滚动、shadcn 完整组件、Vitest/Playwright、任务列表 UI |
| 桌面工程 | Tauri 编译/启动、CI desktop check | Windows WebView 构建验证、`src-tauri` 独立 Cargo.lock 的 cargo-deny/SBOM、安装包/签名 |
| 验证 | C-6、C-6 计划预览、apply/undo/保留期集成 | 24 条自动化语料的逐条断言 harness、C-7 阻塞、C-1/C-2/C-4/C-5/C-8 正式基准 |

### 3.3 未开始（按功能组）

- **阶段 0 文本/编码**：F01–F13（分割/合并/抽列/编码/表头/换行/替换/正则改名等）、C01/C02/C03/C04/C05/C06/C07/C08/C10/C11/C12/C13/C14/C15/C18。
- **阶段 0 安全/环境**：S01 预检、S02 完整 UI 接入、S03 断点续跑；C-7 授权状态集。
- **阶段 1**：C37 处理模板（存储/迁移/导入导出已完成，Phase A/B 的 plan/apply 与 UI 未做）、F15 AI 助手 Phase 1/2/3、设置与持久化 UI。
- **P1/P2**：C21–C63 中未进入阶段 0 的功能、CLI C40 正式命令面、性能基准结果台账。
- **商业/云端/Web/发布**：商业授权签发与 QuotaBased、云端服务端、Web/WASM 链路、安装包/签名/更新检查。

### 3.4 一句话状态

| 问题 | 结论 |
|---|---|
| 有几个产品功能完成 T 验收？ | **0** |
| 有几个功能有可运行纵向切片？ | **1**（C09） |
| 平台核心是否足够支撑第二个功能？ | **不够**：缺重试/任务持久化、Config/UiState、通用文件操作主链、日志/授权注入 |
| 现在最该做什么？ | **Wave 1 续做**：配置/模板/UiState 存储已完成，下一步是设置 UI 与通用文件操作主链；W0-6 仅剩 Windows 长路径与 C09 的 T 全量 UI 验收 |

## 4. 开发顺序（按依赖，不重排基线优先级）

> Wave 是工程依赖顺序；某功能属于阶段 0/1、优先级 G0/G1/G2 仍以 T v1.12 §2.2 与产品方案为准。
> 若本计划与基线冲突，先以基线为准并回来修正 Wave。

### Wave 0：收口当前 C09/IPC 切片

**目标**：让“已有纵向切片”成为可复用、可验收的基线，而不是继续堆功能。

1. **任务队列与事件**
   - TaskQueue/TaskState（queued/running/paused/cancelling/completed/partial-failed/failed/cancelled）；
   - `applyPlan`/`undoPlan` 返回 `{taskId}`，执行走队列；
   - `task://progress`/`task://state`/`task://done` 事件；
   - 取消/进度至少覆盖 C09 apply/undo；
   - UI 改为订阅事件，不再同步等待命令返回结果。
2. **IPC 契约对齐**
   - 将 `cleanupRetention`、`getStorageInfo` 写入设计 01（追加式）或明确标为壳层扩展；
   - 统一 `KernelError` → `IpcError` 的 `taskId` 与文案；
   - 确认 Tauri 命令名/参数 camelCase 与设计 01 一致。
3. **Tauri 端到端与安全边界**
   - WebView 点击级 E2E（至少 plan → apply → undo/cleanup）；
   - CSP/capabilities 最小化；生产构建关闭 devtools；
   - `src-tauri` 独立 Cargo.lock 纳入 cargo-deny/SBOM。
4. **C09 收尾**
   - 长路径（Windows 文件身份与 plan→apply 内容再校验已完成）；
   - C09 的 T 全量 UI 用例与人工项台账；
   - C-6/apply_flow 继续作为回归锚点。

**完成判据**：UI 通过事件驱动完成 C09 全链路；Tauri WebView E2E 绿灯；设计 01 命令形状一致；`src-tauri` 依赖审计通过。

**当前状态（2026-10-29）**：W0-1/W0-2/W0-3/W0-4/W0-5 已完成（W0-5 以 CI 六 job 全绿关闭）；W0-6 部分（仅剩 Windows 长路径与 C09 的 T 全量 UI 验收），Wave 0 仍为“部分”。

### Wave 1：平台核心（阶段 0 的前置）

1. **ConfigDocument + UiState + Recipe 存储**（设计 03）：设置、最近目录、列宽/排序、模板运行偏好；不混入门禁/任务/日志。
   - **当前状态（2026-10-29）**：存储层完成（kernel 类型/默认值/来源视图/模板 v1→v3 迁移 + `platform::config_store` 原子写/损坏兜底/只读降级 + 12 条壳层命令）；设置 UI、`loadRecipe`、迁移链（v1 为空）与 UiState 恢复规则待补。
2. **通用文件操作主链**：C05/C06/C07/C18 的冲突枚举、预览、plan/apply、Journal 撤销；供后续 F/C 功能复用。
   - **当前状态（2026-10-29）**：kernel `conflict`（三类冲突枚举、保留名/非法字符/结尾点空格/空名警告、`sequence` 命名、大小写折叠）与 `generic_apply`（rename/move/delete 执行、冲突二次裁决、Journal 可逆记录、取消回滚、E2010 指纹复核、E2009/E2006/E4001/E4004、outcome→`overwritten` 单列）已落地并含 17 项单测；`copy`、压缩/转码类与长路径分级未接（见台账缺口）；尚未接入具体功能与 UI。
3. **S01/S02/S03**：预检、保护目录执行层复核、断点续跑/任务持久化。
4. **日志/诊断/授权注入**：C17a 最小日志、Journal 保留期、LicenseService 单机实现与商业接口边界。
5. **Hashing/FsAdapter 能力补齐**：长路径、占位文件、分块读（Windows 文件身份已由 ADR-0012 落地）。

**完成判据**：新增任一阶段 0 功能可以只写“参数→PlanDocument 生成器”，不重复实现 plan/apply/Journal/冲突/日志/授权。

### Wave 2：阶段 0 文本与文件主功能

按 T v1.12 的阶段 0 清单分组实现，建议顺序：

1. F01–F04（分割/合并/抽列/编码处理）——依赖文本流式读写与编码检测；
2. F05–F09（表头/换行/替换/映射/清单）——依赖 CSV 结构与冲突策略；
3. F10–F13（清单/建档/正则/改名）——依赖通用文件操作与预览；
4. C01/C02/C03（编码转换/换行/替换）——复用 F 组文本管道；
5. C10（ZIP）——独立流式压缩/解压，安全边界单独验收；
6. C11/C12（CSV 去重/过滤）与 C15（任务队列）按依赖插入；
7. C09 产品化收尾（任务队列/UI/Windows 能力）。

每项完成必须同时满足：plan/apply 结构、错误码、预览、Journal 撤销、T 用例、语料/基准锚点。

### Wave 3：阶段 0 剩余与安全收尾

- C06/C07/C08/C13/C14/C18 的跨功能组合、清单驱动、归档、外排/搜索等；
- S01/S02/S03 的完整验收；
- C-1…C-8 正式基准与结果台账；
- 自动化语料断言 harness（OQ-08）落地，CI 从“生成+范围”升级为“生成+断言”。

### Wave 4：阶段 1（C37 + F15 + 持久化 UI）

- C37 Phase A（单步模板）→ Phase B（多步模板、一键+逐步骤降级）；
- ConfigDocument/UiState/Recipe 的 UI 与导入导出；
- F15 Phase 1（4 个功能）→ Phase 2/3（全量可计划+多计划）；
- 每步仍走 plan/apply，AI 永不直接 apply。

### Wave 5：P1/P2、Web、商业与发布

- C21–C63 中未进阶段 0 的功能按 T/方案排期；
- Web/WASM、云端服务端、商业授权 QuotaBased；
- 安装包、签名、更新检查、CSP/加固断言、性能基准对比。

## 5. 下一批任务（Wave 0 backlog）

| 任务 | 状态 | 产出 | 验收 | 依赖 |
|---|---|---|---|---|
| W0-1 任务队列/状态机 | **完成** | `TaskQueue` + TaskState + 事件 | C09 apply/undo 通过事件完成；取消可见 | 设计 01 §6、设计 12 |
| W0-2 `applyPlan`/`undoPlan` 返回 taskId | **完成** | IPC 形状对齐 | UI 不再同步等待结果 | W0-1 |
| W0-3 Tauri WebView E2E | **完成** | WebDriver 脚本 `scripts/e2e-tauri.mjs` | plan→apply→undo/cleanup 演练 + 取消回滚 | W0-1、Tauri 系统依赖 |
| W0-4 合约化 cleanup/storage 命令 | **完成** | 设计 01 §4.4.1 壳层追加行 | 文档/命令/DTO 一致 | 设计 01 评审 |
| W0-5 src-tauri 依赖审计 | **完成** | cargo-deny/SBOM 覆盖独立 lock | CI deny job 全绿（run 40872864062；含 `cargo deny --manifest-path src-tauri/Cargo.toml` 与 4 份 SBOM artifact） | OQ-13、CI |
| W0-6 C09 收尾 | 部分 | 内容再校验（expected+E2010+CLI 集成锚点）与 Windows 文件身份（WinAPI，ADR-0012）已落地；长路径未开始 | T C09 全量用例 + C-6/apply_flow | W0-1、平台能力 |

## 6. 完成判定与维护

- **完成即标注（项目规则）**：每完成一个任务/功能/切片，按 [AGENTS 协作纪律第 4 条](../AGENTS.md#协作纪律ai-开发四条铁律) 更新本文件、`TRACEABILITY` 及对应 PDR/测试/CHANGELOG/ADR；没有可复核证据只能写“切片/部分”，不得写“完成”。
- **每项功能**：必须过 fmt/clippy/test、相关语料/基准、文档锚点；涉及批量写必须有 plan/apply 与 Journal；涉及授权必须走 LicenseService。
- **每个 Wave**：更新本文件与 `TRACEABILITY`；Wave 完成不等于阶段验收，最终以 T 的四级验收为准。
- **状态更新**：完成/切片/部分/未开始/阻塞五档只能按证据改；没有测试或 E2E 的不写“完成”。
- **范围控制**：不因本计划新增产品阈值或行为；新增功能/改优先级回到方案 §11.6 或 T 升版。
- **已知偏差**：任务队列已实现，但 `retryTask`、任务持久化/S03 恢复、事件合帧未实现；`cleanupRetention`/`getStorageInfo` 已作为壳层追加命令写入设计 01 §4.4.1；`src-tauri` 依赖审计已由 CI deny job 覆盖（W0-5 完成）。W0-6 是 Wave 0 仅剩的关闭项（Windows 长路径、C09 的 T 全量 UI 验收），不是“已完成”。

### 6.1 状态更新记录（模板 + 当前示例）

> 每次状态变化追加一行；证据必须可复核（命令/测试名/文档回链），不得只写“已完成”。

| 日期 | 任务/功能 | 状态 | 证据（命令/测试/文件） | 剩余缺口 | 记录人 |
|---|---|---|---|---|---|
| 2026-10-29 | W0-1 任务队列/状态机/事件 | 完成 | `src-tauri/src/tasks.rs`；`cargo test --manifest-path src-tauri/Cargo.toml --offline` 2 passed；kernel 取消/进度单测 3 项 | `retryTask`、任务持久化、事件合帧 | 开发代理 |
| 2026-10-29 | W0-2 applyPlan/undoPlan 返回 taskId | 完成 | `src-tauri/src/lib.rs` 命令；`web/src/lib/ipc/index.ts` 订阅封装；`pnpm build` 通过 | — | 开发代理 |
| 2026-10-29 | W0-3 Tauri WebView E2E | 完成 | `node scripts/e2e-tauri.mjs`：apply→undo、cleanup 演练、执行中取消回滚全部 PASS；截图 `target/e2e/tauri-task-queue.png` | Playwright/Vitest 矩阵、CSP/capabilities 断言 | 开发代理 |
| 2026-10-29 | W0-4 cleanup/storage 契约 | 完成 | 设计 01 v1.2 §4.4.1；`cleanupRetention`/`getStorageInfo` 命令保持 | — | 开发代理 |
| 2026-10-29 | C09 CLI/platform/IPC/UI 纵向切片 | 切片 | `cargo test --workspace` 63 项；C-6；`pnpm build`；`src-tauri` cargo check；WebView E2E | W0-5 依赖审计、W0-6 Windows/长路径/plan→apply 再校验、T 全量 UI 验收 | 开发代理 |
| 2026-10-29 | CI 首次运行（run 40865148224） | 部分 | lint/bench/desktop/web/test·ubuntu 5 job 绿；deny（src-tauri advisories 命中 RUSTSEC-2024-0370）与 test·windows（check_docs）失败；已修复：`deny.toml` 带理由忽略项、`.gitattributes` LF 统一、`PYTHONUTF8=1`；本地离线复核实证 | 推送后复验全绿 | 开发代理 |
| 2026-10-29 | W0-6 plan→apply 内容再校验 | 切片 | kernel `expected` 指纹 + E2010 复核（单测 3 项）+ platform `content_hash`（BLAKE3）；`apply_flow.rs` 新增真实 CLI 链路集成场景，`cargo test -p batchdesk-cli --test apply_flow` 3 passed | Windows 文件 ID、长路径、T 全量 UI 验收 | 开发代理 |
| 2026-10-29 | Windows 编译缺陷（E0658）修复 | 切片 | CI run 40870120156 注解实证 `use of unstable library feature windows_by_handle`；`scan.rs` 的 Windows `identity_from` 改为“身份不可用”回退（None → kernel 标 needs_manual_review），Windows 测试同步断言回退；本地 `cargo check --target x86_64-pc-windows-msvc`（platform，临时 pure 绕过 blake3 汇编后还原）零 error；Linux `cargo test --workspace` 70 项仍全绿 | Windows 文件 ID 需 WinAPI GetFileInformationByHandle 并新增依赖（过 cargo-deny）；长路径矩阵 | 开发代理 |
| 2026-10-29 | Windows 文件身份（WinAPI，ADR-0012） | 切片 | CI run 40870967145 实证“回退”导致 Windows 上 `stats.total=0`（C09 不可用）；`identity_from` 改经 `GetFileInformationByHandle` 取 volume serial + file index（`windows-sys` 0.61，MIT OR Apache-2.0）；Windows-only 测试断言身份可得且硬链接两路径一致；`cargo check --target x86_64-pc-windows-msvc` 零 error；Linux fmt/clippy/70 项测试/`cargo deny` 四项全绿 | 长路径矩阵、T 全量 UI 验收、Windows 实机复验（依赖 CI） | 开发代理 |
| 2026-10-29 | Wave 1-2 通用文件操作主链 | 切片 | kernel 新增 `conflict`（9 项单测）与 `generic_apply`（8 项单测）；`LicenseService` 端口参数由 `OperationKind` 改为 `&str` 以支持动态功能编号；workspace 124 项测试全绿（kernel 88 + platform 27 + CLI 6 + 集成 3）；fmt/clippy -D warnings 通过 | `copy` 撤销（Journal 删除型逆向，待契约评审）、undo 的 release kind、长路径分级、具体功能与 UI 接入 | 开发代理 |
| 2026-10-29 | Wave 1-1 配置/模板/UiState 存储 | 切片 | kernel 新增 `config`/`recipe`/`ui_state`（kernel 71 项含 25 项新增单测）；platform 新增 `config_store`（27 项含 12 项新增：损坏备份 E5009、高版本只读 E1007、UiState 隔离重置、模板 CRUD/导入导出、recipeId 路径穿越拒绝）；src-tauri 注册 12 条命令；**CI 七 job 全绿（run 40881782445，head 11f981c，0 failure 注解）**；`cargo fmt/clippy -D warnings`/`check_docs.py` 通过 | 设置 UI、`loadRecipe`、迁移链（v1 空）、UiState 离屏/clamp、E5009/E1007 用户提示、Web OPFS | 开发代理 |
| 2026-10-29 | CI 六 job 全绿 + W0-5 关闭 | 完成 | run 40872864062（head 23c1f45，push，0 failure 注解）：lint / deny（advisories+licenses+bans+sources + SBOM artifact）/ test·ubuntu / test·windows / web / desktop / bench 全 success；Windows 侧 `apply_flow` 3 项（含真实 plan→apply E2010 场景，`stats.total=2`）通过 → Windows 文件身份可用、C09 计划非空 | W0-6 剩 Windows 长路径与 C09 的 T 全量 UI 验收 | 开发代理 |

## 7. 关联

- 产品/测试/视觉：[产品方案 v1.19](./baseline/产品方案-v1.19.md)、[T v1.12](./baseline/功能测试方案与验收标准-T-v1.12.md)、[U v1.3](./baseline/UI视觉规范-U-v1.3.md)
- 契约追踪：[TRACEABILITY](./design/TRACEABILITY.md)、[设计 12 v0.4](./design/12-前端与桌面壳架构.md)
- 移植：[PDR-001](./porting/PDR-001-C09-重复文件查找.md)、[来源台账](./porting/feature-source-map.md)
- 待决：[OPEN-QUESTIONS](./OPEN-QUESTIONS.md)、[PENDING-DECISIONS](./baseline/PENDING-DECISIONS.md)
