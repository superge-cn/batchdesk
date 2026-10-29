# 03 配置 Schema 与迁移契约

> 状态：已评审·实现冻结 v1.3（2026-10-27 评审；2026-10-28 增量修订，用户确认） ｜ 适用：crates/kernel（类型）、src-tauri（存储）、web/（设置区） ｜ 日期：2026-10-27
>
> 上游：[01-内核接口与IPC契约](01-内核接口与IPC契约.md)（命令名与错误结构）、[02-Web双通道传输契约](02-Web双通道传输契约.md)（Web 存储形态）。
> 权威：方案 §4.5（三档优先级/存储位置/版本迁移/安全默认值）、§6.7（设置区分组）、§7.3（参数配置）、§4.6／§9.6（日志双轨与保留）、§9.5（配方与配置带版本、位图配置驱动）、§8.5（授权数据单独加密）、§13.8（跨形态只经配方导出迁移）、§15（处理模板与持久化扩展）。

## 0. 权威来源索引

| 议题 | 权威章节 | 本文角色 |
|---|---|---|
| 三档优先级与来源可解释 | §4.5 | 落地为 layered value 模型 |
| 存储位置、配置与应用数据分离 | §4.5 | 落地为存储拓扑表 |
| 配置带版本号、升级迁移 | §4.5 / §9.5 | 落地为 migration 契约 |
| 安全默认值四条 | §4.5 | 落地为默认值登记表 |
| 设置区六分组 | §6.7 | 落地为 scope 枚举 |
| 处理模板（C37）= 单步/多步参数快照、载入需重新预览 | §1.4 / §7.3 / §15 | 落地为 RecipeDocument v2/v3 |
| 配置与界面状态分层 | §15.3 | 落地为 ConfigDocument（用户设置）+ UiState（本机布局/上下文） |
| 授权数据加密单独存储、不混放 | §8.5 | 划定本篇边界（不含授权数据） |
| 功能位图以配置驱动 | §9.5 | 声明位图与配置的关系及强制力归属 |
| 跨形态迁移只经"导出产物/配方" | §13.8 | 落地为配方导入/导出契约 |

## 1. 范围与边界

**本篇管理**：普通用户配置（§6.7 六分组）、处理模板（C37，含配方/最近使用参数）、界面状态（UiState）。
**本篇不管**：授权数据（§8.5 单独加密存储，由 LicenseService 实现自管，见 §8）、任务日志（§4.6 双轨，属应用数据）、Journal（篇 01 §8）、隔离目录内容（应用数据）。

判定规则：**配置可丢失可重建**（恢复默认即回到安全档）；**授权数据、审计日志不可丢失**——前者加密独立、后者落库，均不进本文 schema。

## 2. 三档模型与来源追踪（§4.5）

```text
effective = recipe/task 覆盖  >  user 配置  >  默认值
```

```ts
/** 设置页"为什么是这个值"的数据支撑（§4.5） */
interface SettingValue<T> {
  value: T;                       // 当前生效值
  effectiveSource: "default" | "user" | "recipe" | "task";
  userValue?: T;                  // 用户档显式值（未设置则省略）
  defaultValue: T;
}
```

- 优先级解析为**纯函数**（kernel 可测）：`resolveEffective(layers) -> SettingValue`，输入输出全走参数（不变量 1）。
- 配方/任务档只在**任务上下文**内生效，不回写用户配置；载入配方后必须重新预览，不直接执行（§7.3）。

### 2.1 设置视图类型

```ts
/** 单 scope 的逐项来源视图（getConfig({scope}) / setConfig / resetScope 出参） */
interface ScopeView {
  scope: keyof ConfigDocument["scopes"];
  values: Record<string, SettingValue<unknown>>;
}
```

## 3. 存储拓扑（§4.5 / §13.3）

