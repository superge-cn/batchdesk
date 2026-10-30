# 01 内核接口与 IPC 契约

> 状态：已评审·实现冻结 v1.21（2026-10-27 评审；2026-10-28 增量修订，用户确认；2026-10-29 追加任务队列说明、E2010、S01/S02/S03；2026-10-30 追加 retryTask 实现说明、进度合帧、CleanupOutcome.removedLogRecords、§8 reverse 推导规则与 F01 `split` 动作/`range` 字段、F02/F03 分片基准、F04 `merge` 动作/载荷、F05 `csv` 载荷与 `plan_f05`、F06 `csvMerge` 载荷与 `plan_f06`、F07 `csv-extract` 动作/载荷与 `plan_f07`、F08 `csv-rows` 动作/载荷与 `plan_f08`、F06 T07/T08 告警 `no-header-positional`/`header-normalized`/`duplicate-column-names`、F09 `plan_f09`/`csvRows` 区间字段/`read_ranges`、F10 `listing` 动作/`plan_f10`/`DirectoryLister`/`write_atomic_with`、F11 `plan_f11`/逐级 `create-dir`/`CreateDir`/`RemoveDir` Journal、F12 `plan_f12`/两阶段 rename、2026-10-31 追加 F12 计划级长路径分级与重命名族语料断言、F13 `plan_f13`/正则捕获组替换） ｜ 适用：crates/kernel、src-tauri、web/ ｜ 日期：2026-10-27
>
> 本文件是**工程实现契约**：定义表现层与应用层、应用层与领域引擎之间的可执行接口。
> 产品行为与数值阈值一律回指基线，本文不另立；本文新增的**字段命名、命令名、错误码位**属工程契约，评审冻结后生效（见 §9）。

## 0. 权威来源索引

| 议题 | 权威章节 | 本文角色 |
|---|---|---|
| 分层与依赖方向 | 方案 §4.2 | 落地为调用边界图 |
| 纯函数管道 | 方案 §4.3 | 落地为 trait 签名 |
| plan/apply 两段式 | 方案 §7.1 | 落地为 Plan/Apply DTO |
| 错误分段 | 方案 §4.6 | 落地为具体码位表 |
| 文件系统适配 | 方案 §4.7 | 落地为 FsAdapter trait |
| 撤销与 Journal | 方案 §7.8 | 落地为 Journal 记录结构 |
| 授权抽象 | 方案 §12.4 / §13.8 | 落地为 LicenseService trait 签名 |
| 强制力边界 | 方案 §8.5 / §13.5 | 约束 IPC 面的授权语义 |
| 进度/取消/重试 | 方案 §7.5–§7.7 | 落地为事件与状态机 |
| Web 通道能力 | 方案 §13.1–§13.2 | 约束同一契约的双通道实现 |

## 1. 调用边界

```text
web/ (表现层，零强制力 §8.5/§13.5)
  │  invoke(command, payload) / listen(event)
  ▼
src-tauri (应用层+壳：任务队列 §4.4、文件系统适配 §4.7、授权注入 §12.4)
  │  函数调用（同进程）
  ▼
crates/kernel (领域引擎：纯变换器层 + 执行编排)
  ▲  端口注入（FsAdapter / Clock / Random / ProgressSink / CancelToken / LicenseService）
     —— 实现由壳提供；kernel 内部禁止直接 I/O、时钟、环境读取（§4.3、不变量 1）
```

- **Web 形态**：同一 kernel 编译为 WASM（§13.5），FsAdapter 由 OPFS/FS Access 适配实现，IPC 命令层替换为 WASM 导出面；**WASM 导出面不得暴露授权/配额判定入口**（不变量 11）。
- kernel 不得依赖 Tauri / 桌面 API（不变量 0）；kernel 禁止网络调用（不变量 6）。
- 授权强制力只存在于 kernel 执行层的操作类别复核（§8.5），壳层与 web/ 的门禁仅为体验提示。

## 2. 线协议通用约定

| 项 | 约定 |
|---|---|
| 编码 | JSON（UTF-8）；二进制数据走路径引用或分页句柄，不内嵌大 payload |
| 命名 | 线上统一 `camelCase`；Rust 侧 `#[serde(rename_all = "camelCase")]`，TS 侧直用 |
| 时间 | RFC 3339 UTC 字符串 |
| 路径 | 一律字符串原样传递；合法性、大小写折叠、长路径前缀由 FsAdapter 处理（§4.7，不变量 8） |
| 错误 | 全部命令返回 `Result<T, IpcError>`，禁止 anyhow 字符串透出（不变量 3） |
| 枚举 | 线上用小写下划线字符串（如 `"gb18030"`、`"on_conflict_skip"`） |

### 2.1 错误结构

```ts
interface IpcError {
  code: string;        // E1xxx–E9xxx，见 §7 码表
  message: string;     // 用户可读（文案分级见方案 §7.11）
  detail?: string;     // 技术细节，进日志不出界面
  retryable: boolean;  // 供 C16 失败重试（§7.6）判定
  taskId?: string;     // 任务类错误回填
}
```

### 2.2 进度与取消

- 进度经事件通道推送（事件名见 §4.3），字段：`taskId, phase, done, total, message`。
  `phase ∈ queued | scanning | planning | executing | journaling | finalizing`。
- **节流**：推送频率上限为性能阈值，引用方案 §9.1 既定值，本文不设数值；壳层负责合帧。已实现（v1.5）：桌面壳按方案 §9.1「进度刷新节流 100–200 ms」区间的下限，对同阶段 `task://progress` 做合帧（`TaskSummary` 仍逐条更新）；阶段迁移立即推送并在窗口内补发最新条目计数，`task://state`/`task://done` 不合帧。
- **取消**（§7.7）：壳层持有原子取消标志，以 `CancelToken` 端口传入 kernel 执行循环，每个条目边界检查一次；取消后已落盘动作按 Journal 反向回滚（§7.8）。
- **暂停/恢复**：仅壳层队列语义（§4.4），kernel 不感知暂停。

### 2.3 分页

list 类命令统一入参 `{ offset: number, limit: number }`，出参 `{ items: T[], total: number }`。
`limit` 上限引用方案 §13.10.2 传输既定值，本文不设数值。

## 3. kernel 对外 trait（Rust 签名）

以下 trait 均定义在 kernel、实现在壳（桌面）或浏览器适配层（Web）。签名是契约，字段冻结后改名视为破坏性变更。

