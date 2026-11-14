# 01 内核接口与 IPC 契约

> 状态：已评审·实现冻结 v1.64（2026-11-13 §2.2／§4.3 条目内（字节级）进度字段 `entry_done`／`entry_total`——双层进度的条目层，ADR-0100；2026-11-13 §2.2 取消粒度细化＋§5 `ContentRewriter` 增 cancel 端口：流式重写在**读块边界**响应取消，新增 E3010 区分「取消」与「失败」——ADR-0097；2026-11-13 C10 逐文件报告：`ArchiveService::compress` 返回 `CompressedArchive{bytes,manifest}`，压缩/解压成功条目的 `ApplyEntryReport.detail` 放逐文件清单——ADR-0096；2026-11-10 OQ-45 裁决：`RecipeDocument` v7 增 `origin`——ADR-0090；2026-11-09 C37 Phase B：`loadRecipe` 增 `stepIndex`、`describeRecipe` 增逐段评估与 `oneClickAllowed`——ADR-0089；2026-11-08 D1 再补全：`transcode` 与 `edit-content` 共用就地改写执行路径（C01 旧动作名）；`undo --dry-run` 的网络 `stat` 走 2/4/8 秒重试、耗尽上抛 E2005；2026-11-08 D5：`JournalStatus` 增 `Pending`——隔离移动两阶段「Pending→移动→Applied」，撤销只回滚 Applied，`FileJournal::open` 按隔离路径事实收敛——ADR-0084；2026-11-07 OQ-43②落地：`TemplateStep`/`SaveRecipeInput` 增 `content`（v6，变换正文进模板不进快照），六功能套用由正文补齐、f11 纠偏可套用（ADR-0081）；2026-11-07 OQ-41②落地：§4.4.2 增 `describeRecipe` 与 `RecipeApplySpec`，`TemplateStep` DTO 补齐 v4 的 `feature` 与 v5 的 `sampling`（接口块此前停在 v3 形状，本批钉正）、`SaveRecipeInput` 增 `sampling`，`loadRecipe` 段落改写为「参数映射表闭合形状差异」；§5 计划参数键修订：f04/f06 的输出路径键 `target→output`、c10 压缩计划补 `mode`（ADR-0080）；2026-10-27 评审；2026-10-28 增量修订，用户确认；2026-10-29 追加任务队列说明、E2010、S01/S02/S03；2026-10-30 追加 retryTask 实现说明、进度合帧、CleanupOutcome.removedLogRecords、§8 reverse 推导规则与 F01 `split` 动作/`range` 字段、F02/F03 分片基准、F04 `merge` 动作/载荷、F05 `csv` 载荷与 `plan_f05`、F06 `csvMerge` 载荷与 `plan_f06`、F07 `csv-extract` 动作/载荷与 `plan_f07`、F08 `csv-rows` 动作/载荷与 `plan_f08`、F06 T07/T08 告警 `no-header-positional`/`header-normalized`/`duplicate-column-names`、F09 `plan_f09`/`csvRows` 区间字段/`read_ranges`、F10 `listing` 动作/`plan_f10`/`DirectoryLister`/`write_atomic_with`、F11 `plan_f11`/逐级 `create-dir`/`CreateDir`/`RemoveDir` Journal、F12 `plan_f12`/两阶段 rename、2026-10-31 追加 F12 计划级长路径分级与重命名族语料断言、F13 `plan_f13`/正则捕获组替换、C02 `plan_c02`/流式换行重写、C03 `plan_c03`/查找替换、C10 `plan_c10`/ZIP 解压、C10 `plan_c10_compress`/ZIP 压缩、C11 `plan_c11`/`csv-dedup` 动作/`csvDedup` 载荷/CSV 去重、C12 `plan_c12`/`csv-filter` 动作/`csvFilter` 载荷/CSV 过滤、C15 任务内条目级并发/执行期限额、C04 `plan_c04`/名称查找替换、C14 `plan_c14`/`csv-columns` 动作/`csvColumns` 载荷/列拆分合并、C07 `plan_c07`/清单驱动 move/copy/跨盘校验删源、C08 `plan_c08`/按规则归档（复用 move 动作）、C13 `plan_c13`/`csv-sort` 动作/`csvSort` 载荷/类型感知排序与外排、C18 计划幂等契约（derived_ulid）与横切 harness、UI 接入（planOperation kind 路由 f05/f11/f12＋`renamePreview`/`undoDryRun` 壳层命令；批量接入 f01–f04/c01–c03；第三批接入 f06–f09 共用 `CsvPlanParams` 参数面与 f10 清单、并定稿终态快照剪枝顺序——ADR-0042；第四批接入 c04/c11/c12/c13/c14，共用 `OpPlanParams` 超集参数面——ADR-0043；第五批接入 c07 清单/c08 规则/c10 ZIP 壳层计划，`run_apply` 为 c10 增设专用执行器分支（`apply_archive_plan`＋`ZipArchiveService`，保留逐文件 Journal 撤销语义）——ADR-0044；2026-11-02 追加 F11 的逐级前缀事实（`PlanF11Input.existing_files`）与 `parent-is-file` 的计划期跳过＋集中报告——ADR-0049；2026-11-02 追加 §5 `ApplyEntryReport.detail`（条目级明细）与 `ContentEditor`→`EditedContent` 返回型、§3 的 `Transcoded.replacement_count` 口径改为替换字符个数——ADR-0050；2026-11-02 追加 §2.2 取消处置（`ApplyOptions.cancelPolicy`：缺省回滚／桌面壳保留＋问询后 undo）——ADR-0052；2026-11-02 追加 F04 merge 的流式执行（`read_stream`＋`StreamTranscoder`）——ADR-0053；2026-11-03 追加 F06/F07/F08/C11/C12 执行臂流式化（`csv_merge::RecordFeeder` 增量记录供给器，执行器内部实现、不改 IPC 契约与错误码）——ADR-0054；2026-11-03 追加 §5 `ContentRewriter::rewrite` 返回 `EditOutcome{detail}`（与 `EditedContent.detail` 同载体）——ADR-0055；2026-11-03 §2.3 `TaskSummary.planId` 追加可选字段（serde default＋缺省省略，旧 S03 快照仍可反序列化），供「任务与撤销」视图直接走 `undoPlan`；2026-11-05 追加 §2.3 `TaskSummary.job`（任务类别独立于功能码，v1.49）；2026-11-05 追加 §5 `csv` 载荷可选 `shardBom`——F05 支持 UTF-16 带 BOM 源直接分片、每片补写 BOM（v1.50，ADR-0070）；2026-11-05 追加 §4.4.2 处理模板六条命令与 DTO（`RecipeDocument`／`TemplateStep`／`RecipeMatch`／`SaveRecipeInput`／`RecipePage`），定稿「保存时剔除路径参数、判据只留内核一处」，并写明 `loadRecipe` 仍未实现（阻塞 OQ-36）——C37 前端只做存取半边（v1.51，ADR-0072）；2026-11-04 追加 §5.1 只读/占用的**计划期点名**（F12/F13/C04 的 `accessFacts`、C03 的 `PlanC03File.readonly`，事实由壳层/CLI 经 `platform::preflight` 采集），并如实修正该行"已补执行内重试"的结论——执行期"重试一次后按策略跳过"从未落地，改判语义阻塞 [OQ-37](../OPEN-QUESTIONS.md)；v1.53 C10 加密包的密码注入面——`PlanC10Input.password_provided` 只声明事实、密码只进执行期上下文，`inspect` 改 `by_index_data` 使加密包可出预览，E3008/E3009 两道新码，ZipCrypto＋AES，ADR-0074） ｜ 适用：crates/kernel、src-tauri、web/ ｜ 日期：2026-10-27
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

- 进度经事件通道推送（事件名见 §4.3），字段：`taskId, phase, done, total, message`，v1.64 追加可选 `entry_done`／`entry_total`（条目内字节进度，ADR-0100）：`Some`＝条目内事件（只更新字节字段，`done/total` 填 0、无意义）；缺省＝条目边界事件（只更新条目计数并清空字节字段）。方案 §7.5「当前条目进度（大文件可见）」的落点。
  `phase ∈ queued | scanning | planning | executing | journaling | finalizing`。