| 对象 | 桌面位置 | Web 形态 | 可丢失性 |
|---|---|---|---|
| 用户配置 | 平台惯例配置目录（§4.5：`%APPDATA%` / `~/Library/Application Support` / `~/.config`） | OPFS `config/`（§13.3，可回收） | 可重建 |
| 配方/处理模板 | 同上 `recipes/` | OPFS `recipes/` | 可重建（鼓励导出） |
| 界面状态 UiState | 配置目录 `ui-state.json`（与 config 分离） | OPFS `ui-state/` | 可重置（丢失只回默认布局） |
| 运行日志/操作日志/隔离区/Journal | 应用数据目录（**与配置分离**，§4.5） | 会话内（§11.4-13） | 不可随意丢 |

- 文件格式统一 JSON、camelCase，与篇 01 线协议同源；类型单一来源生成（篇 01 §9 第 4 条同一机制），防手写漂移。
- Web 形态不承诺配置留存（§13.9），设置页须按 §13.8 口径显式声明，不做等价暗示。

## 4. ConfigDocument Schema

```ts
interface ConfigDocument {
  configVersion: number;          // 迁移锚点（§4.5），单调递增，见 §6
  updatedAt: string;              // RFC 3339 UTC
  scopes: {
    general?: GeneralScope;       // 语言、主题、默认输出位置、启动行为（§6.7）
    processing?: ProcessingScope; // 编码策略、换行符、冲突策略、是否保留备份（§6.7）
    performance?: PerformanceScope; // 并发数、内存预算、大文件阈值，均有"恢复自动"（§6.7）
    security?: SecurityScope;     // 保护目录名单、隔离目录位置与保留期、占位文件策略（§6.7）
    logging?: LoggingScope;       // 保留天数、是否记录完整路径（§6.7）
    // 授权组（§6.7 第六组）只读展示数据走 getLicenseState（篇 01 §4.4），不落本文档（§8.5）
  };
}
```

### 4.1 各 Scope 字段清单（工程契约；追加字段必须带默认值，无默认值不得合入）

字段存在性与类型由本节冻结；凡涉及产品阈值的字段一律不设数值，显式数值须先入基线（§9.1／§13.10.2／§9.6 保留策略）方可引用（数值单源）。

```ts
// 通用（§6.7：语言、主题、默认输出位置、启动行为）
interface GeneralScope {
  language?: string;                        // 工程默认 "zh-CN"
  theme?: "system" | "light" | "dark";      // 工程默认 "light"（U v1.3 §5 D-U5 浅色默认；system 仍可选）
  defaultOutputDir?: string;                // 工程默认：省略＝每次询问（不预设写位置，安全默认）
  launchBehavior?: "normal" | "minimized";  // 工程默认 "normal"
}

// 处理（§6.7：编码策略、换行符、冲突策略、是否保留备份）
interface ProcessingScope {
  onConflict?: "skip" | "sequence" | "overwrite" | "ask";  // 默认 "skip"（§4.2 登记）
  encodingPolicy?: "auto" | "strict-utf8";   // 工程默认 "auto"（§5.2 检测流水线；strict 对应 E3002）
  newlinePolicy?: "keep" | "crlf" | "lf";    // 工程默认 "keep"
  deletionMode?: "move-to-quarantine" | "permanent";  // 默认 "move-to-quarantine"（§4.2 登记）
  keepBackup?: boolean;                      // 默认 true（§4.2 登记，查找替换类）
}

// 性能（§6.7：并发数、内存预算、大文件阈值，均有"恢复自动"）
interface PerformanceScope {
  concurrency?: number | "auto";        // 工程默认 "auto"（§9.1 并发行：任务内并发自适应）
  memoryBudget?: number | "auto";       // 工程默认 "auto"；显式数值引用 §13.10.2（Web）／§9.1（桌面）
  largeFileThreshold?: number | "auto"; // 工程默认 "auto"（§1.4 三档策略分档）；显式数值须先入基线 §9.1
}

// 安全（§6.7：保护目录名单、隔离目录位置与保留期、占位文件策略）
interface SecurityScope {
  protectedDirs?: string[];             // 默认 []（v1 不预置，方案 11.6-9；匹配语义见篇 04 §3）
  quarantineDir?: string;               // 工程默认：省略＝应用数据目录内默认隔离子目录（§3 拓扑）
  quarantineRetentionDays?: number;     // 与撤销窗口同一口径（方案 §7.8、11.6-14）；默认值与可配区间引用 §7.8，不在此复述
  placeholderPolicy?: "skip" | "download-then-process";  // 默认 "skip"（§4.2 登记；§4.9"仅处理已下载内容"开关）
  diskSpaceMarginFloorBytes?: number;   // 工程默认取 §9.1 磁盘空间安全余量行的下限（11.6-12）；数值以基线为准
  diskSpaceMarginRatio?: number;        // 工程默认取 §9.1 同一行的比例（11.6-12）；余量判定式引用 §9.1
}

// 日志（§6.7：保留天数、是否记录完整路径）
interface LoggingScope {
  retentionDays?: number;      // 仅指操作日志保留天数；默认/可配区间引用 §9.6，诊断日志不复用本字段
  logFullPaths?: boolean;      // 工程默认 false（隐私保守默认，§4.10 口径）
}
```