```rust
/// 文件系统端口（§4.7）。全部路径操作收口于此，业务代码禁止自拼 \?\ 前缀。
pub trait FsAdapter: Send + Sync {
    fn read_bytes(&self, path: &str) -> Result<Vec<u8>, KernelError>;
    fn write_atomic(&self, path: &str, data: &[u8]) -> Result<(), KernelError>; // 临时文件+改名（§4.3）
    fn rename(&self, from: &str, to: &str, policy: ConflictPolicy) -> Result<PathOutcome, KernelError>;
    // ConflictPolicy = Skip | Sequence | Overwrite（§5.13 四策略中的 ask 已在预览期消化）；
    // 由 apply 编排从 ApplyOptions / 逐项 resolution 翻译而来，FsAdapter 只执行单次路径操作。
    fn copy(&self, from: &str, to: &str, policy: ConflictPolicy) -> Result<PathOutcome, KernelError>;
    fn move_path(&self, from: &str, to: &str, policy: ConflictPolicy) -> Result<PathOutcome, KernelError>;
    fn remove_to_quarantine(&self, path: &str) -> Result<String, KernelError>; // 删除一律进隔离目录（不变量 7）
    fn list_dir(&self, path: &str, filter: &ScanFilter) -> Result<Vec<DirEntryInfo>, KernelError>;
    fn stat(&self, path: &str) -> Result<StatInfo, KernelError>;
}

/// 执行循环的回调端口（§4.3 纯函数的 I/O 出口）
pub trait ProgressSink: Send + Sync { fn report(&self, ev: ProgressEvent); }
pub trait CancelToken: Send + Sync { fn is_cancelled(&self) -> bool; }

/// F10 目录清单的流式遍历/写出端口（apply 期由壳层/平台注入）。
/// ListingOptions { includeFiles, includeDirs }
/// ListingStats { files, dirs, placeholders, skipped, maxDepth }
pub trait DirectoryLister: Send + Sync {
    fn write_listing(&self, root: &str, options: &ListingOptions, out: &mut dyn std::io::Write)
        -> Result<ListingStats, KernelError>;
}

/// 授权端口（§12.4 / §13.8）。三种形态只换实现，接口与调用点不变。
pub trait LicenseService: Send + Sync {
    fn capability(&self, op: OperationKind) -> Capability;  // Allow | Deny(reason) | Quota{used, limit}
    fn consume(&self, op: OperationKind, n: u32) -> Result<(), KernelError>;
    fn release(&self, op: OperationKind, n: u32);           // 配额退还：任务取消/回滚后对不再生效的条目调用
}
// 单机版=AlwaysAllowLicense（consume/release 均 no-op）；商业版=完整授权实现；Web 版=QuotaBased（§13.8）。
// release 语义：仅退还"已 consume 但未实际生效"的条目（cancelling 期与撤销完成后由执行编排调用）；
// QuotaBased 实现必须保证退还后 used 不得为负；退还仅作用于配额计数，不产生任何显示层承诺（不变量 11）。

/// 环境端口：kernel 内任何时间/随机数经此注入（不变量 1）
pub trait Clock: Send + Sync { fn now(&self) -> OffsetDateTime; }
pub trait RngSource: Send + Sync { fn fill(&self, buf: &mut [u8]); }
```

> `KernelError` 是 kernel 内部错误类型：字段与 `IpcError` 对齐（`code`/`message`/`detail`/`retryable`），由壳层在 IPC 边界映射为 `IpcError`；禁止直接序列化到线上。

> **FsAdapter 扩展能力**：分块读、磁盘余量探测、UNC 连通/写权限探测、占用/只读探测、占位文件识别、
> `restore_from_quarantine`、`read_ranges` 等扩展签名见 [04-文件系统适配层契约](04-文件系统适配层契约.md) §4——属同一 trait 的 v1 冻结面，不在本文重复定义。

### 3.1 纯变换器层签名样例（§4.3 / §5.2）

```rust
// 输入输出全走参数与返回值；同类函数不再接受任何环境依赖
pub fn detect_encoding(data: &[u8]) -> EncodingVerdict;                 // §5.2 四级流水线
pub fn plan_f05(input: PlanF05Input) -> PlanDocument;                  // §5.3 CSV 分割（表头/记录边界）
pub fn plan_f06(input: PlanF06Input) -> PlanDocument;                  // §5.4 CSV 合并（列对齐/来源列）
pub fn plan_f07(input: PlanF07Input) -> PlanDocument;                  // §5.15 CSV 抽取指定列
pub fn plan_f08(input: PlanF08Input) -> PlanDocument;                  // §5.15 CSV 抽取指定数量行
pub fn plan_f09(input: PlanF09Input) -> PlanDocument;                  // §5.15 CSV 抽取指定位置行（X–Y）
pub fn plan_f10(input: PlanF10Input) -> PlanDocument;                  // §5.15 生成目录清单（CSV）
pub fn plan_f11(input: PlanF11Input) -> PlanDocument;                  // §5.15/§5.8 按清单批量建文件夹
pub fn plan_f12(input: PlanF12Input) -> PlanDocument;                  // §5.13 批量重命名（两阶段＋长路径分级）
pub fn plan_f01(input: PlanF01Input) -> PlanDocument;                  // §5.1 按字节分片
pub fn plan_f02(input: PlanF02Input) -> PlanDocument;                  // §5.15 按行数分片
pub fn plan_f03(input: PlanF03Input) -> PlanDocument;                  // §5.15 按份数分片
pub fn plan_f04(input: PlanF04Input) -> PlanDocument;                  // §5.15 TXT 合并（N:1）
pub fn plan_f13(input: PlanF13Input) -> PlanDocument;                  // §5.5 正则重命名（复用重命名核心）
```

约定：`plan_*` 一律纯函数、绝不落盘（不变量 2）；`apply` 由执行编排层按 `PlanDocument` 驱动 FsAdapter 完成，并逐条写 Journal（§7.8）。

### 3.2 公共 DTO 字段定义（工程契约，冻结后只增不改）

§3/§4/§5 引用但未展开的类型在此定义；命名冻结规则同 §9.2。