- **节流**：推送频率上限为性能阈值，引用方案 §9.1 既定值，本文不设数值；壳层负责合帧。已实现（v1.5）：桌面壳按方案 §9.1「进度刷新节流 100–200 ms」区间的下限，对同阶段 `task://progress` 做合帧（`TaskSummary` 仍逐条更新）；阶段迁移立即推送并在窗口内补发最新条目计数，`task://state`/`task://done` 不合帧。
- **取消**（§7.7）：壳层持有原子取消标志，以 `CancelToken` 端口传入 kernel 执行循环，**条目边界**检查一次；**流式内容重写**（C02，v1.63）另在 `ContentRewriter::rewrite` 的**读块边界**（64 KiB）检查同一端口，命中即返回 `Err(E3010)`——回调以 Err 收场使 `rewrite_atomic_with` 丢弃临时文件、源文件保持原样，执行器把该条目记 `Skipped`（**不是 failed**）并把整单标成 `Cancelled`。C01／C03 共用同一端口但尚未逐块检查（ADR-0097 §5）。原表述「每个条目边界检查一次」是**当时的能力边界**，现按上述口径细化。停止派发后按 `ApplyOptions.cancelPolicy` 处置已应用条目（v1.42）：缺省 `rollbackApplied`＝立即按 Journal 反向回滚（§7.8，供没有交互面的调用方）；桌面壳传 `keepApplied`＝已应用条目保留在磁盘与 Journal 中（`ApplyReport.undone_by` 为空），由界面询问「已完成 N 项，是否一并撤销？」——答复撤销时调用 `undoPlan`，走同一本 Journal。`c10` 归档专用执行器只有"保留"通道（无执行中回滚），其取消同样以 `undone_by` 为空如实报告。
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
> `restore_from_quarantine`、`read_ranges`、`name_rules` 等扩展签名见 [04-文件系统适配层契约](04-文件系统适配层契约.md) §2／§4——属同一 trait 的 v1 冻结面，不在本文重复定义。

> **环境事实一律经端口注入**（v1.52 明写）：`case_sensitivity`、`name_rules`、`long_path_supported` 三者都是「这个平台／这台机器能做什么」，由适配层采集、随计划输入传入；kernel **不出现 `cfg!(windows)` 之类的平台判断**。`name_rules` 值为 `NameRules`（`windows()`／`posix()` 两个冻结取值），判据见 [04 §2.1](04-文件系统适配层契约.md)；理由见 [ADR-0073](../adr/0073-目标名合法性改为能力值.md)。

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
pub fn plan_c02(input: PlanC02Input) -> PlanDocument;                  // §5.15 换行符转换（流式重写）
pub fn plan_c03(input: PlanC03Input) -> PlanDocument;                  // §5.14 批量查找替换
pub fn plan_c10(input: PlanC10Input) -> PlanDocument;                  // §5.11 ZIP 解压（安全边界；`password_provided` 只声明事实，密码在执行期）
pub fn plan_c10_compress(input: PlanC10CompressInput) -> PlanDocument;  // §5.11 ZIP 压缩
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
                        job: "apply" | "undo";        // 任务类别：执行 or 按 Journal 回滚（v1.49）。kind 不承担这个判别
                        phase: ProgressEvent["phase"]; done: number; total: number;
                        planId?: string;              // 可选：任务对应的计划，撤销（undoPlan）按它走共享 Journal（v1.46）
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
| `planOperation` | `{kind, params, sourcePaths}` | `PlanDocument`（纯函数结果，含 warnings/conflicts）；kind 路由：c09（隔离清理）＋ f05/f11/f12（主流程）＋ f01–f04/c01–c03（批量接入）——壳层事实采集，其余 kind E1002 明确拒绝（UI 接入切片，v1.35/v1.36） | §7.1 / §7.4 |
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
| `task://done` | `{taskId, kind, summary: ApplyReport}` | 终态推送（成功/失败/取消三态由 state 区分）；`kind ∈ apply \| undo`，取消后的"是否一并撤销"只对 apply 任务成立（v1.42） |
| `license://changed` | `LicenseStateSummary` | 仅通知**显示状态**；web 端不得据此做放行/拦截逻辑（不变量 11） |

- 终态顺序固定为 `task://state` → `task://done`；快照剪枝先于该顺序落盘（v1.37，ADR-0042）：任务进入终态时，恢复快照必须先完成剪枝再推送 state/done，使「终态对外可观测」的任何时刻恢复快照都不含该任务（内存终态与写盘之间仍可能有毫秒级窗口，读文件的调用方应等待收敛）；
- 终态顺序固定为 `task://state` → `task://done`；`task://done` 的 `summary.entries` 对撤销任务同样成立：成功恢复记 `ok`、失败记 `failed`，便于 `getTask`/UI 复用同一报告结构。
- `ProgressEvent.done/total` 的阶段语义：`executing` 表示条目总量进度；`journaling`/`finalizing` 可上报阶段内计数。`TaskSummary.done/total` 只保留条目总量，不随后两阶段重置。
- `entry_done`／`entry_total`（v1.64，ADR-0100）：条目内（字节级）进度——N:1 合并（F04）整条任务只有一条 entry，不报字节进度会让界面长时间停在 0/1。仅执行臂在读块回调里上报，壳层按 `entry_total.is_some()` 分流（条目内只更新字节字段；条目边界更新条目计数并清空字节字段）。
- `message` 承载双层进度的条目层明细（C15，v1.28）：`executing` 阶段完成一条即上报该条目源路径；并发路径逐条完成上报，串行路径逐条上报（message 可空）。壳层合帧只合 `done/total` 频率，`message` 取窗口内最新值。

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

### 4.4.2 处理模板命令（C37，v1.51）

六个命令此前只存在于壳层实现、未进本契约；2026-11-05 前端接上模板库后补齐（签名以 `src-tauri/src/lib.rs` 为准，口径以设计 03 §5–§7 为准）。

| 命令 | 入参 | 出参 | 说明 |
|---|---|---|---|
| `listRecipes` | `{offset?, limit?}` | `RecipePage` | 列表只读；坏文件跳过并计入 `warnings`，不整页失败 |
| `getRecipe` | `{recipeId}` | `RecipeDocument` | 不存在报 E2001 |
| `saveRecipe` | `{recipe: SaveRecipeInput}` | `RecipeDocument` | 带 `recipeId` 即覆盖；只给 `kind`＋`paramsSnapshot` 时按单步模板落地（`steps[0]`） |
| `deleteRecipe` | `{recipeId}` | `void` | 幂等 |
| `exportRecipe` | `{recipeId}` | `{payload: string}` | v3 JSON；落盘位置由壳层另存对话框决定 |
| `importRecipe` | `{payload: string}` | `RecipeDocument` | 导入即迁移；`schemaVersion` 高于当前支持（现 v6）报 **E1007**；导入后必须重新预览 |
| `describeRecipe` | `{recipeId: string}` | `RecipeApplySpec` | **v1.56 新增（OQ-41②，ADR-0080）**：静态评估套用面（槽位＋拒绝原因），判定与 `loadRecipe` 同源（壳层 `assess_step`）；界面据此决定给不给「套用」按钮，不给假按钮。**v1.60（方案 §15.2.3）**：多步模板逐段评估——出参增 `steps[]`（`stepId/index/feature/kind/source/applicable/reason/required[]/outputs[]/highRisk`）与 `oneClickAllowed`（全部步骤可套用且无高风险步骤） |

**保存时剔除路径参数（v1.51 定稿）**：`saveRecipe` 在落盘前对每个步骤调 `strip_path_params`，只留声明式参数。名单按**本仓库参数名的真实语义**登记（内核 `PATH_PARAM_KEYS`）：`file`／`files`／`sources`／`output`／`outputDir`／`root(s)`／`manifest`／`list`／`archive`／`dir` 等是真路径；`source`／`target` 在 F04/F06/F07/F08/C01/C11–C14 里是**源/目标编码**，不得入表（旧表把它们里的 `source` 当路径，导入时会把编码删掉）。判据只留内核一处，界面不抄第二份键表。

**`loadRecipe` 已实现（OQ-40 裁决①＋OQ-41 裁决②，v1.56）**：入参 `{recipeId, pathParams}`，壳层按 `recipe_apply::FEATURE_PATH_SLOTS` 把文件集写进该功能路由认的路径键，再调**同一条** `planOperation` 出口，不进队列（方案 §15.2.5「载入后必须重新预览」）。键名逐条取自结构体必填字段，配防漂移断言（注入的键必须真的解进字段）。快照（计划形状）与请求形状的差异由**逐功能参数映射表**（`FEATURE_PARAM_MAP`）闭合（OQ-43② 后含**正文注入**：六功能的规则/列选择/条件/排序键/拆分合并规格来自模板 `content` 块，白名单外拒绝、覆盖快照同名键）：改名（`replace→replacement`、`platformPrior→prior`、`targetEncoding→target` 等）、补采样缺省（模板 `sampling` 块优先，缺省 4096＝界面既有占位）、剔「auto」字面量、`maxRatio` 数值化；f06/f07/c08/c12/c13/c14 六个功能的变换正文不随计划保存（计划只存计数），OQ-43② 后由模板 `content` 块补齐（缺正文的旧模板如实拒绝并提示重存）；**f11 纠偏为可套用**（清单文件是路径槽位、路由重读其内容，ADR-0081 §2）；c11 整行去重可套用、按键列拒绝；c10 旧压缩模板（无 `mode`）拒绝并提示重存。**v1.60（方案 §15.2.3）**：入参增 `stepIndex?`（缺省第 1 步）——多步模板**按步调用**，第 2 步起的输入由调用方给（`source: previous`＝上一步实际产物），壳层不预生成所有步骤；越界步号如实报「没有这一步」。见 [ADR-0077](./../adr/0077-loadRecipe的路径槽位表与形状差异.md)（OQ-40①）、[ADR-0080](./../adr/0080-OQ41裁决二与套用映射表.md)（OQ-41②）与 [ADR-0089](./../adr/0089-C37多步模板执行口径与运行模式缺省.md)（Phase B）。