### 4.2 安全默认值登记表（§4.5 四条，冻结后不得改向）

| scope.key | 默认 | 基线依据 |
|---|---|---|
| `processing.onConflict` | `"skip"` | §4.5 默认不覆盖；篇 01 §5 同源 |
| `security.placeholderPolicy` | `"skip"` | §4.5 默认跳过占位文件；§4.9 |
| `processing.deletionMode` | `"move-to-quarantine"` | §4.5 默认不删除只移动；不变量 7 |
| `processing.keepBackup`（查找替换类） | `true` | §4.5 默认保留备份；§5.14 |

- `performance.*` 各项**不设数值**：取值区间与"恢复自动"的语义引用 §9.1 / §13.10.2 既定值；本文只固定字段存在性与类型。
- 各 scope 的完整字段清单见 §4.1；**新增字段必须带默认值登记**（无默认值的配置项不得合入）；§4.1 清单中标注"工程默认"的默认值属工程契约，改动不构成产品行为变更，但须随本篇修订记录留痕。

## 5. 处理模板（C37）、配方与最近使用（§1.4 / §7.3 / §15）

### 5.1 RecipeDocument v2（Phase A：单步）

```ts
interface RecipeDocumentV2 {
  recipeId: string;                 // ULID
  schemaVersion: 2;
  name: string;
  description?: string;
  kind: OperationKind;              // 单步功能，与篇 01 PlanDocument.kind 同枚举
  paramsSnapshot: Record<string, unknown>;  // 参数快照，载入后重新预览（§7.3）
  match?: {
    extensions?: string[];          // 小写、不带点；仅用于建议，不自动应用
    tags?: string[];                // 用户标签
  };
  sourceFormat?: "desktop" | "web"; // 跨形态导入标记（§13.8）
  createdAt: string;
  updatedAt?: string;
  lastUsedAt?: string;
  usageCount?: number;
  pinned?: boolean;
}
```

### 5.2 RecipeDocument v3（Phase B：单步/多步统一）

```ts
interface RecipeDocumentV3 {
  recipeId: string;
  schemaVersion: 3;
  name: string;
  description?: string;
  steps: TemplateStep[];            // 单步模板 = 1 个 step；顺序即依赖
  match?: { extensions?: string[]; tags?: string[] };
  sourceFormat?: "desktop" | "web";
  createdAt: string; updatedAt?: string; lastUsedAt?: string; usageCount?: number; pinned?: boolean;
}

interface TemplateStep {
  stepId: string;
  kind: OperationKind;
  paramsSnapshot: Record<string, unknown>;
  source: "original" | "previous";  // 原始选择 / 上一步实际产物
}
```