```ts
// —— 操作类型（kind 与方案 §3.2 功能编号一一对应，值＝小写编号）——
// v1 冻结值：f01–f13；c01 c02 c03 c04 c07 c08 c09 c10 c11 c12 c13 c14 c59 c60。
// 不设独立 kind：C05/C15–C20 为横切能力（内嵌于各功能 plan/apply）；F14/C43–C45 并入授权模块；S01/S02 为横切检查。
// P1 功能（C21–C42 等）实现期按 plan 生成器覆盖追加；追加即兼容，禁复用。
type OperationKind = "f01" | "f02" | "f03" | "f04" | "f05" | "f06" | "f07" | "f08" | "f09"
                  | "f10" | "f11" | "f12" | "f13"
                  | "c01" | "c02" | "c03" | "c04" | "c07" | "c08" | "c09" | "c10"
                  | "c11" | "c12" | "c13" | "c14" | "c59" | "c60" | (string & {});

// —— 路径冲突策略（FsAdapter.rename/copy/move_path 入参；"ask" 只在 ApplyOptions 中出现）——
type ConflictPolicy = "skip" | "sequence" | "overwrite";

// —— 授权判定结果（LicenseService.capability 出参）——
type Capability =
  | { kind: "allow" }
  | { kind: "deny"; reason: string }
  | { kind: "quota"; used: number; limit: number };

// —— 扫描过滤（scanDirectory 入参；字段命名属工程契约）——
interface ScanFilter {
  extensions?: string[];      // 不含点，如 ["csv"]；空=不过滤
  includeHidden?: boolean;    // 默认 false（安全默认值，不变量 7）
  recursive?: boolean;        // 默认 false
  maxDepth?: number;          // recursive=true 时生效，未设=不限
}

// —— 条目信息（list_dir / stat 出参）——
interface EntryAttrs { readonly?: boolean; hidden?: boolean; placeholder?: boolean; }
                             // placeholder=网盘占位文件（§4.9 属性位识别，默认跳不处理；E02 语料锚点）
interface DirEntryInfo { path: string; name: string; kind: "file" | "dir" | "symlink";
                         size: number; mtime: string; attrs: EntryAttrs; }
interface StatInfo     { kind: "file" | "dir" | "symlink"; size: number; mtime: string; attrs: EntryAttrs; }

// —— 单次路径操作结果（rename/copy/move 返回；apply 编排据此写 Journal）——
type PathOutcome = { outcome: "applied" | "skipped" | "replaced"; finalPath: string };
                   // skipped=按策略跳过撞名；replaced=执行了覆盖（必须可辨识，§7.7"没有静默处理"）

// —— 进度事件（§2.2 / §4.3）——
interface ProgressEvent {
  taskId: string;
  phase: "queued" | "scanning" | "planning" | "executing" | "journaling" | "finalizing";
  done: number; total: number; message?: string;
}

// —— 授权显示摘要（getLicenseState / license://changed；仅显示，web 零强制力，不变量 11）——
interface LicenseStateSummary {
  tier: "g0" | "g1" | "g2" | "g3" | "trial" | "invalid";
  expiry?: string;                                   // RFC 3339 UTC
  grace?: { active: boolean; until?: string };       // 宽限期（§8 缓冲期）
  quota?: { used: number; limit: number };           // QuotaBased 形态（§13.8）
}

// —— 任务状态（§6 状态机的线协议值）——
type TaskState = "queued" | "running" | "paused" | "cancelling" | "interrupted"
               | "completed" | "partial-failed" | "failed" | "cancelled";
// interrupted：应用异常退出后由启动恢复加载；只能经 recoverTask 续跑/回滚/放弃，不自动执行（§6、S03）。

// —— S01 预检（只读结论；kernel 判定，壳层采集事实）——
type PreflightLevel = "pass" | "warn" | "block";
interface PreflightItem {
  kind: "disk-space" | "protected" | "readonly" | "locked" | "placeholder"
      | "long-path" | "unc" | "path";            // 追加式枚举，冻结后只增不改
  level: PreflightLevel;
  message: string;
  path?: string;
  code?: string;                                  // E1003/E2002/E2003/E2004/E2005/E2006/E2009
  hint?: string;
}
interface PreflightReport {
  items: PreflightItem[];
  level: PreflightLevel;                          // 全项最高级别
  pass: number; warn: number; block: number;
  taskBlocked: boolean;                           // 仅磁盘空间不足等整任务阻断；条目级阻断由计划 skipped
}

// —— 分页容器（§2.3）与各页条目 ——
interface Page<T> { items: T[]; total: number; }
type DirPage      = Page<DirEntryInfo>;
interface TaskSummary { taskId: string; kind: OperationKind; state: TaskState;
                        phase: ProgressEvent["phase"]; done: number; total: number;
                        channel?: "local" | "cloud";  // 仅 Web 形态出现；桌面省略（篇 02 §1）
                        startedAt?: string; finishedAt?: string; message?: string; }
type TaskPage     = Page<TaskSummary>;
interface TaskDetail  { summary: TaskSummary; entries: ApplyReport["entries"]; }
interface UndoableSummary { planId: string; kind: OperationKind; entryCount: number;
                            executedAt: string; expiresAt?: string;
                            scope?: "full" | "products-only"; } // Web 形态为 products-only；桌面省略（篇 02 §3.4）
type UndoablePage = Page<UndoableSummary>;
interface LogRecord { ts: string; level: "debug" | "info" | "warn" | "error"; category: string;
                      taskId?: string; message: string; detail?: string; }  // 结构引用 §9.6
type LogPage      = Page<LogRecord> & { ephemeral?: boolean };  // Web 会话内日志为 true；桌面省略（篇 02 §3.4）
```

- `ConfigDocument` 不在此定义，见 [03-配置schema与迁移契约](03-配置schema与迁移契约.md)。
- 分页数值上限引用 §13.10.2（§2.3），`Page` 容器本身不设数值。

## 4. IPC 命令清单（Tauri command / Web 导出面）

> AI 助手（F15）的 `aiAsk`/`aiCancel`/`getAiStatus` 与 `ai://*` 事件见 [14-AI助手与工具调用契约](14-AI助手与工具调用契约.md)，不在本篇重复定义；AI 不得直接 `applyPlan`。

方向均为 `web → 壳`；返回值见各表。命令名冻结后仅允许追加，不重命名（§9）。

### 4.1 扫描、预览与执行（§7.1–§7.5）

| 命令 | 入参 | 出参 | 依据 |
|---|---|---|---|
| `scanDirectory` | `{path, filter: ScanFilter, page}` | `DirPage` | §4.2 工作区 |
| `planOperation` | `{kind, params, sourcePaths}` | `PlanDocument`（纯函数结果，含 warnings/conflicts） | §7.1 / §7.4 |
| `preflightPlan` | `{planId}` | `PreflightReport`（S01 预检结论，逐项 pass/warn/block；只读，不改变计划） | 方案 §3.3、§4.9、§9.1 |
| `applyPlan` | `{planId, options: ApplyOptions}` | `{taskId}`（立即返回，进度走事件；预检整任务阻断时返回 E2004 且零副作用） | §7.1 |
| `cancelTask` | `{taskId}` | `void`（幂等） | §7.7 |
| `pauseTask` / `resumeTask` | `{taskId}` | `void` | §7.7 / §4.4 |
| `recoverTask` | `{taskId, action: "resume" \| "rollback" \| "discard"}` | `{taskId}`（仅对 interrupted 任务；rollback 返回新的 undo 任务 ID） | 方案 §9.3、§15.3.2（S03） |
| `retryTask` | `{taskId, entryIds?}` | `{taskId}`（C16 只重试失败条目） | §7.6 |
| `listTasks` | `{page}` | `TaskPage` | §6.5 |
| `getTask` | `{taskId}` | `TaskDetail`（含 per-entry ApplyReport） | §7.5 |

### 4.2 撤销（§7.8，不变量 4）

| 命令 | 入参 | 出参 |
|---|---|---|
| `listUndoable` | `{page}` | `UndoablePage`（按 plan 粒度） |
| `undoPlan` | `{planId}` | `{taskId}`（回滚本身也是任务，走同一状态机） |

### 4.3 事件（壳 → web）

