# 设计契约索引

> 本目录承载**工程实现契约**：字段、命令、状态机、错误码、边界。产品行为与数值阈值仍以四份基线为权威；本文档不复制基线原文。

## 1. 契约清单

| 编号 | 标题 | 状态 | 适用 | 依赖 |
|---|---|---|---|---|
| [01](./01-内核接口与IPC契约.md) | 内核接口与 IPC 契约 | 已评审·实现冻结 v1.64（2026-11-13 条目内字节级进度 `entry_done`／`entry_total`——ADR-0100；2026-11-13 §2.2 取消粒度细化＋`ContentRewriter` 增 cancel 端口、新增 E3010——ADR-0097；2026-11-13 C10 逐文件报告：清单进 `ApplyEntryReport.detail`——ADR-0096；2026-11-10 OQ-45 裁决：`RecipeDocument` v7 增 `origin`——ADR-0090；2026-11-09 C37 Phase B：`loadRecipe` 增 `stepIndex`、`describeRecipe` 增逐段评估与 `oneClickAllowed`——ADR-0089；2026-11-08 D1 再补全：`transcode` 与 `edit-content` 共用就地改写执行路径、`undo --dry-run` 网络 stat 走重试；2026-11-08 D5：`JournalStatus` 增 `Pending`（隔离移动两阶段）——ADR-0084；2026-11-07 OQ-43②：content 变换正文进模板，ADR-0081；2026-11-07 OQ-41②：describeRecipe/sampling/参数映射表，ADR-0080；2026-11-05 C10 加密包的密码注入面：`password_provided` 只声明事实＋`ArchiveApplyContext.password` 承载秘密＋`inspect` 改 `by_index_data`＋E3008/E3009——ADR-0074；2026-11-05 环境事实一律经端口注入＋`FsAdapter::name_rules`——ADR-0073；2026-11-05 §4.4.2 处理模板六条命令与 DTO＋保存时剔除路径参数、`loadRecipe` 阻塞 OQ-36——ADR-0072；2026-10-30 retryTask/合帧/注入口/removedLogRecords/§8 reverse 推导规则、F01–F12 动作与载荷、F06 T07/T08 告警、F09 read_ranges、F10 DirectoryLister/write_atomic_with、F11 逐级 create-dir、F12 两阶段 rename；2026-10-31 F12 计划级长路径分级、F13 正则捕获组替换、C02 流式换行转换、C03 查找替换、C10 ZIP 解压/压缩、C11 CSV 去重、C12 CSV 过滤、C15 任务内条目级并发、C04 名称查找替换；v1.42–v1.45 见文末修订记录：取消处置、CSV 五臂流式执行、`ContentRewriter` 返回 `EditOutcome`；2026-11-03 v1.46 `TaskSummary.planId` 可选字段（「任务与撤销」视图据此走 `undoPlan`））；v1.47–v1.50 见文末修订记录（含 E03 收口、`TaskSummary.job` 独立字段）；v1.50 F05 的 `shardBom`（UTF-16 每片写 BOM，ADR-0070）；v1.51 处理模板命令与 DTO（ADR-0072）| kernel、src-tauri、web | 基线 §4.2/4.3/4.6/4.7/7.x/8.x/12.4/13.x |
| [02](./02-Web双通道传输契约.md) | Web 双通道传输契约 | 已评审·实现冻结 v1.15（2026-11-23 A3-15：**wasm32 时间源 P0 修复**——`SystemTime::now()` 在该目标调用即 panic，而 28 个 plan 入口共用 `ulid_like_id`；改走 `js_sys::Date::now()`，防线是真 WASM 的 e2e，ADR-0116。）｜v1.14（2026-11-23 A3-13：**C09 归因纠正**（`content-hash`→`file-identity`，缺的是文件系统身份而非哈希）；**产物执行后搬运**到 OPFS（`batchdesk/products/<planId>/`，内核同步契约零改动，与 staging 前移对称），ADR-0115。）｜v1.13（2026-11-22 A3-12：**计划面与执行面各 22 个视图**——F10 目录清单接上：执行臂要 `DirectoryLister` 端口而非「目录树」，由 WASM 内 `MemoryDirectoryLister` 读浏览器注入的目录事实按桌面同口径写 BOM CSV；新增 `stage_dirs`／`stage_listing` 两个 op；剩余 3 个不可计划视图 C09／F11／F13，ADR-0114。）｜v1.12（2026-11-22 A3-11：**计划面与执行面各 21 个视图**——C04 名称替换／F12 批量改名接上：条目＝目录第一层名字（与桌面 `read_dir` 单层同口径），staging 登记整棵子树，产物可下载可撤销；档位表删掉前端无视图入口的 F13 死条目，ADR-0113。）｜v1.11（2026-11-22 A3-10：**执行面 19 个视图全部接入**——C10 由 WASM 内 `MemoryArchiveService` 提供真 ZIP 读写（与桌面同一个 `zip` crate、同一份名字解码、同一套错误码），解密密码只随执行请求进来；`WebApplyGap` 只剩未知视图的 `not-wired`，ADR-0112。）｜v1.10（2026-11-22 A3-9：**执行面扩到 18 个视图**——C07/C08 按**计划的源口径**登记 staging（目录选择类视图拼「根名/相对路径」），C07 清单不进沙箱；`WebApplyGap` 删 `relative-paths` 与 F10 死条目，只剩 C10 `port-missing`，ADR-0111。）｜v1.9（2026-11-22 A3-8：**执行面扩到 16 个视图**——7 个 CSV 计划入口接受 `outputDir`，目标路径由 wire 层派生（缺省沙箱 `/out`，不回落源文件），ADR-0110。）｜v1.8（2026-11-22 A3-7：**执行面扩到 9 个视图**——C01–C03 的改写器在沙箱内按 `plan.kind` 构造，F01–F06 只走读写；未接的 10 个按 `target-in-place`／`relative-paths`／`port-missing` 分类拒绝，ADR-0109。）｜v1.7（2026-11-22 A3-6：**执行面接上**——会话沙箱（`kernel::memfs::MemoryFs`）＋`EXEC_OPS` 4 op／5 导出，F01–F03 可真执行、真出产物、真撤销；撤销 `products-only`；OPFS 路线降级为可选后续，ADR-0108／§3.1.1。）｜v1.6（2026-11-22 A3-5：C10 压缩侧接上 → `plan_c10_compress`／`EXTRA_PLAN_OPS`／`sources` 多文件槽，档位不变 19 native／6 unsupported；缺口枚举删 `archive-compress`，ADR-0107） | web、WASM、云端（后置） | 01、基线 §13.x |
| [03](./03-配置schema与迁移契约.md) | 配置 schema 与迁移契约 | 已评审·实现冻结 v1.11（2026-11-10 OQ-45 裁决：RecipeDocument v7 增来源字段 `origin`——ADR-0090；2026-11-09 C37 Phase B：`loadRecipe` 增 `stepIndex`、`describeRecipe` 增逐段评估与 `oneClickAllowed`、`templatePrefs` 运行模式；2026-11-07 OQ-43②落地：RecipeDocument v6 记变换正文、f11 纠偏可套用；2026-11-07 OQ-41②落地：RecipeDocument v5 记采样口径、`describeRecipe` 入命令表、`loadRecipe` 由参数映射表闭合形状差异；2026-11-05/06 C37/UiState 增量） | kernel、src-tauri、web | 01、02、基线 §4.5/6.7/9.5/9.6/§15 |
| [04](./04-文件系统适配层契约.md) | 文件系统适配层契约 | 已评审·实现冻结 v1.28（2026-11-22 A3-6 追加 **`MemoryFs`：`FsAdapter` 第三个实现**（ADR-0108）——Web 沙箱承载 staged 输入与产物，语义逐条对齐桌面 `CliFsAdapter`；**不新增 trait 能力**，执行器一行不改。）｜v1.27（2026-11-22 A3-5 追加 **C10 压缩侧导出**：WASM `plan_c10_compress`（ADR-0107，`EXTRA_PLAN_OPS`）——只出计划、不打包；**不新增 FsAdapter 能力**，执行仍待 OPFS 实现（§3.1）。）｜v1.26（2026-11-21 A2-7 追加 **Web 侧 ZIP 条目事实来源**（ADR-0103）：C10 的 `entries[]` 由 `web/src/lib/web/zipIndex.ts` 零依赖读中央目录产出，与桌面 `ArchiveService::inspect` 逐字段同口径；`plan_c10` 已从 WASM 导出，功能面 25/25。**不新增 FsAdapter 能力**——解压执行仍待 OPFS 实现（§3.1）。）｜v1.25（2026-11-20 A2-6 追加 `kernel::csv_sort::key_values`（CSV 排序键列取值；原壳层私有实现下沉，壳层改调同一路径，附跳缺失/BOM/1-based 三项口径测试）。）｜v1.24（2026-11-20 A2-4 追加 **分片布局字节级入口**：`kernel::split::{layout_from_bytes, count_lines}`（F01 按字节/F02 按行/F03 按份数共用，原 platform 流式实现改调此路径）与 `kernel::csv::csv_split_from_bytes`（F05 CSV 分片）；platform 相应删除本地扫描循环。）｜v1.23（2026-11-20 A2-3 收尾追加 **区间状态机与段数评估**：`kernel::csv_range::{RangeSpan, RangeFacts, range_from_bytes}`（原 platform 私有 `RangeState` 下沉，含「扫到 endRow 即停」语义）与 `kernel::csv_columns::parts_from_bytes`（原壳层 `plan_ops.rs` 的列数评估下沉）；platform 相应改为调同一路径并删除本地副本。）｜v1.22（2026-11-20 追加 **CSV 共享事实**（ADR-0102 A2-3）：`kernel::csv_merge::{CsvInspection, inspect_head, complete_records}`——F06–F09／C11–C14 八个功能共用的字节级检查入口；platform 的 CSV 检查改调同一算法并与字节路径对拍（列数/脏行逐字段相等）｜2026-11-20 追加 **CSV 共享事实**（ADR-0102 A2-3）：`kernel::csv_merge::{CsvInspection, inspect_head, complete_records}`——F06–F09／C11–C14 八个功能共用的字节级检查入口；platform 的 CSV 检查改调同一算法并与字节路径对拍（列数/脏行逐字段相等）。；2026-11-20 追加 **字节级探测入口**（ADR-0102 A2-2）：`kernel::line_endings::counts_from_bytes` —— Web 形态拿不到真实 FS，但能把文件读成整段字节；platform 的流式 `LineEndingProcessor` 与本入口共用同一算法，并以分块/整段对拍断言两者等价（末尾孤立 CR 为必测边界）。`inspect_bytes`（C03）本已是字节级。）｜v1.20（2026-11-13 追加 `FsAdapter::medium_kind`：卷句柄 seek-penalty 判别 SSD/NVMe——ADR-0094；2026-11-13 `NetworkRetry` 扩展为致命错误暂停：磁盘满 E2004 第一次就暂停、与网络断 E2005 共用端口——ADR-0093；2026-11-08 D1 再补全（二）：`transcode` 与 `edit-content` 共用就地改写路径、`undo --dry-run` 网络 stat 走重试；2026-11-08 D1 再补全：`edit-content`/`transcode` 隔离备份经 `backup_once` 在重试间复用、纳入重试族；2026-11-08 D1 补全：网络路径判定走 FsAdapter 能力（UNC＋Windows 映射盘）、undo 接入重试；2026-11-08 D1/D2：UNC 断线 3 次重试/2-4-8 秒退避/耗尽暂停、Windows 网络错误 64/59/121/1236→E2005——ADR-0084；2026-11-08 D3 原子复制：`copy`／跨卷移动／隔离备份统一「同目录临时文件＋字节数校验＋改名」，失败不碰既有目标——ADR-0083；2026-11-05 §2.1 目标名合法性 `NameRules`＋macOS 大小写能力归位——ADR-0073；2026-10-30 长路径收口＋隔离备份/路径预留＋F09 区间读取＋F10 流式原子写＋F11 目录创建；2026-11-03 `open_read_stream` 拉取式读取（C13 归并）；2026-10-31 C02 流式原地重写；2026-11-01 C07 同卷判定/单文件删除/逐级建目录；2026-11-02 S01 的 UNC 探测根改为共享名、`read_stream` 流式读取） | kernel、桌面/WASM 适配 | 01、03、基线 §4.7/5.13 |
| [05](./05-Web强制力边界契约.md) | Web 强制力边界契约 | v0.2（三条客户端结论已确认；云端身份后置） | web、WASM、云端（后置） | 01、02、04、ADR-0002/0007 |
| [06](./06-错误码与用户文案契约.md) | 错误码与用户文案契约 | 已评审·实现冻结 v1.8（2026-11-13 追加 E3010「操作已被用户取消」——ADR-0097；2026-11-05 追加 E3008「需要密码」／E3009「密码错误或包损坏」，预警类别 61 → 63——ADR-0074；2026-11-03 §3 澄清 `detail` 只进诊断日志、界面只需 summary/action；首批 21 码位接入，ADR-0061）；§2A 预警类别登记表| 全仓库 | 01/02/03/04、基线 §4.6 |
| [07](./07-云端服务端契约.md) | 云端服务端契约 | v0.1 草案·待评审 | 云端服务端、web | 02、05、基线 §13.7/13.8 |
| [08](./08-授权数据与签发契约.md) | 授权数据与签发契约 | v0.1 草案·待评审 | kernel、src-tauri、签发工具 | ADR-0003、基线 §8.x/12.4 |
| [09](./09-国际化与文案契约.md) | 国际化与文案契约 | v0.8 草案·待评审（2026-11-03 实现落地：`web/src/locales/` 437 key × 2、`i18n.tsx` Provider、设置区语言/主题、`check_locales.py` 八项门禁；v0.3 追加错误码文案首批 21 码位＋统一 `ErrorBanner`）；门禁第 ⑩ 项（枚举不进文案）与第 ⑪ 项（预警类别三方对齐）| web、src-tauri、测试 | 基线 §9.5/10.2、U v1.3、06 |
| [10](./10-日志与诊断契约.md) | 日志与诊断契约 | v0.7 草案·待评审（2026-11-14 §7 第 4 项关闭：日志加密默认不加密；§1 补实现状态：XCT-04 两判据由 CI 测试钉死、CLI 不落操作日志的范围边界显式化；2026-11-10 支持包（桌面）按 ADR-0092 落地、诊断日志轨按 ADR-0091 落地；2026-10-30 C17a 操作日志按 ADR-0013 落地） | kernel、src-tauri、web | 01 §8、03、基线 §4.6/9.6/9.7 |
| [11](./11-发布签名与更新契约.md) | 发布、签名与更新契约 | v0.2 草案·待评审（2026-11-03 追加前端产物 CSP 前置断言） | CI、src-tauri、发布 | ADR-0002/0007、基线 §9.7/12.5 |
| [12](./12-前端与桌面壳架构.md) | 前端与桌面壳架构 | v0.29 草案·待评审（2026-11-10 运行模式缺省按来源（`origin`，OQ-45 裁决）——ADR-0090；v0.28＝处理模板「一键套用」与多步执行器落地（C37 Phase B）——ADR-0089；v0.27＝undo 接入断线重试端口＋映射盘识别（D1 补全）——ADR-0084；v0.26＝UNC 断线重试耗尽的自动暂停（D1 实机缺陷修复）——ADR-0084；2026-11-08 S01 磁盘预检按落盘卷分组（D4 实机缺陷修复）——ADR-0083；2026-11-06 UiState layout/tables 的拖动把手与列宽/排序语义——ADR-0075；2026-11-05 C10 密码字段 `type=password`＋壳层秘密只活在内存——ADR-0074；2026-11-05 处理模板前端「存取半边」：模板库顶层视图＋功能页保存入口＋六条 IPC 封装，套用留待 OQ-36——ADR-0072；2026-11-02 §7.3 预检竞态窗口关闭＋危险二次确认对话框＋集中报告逐项点名＋S01 预检门可测化与阻断面断言；2026-11-03 v0.17 `VirtualList` 列表窗口化＋「任务与撤销」视图＋`labels.tsx` 文案单一来源；v0.18 **中英双语（`locales/`＋`i18n.tsx`＋`check_locales.py`）＋设置区可用（设计 03 五组读写）**；v0.19 **壳层加固：CSP／`devCsp` 分离＋capabilities 最小权限＋`withGlobalTauri=false`＋`freezePrototype`，6 项静态断言**——ADR-0060；v0.20 **产物层 CSP 前置断言 `check_dist_csp.py` 进 CI**）；v0.20 CSP 产物断言、v0.21 导航目录单一来源、v0.22 处理模板存取半边| web、src-tauri | 01/02/03/05、U v1.3、基线 §4.2/4.4/§15 |
| [13](./13-CLI契约.md) | CLI 契约（C40） | v0.26 草案·占位（2026-11-05 C10 加密包旗标 `--expect-password`／`--password-stdin`—ADR-0074；2026-11-01 C18 横切干跑 harness；2026-11-02 执行报告条目级 `detail`、`f11` 逐级前缀事实） | crates/cli | 基线 §9.5 |
| [14](./14-AI助手与工具调用契约.md) | AI 助手与工具调用契约（F15 Phase 1/2/3） | 已评审·实现冻结 v1.2（2026-11-09 loadRecipe 定为本地 UI；v1.1 多轮历史出端；v1 于 2026-10-28 冻结） | src-tauri/ai、web | 方案第 14 章 v1.19、01/02/05/06/09/10/12 |

