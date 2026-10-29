# 02 Web 双通道传输契约

> 状态：已评审·实现冻结 v1.1（2026-10-27 评审；2026-10-28 增量修订，用户确认） ｜ 适用：web/、crates/kernel（WASM 面）、云端服务端（后置） ｜ 日期：2026-10-27
>
> 上游：[01-内核接口与IPC契约](01-内核接口与IPC契约.md)（命令名、IpcError、PlanDocument、Task 状态机沿用篇 01，不重复定义）。
> 权威：方案 §13.1（D9 双通道定型）、§13.2（能力边界）、§13.3（浏览器能力表）、§13.5（复用边界）、§13.6（性能）、§13.7（云端处理）、§13.9（不承诺项）、§13.10（阈值与选择器）。数值一律引用 §13.10.2 单点表，本文不设数值。

## 0. 权威来源索引

| 议题 | 权威章节 | 本文角色 |
|---|---|---|
| 双通道并存/锁定/默认本地 | §13.1 三条硬约束 | 落地为任务字段与传输流程 |
| 通道能力差异（写回/下载） | §13.2 | 约束 FsAdapter Web 实现边界 |
| OPFS/配额/下载 | §13.3 | 落地为配额命令与产物下载契约 |
| 内核复用、FsAdapter 不复用但同接口 | §13.5 | 落地为浏览器适配器映射表 |
| 主线程保护、WASM 开销 | §13.6 | 落地为 Worker 边界与事件要求 |
| 云端隔离/白名单/留存/日志边界 | §13.7 | 落地为云端 REST 草案与错误码 |
| 撤销仅本页产物、日志仅会话内 | §13.4 / §13.8 / §11.4-13 | 声明契约口径差异 |
| 模式选择器/体检/引导/配额自检 | §13.10.1–§13.10.5 | 对应命令与字段 |

## 1. 总原则

1. **同一命令面**：篇 01 的命令名与 DTO 在 Web 形态不变；通道差异收敛在传输层与 FsAdapter 实现，不新增并行命令体系（§13.5）。
2. **通道定型于任务**：`TaskSummary` 增加可选只读字段 `channel: "local" | "cloud"`（Web 形态必填、桌面省略），随任务创建锁定，契约层禁止迁移命令（§13.1 硬约束 2；字段已回写篇 01 §3.2）。
3. **默认本地**：会话初始化返回 `lastChoice` 供界面恢复"上次选择"，但 `systemDefault` 恒为 `local` 且不可被记忆覆盖（§13.1 硬约束 1）——该恢复逻辑属显示层，web/ 内不得产生放行/拦截语义（不变量 11）。
4. **零内嵌大 payload**：文件内容不进 JSON；一切数据引用走 `BlobRef` 句柄（§4）。

## 2. 数据引用与分块句柄

```ts
/** 一切文件内容的引用；plan/apply/progress 中的数据只以此出现 */
interface BlobRef {
  kind: "user-file" | "opfs" | "remote";
  ref: string;          // user-file: File API 会话内句柄 id；opfs: opfs://<bucket>/<path>；remote: 服务器对象 id
  size: number;         // 前置体检用（§13.10.3）
  mime: string;
  sha256?: string;      // 可选：小文件直读时计算，供去重/哈希类功能复用（C34）
}
```

- 分块是**传输实现细节**：`size` 上限、分块大小均引用 §13.10.2 配置项，不在 DTO 中固化；句柄消费方（kernel/WASM、服务端）只感知 `BlobRef`。
- `PlanEntry.source/target` 在 Web 形态使用 `blobref://<ref>` 字符串；桌面形态不出现该 scheme（篇 01 §2 契约的形态差异点，仅此一处）。

## 3. 本地通道

### 3.1 FsAdapter 的浏览器实现（§13.5"同接口、不复用实现"）

| 篇 01 §3 方法 | Web 实现 | 边界（§13.2/§13.3） |
|---|---|---|
| `read_bytes` | File API slice 分块读 / OPFS read | 受内存建议档（§13.10.2）约束，超档走强提示（§13.6） |
| `write_atomic` | OPFS 写入（临时名 + `move` 原子替换），对应 §4.3 原子落盘 | 产出仅存 OPFS，不承诺留存 |
| `rename` / `move_path` / `copy` | 仅限 OPFS 内部对象 | **用户磁盘不可写**；用户文件只读引用 |
| `remove_to_quarantine` | OPFS 内隔离目录（不变量 7 的 Web 等价实现） | 页面关闭后随配额回收（§13.3） |
| `list_dir` | 目录授权句柄遍历 | 部分可用、跨浏览器不一致，先探测后使用（§13.9） |
| `stat` | File API 元数据 | 零成本，支撑前置体检（§13.10.3） |