| 事件名 | payload | 说明 |
|---|---|---|
| `task://progress` | `ProgressEvent` | §2.2 |
| `task://state` | `{taskId, state: TaskState}` | 状态机迁移推送 |
| `task://done` | `{taskId, summary: ApplyReport}` | 终态推送（成功/失败/取消三态由 state 区分） |
| `license://changed` | `LicenseStateSummary` | 仅通知**显示状态**；web 端不得据此做放行/拦截逻辑（不变量 11） |

- 终态顺序固定为 `task://state` → `task://done`；`task://done` 的 `summary.entries` 对撤销任务同样成立：成功恢复记 `ok`、失败记 `failed`，便于 `getTask`/UI 复用同一报告结构。
- `ProgressEvent.done/total` 的阶段语义：`executing` 表示条目总量进度；`journaling`/`finalizing` 可上报阶段内计数。`TaskSummary.done/total` 只保留条目总量，不随后两阶段重置。

### 4.4 配置、日志、授权

| 命令 | 入参 | 出参 | 依据 |
|---|---|---|---|
| `getConfig` | `{scope}` | `ConfigDocument`（带 `configVersion`） | §4.5 |
| `setConfig` | `{scope, patch}` | `ConfigDocument`（含升级迁移结果） | §4.5 |
| `listLogs` | `{filter, page}` | `LogPage` | §9.6 |
| `getLicenseState` | — | `LicenseStateSummary` | §12.4 / §8.9 |
| `activateLicense` | 商业版线；`{mode: "online"\|"offline", payload}` | `LicenseStateSummary` | §8.6–§8.7（单机版注入 AlwaysAllow，命令仍注册但恒允许，保证契约一致 §13.8） |
| `getIpcMeta` | — | `{contractVersion: number, kernelVersion: string}` | §9 第 1 条（启动握手） |

### 4.4.1 壳层追加命令（append-only）

以下命令由桌面壳提供、不属于 kernel 领域契约；命名与 DTO 属工程契约，冻结后只增不改。

| 命令 | 入参 | 出参 | 说明 |
|---|---|---|---|
| `cleanupRetention` | `{retentionDays?: number, apply?: boolean}` | `CleanupOutcome`（v1.6 增量可选字段 `removedLogRecords`：C17a 操作日志随保留期清理的记录数，桌面壳填写；`apply` 缺省 false） | 保留期演练/清理 |
| `getStorageInfo` | — | `{quarantineDir: string, journalFile: string}` | 展示隔离目录与 Journal 路径 |

> 任务队列命令 `cancelTask`/`pauseTask`/`resumeTask`/`listTasks`/`getTask` 已按 §4.1 的入出参实现；**S03 已实现任务快照持久化与 `recoverTask`**（`interrupted` 状态不自动续跑）。**`retryTask` 已实现（v1.5）**：仅对终态 `partial-failed`/`failed` 的 apply 任务生效，缺省重试全部失败条目、`entryIds` 可选子集（必须都是该任务报告中的失败条目，否则 E1002）；生成**新 taskId** 引用原 planId（§6），原任务保持终态；Journal 已 applied 条目幂等排除，编排级失败（报告零条目）时重跑全部条目；undo 任务不可重试（重试语义见方案 §7.6：沿用原参数与冲突策略，apply 期重新裁决冲突）。分页默认值属壳层实现参数，不改变 §2.3 的契约。

## 5. Plan/Apply 数据模型