```ts
interface SaveRecipeInput {
  recipeId?: string;                    // 带即覆盖
  name: string;
  description?: string;
  match?: RecipeMatch;                  // 只读扩展名与用户标签，仅作"可能适用"建议
  kind?: PlanAction;                    // Phase A：动作级类型（不是功能码）
  paramsSnapshot?: Record<string, unknown>;
  steps?: TemplateStep[];                // Phase B：多步骤（≤10）
  feature?: string;                      // v4：单步模板的功能码；缺省时壳层只在动作唯一时补，不猜
  sampling?: RecipeSampling;             // v5：单步模板的采样口径存档（保存时从表单采集，没这项不传）
  content?: Record<string, unknown>;     // v6：单步模板的变换正文存档（保存时从表单采集）
}

/** v5（OQ-41②附带裁决，方案 v1.20 §15.2.1）：采样类技术口径**进模板、不进快照**；
 *  值 ≥1；只存该功能实际用到的键。套用时存档值优先，未存由壳层映射表按缺省补。 */
interface RecipeSampling {
  sampleWindow?: number;
  sampleBlocks?: number;
  sampleBytes?: number;
}

interface RecipeDocument {
  recipeId: string;
  schemaVersion: number;                 // 独立于 configVersion；v3＝steps，v4＝feature，v5＝sampling，v6＝content，v7＝origin
  name: string;
  description?: string;
  steps: TemplateStep[];
  match?: RecipeMatch;
  sourceFormat?: "desktop" | "web";
  origin?: "local" | "imported";         // v7（OQ-45 裁决，方案 §15.2.3／§15.2.4）：来源——
                                         //   本机新建 local／导入 imported；缺省＝来源未知（迁移不猜，
                                         //   按导入默认＝逐步骤）。保存写 local、导入一律改写 imported
  createdAt: string;
  updatedAt?: string;
  lastUsedAt?: string;
  usageCount?: number;
  pinned?: boolean;
}

interface TemplateStep {
  stepId: string;
  kind: PlanAction;                      // 动作级；功能码在 feature（v4，OQ-36 裁决①——本接口块此前停在 v3 形状，v1.56 钉正）
  paramsSnapshot: Record<string, unknown>; // 不含源路径/输出目录/脚本（不变量 13）
  source: "original" | "previous";
  feature?: string;                      // v4：该步绑定的功能码；缺省＝不可套用（一对多动作不猜）
  sampling?: RecipeSampling;             // v5：采样类技术口径（进模板、不进快照，ADR-0080）
  content?: Record<string, unknown>;     // v6：变换正文（进模板、不进快照，ADR-0081）——
                                         //   值只允许标量/标量数组；路径/脚本键同快照剔除/拒绝
}

interface RecipePage { items: RecipeDocument[]; total: number }
```

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
                             // extract=解压产物（C10）、transcode=就地转码（与 edit-content 同一执行路径，C01 旧动作名）、create-dir=建目录（F11，逐级 entry、source 为空、target=目录）、split=文本分片（F01/F02/F03）、merge=TXT 合并（F04，单条汇总 N 个输入）/CSV 合并（F06，携带 `csvMerge` 载荷）、csv-extract=CSV 抽取指定列（F07，1:1，携带 `csvExtract` 载荷）、csv-rows=CSV 行抽取（F08 数量/F09 位置区间，1:1，携带 `csvRows` 载荷）、csv-dedup=CSV 去重（C11，1:1，携带 `csvDedup` 载荷）、csv-filter=CSV 过滤（C12，1:1，携带 `csvFilter` 载荷）、csv-columns=CSV 列拆分/合并（C14，1:1，携带 `csvColumns` 载荷）、move/copy=清单驱动移动/复制（C07，1:1，复用既有动作，Journal move 跨盘仍记 rename）与按规则归档（C08，1:1，规则判定器链在计划层求值、命中与未分类生成 move 条目）、csv-sort=CSV 排序（C13，1:1，携带 `csvSort` 载荷，类型感知＋可选外排）、listing=目录清单（F10，1:1，source=根目录、target=清单文件，内容在 apply 期流式生成）；**计划幂等契约（C18，§5.15 重复预览幂等）**：固定 planId 与输入时，重复预览的输出必须逐字节一致——计划内生成的派生 id（groupId 等）一律经 `platform::ids::derived_ulid(seed)` 确定性派生，禁止时钟/熵池 id；追加式枚举，只增不改
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
                            // csvColumns 条目：size=源文件大小，algorithm="size"；apply 复核源大小并在执行期重验表头
                            // csvSort 条目：size=源文件大小，algorithm="size"；apply 复核源大小并在执行期重验表头
                             // listing 条目：省略 expected；目录树在 apply 期重新遍历，清单是快照
                             // F05 的 split 条目：size=补写前缀＋表头区间字节数＋该片记录区间字节数，algorithm="csv-byte-range"
  range?: { start: number; end: number };
                             // 追加式可选字段：分片在源文件中的字节区间（左闭右开），仅 split 使用；
                             // apply 复核 range.end ≤ 源文件当前大小，否则 skipped + E2010
  merge?: { sources: { path: string; encoding?: SourceEncoding; size: number }[];
           targetEncoding: string; invalid: string };
                             // 追加式可选字段：F04 合并载荷，仅 merge 使用；
                             // sources 按合并顺序排列，size 为 plan 期输入大小（apply 逐项复核，变化 skipped + E2010）；
                             // encoding 省略=需人工确认（计划条目 blocked E3001）；targetEncoding 含 BOM 策略，
                             // 产物只在最前面写一次 BOM，各输入自身 BOM 由转码剥离（ADR-0016）；
                             // apply 逐源 read_stream→StreamTranscoder→write_atomic_with，内存 O(块)（v1.43，ADR-0053）
  csv?: { header?: { start: number; end: number };
          shardBom?: Bom;      // "utf16-le" | "utf16-be"：本片写入前要补写的 BOM
          delimiter: string;   // "," | ";" | "\t" | "|"（稳定字符串）
          rows: number;        // 本片记录数（不含表头）
          dirtyRows: number }; // 本片列数与表头不一致的记录数（只上报，不静默修正）
                             // 追加式可选字段：F05 CSV 分片载荷，仅 f05 的 split 使用；
                             // header 为源文件表头区间，每片先写 header 再写 range（一次原子成文件，ADR-0017）；
                             // header 省略=不保留表头；range 复核与 header 一并校验（apply 均 ≤ 源文件当前大小）；
                             // shardBom 省略=该片通过 header/首片区间已自带 BOM 或源编码不需要 BOM：
                             // UTF-16 源**每片都必须带 BOM**（方案 §5.1），首片区间从 0 起时源字节已含它，
                             // 其余分片由 apply 在全部区间之前补写一次（ADR-0070）
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
  csvDedup?: { encoding?: SourceEncoding; size: number; delimiter: string;
               hasHeader: boolean; columns: string[]; columnCount: number;
               key: "row" | "columns"; keyColumns?: { index: number; name?: string }[];
               dataRows: number; uniqueRows: number; duplicateRows: number;
               targetEncoding: string; invalid: string; dirtyRows: number };
                             // 追加式可选字段：C11 CSV 去重载荷，仅 csv-dedup 使用（1:1）；
                             // key="row" 整行判重、"columns" 按 keyColumns（0-based）拼键；
                             // size=源文件大小，apply 复核源大小并在执行期重验表头
  csvFilter?: { encoding?: SourceEncoding; size: number; delimiter: string;
                hasHeader: boolean; columns: string[]; columnCount: number;
                condition: { column: { index: number; name?: string };
  csvColumns?: { mode: "split" | "merge"; encoding?: SourceEncoding; size: number;
    delimiter: string; has_header: boolean; columns: string[]; column_count: number;
    split?: { index: number; name?: string; separator?: string; regex?: string;
              parts: number; new_names: string[] };
    merge?: { picks: { index: number; name?: string }[]; joiner: string;
              target_name: string };
    target_encoding: string; invalid: string; dirty_rows: number };
                             op: "contains" | "not-contains" | "eq" | "ne" | "gt" | "ge" | "lt" | "le";
                             value: string };
                mode: "keep" | "drop";
                dataRows: number; matchedRows: number; keptRows: number;
                nonNumericCells: number;
                targetEncoding: string; invalid: string; dirtyRows: number };
                             // 追加式可选字段：C12 CSV 过滤载荷，仅 csv-filter 使用（1:1）；
                             // 非数值单元格对一切数值算子不满足判定并计入 nonNumericCells；
                             // size=源文件大小，apply 复核源大小并在执行期重验表头
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
  entryConcurrency?: number;     // 任务内条目级并发 1–16（方案 §5.12）；缺省 1＝串行；介质自适应自动档待补
}
interface ApplyReport {
  planId: string; taskId: string;
  entries: { entryId: string; status: "ok" | "overwritten" | "skipped" | "failed"; error?: IpcError; detail?: string }[];
                             // overwritten 单列：§7.7"没有静默处理"，被覆盖条目必须可辨识
                             // detail＝条目级明细（方案 §5.2"替换并计数上报"落点），成功条目也可带；缺省省略字段
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
| 只读/被占用 → 预检标记＋执行内重试一次后按策略跳过 | **v1.48 定稿（OQ-37 裁决①）**：plan 期 `readonly`/`locked` 告警（F12/F13/C04 经 `accessFacts`，C03 经 `PlanC03File.readonly`，事实由 `platform::preflight::path_check_light`/`is_read_only` 采集）＋S01 预检 Warn 项＋执行期 `perform_with_retry`（只对 `retryable` 的 `E2003` 重打一次；`E2002` 按设计 06 不可重试）＋`settle_entry` 把 `E2002`/`E2003` 记 **Skipped＋带码报告**、不增 failed、不写 Journal，任务终态 `completed`。**范围＝改名族与 C03**（裁决①）：其余写臂仍 `Failed`。连带后果：C16 `retryTask` 不再把这些条目当失败项重试，用户须清除只读/释放占用后重新预览 | 已落地（真实 FS 证据限 POSIX，Windows 属性位待实机） |
| 执行中断 → 已完成项保留、可一键回滚 | 篇 01 §8 Journal 逆序回滚，覆盖 | 一致 |
| 杀软敏感 → 批次内节流（每 N 项微暂停） | **已定值（来源：用户确认 2026-10-27；已收编基线 §9.1——11.6-10，历史收编于方案 v1.7 R72）**：批次项数与微暂停时长以 §9.1 的杀软防护节流行准，参数可配置 | 已定值（契约先行，基线已收编） |

- `PlanDocument` 可序列化落盘为草稿，但**生成过程绝不落盘**（不变量 2）；`planId` 是 apply、撤销、日志三方共同的关联键。
- **C15 任务内条目级并发（v1.28）**：`ApplyOptions.entryConcurrency`（1–16，缺省 1＝串行，行为与历史版本一致）；通用执行器对 1:1 独立条目类（copy/edit-content/csv-dedup/csv-filter/csv-extract/csv-rows/delete）按计划序收块（块大小＝并发上限、规范化源/目标资源键不相交），块内并发执行、按计划序 join 与 Journal 记账（撤销按 seq 逆序语义不变）；rename/move/split/merge/csv-merge/listing/create-dir 始终串行；取消＝停止派发＋在途块收尾＋既有回滚；Journal 写失败先收完块再中止（成功条目必须进 Journal）。执行期限额：单任务条目上限 20 万（超出 E3007，方案 §5.12）、并发越界 E1002。报告按计划序落槽（回滚定位依赖位置对齐）。介质自适应默认值、C09/C10 专用执行器并发、运行中调参、致命错误自动暂停为已知缺口（ADR-0033）。
- **C12 CSV 过滤（v1.27）**：`plan_c12` 每个输入一条 `csv-filter` entry（1:1）；单条件 `contains`/`not-contains`/数值比较（eq/ne/gt/ge/lt/le，f64 解析、比较值计划期 E1002），`mode="keep"|"drop"` 显式选向；非数值单元格对一切数值算子不满足判定并计 `nonNumericCells` 发 Caution 告警，零匹配发 `no-matching-rows` Safe 警告；表头单独保留不参与判定；编码未确认 E3001、源/目标保护 E2009、列缺失/越界 E3004、重复输出目标 E1002；执行期整文件解码→逐行求值→按目标编码一次 `write_atomic`，apply 复核源大小与表头（E2010），撤销同 §8。单条件/整文件内存为已知缺口（ADR-0032）。
- **C11 CSV 去重（v1.26）**：`plan_c11` 每个输入一条 `csv-dedup` entry（1:1）；`key="row"` 整行判重或 `key="columns"` 按 `keyColumns`（名称经规范化表头解析，序号 1-based 越界 E3004）拼键，哈希集合判重、保留首次出现；表头单独保留，脏行只计数告警不修正；重复行发 `duplicates-removed` Caution 警告、零重复发 `no-duplicates` Safe 警告；编码未确认 E3001、源/目标位于保护目录 E2009、重复输出目标 E1002；执行期整文件解码→判重→按目标编码一次 `write_atomic`，apply 复核源大小与表头（E2010），撤销与覆盖备份同 §8 通用规则。整文件读入内存为已知缺口（ADR-0031）。
- **C10 ZIP 压缩（v1.25）**：`plan_c10_compress` 每个输出一条 `archive` summary entry（N:1），`params.sources` 保存源清单、`compression` 记录 store/deflate；输出等于源或位于源目录内 blocked E1002，目标保护 E2009，目标已存在 skip；执行期 `ArchiveService::compress`（UTF-8 名、目录递归、符号链接跳过）产出 ZIP 字节后一次 `write_atomic`，Journal forward=`WriteAtomic{None}`、reverse=`Quarantine`。内存 O(输出)；GBK 名写入/密码/时间戳等为后续缺口（ADR-0030）。
- **C10 ZIP 解压（v1.24；加密包 v1.53）**：`plan_c10` 每个压缩包一条 `extract` summary entry（原 1:N 说明见 ADR-0029 §3 的偏差登记）；`params.maxTotalBytes/maxRatio` 为显式限额（无产品默认）；平台以 zip 原始名 + 通用标志位 bit 11 判定 UTF-8，无标志位非 ASCII 回退 GB18030；kernel 拒绝绝对路径/盘符/`..`/NUL/非法名，越界条目跳过并告警，超限/不支持压缩方法 blocked E3007/E3003；执行期逐文件 `WriteAtomic`、逐目录 `CreateDir` 写 Journal，撤销走统一 undo。**加密包（v1.53，ADR-0074）**：密码不进计划——`PlanC10Input.password_provided` 只声明"执行期会提供密码"，成立时计划写 `params.passwordProvided=Bool(true)`，密码本体经执行期上下文 `ArchiveApplyContext.password` 传入；`inspect` 走 `by_index_data`（只读中央目录元数据，不需密码），故加密包能正常出预览；三道门分别是整单前置 E3008（计划声明需要密码而执行期没给，未触盘）、逐条目兜底 E3008、解密失败 E3009；ZipCrypto 与 AES-128/192/256 均支持（`zip/aes-crypto`）。压缩、GBK 名写入为后续切片。
- **C03 批量查找替换（v1.23）**：`plan_c03` 每个输入一条 `edit-content` 条目，计划 `params` 快照 `mode/find/replace/csvMode/delimiter/platformPrior/source`；普通模式用非重叠左到右 `str::replace`，正则模式复用 F13 的 regex 引擎与捕获组引用校验；CSV 默认 `cell` 按字段替换（引号结构保护，未闭合引号 E3004），`text` 模式发结构风险警告；二进制魔数/非 UTF-8 blocked E3001；空替换发 Danger 警告、无匹配 skip；执行经 `ContentRewriter` 整段变换（流式窗口与行区间为已知缺口）。决策见 [ADR-0028](../adr/0028-C03-查找替换整段变换与CSV单元格保护.md)。
- **C02 换行转换（v1.22）**：`plan_c02` 每个输入一条 `edit-content` 条目，计划 `params` 快照 `target/structure/platformPrior/source`；执行经 `ContentRewriter`＋`FsAdapter::rewrite_atomic_with` 流式读写（临时文件＋原子改名，备份与 Journal reverse 同 edit-content）；CSV 只转换引号外记录终止符，文本转换全部换行；二进制魔数、UTF-16 源、编码未确认均 blocked E3001；目标换行必须显式给出。决策见 [ADR-0027](../adr/0027-C02-换行转换流式重写与CSV引号保护.md)。
- **C04 名称查找替换（v1.29）**：`plan_c04` 复用 `plan_rename` 引擎（kind `c04`，条目仍为 `rename` 动作、无新增载荷）；plain＝字面非重叠左到右替换、regex＝与 F13 同引擎（`compile_regex`＋`validate_replacement`＋捕获组引用校验，非法/缺失引用计划期 E1002）；`find` 为空 E1002；params 快照 `mode/find/replace/scope`；冲突扫描/保留名/长路径分级/两阶段/Journal 撤销同 F12。纯替换规则无前缀/后缀叠加；大小写折叠选项与序号叠加未做（ADR-0034）。
- **F13 正则重命名（v1.21）**：`plan_f13` 复用 `rename::plan_rename` 的冲突扫描、长路径分级与两阶段编排；正则用 `regex` crate（有限自动机、线性时间、无回溯爆炸），替换模板支持 `$1`/`${name}`/`$$`，计划期校验缺失捕获组并返回 E1002；空 `pattern` 拒绝；无匹配转 `no-change` 警告且不产条目；非法目标名/冲突/长路径与 F12 同规则。决策见 [ADR-0026](../adr/0026-F13-正则重命名引擎与捕获组替换.md)。
- **F12 长路径分级（v1.20）**：`PlanF12Input.longPathSupported` 由壳层探测（`platform::preflight::long_path_supported`）后传入，kernel 保持纯函数（v1.52 起同一条也适用于 `PlanF12Input.nameRules`，见 §3 的「环境事实一律经端口注入」）；目标完整路径按方案 §5.13 三级判定——≤200 正常、200–259 发 `long-path-caution` 且条目照常执行、≥260 且未开启长路径支持时该条目 `resolution="skip"` 并发 `long-path-disabled`（提示开启方法或缩短目标路径），≥260 且已开启时正常执行。计划 `params.longPathSupported` 记录探测快照；apply 对 `resolution="skip"` 条目零副作用。
- **1:N / N:1 分组语义**（`relation`）：F01–F03/F05＝1:N（N 条 entry 共享 `groupId`，`role="output"`，`source` 相同）；**C10 解压首版切片按 ADR-0029 采用"每压缩包一条 `extract` summary entry＋执行期逐文件 Journal"，后续再按需拆 1:N；~~C10 压缩为后续切片~~（**逐文件报告已于 2026-11-13 补**，ADR-0096：清单进 `ApplyEntryReport.detail`，条目结构不变）**；**F04 合并为单条 `merge` entry（N:1 的输入清单记录在 `entry.merge.sources`），F06 CSV 合并为单条 `merge` entry（输入清单在 `entry.csvMerge.files`），F07 CSV 抽取为每个输入一条 `csv-extract` entry（1:1），F08 CSV 抽取数量行为每个输入一条 `csv-rows` entry（1:1），F09 位置区间为每个输入一条 `csv-rows` entry（1:1），F10 目录清单为每个根目录一条 `listing` entry（1:1），均不设 `relation`**（ADR-0016/ADR-0018/ADR-0019/ADR-0020：单条 `write_atomic` 保证产物原子性，撤销只需一条 Journal reverse）。Journal 仍按条目逐条记录；组回滚＝组内条目按 Journal 逆序逐条 `reverse`，无需组级操作；`stats.total` 按条目计数。

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
| E3xxx | 编码/格式 | E3001 无法识别编码；E3002 严格 UTF-8 校验失败（§5.2）；E3003 解压/压缩格式或压缩方法不支持（§5.11）；E3004 表头对齐失败（§5.4）；E3005 内容写回失败；E3006 解压条目越界/路径穿越拒绝（Z02，§5.11）；E3007 解压总量/压缩比超过调用方限额（C10，§5.11）；E3008 加密包需要密码而未提供（C10，§5.11）；E3009 密码错误或加密包损坏（C10，§5.11） |
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
- **隔离移动两阶段（v1.58，2026-11-08，D5 实机缺陷修复）**：`JournalStatus` 追加 `Pending`；c09 apply 先 `append_pending_with_kind`（意图落盘）→ 移动文件 → `mark_applied`（确认）；移动失败 `discard_pending`。`FileJournal::open` 打开时收敛 Pending：隔离路径存在 → 补成 Applied（撤销可见），不存在 → 丢弃（不留假记录）。撤销只回滚 `Applied`；Pending 不参与。锚点：`apply::tests::c09_crash_after_quarantine_move_leaves_a_journal_entry`、`platform::journal::tests::pending_quarantine_entry_recovers_applied_when_the_file_exists`、`pending_entry_is_dropped_when_the_move_never_happened`；决策见 [ADR-0084](../adr/0084-实机缺陷D1D2D5的断线重试与两阶段Journal.md)。
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
| v1.22 | 2026-10-31 | §3.1 追加 `plan_c02`；§5.1 落地 C02 换行转换：`edit-content`＋params 快照 target/structure/platformPrior/source；新增 `ContentRewriter` 端口与 `FsAdapter::rewrite_atomic_with` 流式重写；CSV 引号内换行保留、二进制魔数/UTF-16/编码未确认 blocked E3001。决策见 [ADR-0027](../adr/0027-C02-换行转换流式重写与CSV引号保护.md)。 |
| v1.23 | 2026-10-31 | §3.1 追加 `plan_c03`；§5.1 落地 C03 查找替换：plain/regex、CSV 单元格默认保护与 text 警告、匹配计数/上下文、空替换警告、无匹配 skip；二进制/非 UTF-8 blocked E3001；执行经 `ContentRewriter` 整段变换。决策见 [ADR-0028](../adr/0028-C03-查找替换整段变换与CSV单元格保护.md)。 |
| v1.25 | 2026-10-31 | §3.1 追加 `plan_c10_compress`；§5.1 落地 C10 压缩 summary entry、UTF-8 名写入、自包含/保护目录/已存在处理与一次原子写 Journal。决策见 [ADR-0030](../adr/0030-C10压缩UTF8名写入与自包含防护.md)。 |
| v1.24 | 2026-10-31 | §3.1 追加 `plan_c10`；§5.1 落地 C10 ZIP 解压 summary entry、GBK 名回退、路径穿越拒绝、E3007 限额与逐文件 Journal；E3xxx 增补 E3007。决策见 [ADR-0029](../adr/0029-C10解压安全边界与GBK名回退.md)。 |
| v1.26 | 2026-10-31 | §3.1 追加 `csv-dedup` 动作与 `plan_c11`；§3 追加 `csvDedup` 载荷（key="row"/"columns"、keyColumns、uniqueRows/duplicateRows/dirtyRows）；§5.1 落地 C11 CSV 去重：哈希集合判重保留首次出现、表头单独保留、脏行告警、编码未确认 E3001、保护目录 E2009、重复输出目标 E1002；执行期一次 `write_atomic`，撤销与覆盖备份同 §8。决策见 [ADR-0031](../adr/0031-C11-CSV去重内存判重与键序列化.md)。 |
| v1.27 | 2026-10-31 | §3.1 追加 `csv-filter` 动作与 `plan_c12`；§3 追加 `csvFilter` 载荷（condition{column,op,value}、mode、matchedRows/keptRows/nonNumericCells）；§5.1 落地 C12 CSV 过滤：contains/not-contains/数值比较、keep/drop 显式选向、非数值单元格保守不匹配＋告警、表头保留、E3001/E2009/E3004/E1002；执行期一次 `write_atomic`，撤销同 §8。决策见 [ADR-0032](../adr/0032-C12-CSV过滤判定器与类型解析.md)。 |
| v1.28 | 2026-10-31 | `ApplyOptions` 追加 `entryConcurrency`（1–16，缺省 1＝串行；方案 §5.12）；通用执行器落地任务内条目级并发（分块有界池＋资源键不相交＋Journal 计划序记账）；执行期限额：单任务 20 万条目 E3007、并发越界 E1002；`ProgressEvent.message` 明确为双层进度的条目层明细。决策见 [ADR-0033](../adr/0033-C15任务内条目级并发分块有界池.md)。 |
| v1.29 | 2026-11-01 | §3.1 追认 `plan_c04`（kind `c04`，复用 `rename` 动作）；§5.1 落地 C04 名称查找替换：plain 字面/regex 与 F13 同引擎、find 空与非法正则/缺失捕获引用计划期 E1002、冲突/长路径/两阶段继承 F12。决策见 [ADR-0034](../adr/0034-C04名称查找替换复用重命名引擎.md)。 |
| v1.30 | 2026-11-01 | §3.1 追加 `plan_c14`；§5 追加 `csv-columns` 动作与可选 `csvColumns{mode,encoding,size,delimiter,hasHeader,columns,columnCount,split,merge,targetEncoding,invalid,dirtyRows}` 载荷（C14 1:1，每个输入一条 entry；split=按分隔符/正则切段新列紧随源列且源列保留、parts 为采样评估值；merge=多列拼接新列追加表尾）。决策见 [ADR-0035](../adr/0035-C14-CSV列拆分合并.md)。 |
| v1.31 | 2026-11-01 | §3.1 追加 `plan_c07`（清单驱动 move/copy，1:1，路径安全 E1003/缺失 skip-error E2001/自指向 E1002 阻断，复用 `move`/`copy` 动作）；§4.7 FsAdapter 追加 `same_volume`/`remove_file`/`create_dir_all` 可选能力（缺省 false/E5001，设计 04 v1.5 同步）；Move 臂跨盘＝复制→校验大小与哈希→删源，校验失败 E3005 绝不删源并清理坏副本。决策见 [ADR-0036](../adr/0036-C07-清单驱动移动与跨盘校验删源.md)。 |
| v1.32 | 2026-11-01 | §3.1 追加 `plan_c08`（按规则归档：规则判定器链首中生效，四类条件 ext/mtime/size/name，未命中留原位或未分类目录，命中与未分类生成 `move` 条目复用 C07 移动引擎；高危命中>50% Danger 警告）。决策见 [ADR-0037](../adr/0037-C08按规则归档判定器链.md)。 |
| v1.33 | 2026-11-01 | §3.1 追加 `plan_c13`；§5 追加 `csv-sort` 动作与可选 `csvSort{encoding,size,delimiter,hasHeader,columns,columnCount,keys,externalChunkRows,targetEncoding,invalid,dirtyRows}` 载荷（C13 1:1；类型感知 numeric/date/lexicographic 采样推断、执行期解析失败回退字典序；externalChunkRows 显式传入触发分块排序＋多路归并，OQ-28 同口径）。决策见 [ADR-0038](../adr/0038-C13-CSV排序类型感知与外排.md)。 |
| v1.34 | 2026-11-01 | §5 追加**计划幂等契约**（C18）：固定 planId＋输入时重复预览输出逐字节一致；计划内派生 id（groupId 等）经 `platform::ids::derived_ulid` 确定性派生；F01–F03/F05 groupId 改确定性派生（横切 harness 首跑抓到其非幂等真缺陷）。决策见 [ADR-0039](../adr/0039-C18-横切干跑收口与计划幂等.md)。 |
| v1.35 | 2026-11-01 | §4.1 `planOperation` kind 路由扩展（c09/f05/f11/f12，其余 E1002）；追加 `renamePreview`/`undoDryRun` 壳层命令（C05/C06）。UI 接入切片，决策见 [ADR-0040](../adr/0040-UI接入壳层kind路由与主流程页面.md)。 |
| v1.36 | 2026-11-01 | §4.1 `planOperation` kind 路由批量扩展（f01–f04/c01–c03，壳层事实采集经 platform 同源函数）。决策见 [ADR-0041](../adr/0041-UI批量接入与壳层计划路由扩展.md)。 |
| v1.37 | 2026-11-01 | UI 接入第三批：CSV 家族 f06–f09 与目录清单 f10 的壳层参数面（`CsvPlanParams` 超集）；§4.3 定稿任务终态顺序的快照剪枝位置（终态事件顺序条款）。**补录行**：文首版本已声明 v1.37，修订表当时未落行（2026-11-02 发现并修复）。决策见 [ADR-0042](../adr/0042-UI接入第三批CSV家族与终态快照顺序.md)。 |
| v1.38 | 2026-11-01 | UI 接入第四批：C04 名称替换与 C11–C14 的 `OpPlanParams` 超集参数面（`plan_ops.rs`）。**补录行**（同 v1.37）。决策见 [ADR-0043](../adr/0043-UI接入第四批CSV处理族与名称替换参数面归一.md)。 |
| v1.39 | 2026-11-01 | UI 接入第五批：C07 清单／C08 规则／C10 ZIP 的壳层计划；`run_apply` 为 c10 增设专用执行器分支（`apply_archive_plan`＋`ZipArchiveService`，保留逐文件 Journal 撤销语义）。**补录行**（同 v1.37）。决策见 [ADR-0044](../adr/0044-UI接入第五批清单规则ZIP与桌面专用执行器.md)。 |
| v1.40 | 2026-11-02 | §3.1 `PlanF11Input` 追加 `existing_files` 事实（与 `existing_targets` 同覆盖**每一级前缀**）；§5 `create-dir` 明确：文件占据祖先位置时该整条子树按 `conflict.kind=parent-is-file`＋`resolution=skip` 处理，并出计划级 `parent-is-file` 集中报告（不新增错误码，复用既有冲突形态；方案 §5.15/§5.8）。入口两侧（CLI `run_f11`、壳层 `f11_existing_facts`）事实采集同步。决策见 [ADR-0049](../adr/0049-逐级前缀事实与集中报告的界面点名.md)。 |
| v1.41 | 2026-11-02 | §5 `ApplyEntryReport` 追加可选 `detail`（serde default＋缺省省略，旧报告仍可反序列化）：条目级明细由注入的内容变换产出，落点＝方案 §5.2 的"替换并计数上报"；`ContentEditor` 返回型由 `Vec<u8>` 改为 `EditedContent{bytes,detail}`（新增 `pub` 类型），`Performed` 同步搬运；`encoding::Transcoded.replacement_count` 口径由"有错=1"改为**解码产生的 U+FFFD 个数**（源文本自带 U+FFFD 会一并计入，边界见 ADR §4）。决策见 [ADR-0050](../adr/0050-条目级detail的载体与替换计数口径.md)。 |
| v1.42 | 2026-11-02 | §2.2 取消处置：`ApplyOptions.cancelPolicy`（缺省 `rollbackApplied`＝按 Journal 反向回滚；`keepApplied`＝保留已应用条目并留 Journal，`undone_by` 为空）。桌面壳对所有 apply 任务传 `keepApplied`，取消后询问「已完成 N 项，是否一并撤销？」，答复撤销走 `undoPlan`；c10 归档执行器只有保留通道。§4.3 `task://done` 追加 `kind`（apply/undo），使界面只对 apply 任务的取消发问。决策见 [ADR-0052](../adr/0052-取消处置的询问与保留语义.md)。 |
| v1.43 | 2026-11-02 | §5 `merge` 载荷的执行方式：逐源 `read_stream`→`StreamTranscoder`→`write_atomic_with`，内存 O(块) 而非 O(产物)；目标 BOM 仍只写一次，源大小变化按 E2010 跳过；`FsAdapter` 新增 `read_stream`（设计 04 v1.11）。决策见 [ADR-0053](../adr/0053-流式读取端口与F04合并的流式执行.md)。 |
| v1.44 | 2026-11-03 | §5 CSV 家族执行臂的执行方式（契约面不变）：`csv-merge`（F06）、`csv-extract`（F07）、`csv-rows` first/last（F08）、`csv-dedup`（C11）、`csv-filter`（C12）改逐源 `read_stream`→`RecordFeeder` 增量记录→逐条转码写出，内存 O(块＋单条未完记录)（判重键集合/last-N 环形缓冲为固有驻留）；逐源大小与表头复核沿 E2010 原码与消息，输出字节与整段路径一致（BOM 仍只写一次）。决策见 [ADR-0054](../adr/0054-流式CSV执行臂与增量记录供给器.md)。 |
| v1.45 | 2026-11-03 | §5 `ContentRewriter::rewrite` 返回型改为 `EditOutcome { detail: Option<String> }`（明细与 `ContentEditor` 的 `EditedContent.detail` 同载体，C01 转码计数由此上报）；`csv-columns`（C14）执行臂改流式（同 ADR-0054 管道）。CLI 侧 C01 转码编辑器改走流式 `ContentRewriter`（源编码计划期已确认 ⇒ 按块解码写出；替换计数进 detail）。决策见 [ADR-0055](../adr/0055-C01流式转码编辑器与ContentRewriter明细返回位.md)。 |
| v1.46 | 2026-11-03 | §2.3 `TaskSummary` 追加可选 `planId`（serde `default`＋缺省省略）：撤销按 plan 走共享 Journal，任务列表要能直接回滚"刚才那次执行"就必须知道计划 ID，否则界面只剩取消/重试。旧 S03 快照无该字段时按 `None` 处理，启动恢复会从 job 回填；壳层 apply/undo 两类任务都填同一个 planId。锚点：`tasks::tests::legacy_task_snapshots_without_plan_id_still_deserialize`、`tasks::tests::c03_replace_plan_executes_through_the_desktop_queue_and_undoes`（内含 list 断言） |
| v1.47 | 2026-11-04 | §5.1「只读/被占用」行的结论**如实改标**：此前写"已补执行内重试"，实测 `generic_apply` 既无重试也不产 `readonly`/`locked` 告警（只读失败按 `Failed`＋适配层 `E2002` 记账）。本批落地点名半程——F12/F13/C04 计划输入追加 `accessFacts`（`Vec<PathCheck>`，与 S01 同型），C03 的 `PlanC03File` 追加 `readonly`/`locked`，命中即发 `PlanWarning{kind:"readonly" \| "locked"}`（kind 名取自 §5 既有枚举，不新增 token）；事实由 `platform::preflight::path_check_light`/`is_read_only` 采集（轻量档**不开写句柄探测**：占用是瞬时状态，十万条目下这个代价不该由"生成预览"来付，占用判定留在预检 `path_check`，故计划期 `locked` 恒 false）。执行期"重试一次→再失败 skipped"仍不实现：把 `E2002/E2003` 由 Failed 改判 Skipped 连带影响 C16 重试失败项集合与"部分失败"统计，属产品可见语义，登记 OQ-37。线协议、错误码表与 `PlanWarning.kind` 枚举均未变。验收：`rename::tests::readonly_and_locked_facts_are_named_in_preview_warnings`、`rename::tests::absent_access_facts_produce_no_readonly_or_locked_warning`、`replace_content::tests::readonly_and_locked_files_are_named_in_preview_warnings`、`replace_content::tests::blocked_files_are_not_named_as_readonly`（先红实证：临时摘掉告警发射后两条正例转红、两条反向对照仍绿）；`platform::preflight::path_check_light_reads_the_attribute_bit_without_probing_writes`；`apply_flow::f12_and_c03_name_read_only_files_in_plan_warnings`（真实 CLI 出口）；`corpus::e03_readonly_target_is_marked_by_preflight_and_named_in_preview`（E03 语料，assertions.tsv 由 pending 升 partial） |
| v1.48 | 2026-11-04 | OQ-37 裁决①落地：`generic_apply` 对改名族（f12/f13/c04）与 C03 的 `E2002`/`E2003` 记 **Skipped＋带码报告**（不增 failed、不写 Journal、源文件留原位），并新增 `perform_with_retry`——只对 `retryable=true` 的 `E2003` 重打一次，`E2002` 不重试（设计 06）。其余写臂维持 `Failed`。§5.1 该行由"半程"定稿。验收：`generic_apply::tests::readonly_rename_is_skipped_and_reported_without_failing_the_task`、`readonly_source_is_skipped_for_content_rewrites`、`occupied_target_is_retried_once_and_then_applies`（断 `rename` 调用次数＝2）、`non_retryable_permission_error_is_not_retried`、`permission_failure_still_fails_arms_outside_the_named_scope`（范围外仍 Failed）、`apply_flow::read_only_rename_target_is_skipped_without_failing_the_task`（POSIX 真实 FS：chmod 555 → EACCES → E2002 → skipped）。先红实证：分别摘掉改判分支与重试分支，对应正例转红、反向对照仍绿 |
| v1.49 | 2026-11-05 | §2.3 `TaskSummary` 追加 **必填** `job: "apply" \| "undo"`（Rust 侧 `#[serde(default)]`＝`Apply`，旧 S03 快照仍可反序列化）：`kind` 装的是被处理计划的功能码，撤销任务也是它，界面此前按 `kind === "undo"` 判别回滚任务，那个值壳层从不发出——于是回滚任务照样提供「重试失败项」（`retryTask` 必拒 E1002）与二次「撤销」（回滚是终态）。恢复路径按 `StoredJob` 重算 `job`，不信任快照里的旧值。决策见 [ADR-0066](../adr/0066-任务类别需要独立于功能码的字段.md)。 |
| v1.50 | 2026-11-05 | §5 的 `csv` 载荷追加可选 `shardBom`（`Bom`，缺省省略）：F05 支持 **UTF-16 带 BOM** 源直接分片。方案 §5.1 的 BOM 策略要求 UTF-16/UTF-32 **每片都写 BOM**（无 BOM 则分片无法被识别），而源字节直通时分片区间只覆盖 0 起的那段，其余分片没有 BOM。落地方式：内核 `CsvRecordScanner::new_utf16` 按码元投影扫描（结构字符只可能是 ASCII，记录长度仍按源字节计），表头区间因含 BOM 而自行带出首片前缀；首片区间不从 0 起时（`keepHeader=false` 的第 2 片起）由计划给该条目标 `shardBom`，执行层 `FsAdapter::copy_ranges_atomic` 新增 `prefix` 参数在全部区间之前写一次，`expected.size` 计入前缀。无 BOM 的 UTF-16 不猜测重写形态，仍 E3001 提示先走 C01。验收：K08 语料（T v1.12 §3.2）由 partial 转 asserted——`corpus::k08_utf16_bom_sample_detects_transcodes_and_splits_with_a_bom_per_part`、`apply_flow::k08_utf16_csv_parts_each_start_with_a_bom`（真实 FS：三片产物逐片 FF FE 开头）、`apply_flow::k08_utf16_csv_parts_without_header_still_start_with_a_bom`（第 2 片起走补写路径，并逐字节比对源区间）。线协议追加式、旧计划仍可反序列化；决策见 [ADR-0070](../adr/0070-UTF-16分片每片写BOM.md)。 |
| v1.51 | 2026-11-05 | §4.4.2 补**处理模板六条命令**（`listRecipes`/`getRecipe`/`saveRecipe`/`deleteRecipe`/`exportRecipe`/`importRecipe`）与 DTO（`RecipeDocument`/`TemplateStep`/`RecipeMatch`/`SaveRecipeInput`/`RecipePage`）：这六条此前只在壳层实现、从未进契约，前端接上模板库后补齐。同时定稿**保存时剔除路径参数**——`saveRecipe` 落盘前对每步调 `strip_path_params`，名单按本仓库参数名真实语义校正：`source`／`target` 在 F04/F06/F07/F08/C01/C11–C14 里是**编码**，旧表把 `source` 当路径删掉会让模板丢编码；而 UI 真用的 `file`／`files`／`output`／`manifest`／`list`／`archive`／`dir` 等又不在旧表里（导入自家模板会留绝对路径）。判据只留内核一处，界面不抄第二份键表。**`loadRecipe` 仍未实现**，阻塞于 OQ-36（动作→功能码一对多），界面因此不提供「一键套用」。验收：`recipe::tests::strip_path_params_drops_real_paths_and_keeps_encodings`、`recipe::tests::strip_paths_covers_every_step_and_reports_each_key_once`；界面 `TemplatesPage.test.tsx`（4 项）＋`FeaturePages.template.test.tsx`（3 项）＋`App.navigation` 的模板可达断言；决策见 [ADR-0072](../adr/0072-处理模板前端只做存取半边.md)。 |
| v1.52 | 2026-11-05 | §3 明确「环境事实一律经端口注入」并补 `FsAdapter::name_rules`：目标名合法性从内核常量（Windows 保留名／`< > : " \| ? *`／结尾点空格）改为**能力值** `NameRules`（`windows()`/`posix()`），经 `PlanF12Input`/`PlanF13Input`/`PlanC04Input`/`PlanF11Input`/`PlanC10Input` 与 apply 期 `ApplyContext`/`ArchiveApplyContext` 注入；POSIX 只拦 `/` 与 NUL。同时修正 `case_sensitivity()` 只判 `cfg!(windows)`、把 macOS 误归 `Sensitive` 的缺陷。详见 [04 §2.1](04-文件系统适配层契约.md) 与 [ADR-0073](../adr/0073-目标名合法性改为能力值.md)。 |
| v1.55 | 2026-11-06 | §4.4.2 `loadRecipe` 由「未实现」改为「已实现（入参 `{recipeId, pathParams}`）」——OQ-40 裁决①落地，路径槽位表与防漂移断言见 ADR-0077；同时登记查出的形状差异（c01/c02/c03）为 OQ-41，界面套用入口未接。 |

| v1.56 | 2026-11-07 | OQ-41 裁决②落地（[ADR-0080](./../adr/0080-OQ41裁决二与套用映射表.md)）：§4.4.2 新增 `describeRecipe` 命令与 `RecipeApplySpec`/`RecipeSampling` DTO；`SaveRecipeInput`/`TemplateStep` 增 `sampling`（v5，进模板不进快照）；**`TemplateStep` 接口块补上 v1.54 应增而漏增的 `feature` 字段**（接口块停在 v3 形状、注释与 v4 相悖，本批钉正）；`loadRecipe` 段落改写为「参数映射表闭合形状差异」——17 功能可套用、7 功能正文缺失如实拒绝（OQ-43 登记）、c11 按键列拒绝、c10 旧压缩模板拒绝重存；§5 计划参数键修订（OQ-41②清点顺带钉出的三处形状缺陷）：f04/f06 输出路径键 `target→output`（`target` 剔不进 PATH_PARAM_KEYS，模板保存/导出带出绝对路径＝不变量 13 泄漏）、c10 压缩计划补 `mode:"compress"`（套用通道按快照 mode 分流槽位）；c10 `maxRatio` 由映射层数值化（不动 `PlanParamValue` 的 `Eq` 派生链）。验收：kernel 437 项、壳层 69 项、Vitest 120 项、Playwright 18 项、`check_docs.py` 含新增配方套用表核对（壳层两张表↔浏览器 mock 逐键相等）。 |

| v1.57 | 2026-11-07 | OQ-43 裁决②落地（[ADR-0081](./../adr/0081-OQ43裁决二正文进模板.md)）：§4.4.2 `TemplateStep`/`SaveRecipeInput` 增 `content`（v6，变换正文进模板不进快照，值只允许标量/标量数组）；六功能（f06/f07/c08/c12/c13/c14）套用由正文补齐、缺正文的旧模板拒绝并提示重存；**f11 纠偏为可套用**——清单文件与建档根都是路径槽位、路由重读清单内容，ADR-0080 §2 把它误列为「正文缺失」；`PATH_PARAM_KEYS` 补 `basedir`（f06 相对来源列的基准目录是路径，不在名单会随 v6 正文/快照泄漏）。验收：kernel 436 项、壳层 63 项（六功能正文键逐个落进路由结构体字段＋缺正文拒绝＋白名单外拒绝＋f11 槽位恢复）、Vitest/Playwright 绿、check_docs 配方套用表核对改核 content 键集。 |
| v1.59 | 2026-11-08 | §5 动作表的 `transcode` 落执行语义：与 `edit-content` 共用通用执行器的就地改写路径（备份→变换→原子写回→Journal `WriteAtomic{dataRef}`），变换仍由壳层按计划 kind 注入编辑器/重写器；`undo --dry-run` 的网络路径 `stat`/`list_dir` 改走 D1 重试（2/4/8 秒），耗尽/无暂停面时返回 E2005 而非记成 `missing`（`undo_dry_run_with_retry`）。决策见 [ADR-0084](../adr/0084-实机缺陷D1D2D5的断线重试与两阶段Journal.md)。 |
| v1.60 | 2026-11-09 | C37 Phase B 多步模板落地（[ADR-0089](../adr/0089-C37多步模板执行口径与运行模式缺省.md)）：§4.4.2 `loadRecipe` 入参增 `stepIndex?`（缺省第 1 步，多步按步调用、不预生成所有步骤；越界如实报错）；`describeRecipe` 出参增 `steps[]`（逐段评估）与 `oneClickAllowed`（全部步骤可套用且无高风险步骤），`applicable`/`feature`/`required`/`outputs` 保持单步语义不变。运行模式（一键/逐步骤）是 UiState `templatePrefs` 的本机偏好，不写进 `RecipeDocument`／导出。 |
| v1.64 | 2026-11-13 | §2.2／§4.3 追加条目内（字节级）进度字段 `entry_done`／`entry_total`（serde 缺省省略）：方案 §7.5「双层进度……当前条目进度（大文件可见）」的落点——F04 合并是 N:1 单条目，只报条目计数会停在 0/1。执行臂（`WorkerView` 带 `&dyn ProgressSink`）在读块回调里上报；壳层 `TaskSummary` 同名字段按 `entry_total.is_some()` 分流；界面另起一行显示人类可读字节。决策见 [ADR-0100](../adr/0100-条目内字节级进度双层进度的条目层.md)。 |
| v1.63 | 2026-11-13 | §2.2 取消粒度细化＋§5 `ContentRewriter::rewrite` 增 `cancel: &dyn CancelToken`：C02 在 64 KiB 读块边界响应取消，返回 **`Err(KernelError::cancelled())`（E3010）**——回调必须以 Err 收场，否则 `rewrite_atomic_with` 会把半截内容原子改名盖到源文件；`settle_entry` 把 E3010 记 `Skipped`（释放授权、不进 failed），串行/并发两条循环在**循环结束后**再确认一次 token（最后一条被取消时顶部检查没机会跑）；新增 `progress::NoCancel` 空实现供无取消端口的调用方。C01／C03 签名同步但实现仍为 `_cancel`。错误码见设计 06 v1.8。决策见 [ADR-0097](../adr/0097-流式重写mid-stream取消传cancel-token与E3010区分取消失败.md)。 |
| v1.62 | 2026-11-13 | C10 逐文件报告（方案 §5.11「压缩完成后自动生成文件清单」、设计 06 §3 的条目级 detail 口径）：`ArchiveService::compress` 返回 `Vec<u8>` → **`CompressedArchive { bytes, manifest }`**，`manifest: Vec<ArchiveManifestItem { name, size }>`（未压缩体积、按写入顺序、不含目录条目），压缩侧在遍历源时顺手采集（不二次解析产物）；`apply_archive_plan` 的压缩与解压成功条目填 `detail`＝`format_archive_manifest`（表头 `N 个文件，共 X 字节`＋逐行 `名称 体积`，超 200 条截断并注明剩余）。条目结构仍是每压缩包一条 summary（**不拆 1:N**）；Journal/撤销语义与并发口径不变。界面：功能页终态横幅渲染 `detail`（新增 `.manifest` 样式，`pre-line` 保留换行）。决策见 [ADR-0096](../adr/0096-C10逐文件报告清单进ApplyEntryReport-detail.md)。 |
| v1.61 | 2026-11-10 | OQ-45 裁决落地（[ADR-0090](../adr/0090-模板来源字段与运行模式缺省.md)）：§4.4.2 `RecipeDocument` DTO 增 `origin?: "local" | "imported"`（v7；保存写 `local`、导入一律改写 `imported`，v6→v7 迁移不猜来源；方案 v1.22 §15.2.3／§15.2.4 收编）。运行模式缺省由「一律逐步骤」改为按来源（本机新建默认一键）。 |
| v1.54 | 2026-11-06 | §4.4.2：`TemplateStep`/`SaveRecipeInput` 增 `feature`（OQ-36 裁决①，schema v4）；`importRecipe` 的高版本阈值措辞改为跟常量走；`loadRecipe` 段落改写——**功能码已不是缺口，缺的是输入通道**（本命令表与方案 §15.2.5 冲突），登记 OQ-40。 |
| v1.53 | 2026-11-05 | §5.11 C10 解压落地**加密包的密码注入面**（此前写"密码包为后续切片"，实测缺口比"没做参数"深：平台 `inspect` 走 `by_index`，遇加密条目直接 `UnsupportedArchive(PASSWORD_REQUIRED)`，计划连一条 `encrypted` 条目都生不出来，`plan_c10` 里那段 E3003 加密分支是**死代码**）。定稿：`PlanC10Input` 追加 `password_provided: bool`——**只声明"执行期会提供密码"这一事实，不是密码**；成立时计划写 `params.passwordProvided=Bool(true)`，密码本体只经执行期上下文 `ArchiveApplyContext.password` 传入，`PlanDocument.params`／`StoredPlan`／`StoredJob`／`PersistedQueue` 与 C18 幂等 harness 因此都不再有机会见到秘密。平台 `inspect` 改 `by_index_data`（只读中央目录元数据，无需密码），`extract_entry` 增 `password` 参数走 `by_index_decrypt`；启用 `zip/aes-crypto` 同时支持 ZipCrypto 与 AES-128/192/256。三道门：整单前置（计划声明需要密码而执行期没给 → E3008，未触盘）、逐条目兜底（选中条目加密且无密码 → E3008）、解密失败（E3009）。`E3003` 回归"格式/压缩方法不支持"。新增预警 `encrypted-archive`、`password-unused`（设计 06 §2A 61 → 63 类）。CLI：`c10 --extract` 的 `--expect-password` 声明事实、`apply --password-stdin` 供密码（只剥 CR/LF、不 trim，空串按未给，与 `--compress` 互斥）；壳层：`AppState.plan_secrets` 只活在内存、`Job.secret` 不进 `StoredJob`/`PersistedQueue`，崩溃恢复与重试一律无密码（拿到旧密码反而更危险），故恢复后执行加密包必报 E3008，这是决策的代价而非缺陷。验收：`archive_password.rs` 6 项（含 AES/ZipCrypto 两条正例、错密码 E3009 且产物为零、缺密码整单前置、空串算缺）、`apply_flow` 3 项端到端（其中一条 `assert!(!plan_file.contains("secret123"))`）、壳层 `the_archive_password_never_reaches_the_task_snapshot`、前端 `FeaturePages.password.test.tsx` 2 项；决策与六条残余见 [ADR-0074](../adr/0074-C10密码包的秘密注入面.md)。 |