- 桌面 `rename` 语义在 Web 由功能层降级实现（生成新名清单 + 打包下载，§13.4），FsAdapter 不提供"改用户磁盘文件名"的能力，从类型上杜绝误用。
- 判定规则对齐 §13.4：凡"直接改写用户磁盘"的功能，Web 侧 plan 的 `action` 产物一律导向 `download`。

### 3.2 Worker 与事件（§13.6）

- WASM 内核、解析、哈希运行于 Web Worker；主线程仅渲染与转发事件。
- 篇 01 事件（`task://progress` 等）经 Worker → 主线程 → 框架事件桥接，事件名与字段不变；任何超一屏等待的操作界面必须具备骨架屏与取消入口（§13.6，属前端实现约束）。

### 3.3 本地通道新增命令

| 命令 | 入参 | 出参 | 依据 |
|---|---|---|---|
| `registerUserFiles` | `{handles: FileSystemFileHandle[]}` | `BlobRef[]`（会话内句柄登记） | §13.3 File API |
| `requestDirectoryAccess` | — | `DirGrant \| null`（用户拒绝返回 null，不报错） | §13.3 目录授权 |
| `getStorageQuota` | — | `{usage, quota}`（`navigator.storage.estimate()` 原样透出） | §13.10.5 配额自检 |
| `downloadProducts` | `{planId}` | `void`（触发浏览器下载清单，ZIP 打包重命名类产物） | §13.2 产出只下载、§13.4 降级口径 |
| `getChannelDefaults` | — | `{systemDefault: "local", cloudAvailable: boolean, lastChoice?: Channel}` | §13.1 / §13.10.1 / §6.3 降级 |

- `type Channel = "local" | "cloud"`（§1 总原则 2 的任务字段同型）；`cloudAvailable: false` 时云端选项置灰但保留能力预期说明（§13.1 硬约束 3、§6.3），`lastChoice` 恢复属显示层逻辑（总原则 3）。
- `type DirGrant = { grantId: string; displayName?: string };` —— 浏览器适配层的会话内目录授权句柄，不落盘、不跨会话复用。
- **F15-lite 不做 Web**；阶段 2 若开放云端 AI，只走云端通道、仍零强制力（05 §3/§4），并与设计 14 对齐。

### 3.4 撤销与日志的 Web 口径（§13.4 / §13.8 / §11.4-13）

- `undoPlan` 仅作用于**本页产物**（OPFS 内对象）；对用户源文件的引用无从回滚，契约层在 `UndoablePage` 条目上以 `scope: "products-only"` 显式标注，界面须同步声明"不等同桌面版留存/回滚"（§13.8 一致性约束）。
- 操作日志仅会话内留存 + 导出（§11.4-13）；`listLogs` 返回 `ephemeral: true`，禁止写持久化存储。

## 4. 云端通道（服务端可后置，接口先冻结）

### 4.1 传输流程（§13.7）

```text
前置体检(§13.10.3) → createUploadSession（服务端校验白名单+建沙箱）
  → uploadChunk*（分块、可断点续传，块序号+校验和；断线恢复时先 GET session 查缺失块）
  → completeUpload → 服务端执行（任务级沙箱，§13.7 隔离）
  → pollTask / task://progress（与本地同一事件语义）
  → downloadResult（结果保留至下载完成或 §13.7 的到期兜底，先到为准）
  → 任务终态；输入与中间产物服务端即时删除
```

- 强制 HTTPS、证书校验不可关闭（§13.7）；HTTP 明文一律在客户端拒绝（E5007）。
- 双重白名单校验：客户端预检（体验，非安全边界）＋服务端强制（扩展名+魔数，§13.7）。浏览器侧校验**不是**强制力（不变量 11），拒绝与否以服务端返回为准。

### 4.2 云端 REST 草案（本文新增，评审冻结后生效）