```ts
interface PlanDocument {
  planId: string;            // ULID
  kind: OperationKind;       // 与功能编号 F/C 对应（§3.2 清单）
  params: Record<string, unknown>;  // 生成计划时的参数快照（§7.1 溯源）
                             // listing 计划：format="csv"、includeFiles/includeDirs、targetEncoding="utf-8-bom"
  entries: PlanEntry[];
  stats: { total: number; conflict: number; warning: number; blocked: number };
  warnings: PlanWarning[];   // 预警，不阻断；结构见 §5.1 核对表
}
interface PlanEntry {
  entryId: string;
  source: string;
  target?: string;           // 无目标动作（如删除→隔离）可省
  action: "rename" | "move" | "copy" | "archive" | "extract" | "create-dir"
        | "transcode" | "edit-content" | "delete" | "split" | "merge"
        | "csv-extract" | "csv-rows" | "listing" | "create-dir";
                             // extract=解压产物（C10）、create-dir=建目录（F11，逐级 entry、source 为空、target=目录）、split=文本分片（F01/F02/F03）、merge=TXT 合并（F04，单条汇总 N 个输入）/CSV 合并（F06，携带 `csvMerge` 载荷）、csv-extract=CSV 抽取指定列（F07，1:1，携带 `csvExtract` 载荷）、csv-rows=CSV 行抽取（F08 数量/F09 位置区间，1:1，携带 `csvRows` 载荷）、listing=目录清单（F10，1:1，source=根目录、target=清单文件，内容在 apply 期流式生成）；追加式枚举，只增不改
  relation?: PlanRelation;   // 1:N / N:1 分组（拆分/合并/压缩解压），见下方语义（F04 由 §5 的 `merge` 载荷表达、F06 由 `csvMerge` 载荷表达；F07/F08 为 1:1，均不设 relation）
  reason: string;            // 预览面板展示（§6.4）
  conflict?: ConflictInfo;   // §5.13 三类冲突，预览期全部暴露；默认处置=跳过（不变量 7）
  resolution?: "proceed" | "skip" | "sequence" | "overwrite";
                             // 逐项裁决；"询问"模式在预览期批量完成，不打断执行（§7.7）
  blocked?: { code: string; reason: string; hint?: string };
                             // 预检判定不可执行（如 §5.13 长路径分级）；apply 时记 skipped，不报错
  expected?: { size: number; algorithm: string; hash?: string };
                             // plan 期内容指纹；apply 前复核，变化记 skipped + E2010
                             // split 条目：size=该分片输出字节数，algorithm="byte-range"，hash 省略
                             // merge 条目：size=首个输入大小，algorithm="size"；全部输入在 §merge.sources 中逐项复核
                             // csvMerge 条目：size=首个输入大小，algorithm="size"；全部输入在 §csvMerge.files 中逐项复核
                             // csvExtract 条目：size=源文件大小，algorithm="size"；apply 复核源大小并在执行期重验表头
                             // csvRows 条目：size=源文件大小，algorithm="size"；apply 复核源大小并在执行期重验表头
                             // listing 条目：省略 expected；目录树在 apply 期重新遍历，清单是快照
                             // F05 的 split 条目：size=表头区间字节数＋该片记录区间字节数，algorithm="csv-byte-range"
  range?: { start: number; end: number };
                             // 追加式可选字段：分片在源文件中的字节区间（左闭右开），仅 split 使用；
                             // apply 复核 range.end ≤ 源文件当前大小，否则 skipped + E2010
  merge?: { sources: { path: string; encoding?: SourceEncoding; size: number }[];
           targetEncoding: string; invalid: string };
                             // 追加式可选字段：F04 合并载荷，仅 merge 使用；
                             // sources 按合并顺序排列，size 为 plan 期输入大小（apply 逐项复核，变化 skipped + E2010）；
                             // encoding 省略=需人工确认（计划条目 blocked E3001）；targetEncoding 含 BOM 策略，
                             // 产物只在最前面写一次 BOM，各输入自身 BOM 由转码剥离（ADR-0016）
  csv?: { header?: { start: number; end: number };
          delimiter: string;   // "," | ";" | "\t" | "|"（稳定字符串）
          rows: number;        // 本片记录数（不含表头）
          dirtyRows: number }; // 本片列数与表头不一致的记录数（只上报，不静默修正）
                             // 追加式可选字段：F05 CSV 分片载荷，仅 f05 的 split 使用；
                             // header 为源文件表头区间，每片先写 header 再写 range（一次原子成文件，ADR-0017）；
                             // header 省略=不保留表头；range 复核与 header 一并校验（apply 均 ≤ 源文件当前大小）
  csvMerge?: { files: { path: string; encoding?: SourceEncoding; size: number;
                        delimiter: string; columns: string[]; sourceLabel?: string;
                        dirtyRows: number }[];
               union: string[];         // 输出列并集，按首次出现顺序；重复列名保留
               positionalColumns: number; // 有表头=union.length；无表头=各文件最大列数
               hasHeader: boolean;
               sourceColumn?: string;   // 来源列名；省略=不添加来源列
               targetEncoding: string; invalid: string; delimiter: string;
               strict: boolean };
                             // 追加式可选字段：F06 CSV 合并载荷，仅 merge 使用（与 `merge` 互斥，同存 E1002）；
                             // files 按合并顺序，size 为 plan 期输入大小（apply 逐项复核，变化 skipped + E2010）；
                             // columns 为规范化（剥 BOM/trim）表头，重复名按出现顺序保留；无表头为空；
                             // 输出按 (name, occurrence) 对齐：缺失补空、额外列按首次出现顺序追加；
                             // 无表头按列序号对齐并发 `no-header-positional` 告警；表头 BOM/trim 规范化与重复列名分别发
                             // `header-normalized`/`duplicate-column-names` 告警；无表头+来源列、表头模式混合、strict 不一致均 blocked E3004（ADR-0018）
  csvExtract?: { encoding?: SourceEncoding; size: number; delimiter: string;
                 hasHeader: boolean; columns: string[]; columnCount: number;
                 selections: { index: number; name?: string }[];
                 targetEncoding: string; invalid: string; dirtyRows: number };
                             // 追加式可选字段：F07 CSV 抽取载荷，仅 csv-extract 使用（1:1，每个输入一条 entry）；
                             // columns 为规范化（剥 BOM/trim）表头，重复名保留；无表头为空；
                             // selections 为已解析的 0-based 字段序号与输出顺序，同一列重复选择 E1002；
                             // 名称选择命中重复表头时取首次出现并告警；列不存在/序号越界 blocked E3004；
                             // 无表头只允许序号选择；输出编码与非法序列策略同 F06，BOM 只写一次（ADR-0019）
  csvRows?: { encoding?: SourceEncoding; size: number; delimiter: string;
              hasHeader: boolean; columns: string[]; columnCount: number;
              dataRows: number; window: "first" | "last" | "range"; count: number;
              startRow?: number; endRow?: number; selectedRows?: number;
              headerRange?: { start: number; end: number };
              dataRange?: { start: number; end: number };
              totalKnown?: boolean;
              targetEncoding: string; invalid: string; dirtyRows: number };
                             // 追加式可选字段：F08/F09 CSV 行抽取载荷，仅 csv-rows 使用（1:1）；
                             // window=first/last 时 count 为请求行数；window=range 时 startRow/endRow 为
                             // 1-based 数据记录区间（不含表头），count 为实际选中行数，headerRange/dataRange
                             // 为表头/选中记录的源字节区间，totalKnown=false 表示计划在 endRow 处提前终止；
                             // F09 apply 只读取 headerRange/dataRange 的字节（不整读源文件），行尾保持源字节
                             // 形态，BOM 由目标编码决定；UTF-16/多行表头/E3001/E3004 等规则见 ADR-0021；
                             // first/last 的执行与输出规则同 F08（ADR-0020）
}
interface PlanRelation {
  groupId: string;           // 同一逻辑操作组（ULID）：1:N 的全部输出分片、N:1 的全部输入共享同组
  role: "source" | "output"; // 组内角色：拆分组的 N 条为 output（source 相同）；合并/压缩组的 N 条为 source（target 相同）
  partIndex?: number;        // 组内序号（1 起）
  partTotal?: number;        // 组内总数（plan 期确定）
}
interface ConflictInfo {
  kind: "existing" | "within-batch" | "case-only";  // 与现存重名 / 批内重名 / 仅大小写不同（§5.13）
  riskLevel: "safe" | "caution" | "danger";          // 平台统一分级（§7.11），界面禁自定口径
}
interface PlanWarning {
  kind: "invalid-char" | "reserved-name" | "trailing-dot-space" | "empty-name"
      | "long-path-caution" | "readonly" | "locked" | string;   // 追加式枚举，冻结后只增不改
  suggestion?: string;       // 如非法字符建议替换为下划线（§5.13）
  riskLevel: "safe" | "caution" | "danger";
}
interface ApplyOptions {
  onConflict: "skip" | "sequence" | "overwrite" | "ask";  // §5.13 四策略；默认 skip，逐项 resolution 优先于全局
  overwriteConfirmed?: boolean;  // 任一条目将 overwrite 时必须 true（§7.11 危险级：二次确认、确认按钮不带默认焦点）
}
interface ApplyReport {
  planId: string; taskId: string;
  entries: { entryId: string; status: "ok" | "overwritten" | "skipped" | "failed"; error?: IpcError }[];
                             // overwritten 单列：§7.7"没有静默处理"，被覆盖条目必须可辨识
  undoneBy?: string;         // 若部分回滚，指向回滚 Journal 段
}
```

### 5.1 与方案 §5.13 冲突矩阵逐项核对

