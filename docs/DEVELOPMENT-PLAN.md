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
- kernel：C09 纯分组、`PlanDocument`/`plan_c09`、`FsAdapter`/`LicenseService`/`Journal`/`apply_plan`/`undo_plan`、`ProgressSink`/`CancelToken`（逐条进度、取消回滚）、路径 normalize/is_under 与 S02 blocked、plan→apply 内容指纹复核（`expected`+E2010，设计 01 v1.3）；通用执行器 `generic_apply` 覆盖 rename/move/delete/`copy`/`edit-content`（copy 新建逆向＝产物进隔离目录、覆盖/内容改写逆向＝`dataRef` 备份还原，设计 01 v1.7），撤销统一走共享 `execute_reverse`；编码检测与转码纯函数层 `encoding`（ADR-0014：chardetng 主路径＋encoding_rs，方案 §5.2 四级流水线，零阈值置信门控）。
- `crates/platform`：扫描/BLAKE3、原生 FsAdapter、FileJournal、保留期 cleanup，CLI 与桌面壳共用。
- CLI：C09 scan → plan → apply → undo/cleanup、S02、`--protected-dir`、`--retention-days`。
- 桌面：`src-tauri` Tauri v2 任务队列（`tasks.rs`）与 `task://progress`/`task://state`/`task://done`；`applyPlan`/`undoPlan` 返回 `{taskId}`；取消/暂停/恢复/重试（`retryTask`，仅失败条目、新 taskId 引用原 planId）/崩溃恢复（`recoverTask`）/预检（`preflightPlan`）与 list/get 已注册；`task://progress` 按方案 §9.1 节流区间下限合帧；LicenseService 由装配点注入（`TaskQueue::with_license`，方案 13.8）；C17a 最小操作日志落库（rusqlite bundled，ADR-0013：任务收尾单事务批量写、`logFullPaths` 脱敏、E1007/损坏降级不阻断批处理）；应用数据目录下 quarantine/journal/oplog。
- Web：React 19/Vite 7 C09 页面，U v1.3 token + 液态玻璃外壳；计划预览、确认执行、事件驱动进度/暂停/取消、撤销、保留期演练/永久清理；任务事件按 taskId 归档，避免 IPC 返回前完成导致丢报告；浏览器 mock IPC 流已验证。
- 验证：`cargo test --workspace` **167 项**通过（kernel 117 + platform 41 + CLI 6 + CLI 集成 3）；`cargo test --manifest-path src-tauri/Cargo.toml` 13 项通过；`scripts/e2e-tauri.mjs` 真实 WebView apply→undo、cleanup 演练与执行中取消回滚通过；C-6 断言通过；`pnpm build` 通过；`src-tauri` cargo check 通过；**CI 六 job 全绿（run 40872864062，2026-10-29；lint/deny/test·ubuntu/test·windows/web/desktop/bench 全 success；head 1420a3d 复验 run 40907291374 七 job 全 success）**。
- 门禁：fmt/clippy、docs、语料范围/哈希、迁移 schema。

### 3.2 部分完成/有明确缺口

| 域 | 已完成 | 未完成/缺口 |
|---|---|---|
| C09 产品化 | 纯分组、计划、apply/undo、S02、保留期、CLI/UI 切片、任务队列/事件、取消回滚、WebView E2E、plan→apply 内容再校验（E2010）、Windows 文件身份（ADR-0012）、Windows 长路径收口（设计 04 v1.3） | C09 的 T 全量 UI 验收（UNC 长路径行为矩阵待 Windows 实机） |
| 平台核心 | Native FsAdapter、Journal（含 dataRef 备份语义与三类 reverse 执行）、AlwaysAllow、路径/保留期、TaskQueue/TaskState/进度/取消/暂停、`retryTask` 手动重试、进度事件合帧、LicenseService 装配点注入、C17a 最小操作日志（rusqlite bundled，ADR-0013）、ConfigDocument + UiState + Recipe 存储与命令、通用执行器（rename/move/delete/`copy`/`edit-content`）+ 冲突枚举、S01 预检判定与采集、S02 目标路径执行期复核、S03 任务快照持久化与恢复 | 诊断日志轨（滚动文件/轮转参数待 ADR）、商业授权实现（注入口 seam 已建）、设置 UI、`loadRecipe`、Hashing 端口、archive/extract 执行器（C10 时接） |
| IPC/UI | C09 任务队列命令、`{taskId}+事件`、cleanup/storage 合约化、事件驱动 UI | CSP/capabilities、i18n、虚拟滚动、shadcn 完整组件、Vitest/Playwright、任务列表 UI |
| 桌面工程 | Tauri 编译/启动、CI desktop check | Windows WebView 构建验证、`src-tauri` 独立 Cargo.lock 的 cargo-deny/SBOM、安装包/签名 |
| 验证 | C-6、C-6 计划预览、apply/undo/保留期集成 | 24 条自动化语料的逐条断言 harness、C-7 阻塞、C-1/C-2/C-4/C-5/C-8 正式基准 |