| 端点 | 方法 | 说明 |
|---|---|---|
| `/v1/sessions` | POST | 建 `uploadSessionId`；入参：文件元数据（名不上服务端日志，§13.7）、大小、类型、任务 kind 与参数快照；返回配额判定（QuotaBased，§13.8） |
| `/v1/sessions/{id}` | GET | 断点续传查询：返回 `expiresAt`、`receivedChunks` 与 `totalChunks`；过期/失效返回 E5005（对应 ADR-0006 P2） |
| `/v1/sessions/{id}/chunks` | PUT | 分块上传；入参：`index, checksum`；幂等重放安全 |
| `/v1/sessions/{id}/complete` | POST | 结算校验和 → 返回 `taskId`（对应 §13.7 沙箱启动） |
| `/v1/tasks/{id}` | GET | 轮询任务状态与 ApplyReport（结构与篇 01 §5/§6 同构） |
| `/v1/tasks/{id}/result` | GET | 下载结果；`410 Gone` = 已到期删除（结果码可查 `resultCode`，§13.7 仅留结果码） |
| `/v1/tasks/{id}/cancel` | POST | 中止执行中的云端任务 → 服务端中止沙箱并即时清理（取消映射，见 §4.3） |
| `/v1/tasks/{id}` | DELETE | 用户主动放弃 → 服务端即时清理并销毁沙箱（§13.7；终态后不可再取结果，亦即云端"撤销"的落点） |

**请求/响应 schema**（错误响应统一为 `{ error: IpcError }`，`IpcError` 结构与码位沿用篇 01 §2.1/§7；HTTP 状态码与 `IpcError.code` 并用，本文新增码位见 §5）：

```ts
// POST /v1/sessions
// 请求：
{ fileName: string;        // 仅用于白名单校验（扩展名+魔数），服务端日志不落名（§13.7）
  size: number; mime: string;
  kind: OperationKind;     // 篇 01 §3.2 冻结值
  params: Record<string, unknown>;  // 参数快照，与篇 01 PlanDocument.params 同构；服务端按 kind+params 执行
  totalChunks: number; chunkSize: number }  // 二值均引用 §13.10.2 配置项，服务端复核
// 201 响应：
{ uploadSessionId: string; expiresAt: string }   // 过期后重建会话（E5005）
// 拒绝：403 E1006（白名单外）；429 E4004（配额耗尽，QuotaBased）

// GET /v1/sessions/{id}
// 200 响应：{ uploadSessionId: string; expiresAt: string; receivedChunks: number[]; totalChunks: number }
// 410 响应：{ error: IpcError(E5005) }（会话过期/失效；客户端重建会话并按缺失块续传）

// PUT /v1/sessions/{id}/chunks
// 请求体：二进制分块；头：X-Chunk-Index、X-Chunk-Checksum（sha256，规格见 ADR-0006）
// 200 响应：{ received: number }（累计已收块数）；422 E5012（仅该块作废、会话保留，重传该块）

// POST /v1/sessions/{id}/complete
// 请求：无体（会话内元数据已齐）；201 响应：{ taskId: string; state: TaskState }

// GET /v1/tasks/{id}
// 200 响应：{ summary: TaskSummary; report?: ApplyReport; resultCode?: string }
//          TaskSummary/ApplyReport 结构沿用篇 01 §3.2/§5，不重复定义；轮询间隔引用 §13.10.2

// GET /v1/tasks/{id}/result
// 200 响应：结果二进制流（带 Content-Length 与 ETag 供续传校验）；410 → { error: IpcError(E5006) }

// POST /v1/tasks/{id}/cancel
// 202 响应：{ taskId: string; state: "cancelled" }
// 语义：云端无 Journal（沙箱内即时删除，§13.7），取消＝中止＋清理，不产生回滚；
//       与篇 01 §6 本地 cancelled（Journal 反向回滚后终态）是刻意差异，界面文案不得声称"已恢复原状"

// DELETE /v1/tasks/{id}
// 204 响应：无体；沙箱销毁、结果不可再取（再取即 E5006）
```

- 认证/配额按 §8.1 功能位图与 §11.4-12 账号取舍走，具体身份方案不在本篇（服务端契约后置细化时另立）。
- 服务端日志仅含任务 ID、大小、类型、耗时、结果码（§13.7），对齐 §9.7；本契约所有云端入参不得要求服务器记录文件名/内容。

### 4.3 云端与本地契约的差异点汇总