| §5.13 要点 | 契约落点 | 结论 |
|---|---|---|
| 三类冲突预览期全部暴露 | `ConflictInfo.kind` 三枚举；plan 必填标注 | 已修正（初稿未区分三类） |
| 默认策略跳过并报告 | `onConflict` 默认 `"skip"`；skipped 进 ApplyReport | 一致 |
| 可选：自动加序号 | `"sequence"`（初稿误作 `"rename"`，已改） | 已修正 |
| 可选：覆盖，显式勾选＋二次确认 | `"overwrite"` + `overwriteConfirmed`；缺省拒绝执行（E1002） | 已补确认标志 |
| 可选：逐项询问 | `entry.resolution`；"询问"在预览期批量完成，不打断执行（§7.7） | 已补；状态机无需加交互态 |
| 执行时批内冲突按所选策略二次裁决，绝不静默覆盖 | apply 开始对每个 conflict 条目重新 stat，plan→apply 间文件系统变化即重裁；`overwritten` 单列 | 已补二次裁决语义 |
| 大小写折叠判定、仅改大小写走两阶段 | FsAdapter 收口（§4.7）；两阶段为 kernel 编排内部实现，不进线协议 | 一致（实现约束） |
| 非法字符/保留名/结尾点空格/空名 → 建议＋预览标注 | `PlanWarning.kind` 四枚举 + `suggestion`；不阻断 | 已补 |
| 路径超长三级分级（数值引用 §5.13） | 安全档 → 正常；警告档 → `long-path-caution`；超长档且未开启长路径支持 → `entry.resolution="skip"` + `long-path-disabled` 警告（hint 含开启方法/缩短路径） | 已补（v1.20）；`longPathSupported` 由壳层探测传入，分级复用 kernel `preflight::path_length_level` |
| 改名环检测：自环/链/环，环走两阶段，无环拓扑序 | kernel apply 编排内部实现，不进线协议；语料锚点挂改名环用例 | 一致（实现约束） |
| 只读/被占用 → 预检标记＋执行内重试一次后按策略跳过 | plan 期 `readonly`/`locked` warning；执行内对 retryable 条目自动重试一次，再失败 skipped+报告 | 已补执行内重试 |
| 执行中断 → 已完成项保留、可一键回滚 | 篇 01 §8 Journal 逆序回滚，覆盖 | 一致 |
| 杀软敏感 → 批次内节流（每 N 项微暂停） | **已定值（来源：用户确认 2026-10-27；已收编基线 §9.1——11.6-10，历史收编于方案 v1.7 R72）**：批次项数与微暂停时长以 §9.1 的杀软防护节流行准，参数可配置 | 已定值（契约先行，基线已收编） |

- `PlanDocument` 可序列化落盘为草稿，但**生成过程绝不落盘**（不变量 2）；`planId` 是 apply、撤销、日志三方共同的关联键。
- **F13 正则重命名（v1.21）**：`plan_f13` 复用 `rename::plan_rename` 的冲突扫描、长路径分级与两阶段编排；正则用 `regex` crate（有限自动机、线性时间、无回溯爆炸），替换模板支持 `$1`/`${name}`/`$$`，计划期校验缺失捕获组并返回 E1002；空 `pattern` 拒绝；无匹配转 `no-change` 警告且不产条目；非法目标名/冲突/长路径与 F12 同规则。决策见 [ADR-0026](../adr/0026-F13-正则重命名引擎与捕获组替换.md)。
- **F12 长路径分级（v1.20）**：`PlanF12Input.longPathSupported` 由壳层探测（`platform::preflight::long_path_supported`）后传入，kernel 保持纯函数；目标完整路径按方案 §5.13 三级判定——≤200 正常、200–259 发 `long-path-caution` 且条目照常执行、≥260 且未开启长路径支持时该条目 `resolution="skip"` 并发 `long-path-disabled`（提示开启方法或缩短目标路径），≥260 且已开启时正常执行。计划 `params.longPathSupported` 记录探测快照；apply 对 `resolution="skip"` 条目零副作用。
- **1:N / N:1 分组语义**（`relation`）：F01–F03/F05/C10 解压＝1:N（N 条 entry 共享 `groupId`，`role="output"`，`source` 相同）；C10 压缩＝N:1（N 条 entry 共享 `groupId`，`role="source"`，`target` 相同）；**F04 合并为单条 `merge` entry（N:1 的输入清单记录在 `entry.merge.sources`），F06 CSV 合并为单条 `merge` entry（输入清单在 `entry.csvMerge.files`），F07 CSV 抽取为每个输入一条 `csv-extract` entry（1:1），F08 CSV 抽取数量行为每个输入一条 `csv-rows` entry（1:1），F09 位置区间为每个输入一条 `csv-rows` entry（1:1），F10 目录清单为每个根目录一条 `listing` entry（1:1），均不设 `relation`**（ADR-0016/ADR-0018/ADR-0019/ADR-0020：单条 `write_atomic` 保证产物原子性，撤销只需一条 Journal reverse）。Journal 仍按条目逐条记录；组回滚＝组内条目按 Journal 逆序逐条 `reverse`，无需组级操作；`stats.total` 按条目计数。

## 6. 任务状态机（§4.4 / §7.5–§7.7）

```text
queued → running → completed            （全部条目 ok/overwritten）
           │ ↘ partial-failed ──────────（存在 failed 且存在 ok/overwritten；§7.6 可重试失败条目）
           │        └─retry──▶ queued（新 taskId，仅 retryable 失败条目）
           ├─ failed ──retry──▶ queued（全部条目 failed，或编排级错误如 Journal 不可写）
           ├─ paused ⇄ running
           ├─ cancelling ──▶ cancelled（已执行部分按 Journal 反向回滚后终态）
           └─ interrupted ──recover(resume)──▶ queued（同 taskId；按 Journal 跳过已完成条目）
                          ├─recover(rollback)──▶ cancelled（另起 undo 任务按 Journal 回滚）
                          └─recover(discard)──▶ cancelled（保留已执行状态，Journal 仍可后续撤销）
```

- 终态三分类：`completed`／`partial-failed`／`failed`／`cancelled` 均为终态；`partial-failed` 与 `failed` 的区分依据 ApplyReport 条目级状态（§5），展示层据此呈现"部分失败"（方案 §7.6 的失败明细与重试入口）。
- `interrupted` 不是终态，但**不得自动执行**：只能由 `recoverTask` 显式选择续跑/回滚/放弃（方案 §9.3、§15.3.2）。
- `cancelling → cancelled` 的中间回滚步骤以 `phase = journaling` 进度上报；崩溃发生在 `cancelling` 时，恢复后 `resume` 继续回滚并以 `cancelled` 收尾。
- 终态不可再迁移；`retryTask` 生成**新 taskId** 引用原 planId，不复活旧任务。

## 7. 错误码位表（§4.6 分段的落地）

> 分段与语义为基线既定；**具体码位为本文新增**，评审冻结后不得复用/挪位。

| 段 | 域 | 本文码位（草案） |
|---|---|---|
| E1xxx | 输入/参数 | E1001 参数缺失；E1002 参数非法；E1003 路径不合法；E1004 清单解析失败（§5.8）；E1005 正则非法（§5.5） |
| E2xxx | 文件系统/权限 | E2001 不存在；E2002 无权限；E2003 被占用（E01 语料）；E2004 磁盘满（E04）；E2005 网络路径断开（R05）；E2006 占位文件（E02）；E2007 冲突且策略为 skip；E2008 改名失败；E2009 保护目录命中（篇 04 §3）；E2010 内容指纹在 plan→apply 间变化（跳过） |
| E3xxx | 编码/格式 | E3001 无法识别编码；E3002 严格 UTF-8 校验失败（§5.2）；E3003 解压/压缩格式不支持（§5.11）；E3004 表头对齐失败（§5.4）；E3005 内容写回失败；E3006 解压条目越界/路径穿越拒绝（Z02，§5.11） |
| E4xxx | 授权 | E4001 未授权功能；E4002 授权过期；E4003 设备不匹配（§8.4）；E4004 配额耗尽（QuotaBased）；E4005 激活码无效 |
| E5xxx | 环境/依赖 | E5001 运行环境不支持；E5002 WASM 能力缺失（§13.3）；E5003 存储配额不足（OPFS，§13.3；须与"文件问题"可区分，§13.6） |
| E9xxx | 未知 | E9001 未分类（必须附 detail 与日志定位） |