### 3.3 未开始（按功能组）

- **阶段 0 文本/编码**：F01–F13（分割/合并/抽列/编码/表头/换行/替换/正则改名等）、C02/C03/C04/C05/C06/C07/C08/C10/C11/C12/C13/C14/C15/C18（C01 的 kernel 编码层已建，采样读取/plan 生成/执行接线待做，见 §6.1 2026-10-30 Wave 2-0a 行）。
- **阶段 0 安全/环境**：C-7 授权状态集（阻塞；S01/S02/S03 的内核与壳层切片已完成，见 §3.2 平台核心行）。
- **阶段 1**：C37 处理模板（存储/迁移/导入导出已完成，Phase A/B 的 plan/apply 与 UI 未做）、F15 AI 助手 Phase 1/2/3、设置与持久化 UI。
- **P1/P2**：C21–C63 中未进入阶段 0 的功能、CLI C40 正式命令面、性能基准结果台账。
- **商业/云端/Web/发布**：商业授权签发与 QuotaBased、云端服务端、Web/WASM 链路、安装包/签名/更新检查。

### 3.4 一句话状态

| 问题 | 结论 |
|---|---|
| 有几个产品功能完成 T 验收？ | **0** |
| 有几个功能有可运行纵向切片？ | **1**（C09） |
| 平台核心是否足够支撑第二个功能？ | **是**：S01/S02/S03、Config/UiState、通用文件操作主链、retryTask/合帧、授权注入 seam、C17a 最小日志已建；仍缺诊断日志轨、商业授权实现与 Hashing 端口（均为可后置平台件） |
| 现在最该做什么？ | **进入 Wave 2**（阶段 0 文本/文件主功能，F01–F04 起，依赖文本流式读写与编码检测）：Wave 1 平台件与通用执行器撤销链（copy/edit-content）已齐；剩余平台件（诊断日志、Hashing 端口）按依赖插入；W0-6 仅剩 C09 的 T 全量 UI 验收（人工项） |

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