> 视觉评审资产（OQ-03 品牌色已确认 `#2F6BFF`，历史候选图与液态玻璃双模式示例）见 [assets/README.md](./assets/README.md)。
> **方案提案（历史/已收编）**：[F15 AI 助手方案变更提案](./proposals/F15-AI助手-方案变更提案.md)（第 14 章已收编产品方案 v1.19；设计 14 v1.2 已冻结，v1.1 加入多轮历史出端，v1.2 定 loadRecipe 为本地 UI）。
> **方案提案（历史/已收编）**：[C37 处理模板增强提案](./proposals/C37-处理模板增强-方案变更提案.md)（v1.0；已收编方案 v1.19/T v1.12/设计 03 v1.3）。
> **方案提案（历史/已收编）**：[配置与界面状态持久化方案提案](./proposals/配置与界面状态持久化-方案提案.md)（v1.0；已收编方案 v1.19/设计 03 v1.3）。
> U v1.3 仍为当前视觉基线；方案 v1.19 的 C37/持久化属于交互与数据契约，未新增视觉 token，故未升 U；下次视觉变更再升版。

## 2. 依赖与阅读顺序

```text
01 内核接口
 ├─ 02 Web 传输 ── 05 Web 强制力 ── 07 云端服务端
 ├─ 03 配置 schema
 ├─ 04 文件系统适配
 ├─ 06 错误码/文案（横切 01/02/03/04）
 ├─ 08 授权签发（横切 01/03/12）
 ├─ 09 i18n（横切 06/12）
 ├─ 10 日志诊断（横切 01/03）
 └─ 11 发布更新（横切 02/07/12）
12 前端与桌面壳（消费 01/02/05/06/09/10）
13 CLI（P1，消费 01/03/06）
14 AI 助手（F15 Phase 1/2/3，消费 01/02/05/06/09/10/12）
```