`retryable` 默认值：E2003/E2004/E2005/E2006 = true，其余 false（E5003 为 false：清理配额后由用户重新发起，自动重试不改变结果）；码位表与语料编号（K/T/R/E/Z，T v1.12 §3.2）在回归中互相锚定。

## 8. Journal 与撤销契约（§7.8）

Journal 每执行条目一条 JSONL 记录，字段：

```ts
interface JournalEntry {
  seq: number; planId: string; entryId: string;
  planKind?: string;                // 追加式可选字段：撤销时按该 kind 向 LicenseService 退还配额；旧日志缺省按 c09
  ts: string;                       // 经 Clock 端口取得
  forward: FsOp; reverse: FsOp;     // FsOp = {op:"rename"|"copy"|"write_atomic"|"quarantine", from?, to?, dataRef?}
  status: "applied" | "reverted";
}
```

- 回滚 = 按同一 planId 的 Journal 逆序执行 `reverse`；`reverse` 由 plan 生成时一并推导（纯函数），执行期不得现场发明。
- **备份与 `dataRef`**：`overwrite` 与 `quarantine` 类动作的 `reverse` 需要原始内容——apply 期先把原文件/原内容落为备份文件（隔离目录内，经 FsAdapter），Journal 的 `dataRef` 指向备份路径；无备份需求的动作（如纯 rename 的 reverse 是逆 rename）`dataRef` 省略。
- **备份生命周期**：备份在撤销窗口内保留，窗口长度引用方案 §7.8 的"剩余可撤销时间"呈现与 §9.6 日志保留策略，本文不设数值；Journal 关账（窗口过期或用户确认不再撤销）后，备份随任务日志清理批次一并删除，备份清理动作本身不记 Journal（不可撤销，删除走隔离目录机制，不变量 7）。
- Journal 文件与 SQLite 任务日志（§4.4/§9.6）并存：Journal 管"怎么撤"，任务日志管"发生了什么"。
- **reverse 推导规则（v1.7，覆盖 `copy` 与内容写回；v1.8 追加 `split`；v1.18 追加 `create-dir`；v1.19 追加 rename 两阶段）**：`create-dir` 的 forward=`CreateDir{path}`、reverse=`RemoveDir{path}`，撤销只删除空目录（非空失败并报告，不递归）；F12 两阶段 rename 仍由多条 `Rename` 条目组成（临时名阶段＋目标名阶段），每条 reverse 为逆改名，撤销按 Journal 逆序执行即可恢复原路径；`rename`/`move`/`quarantine` 的逆由 `FsOp::inverse` 规范推导（逆改名/改名还原）；`copy` 新建目标的逆是**产物进隔离目录**（reverse=`Quarantine{from: 产物, to: 适配层预留路径}`，方案 §4.5 删除一律进隔离目录）；`copy` 覆盖与 `write_atomic` 先经 FsAdapter `backup_to_quarantine` 备份原内容（`dataRef`），逆是 `WriteAtomic{path, dataRef}` 备份还原；新建产物（无原始内容）的 forward `dataRef` 省略。`split` 新建分片与 `copy` 新建同规则：forward=`WriteAtomic{path, dataRef 省略}`、reverse=`Quarantine`；分片为覆盖时按 `copy` 覆盖流程备份还原。执行侧由共享的 `execute_reverse` 按字面执行三类 reverse（撤销任务与执行中取消回滚同一实现，执行期不发明新操作）。

## 9. 版本与兼容

1. 提供 `getIpcMeta` 命令返回 `{ contractVersion: number, kernelVersion: string }`；web 启动握手，不匹配时提示升级而非静默失败。
2. 语义化演进：新增命令/可选字段 = 兼容（minor）；重命名、删除、改语义 = 破坏（major），旧命令保留一个迭代。
3. 错误码位表冻结后，已发布码位不得改语义；新码位只追加。
4. 本契约与代码的一致性由契约测试保障：TS 类型与 Rust serde 结构以 specta 家族单一来源生成，机制与退路见 [ADR-0004](../adr/0004-类型契约单一来源.md)（已采纳，2026-10-27；执行清单实现期完成，含 wasm32 验证与 CI 生成物门禁）。

## 10. 待决与后续

- [x] `tauri-specta` 选型 ADR（§9 第 4 条）→ 已完成，见 [ADR-0004](../adr/0004-类型契约单一来源.md)（已采纳；§5 执行清单实现期完成）
- [x] ConflictPolicy/ApplyOptions 与方案 §5.13 冲突矩阵的逐项核对 → 已完成，见 §5.1 核对表（杀软节流已定值，数值以基线 §9.1 的杀软防护节流行准）
- [x] Web 双通道（§13.1）下大数据量传输的分块句柄设计 → 已完成，见 [02-Web双通道传输契约](02-Web双通道传输契约.md)；断点续传协议已定自研分块，见 [ADR-0006](../adr/0006-云端上传协议自研分块.md)
- [x] 配置 schema 与迁移（§4.5）→ 已完成，见 [03-配置schema与迁移契约](03-配置schema与迁移契约.md)（迁移样本已挂账至 `fixtures/migrations/`，见其 §10）
- [x] 本轮契约补齐（2026-10-28）：公共 DTO 字段定义（§3.2）、OperationKind 冻结值、Plan 1:N/N:1 分组语义（§5）、任务状态 partial-failed（§6）、getIpcMeta 入命令表（§4.4）、LicenseService 配额退还 release（§3）、Journal 备份与生命周期（§8）、E5003 行格式与 retryable 修复（§7）；FsAdapter 扩展能力已挂引用 04 §4

## 11. 修订记录