**当前状态（2026-10-30）**：W0-1/W0-2/W0-3/W0-4/W0-5 已完成（W0-5 以 CI 六 job 全绿关闭）；W0-6 的工程项（内容再校验、Windows 文件身份、Windows 长路径收口）已全部落地——长路径经适配层 `\\?\`/`\\?\UNC\` 前缀统一收口（设计 04 v1.3，Windows CI 长路径回归待推送后复验）；仅剩 C09 的 T 全量 UI 验收（人工项），Wave 0 仍为“部分”。

### Wave 1：平台核心（阶段 0 的前置）

1. **ConfigDocument + UiState + Recipe 存储**（设计 03）：设置、最近目录、列宽/排序、模板运行偏好；不混入门禁/任务/日志。
   - **当前状态（2026-10-29）**：存储层完成（kernel 类型/默认值/来源视图/模板 v1→v3 迁移 + `platform::config_store` 原子写/损坏兜底/只读降级 + 12 条壳层命令）；设置 UI、`loadRecipe`、迁移链（v1 为空）与 UiState 恢复规则待补。
2. **通用文件操作主链**：C05/C06/C07/C18 的冲突枚举、预览、plan/apply、Journal 撤销；供后续 F/C 功能复用。
   - **当前状态（2026-10-30，切片完成）**：kernel `conflict`（三类冲突枚举、保留名/非法字符/结尾点空格/空名警告、`sequence` 命名、大小写折叠）与 `generic_apply`（rename/move/delete/`copy`/`edit-content` 执行、冲突二次裁决、Journal 可逆记录、取消回滚、E2010 指纹复核、E2009/E2006/E4001/E4004、outcome→`overwritten` 单列）已落地并含单测；**Journal 逆向补全**（设计 01 v1.7）：copy 新建目标逆＝产物进隔离目录（reverse=Quarantine），copy 覆盖与 edit-content 先 `backup_to_quarantine` 备份、逆＝`WriteAtomic{dataRef}` 还原；撤销任务与取消回滚共用 `execute_reverse`。长路径分级已随 W0-6 收口。**剩余**：archive/extract 执行器（C10 时接）、具体功能与 UI 接入。
3. **S01/S02/S03**：预检、保护目录执行层复核、断点续跑/任务持久化。
   - **当前状态（2026-10-29）**：S01 内核纯函数预检（磁盘余量判定式、只读/占用/占位、路径长度分级、UNC 降级、保护命中）与 platform 事实采集（statvfs/GetDiskFreeSpaceExW、写探测、2 秒 UNC 超时、LongPathsEnabled）已落地；壳层 `preflightPlan` 与 `applyPlan` 前置阻断（E2004 零副作用）。S02 执行期补 `rename/move` **目标路径**保护复核；配置来源名单在 plan→apply 间变化时旧预览失效（E1002）。S03 非终态任务快照持久化（`tasks.json` 原子写、损坏备份）、启动标记 `interrupted` 不自动续跑、`recoverTask` 的 resume/rollback/discard；Journal 增 `planKind`，undo 按计划类别退还配额。**剩余**：TC S01/S02 的 10–12 条 UI 用例尚未逐条执行、S03 实机崩溃演练、retryTask/事件合帧、UNC 探测线程超时残留的系统调用（已知取舍）。
4. **日志/诊断/授权注入**：C17a 最小日志、Journal 保留期、LicenseService 单机实现与商业接口边界。
   - **当前状态（2026-10-30，切片完成）**：Journal 保留期由 platform `retention` 覆盖（CLI/壳层 cleanup）；LicenseService 注入口 seam 已建（`with_license` 唯一构造器 + lib.rs 显式 AlwaysAllow，方案 13.8）；**C17a 最小操作日志已落地**（ADR-0013，用户确认三项决策：rusqlite bundled、拆分列恒写＋完整路径受 `logFullPaths`、任务收尾批量事务写）——设计 10 §7 第 1 项待决关闭。剩余：诊断日志轨（滚动文件/轮转参数待 ADR）、CLI 侧操作日志（OQ-30）、商业授权实现（C-7 阻塞）。
5. **Hashing/FsAdapter 能力补齐**：长路径、占位文件、分块读（Windows 文件身份已由 ADR-0012 落地）。
   - **当前状态（2026-10-30）**：长路径已随 W0-6 收口（适配层 `\\?\`/`\\?\UNC\` 前缀统一，设计 04 v1.3）；剩余为占位文件探测增强与分块读（流式哈希已有 64 KiB 分块实现，缺口在大文件专用通道）。

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

## 5. 下一批任务（Wave 2 起步）

> Wave 0 的 W0-1…W0-5 已完成、W0-6 仅剩人工项；backlog 已滚动到 Wave 2，历史证据见 §6.1。

| 任务 | 状态 | 产出 | 验收 | 依赖 |
|---|---|---|---|---|
| W2-0a 编码检测/转码纯函数层 | **切片** | kernel `encoding`（ADR-0014） | kernel 10 项单测；K01–K08 语料接线待做 | 方案 §5.2 |
| W2-0b 文本采样/流式读取（platform） | 未开始 | 采样读取、行数估算、逐行流式读（供 C01/F01–F04） | 不整文件读入内存；T03 超长行、T06 无换行可报；采样窗口取方案值或调用方显式入参，不自行定值 | W2-0a |
| C01 编码转换纵向切片 | 未开始 | kernel plan 生成 + apply 复用 `edit-content`（备份→转码→原子写回）+ CLI/壳层接线 | TC C01 PRE/EXE/RBK；K01–K08 | W2-0b、Wave 1-2 执行器 |
| F01 TXT 按大小分割 | 未开始 | plan/apply/Journal | TC F01（仅行边界断开、超长行独占并标注、T03/T06、C-2） | W2-0b |
| F02–F04 TXT 分割/合并 | 未开始 | 同上 | TC F02/F03/F04 | F01 |
| OQ-08 语料断言 harness | 未开始 | `crates/kernel/tests/corpus.rs` + `assertions.tsv` | 24 条自动化语料从“生成+范围核对”升级为“逐条断言” | OQ-08 已批准 |
| 人工项收尾 | 部分 | C09 T 全量 UI、UNC/长路径矩阵、S03 崩溃演练 | `fixtures/corpus/manual-results.md` 台账 | Windows 实机 |

## 6. 完成判定与维护

- **完成即标注（项目规则）**：每完成一个任务/功能/切片，按 [AGENTS 协作纪律第 4 条](../AGENTS.md#协作纪律ai-开发四条铁律) 更新本文件、`TRACEABILITY` 及对应 PDR/测试/CHANGELOG/ADR；没有可复核证据只能写“切片/部分”，不得写“完成”。
- **每项功能**：必须过 fmt/clippy/test、相关语料/基准、文档锚点；涉及批量写必须有 plan/apply 与 Journal；涉及授权必须走 LicenseService。
- **每个 Wave**：更新本文件与 `TRACEABILITY`；Wave 完成不等于阶段验收，最终以 T 的四级验收为准。
- **状态更新**：完成/切片/部分/未开始/阻塞五档只能按证据改；没有测试或 E2E 的不写“完成”。
- **范围控制**：不因本计划新增产品阈值或行为；新增功能/改优先级回到方案 §11.6 或 T 升版。
- **已知偏差**：S03 任务快照持久化与 `recoverTask` 已实现；`retryTask`（仅失败条目、新 taskId 引用原 planId、Journal 幂等排除）与 `task://progress` 壳层合帧（方案 §9.1 区间下限）已实现，E2E 尚未覆盖重试场景（需可复现失败注入）。Windows 长路径已收口（适配层统一 `\\?\` 前缀，设计 04 v1.3），UNC 长路径 canonicalize 行为矩阵仍需 Windows 实机核对。C17a 最小操作日志已落地（ADR-0013；审计轨尽力而为、降级不阻断；CLI 侧与诊断日志轨未做）。S01/S02 已有内核/壳层判定与最小 UI 提示，TC 的 10–12 条 S01/S02 用例尚未逐条执行，S03 尚未做实机崩溃演练。`cleanupRetention`/`getStorageInfo` 已作为壳层追加命令写入设计 01 §4.4.1（v1.6 增 `removedLogRecords`）；`src-tauri` 依赖审计已由 CI deny job 覆盖（W0-5 完成）。W0-6 仅剩 C09 的 T 全量 UI 验收（人工项），不是“已完成”。

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
| 2026-10-29 | Wave 1-3 S01/S02/S03 | 切片 | kernel 新增 `preflight`（9 项单测：余量临界 ±1、临时峰值、200/259/260 分级、保护 canonical/字面、只读/占用/占位、UNC 超时降级、空输入逐项结论）；`generic_apply` 补目标路径保护复核（+2 项）；Journal 增可选 `planKind`（+2 项，旧日志兼容），undo 按 kind 退还配额；platform 新增 `preflight`（35 项中 +8：statvfs、写探测、UNC 2 秒超时、long-path、same-volume）；`src-tauri/src/tasks.rs` 快照持久化 + `interrupted` + `interrupted → resume/rollback/discard`（6 项测试，含已完成条目按 Journal 幂等跳过、回滚恢复、放弃保留、损坏快照备份）；壳层 `preflightPlan`/`recoverTask` 命令、`applyPlan` E2004 前置阻断与配置名单变更 E1002；web 预检报告与恢复提示条；`cargo test --workspace` 146 项、壳层 8 项、`./node_modules/.bin/tsc --noEmit`/`vite build`、fmt/clippy -D warnings/`cargo deny`（root + src-tauri）全绿；**CI run 40907291374（head 1420a3d）七 job 全 success** | TC S01/S02 的 10–12 条 UI 用例逐条执行、S03 实机崩溃演练、retryTask/事件合帧、UNC 超时线程残留（已知取舍）、C09 长路径 | 开发代理 |
| 2026-10-30 | Wave 1-4a：`retryTask` + 进度合帧 + LicenseService 注入口 | 切片 | `src-tauri/src/tasks.rs` 新增 `retry`（仅对终态 partial-failed/failed 的 apply 任务；缺省重试全部失败条目、`entryIds` 子集校验 E1002、Journal 已 applied 幂等排除、编排级失败重跑全部条目、undo 任务拒绝；单测 3 项）与 `PROGRESS_COALESCE_WINDOW` 合帧（取方案 §9.1「进度刷新节流 100–200 ms」区间下限；同阶段合帧、阶段迁移即推并补发最新条目计数、`task://state`/`task://done` 不合帧；单测 1 项）；`with_license` 成为唯一构造器即方案 13.8 注入口，lib.rs 装配点显式注入 AlwaysAllow；lib.rs 注册 `retryTask` 命令；web 新增"重试失败项"入口（partial-failed/failed 终态）与 mock 失败→重试演示、undo 总数改按壳层 Journal 统计修正；设计 01 升 v1.5、设计 12 升 v0.6；`cargo test --workspace` 146 项、壳层 12 项、fmt/clippy -D warnings（root + src-tauri）、`pnpm build`、check_docs/check_schema、`cargo deny`（root + src-tauri）全绿（本地，待推送后 CI 复验） | C17a 操作/诊断日志（设计 10 §7 schema 待决）、商业授权实现、E2E 重试场景（需可复现失败注入）、TC S01/S02 UI 用例、S03 实机崩溃演练 | 开发代理 |
| 2026-10-30 | W0-6 Windows 长路径收口 | 切片 | `platform::fs` 的 `fs_path`/`strip_extended_path` 提为 crate 收口点并新增 `canonicalize_display`（Windows canonicalize 恒带 `\\?\`，剥回显示形式）；`extended_path` Windows 分支加前缀前经 kernel `try_normalize` 归一分隔符与 `.`/`..` 段（`\\?\` 不吃 Win32 归一化；相对路径与 `C:` 盘符相对形式不加前缀）；`scan.rs` 枚举/哈希/文件身份、`preflight.rs` probe_root/probe_write/path_check/probe_unc、`protected.rs` prepare、CLI 与 src-tauri 的 PlanFile canonical 全部改走收口点；新增 Windows 单测 `extended_path_normalizes_before_prefixing_and_keeps_relative_paths` 与长路径回归 `scans_long_paths_beyond_the_win32_limit`（>260 字符真实深树，Windows CI 执行）；`cargo test --workspace` 146 项、clippy -D warnings、`cargo check --target x86_64-pc-windows-msvc`（临时 blake3 pure 绕过汇编后还原）零 error、check_docs 全绿（本地，待推送后 Windows CI 复验）；设计 04 升 v1.3 | T C09 全量 UI 验收（人工）、UNC/长路径 canonicalize 行为矩阵（Windows 实机，R01/R06）、retention cleanup 的直接 fs 调用（应用数据目录内，超长场景待定） | 开发代理 |
| 2026-10-30 | Wave 1-4b：C17a 最小操作日志（ADR-0013） | 切片 | **依赖**：`rusqlite 0.41`（bundled，MIT OR Apache-2.0）入 `crates/platform`，`cargo deny` 四项 root + src-tauri 全绿（advisories 因本地 github.com 不可达以 2026-10-29 缓存库 `--offline` 复核，CI 推送后拉新库复验）。**存储**：`platform::oplog` 单表 `operation_log` + `PRAGMA user_version` 只增不改迁移、`plan_id`/`task_id`/`ts` 索引、损坏留 `.corrupt.<ts>` 副本重建（E5009）、高版本 E1007；单测 5 项。**接线**：`TaskQueue::with_license` 增 `log_full_paths` 读取口（03 §4.1 默认 false）；任务收尾单事务批量写——apply 记批次 start/finish＋逐条 entry（路径取自 Journal forward），undo 记 `phase=rollback`/`status=reverted`，编排级失败记批次＋错误码；审计轨写失败/未启用降级 stderr 不阻断批处理（壳层测试 1 项实证）；`cleanupRetention` apply 时清理操作日志并回填 `removedLogRecords`（设计 01 v1.6 增量字段）。**文档**：ADR-0013（用户确认三项决策）、设计 10 升 v0.2（§7 第 1 项待决关闭）、设计 01 升 v1.6。验证：`cargo test --workspace` **151 项**（platform 40 含 oplog 5）、壳层 13 项、fmt/clippy -D warnings（root + src-tauri）、check_docs/check_scope/check_schema 全绿（本地，待推送后 CI 复验；rusqlite bundled 的 Windows C 编译由 CI Windows job 实测） | 诊断日志轨（滚动文件/轮转待 ADR）、`logging.retentionDays` 的设置 UI、CLI 侧操作日志（OQ-30）、C06 反查 UI、支持包导出 | 开发代理 |
| 2026-10-30 | Wave 2-0a：编码检测与转码纯函数层（方案 §5.2，ADR-0014） | 切片 | kernel 新增 `encoding` 模块＋依赖 `chardetng 1.0`/`encoding_rs 0.8`（许可经 `cargo info` 实证 Apache-2.0 OR MIT / (Apache-2.0 OR MIT) AND BSD-3-Clause，`cargo deny` 四项双 lock 全绿）。**检测**：① BOM 优先（UTF-32 先于 UTF-16 防前缀误判）② std 严格 UTF-8（纯 ASCII/过短默认 UTF-8 并提示；含控制字符判据防二进制蒙混）③ chardetng 主路径（Iso2022Jp=Deny）④ 平台先验仅报告不覆盖结论；置信规则零数值阈值：判定编码解码采样零替换错误且无 \t\n\r 外控制字符→高置信，否则需人工确认（§5.2 绝不静默猜错）；`require_encoding` 未确认取编码→E3001。**转码**：`transcode(data, source, target, invalid)`——UTF-8/UTF-8 BOM/GB18030/UTF-16LE/BE 输出（BOM 由调用方按片控制，§5.1 UTF-16 每片必须 BOM；UTF-16 输出自编码——WHATwg encode 只定义到 UTF-8，实测 encoding_rs 对 UTF-16 目标返回 UTF-8）；Abort 策略严格解码失败→E3002，ReplaceAndCount 计数上报；GB18030 为 GBK/GB2312 超集统一处理。单测 10 项（BOM 顺序/ASCII/UTF-8/GB18030 采样/非法字节/转码往返/UTF-16 BOM/E3002）。`cargo test --workspace` **167 项**、fmt/clippy -D warnings、deny、check_docs 全绿（本地，待推送后 CI 复验） | C01 接线（platform 采样读取/plan 生成/apply 接入 generic_apply edit-content）、F01–F04 接线、legacy 输出端不可映射字符策略（暂 WHATwg 数字实体）、>2 GB 流式检测/转码、chardetng 采样规模的实机语料校准（R 系语料） | 开发代理 |
| 2026-10-30 | Wave 1-2 收口：Journal 逆向补全 + copy/edit-content 执行器 | 切片 | **契约**：设计 01 升 v1.7（§8 追加 reverse 推导规则——copy 新建目标逆＝产物进隔离目录、copy 覆盖与 write_atomic 逆＝dataRef 备份还原、新建产物 `dataRef` 省略对齐契约字面）；设计 04 升 v1.4（FsAdapter 追加 `backup_to_quarantine`/`quarantine_target_path`，默认 E5001、CliFsAdapter 实现，长路径前缀收口同样生效）。**kernel**：`FsOp::data_ref` 改 `Option<String>`；`FsOp::inverse` 支持 WriteAtomic 备份还原；undo 执行器从"仅 quarantine rename"扩为三类 reverse 共享 `execute_reverse`（撤销任务与取消回滚同一实现）；`generic_apply` 接入 `copy`（新建/覆盖两路 reverse）与 `edit-content`（`ContentEditor` 注入变换，备份→改写→原子写回），copy 目标纳入 S02 保护复核。**测试**：journal 2 项（逆推/dataRef 省略）、generic_apply 4 项（copy 新建撤销→产物进隔离、copy 覆盖撤销→备份还原原内容、edit-content 撤销→字节还原、缺 editor E1002）、platform 1 项（备份复制保留原件＋预留路径唯一）；`cargo test --workspace` **157 项**（kernel 107 + platform 41 + CLI 6 + 集成 3）、壳层 13 项、fmt/clippy -D warnings 全绿、check_docs 通过（本地，待推送后 CI 复验） | archive/extract 执行器（C10 时接）、F01–F04 接入本执行器、TC 语料回归 harness | 开发代理 |

## 7. 关联

- 产品/测试/视觉：[产品方案 v1.19](./baseline/产品方案-v1.19.md)、[T v1.12](./baseline/功能测试方案与验收标准-T-v1.12.md)、[U v1.3](./baseline/UI视觉规范-U-v1.3.md)
- 契约追踪：[TRACEABILITY](./design/TRACEABILITY.md)、[设计 12 v0.4](./design/12-前端与桌面壳架构.md)
- 移植：[PDR-001](./porting/PDR-001-C09-重复文件查找.md)、[来源台账](./porting/feature-source-map.md)
- 待决：[OPEN-QUESTIONS](./OPEN-QUESTIONS.md)、[PENDING-DECISIONS](./baseline/PENDING-DECISIONS.md)