- 01–04 的冻结面变更遵循“只增不改、追加即兼容”；破坏性变更需新版本并在文末修订记录说明。
- 06 已冻结 v1.3、14 已冻结 v1.2；07–13 当前是草案，**不得**作为“已实现”依据；评审通过后改为“已评审·实现冻结 v1”，并接入对应测试/CI。

## 3. 写作与变更规则

1. 新契约必须包含：状态、日期、适用范围、权威来源索引、待决与后续、修订记录。
2. 不复制基线段落；数值只引用章节号，不写第二份具体值。
3. 新增/修改命令、DTO、错误码、状态机：先改本目录契约，再改代码与测试；三者同一 PR。
4. 任何“Web/桌面差异”必须显式标注；不得用同一字段暗示不同语义。
5. 变更后运行 `python scripts/check_docs.py`；涉及 DTO 生成时按 ADR-0004 核对生成物门禁。

## 4. 当前缺口

- 01–04 已有冻结基线（01 v1.45、02 v1.1、03 v1.3、04 v1.12），06 已冻结 v1.3；契约到实现的映射见 [TRACEABILITY.md](./TRACEABILITY.md)：当前实现集中在 C09、C01/C02/C03/C04/C10/C11/C12/C15 与 F01–F13 纵向切片及平台核心（01/04/12 部分落地；S01/S02/S03 已有内核/壳层切片；13 有 C09＋C01/C02/C03/C04/C10/C11/C12/C15＋F01–F13 实验命令切片但命令面未冻结，处置见 OQ-30），**09 已有实现（`web/src/locales/`＋`i18n.tsx`＋`check_locales.py`，见设计 09 v0.2）**，02/05/07/08/11/14 尚无代码。
- 05 的云端身份/配额、07 的身份与计费、08 的签发字段与算法、10 的支持包内容、11 的防降级与更新源，均需评审或外部输入；见 `docs/OPEN-QUESTIONS.md`。09 的剩余待决是 `error.<code>.*` 错误码文案族与 I18N-01…08 进 TC 实例库（§7）。
- 06–13 草案中的“待决”项不得由实现者自行补值；按 AGENTS“何时停下来问人”处理。