- **运行模式**（一键/逐步骤）是**本机 UI 偏好**，存 UiState（§11），不写入 `RecipeDocument`/导出文件；导入模板默认逐步骤；含高风险步骤时一键不可用。
- 模板是**可导入导出的迁移载体**（§13.8）：导出 v3 JSON；导入接受 v1/v2/v3，低版本迁移、高版本拒绝（E1007）。
- 模板不保存源文件路径、预览结果、文件内容/文件名清单；输出目录默认不保存，应用时重选。
- 多步骤上限沿用 F15 单束 **10 步**（§15.1；不新增数值）。
- 最近使用参数：内存态 + `recent` 私有档（§4.2 应用层职责），不入 ConfigDocument 主文件，崩溃可丢。

## 6. 迁移契约（§4.5 / §9.5）

### 6.1 规则

1. **线性迁移链**：每个 `configVersion` 注册一个迁移函数 `migrate(N → N+1): ConfigDocument`，链式执行到当前版本；禁止跳版本合并迁移（可测性）。
2. **幂等与原子**：迁移在内存完成、整体原子落盘（临时文件 + 改名，§4.3 习惯）；失败保留原文件并报 E5010，不得写半截。
3. **首次读写兜底**：文件不存在 → 生成全默认档；文件损坏 → 保留损坏副本 `config.corrupt.<ts>.json`，回默认档并报 E5009（不静默清空）。
4. **降级策略**：旧版本读到更高 `configVersion` → 只读模式 + 提示升级，不猜测字段语义。
5. **模板迁移**：同规则独立执行，`schemaVersion` 与 `configVersion` 互不耦合；v1→v2 升元数据（match/description/时间戳，match 为空），v2→v3 把 `kind/paramsSnapshot` 包成 `steps[0]`（`source="original"`）；未知字段不猜测。
6. **UiState 不迁移**：`uiStateVersion` 不兼容或文件损坏时仅重置 UI 状态（回默认布局），不触碰 ConfigDocument/任务/日志。

### 6.2 迁移函数注册（kernel 内、纯函数）

```rust
pub const CURRENT_CONFIG_VERSION: u32 = 1;          // v1 schema 起步；schema 演进时 +1 并注册迁移函数
pub const CURRENT_RECIPE_SCHEMA_VERSION: u32 = 3;   // 与 configVersion 互不耦合（§6.1-5）
pub const CURRENT_UI_STATE_VERSION: u32 = 1;         // UiState 独立版本，不兼容即重置

pub type MigrationFn = fn(ConfigDocument) -> Result<ConfigDocument, KernelError>;
pub fn migration_chain() -> &'static [(u32, MigrationFn)];  // [(1, m1_to_2), (2, m2_to_3), ...]
// v1 阶段 migration_chain() 返回空切片：全默认档直接落 CURRENT_CONFIG_VERSION
```