| 项 | 本地 | 云端 |
|---|---|---|
| BlobRef kind | `user-file` / `opfs` | `remote` |
| 进度来源 | Worker 内核推送 | 轮询 `/v1/tasks/{id}` 汇入同一 `task://progress` |
| 撤销 scope | `products-only`（OPFS） | `products-only`（服务端结果，下载前可撤）——**落点＝DELETE `/v1/tasks/{id}`**，服务端无 Journal、无回滚 |
| 取消（cancelTask） | 篇 01 §4.1，已执行部分按 Journal 反向回滚 | `POST /v1/tasks/{id}/cancel`：中止沙箱＋即时清理，不回滚（无本地副作用） |
| 断网 | 可用（§13.2） | 不可用：任务创建前置检查，失败 E5008 |
| 产物落地 | `downloadProducts` | `/result` 直接下载 |

## 5. 错误码位（延续篇 01 §7，仅追加）

| 码位 | 域 | 语义 | retryable |
|---|---|---|---|
| E1006 | 输入 | 类型白名单外（扩展名+魔数双校验失败，§13.7） | false |
| E5004 | 环境 | 云端任务超限（CPU/墙钟/内存，取值引用 §13.10.2 配置项） | false |
| E5005 | 环境 | 上传会话过期/失效 | false（重建会话） |
| E5006 | 环境 | 结果已到期删除（对应 HTTP 410） | false |
| E5007 | 环境 | 不安全传输（明文回退被拒） | false |
| E5008 | 环境 | 网络不可达/断网（云端通道前置检查） | true |
| E5012 | 环境 | 上传分块校验失败：仅该块作废、会话保留，客户端重传该块；验收性质见 [ADR-0006](../adr/0006-云端上传协议自研分块.md) P3 | true |

## 6. 预置与降级策略（§13.9"不做能力假设"）

1. 启动时运行**能力探测**（WASM、OPFS、目录授权、File System Access、Worker），结果汇总为 `BrowserCapabilities`；功能可用性由内核按能力决定，前端只负责显示/隐藏（不变量 11）。
2. 探测失败的功能项显示降级提示并引导桌面客户端（§13.10.4 超限引导卡复用此入口）。
3. 云端通道未上线时按 §13.1 硬约束 3 处理：置灰但保留能力预期说明，不得死链；契约层表现为 `getChannelDefaults().cloudAvailable: false`。

## 7. 测试锚点与待决

- [x] E02/R05 类语料（T v1.12 §3.2）Web 口径 → 已拍板（用户确认 2026-10-27）：占位文件/UNC 属桌面专属（§13.4/§13.9），Web 语料**不承接**此两条；测试方案补篇建立时直接引用本决策
- [x] 分块协议是否采用现成断点续传规范（如 tus）或自研 → 已定，见 [ADR-0006](../adr/0006-云端上传协议自研分块.md)：自研轻量分块（本篇 §4.2 规格冻结不变），排除 tus 的理由（互操作价值为零 + 与任务级沙箱耦合）与迁移退路（传输层单点替换，命令契约不动）均在该 ADR；已采纳（2026-10-27）
- [x] 云端身份与配额服务端完整契约（对齐 §8.1/§11.4-12）→ 已拍板：后置独立篇（云端服务端可后置，§13.1；身份方案依赖 §11.4-12 产品取舍）
- [x] `BrowserCapabilities` 探测项清单与降级映射表 → 已拍板：实现期细化（依赖真实浏览器差异实测，不做纸面清单）
- [x] 本轮补齐（2026-10-28）：REST 请求/响应 schema（§4.2）、`cancel` 端点与 cancelTask 云端映射、云端"撤销"落点＝DELETE 任务（§4.3）、`getChannelDefaults` 补 `cloudAvailable` 并补 `Channel` 类型定义（§3.3，消除与 §6.3 的不一致）

## 8. 修订记录

| 版本 | 日期 | 变更 |
|---|---|---|
| v1 | 2026-10-27 | 首版冻结（用户评审确认）。 |
| v1.1 | 2026-10-28 | 明确 `TaskSummary.channel` 并回写篇 01；补 `DirGrant` 定义；补 `GET /v1/sessions/{id}` 断点查询；结果保留时长改为引用 §13.7；与篇 01 的 `scope`/`ephemeral` 字段对齐。 |