| 版本 | 日期 | 变更 |
|---|---|---|
| v1 | 2026-10-27 | 首版冻结（用户评审确认）。 |
| v1.1 | 2026-10-28 | 补 `TaskSummary.channel`、`UndoableSummary.scope`、`LogPage.ephemeral`；补 `ConflictPolicy`/`Capability` 类型与 `KernelError` 映射说明；修正 §9 内部引用；E3xxx 增补 E3006。 |
| v1.2 | 2026-10-29 | 追加 §4.4.1 `cleanupRetention`/`getStorageInfo` 壳层命令；补事件顺序、撤销报告与 `ProgressEvent` 阶段计数说明；标注 `retryTask` 未实现。 |
| v1.3 | 2026-10-29 | 追加 `PlanEntry.expected` 内容指纹与 E2010；apply 在隔离前复核大小/哈希，变化条目 skipped。 |
| v1.4 | 2026-10-29 | 追加 S01 `preflightPlan`/`PreflightReport`、S03 `interrupted` 状态与 `recoverTask`、任务快照持久化说明；Journal 追加可选 `planKind`（undo 按 kind 退还配额）；`retryTask` 仍未实现。 |
| v1.5 | 2026-10-30 | `retryTask` 实现语义定稿（仅失败条目、新 taskId 引用原 planId、Journal 幂等排除、undo 不可重试）；§3.2 节流行标注壳层合帧已实现（窗口取方案 §9.1 区间下限）；LicenseService 注入口收敛为壳层装配点（方案 13.8）。 |
| v1.6 | 2026-10-30 | §4.4.1 `CleanupOutcome` 追加可选字段 `removedLogRecords`（C17a 操作日志随保留期清理，ADR-0013；只增不改）。 |
| v1.7 | 2026-10-30 | §8 追加 reverse 推导规则：copy 新建目标逆为产物进隔离目录（reverse=Quarantine）、copy 覆盖与 write_atomic 逆为 dataRef 备份还原、新建产物 forward 的 `dataRef` 省略（对齐既定契约字面）；`copy`/`edit-content` 纳入通用执行器，回滚统一走共享 `execute_reverse`。 |
| v1.8 | 2026-10-30 | §5 追加 `split` 动作与可选 `range{start,end}`（F01 1:N 分片，追加式）；§8 明确 split 新建分片 reverse=Quarantine、覆盖按 copy 流程备份还原；split 的 `expected.size` 为分片输出长度（algorithm="byte-range"）。决策见 [ADR-0015](../adr/0015-F01分片计划与撤销语义.md)。 |
| v1.9 | 2026-10-30 | §5 `split` 动作说明扩展至 F02（按行数，`params.linesPerPart`）与 F03（按份数，`params.parts`，行数预扫描配额）；无新增动作/字段，撤销与复核规则不变（ADR-0015 §5）。 |
| v1.10 | 2026-10-30 | §3.1 追加 `plan_f04`；§5 追加 `merge` 动作与可选 `merge.sources/targetEncoding/invalid` 载荷（F04 N:1，单条 entry）；§5.1 修订 F04 分组语义为单条 entry、不设 `relation`；BOM 只在产物开头写一次。决策见 [ADR-0016](../adr/0016-F04合并计划与编码统一语义.md)。 |
| v1.11 | 2026-10-30 | §3.1 追加 `plan_f05`；§5 追加可选 `csv{header,delimiter,rows,dirtyRows}` 载荷（F05 复用 `split` 动作）；FsAdapter 增 `copy_ranges_atomic`（表头＋记录一次原子成文件）。决策见 [ADR-0017](../adr/0017-F05-CSV分片记录边界与多区间原子写.md)。 |
| v1.12 | 2026-10-30 | §3.1 追加 `plan_f06`；§5 追加可选 `csvMerge{files,union,positionalColumns,hasHeader,sourceColumn,targetEncoding,invalid,delimiter,strict}` 载荷（F06 复用 `merge` 动作，单条 entry）；§5.1 修订 F06 分组语义与 `relation` 省略；`expected` 口径补 csvMerge 条目。决策见 [ADR-0018](../adr/0018-F06-CSV合并列对齐与来源列.md)。 |
| v1.13 | 2026-10-30 | §3.1 追加 `plan_f07`；§5 追加 `csv-extract` 动作与可选 `csvExtract{encoding,size,delimiter,hasHeader,columns,columnCount,selections,targetEncoding,invalid,dirtyRows}` 载荷（F07 1:1，每个输入一条 entry）；§5.1 修订 F07 分组语义与 `relation` 省略；`expected` 口径补 csvExtract 条目。决策见 [ADR-0019](../adr/0019-F07-CSV抽取指定列.md)。 |
| v1.14 | 2026-10-30 | §3.1 追加 `plan_f08`；§5 追加 `csv-rows` 动作与可选 `csvRows{encoding,size,delimiter,hasHeader,columns,columnCount,dataRows,window,count,targetEncoding,invalid,dirtyRows}` 载荷（F08 1:1，每个输入一条 entry）；§5.1 修订 F08 分组语义与 `relation` 省略；`expected` 口径补 csvRows 条目。决策见 [ADR-0020](../adr/0020-F08-CSV抽取数量行与前后窗口.md)。 |
| v1.15 | 2026-10-30 | §5 F06 `csvMerge` 说明补 T07/T08 告警：无表头 `no-header-positional`、表头 BOM/trim 规范化 `header-normalized`、重复规范化列名 `duplicate-column-names`（均 Caution、不阻断）。决策见 [ADR-0018](../adr/0018-F06-CSV合并列对齐与来源列.md) §2.5。 |
| v1.16 | 2026-10-30 | §3.1 追加 `plan_f09`；§5 `csvRows` 追加 `window="range"`、`startRow/endRow/headerRange/dataRange/totalKnown`（F09 1:1，每个输入一条 entry）；FsAdapter 追加 `read_ranges`（只读选中字节区间）。决策见 [ADR-0021](../adr/0021-F09-CSV位置行区间与流式读取.md)。 |
| v1.17 | 2026-10-30 | §3.1 追加 `plan_f10`；§3 追加 `DirectoryLister` 端口；§5 追加 `listing` 动作（F10 1:1，source=根目录、target=CSV 清单，`expected` 省略）与 `write_atomic_with` 流式原子写能力。决策见 [ADR-0022](../adr/0022-F10-目录清单CSV与流式写出.md)。 |
| v1.18 | 2026-10-30 | §3.1 追加 `plan_f11`；§5 明确 `create-dir` 的 F11 逐级条目语义；§8 追加 `CreateDir/RemoveDir` Journal 反向推导（空目录删除）。决策见 [ADR-0023](../adr/0023-F11-清单建档与目录撤销.md)。 |
| v1.19 | 2026-10-31 | §3.1 追加 `plan_f12`；§8 明确 F12 循环/大小写改名的两阶段 rename 由多条 `Rename` 条目组成，撤销按 Journal 逆序恢复。决策见 [ADR-0024](../adr/0024-F12-批量重命名与两阶段计划.md)。 |
| v1.20 | 2026-10-31 | §5.1 落地 F12 计划级长路径分级：`PlanF12Input.longPathSupported` 由壳层探测传入，分级复用 `preflight::path_length_level`；200–259 发 `long-path-caution`，≥260 未开启时长路径条目 `resolution=skip`＋`long-path-disabled` 警告，已开启时正常执行；计划 `params.longPathSupported` 记录快照。决策见 [ADR-0025](../adr/0025-F12计划级长路径分级与重命名族语料断言.md)。 |
| v1.21 | 2026-10-31 | §3.1 将占位 `plan_regex_rename`/`PlanRenameInput` 落为 `plan_f13`/`PlanF13Input`；§5.1 追加 F13 正则规则：`regex` 引擎、`$1`/`${name}`/`$$` 替换、缺失捕获组计划期 E1002、空 pattern 拒绝、无匹配 no-change；冲突/长路径/两阶段复用 F12 核心。决策见 [ADR-0026](../adr/0026-F13-正则重命名引擎与捕获组替换.md)。 |