- 迁移函数禁止 I/O 与时钟（不变量 1）；读旧文件、写新文件由壳层完成。
- 每个迁移函数必须有对应单测：旧版本样本 → 迁移 → 断言新版本字段。样本落 `fixtures/migrations/v{N}/config-v{N}.json` 并在 [fixtures/README §7](../../fixtures/README.md#7-配置迁移样本migrations) 登记——**不进 corpus 28 条语料体系**（迁移样本无 T 基线行，且非文件管道异常语料；关系声明见该节）。

## 7. IPC 命令细化（扩展篇 01 §4.4）

| 命令 | 入参 | 出参 | 说明 |
|---|---|---|---|
| `getConfig` | `{scope?}` | `ConfigDocument \| ScopeView` | `ScopeView` 含逐项 `SettingValue`（§2 来源追踪） |
| `setConfig` | `{scope, patch}` | `ScopeView` | patch 只接受合法 key；未知 key 报 E1002；写入即迁移后版本 |
| `resetScope` | `{scope}` | `ScopeView` | 恢复默认档（含 `performance` 的"恢复自动"，§6.7） |
| `saveRecipe` | `{name, description?, match?, kind?, paramsSnapshot?, steps?}` | `RecipeDocumentV2/V3` | C37；Phase A 用 kind/params，Phase B 用 steps；参数按 schema 校验 |
| `getRecipe` | `{recipeId}` | `RecipeDocumentV3` | Phase B 读取完整步骤；不重载 `loadRecipe` 返回类型 |
| `listRecipes` | `{page}` | `RecipePage`（`type RecipePage = Page<RecipeDocumentV2 | RecipeDocumentV3>`，`Page` 见篇 01 §3.2） | 列表只读；载入前按需迁移 |
| `loadRecipe` | `{recipeId}` | `PlanDocument`（**直接产出预览计划**，不执行，§7.3） | 仅单步；与 `planOperation` 同出口；多步用 getRecipe + 分步 plan |
| `deleteRecipe` | `{recipeId}` | `void`（幂等） | |
| `exportRecipe` | `{recipeId}` | `{payload: string}`（序列化 `RecipeDocumentV2/V3` JSON，含 `schemaVersion`；落盘位置由壳层另存对话框决定，kernel 不碰磁盘） | §13.8 导出 |
| `importRecipe` | `{payload: string}` | `RecipeDocumentV3`（导入即低版本升级迁移；高版本拒绝报 E1007） | §13.8 导入；导入后须重新预览（§7.3） |
| `getUiState` | — | `UiState` | 读取本机布局/上下文（§11） |
| `setUiState` | `{patch}` | `UiState` | 只接受白名单字段；原子写、防抖由壳层负责 |
| `resetUiState` | — | `void` | 重置界面布局；不影响 Config/任务 |

- 事件：`config://migrated`（`{fromVersion, toVersion}`）——迁移发生时通知设置页刷新显示。

## 8. 授权数据的边界（§8.5 / §9.5）

- 功能位图"以配置驱动"（§9.5）指**工程上以数据文件驱动校验核心**；该文件属授权数据：**单独加密存储、不与普通配置混放**（§8.5），由 LicenseService 实现自管（篇 01 §3 trait），不进 ConfigDocument、不暴露明文给 web/。
- web/ 对授权信息的全部感知仅来自 `getLicenseState` / `license://changed`（显示用）；任何"读配置放行功能"的路径都不存在（不变量 11）。

## 9. 错误码位（延续篇 01 §7 / 篇 02 §5，仅追加）

| 码位 | 域 | 语义 | retryable |
|---|---|---|---|
| E1002（复用） | 输入 | `setConfig` 未知 key / 非法 patch | false |
| E1007 | 输入 | 模板/配方 schemaVersion 高于当前支持（§5） | false |
| E5009 | 环境 | 配置文件损坏（已留存损坏副本，§6.1-3） | false |
| E5010 | 环境 | 配置迁移失败（已回滚保留原文件，§6.1-2） | false |
| E5011 | 环境 | 存储不可写（桌面：配置目录无权限；Web：OPFS 不可用） | false |

## 10. 测试锚点与待决

- [x] 迁移用例样本挂账 → 已完成：落 `fixtures/migrations/`（v1 样本已建并由 `check_schema.py` 校验字段名/类型/枚举；登记见 [fixtures/README §7](../../fixtures/README.md#7-配置迁移样本migrations)），**不进 corpus 体系**——修正了本篇原"挂 manifest 新 T 条目"的预设：manifest 28 条已与 T v1.12 §3.2 核对锁定，迁移样本无 T 基线行；未来若 T 基线扩展覆盖配置迁移用例，先修订 T 再按 §5 沉淀流程并入
- [x] 配置文件格式定版 JSON → 已冻结，见 [ADR-0005](../adr/0005-配置文件格式定版JSON.md)（追认冻结：排除 TOML 的理由与退路；已采纳）
- [x] schema 单一来源机制：已并入 [ADR-0004](../adr/0004-类型契约单一来源.md)（specta 家族，已采纳）
- [x] 保护目录名单（§6.7）的匹配语义 → 已完成，见 [04-文件系统适配层契约](04-文件系统适配层契约.md)（子树语义、双形态匹配、v1 不支持通配符；默认名单已拍板：不预置，见篇 04 §7）
- [x] 本轮补齐（2026-10-28）：各 Scope 字段清单（§4.1，含工程默认值登记）、configVersion/配方 schemaVersion 常量（§6.2）、导入导出与配方命令签名及 `RecipePage` 定义（§7）；安全默认值登记表相应改列 §4.2（原引用点已同步）
- [x] v1.3（2026-10-28）：RecipeDocument v2/v3（match/steps/迁移）、getRecipe、UiState 契约与 get/set/reset 命令；对应方案 §15 与 OQ-26/27。迁移样本与 UiState 单测随实现落地；T v1.12 已收编 C37/B9。

---
## 11. 界面状态（UiState）

**定位**：UiState 是**本机、可重置**的界面/工作区上下文，与 ConfigDocument（用户设置）分离；不导出、不跨设备、不参与配方/模板迁移（方案 §15.3）。

### 11.1 存储

| 形态 | 位置 | 可丢失性 |
|---|---|---|
| 桌面 | 配置目录 `ui-state.json`（与 `config.json` 分离） | 可重置；丢失只回默认布局 |
| Web | OPFS `ui-state/` | 尽力而为；清理浏览器数据后回默认 |

- 原子写、防抖；关闭时 flush；损坏/版本不符只重置 UiState。
- 不保存：已选文件、搜索/筛选、滚动位置、预览、未保存草稿、AI 对话、任务状态、许可证/密钥。

### 11.2 Schema（草案）

```ts
interface UiState {
  uiStateVersion: number;          // CURRENT_UI_STATE_VERSION
  updatedAt: string;
  window?: { width: number; height: number; x?: number; y?: number; maximized?: boolean; monitorId?: string };
  layout?: { sidebarWidth?: number; sidebarCollapsed?: boolean; aiPanelWidth?: number; aiPanelOpen?: boolean; taskPanelOpen?: boolean };
  navigation?: { lastSection?: string; lastFeature?: string; lastDirectory?: string; recentDirectories?: string[] };
  tables?: Record<string, { columnWidths?: Record<string, number>; sort?: { column: string; direction: "asc" | "desc" } }>;
  templatePrefs?: Record<string, { runMode?: "stepwise" | "one-click" }>; // 本机，不导出
}
```

### 11.3 恢复规则

- 窗口矩形须在可用显示器内；离屏/显示器移除 → 居中主屏；面板宽度 clamp。
- 上次目录不存在/无权限 → 空状态并提示重选，不阻塞启动。
- UiState 损坏/版本过高 → 仅重置布局，不触碰 Config/任务/日志。
- 任务恢复由 S03 负责：启动检测未完成任务并询问续跑/回滚/放弃，**不自动续跑**。
- “重置界面布局/清除最近位置”只影响 UiState；一键清理是否包含 Config/UiState 须显式列出，仍不含授权（OQ-12）。

## 12. 修订记录

| 版本 | 日期 | 变更 |
|---|---|---|
| v1 | 2026-10-27 | 首版冻结（用户评审确认）。 |
| v1.1 | 2026-10-28 | 补 `ScopeView` 定义；澄清 `logging.retentionDays` 仅指操作日志保留并修正 §9.6 引用；修正篇 01 §9 引用；迁移样本字段名/枚举与 §4.1 对齐并由 `check_schema.py` 校验。 |
| v1.2 | 2026-10-28 | `GeneralScope.theme` 工程默认由 `"system"` 改为 `"light"`（U v1.3 §5 D-U5 浅色默认，用户确认）；枚举 `system/light/dark` 不变。 |
| v1.3 | 2026-10-28 | RecipeDocument v2/v3（match/steps/迁移、CURRENT_RECIPE_SCHEMA_VERSION=3）；新增 getRecipe 与 UiState 契约（get/set/reset、独立 uiStateVersion、重置规则）；对应方案 §15 与 OQ-26/27。 |
