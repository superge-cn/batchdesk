# 0077. `loadRecipe` 的路径槽位表：键名取自结构体，形状差异如实钉住

- **状态**：已采纳（命令面与槽位表已落地；由它查出的**快照形状≠请求形状**转 [OQ-41](../OPEN-QUESTIONS.md)，界面「套用」入口在裁决前不接）
- **日期**：2026-11-06
- **决策者**：输入通道形态＝用户裁决（OQ-40 取①）；槽位表与断言＝开发代理（工程实现）
- **关联**：[设计 03 §5.2／§7 命令表](./../design/03-配置schema与迁移契约.md)、[设计 01 §4.4.2](./../design/01-内核接口与IPC契约.md)、[方案 v1.19 §15.2.5／§7.3／§7.1](./../baseline/产品方案-v1.19.md)、[ADR-0076](./0076-配方v4记功能码与套用通道缺口.md)（v4 记功能码）、[ADR-0072](./0072-处理模板前端只做存取半边.md)（快照＝计划自带那份）、[ADR-0075](./0075-界面状态把手的尺寸来源与排序语义.md)（同一"两处副本就门禁化"手法）

---

## 1. 问题背景

OQ-40 裁①：`loadRecipe` 的入参带上文件集。听起来是加一个参数，实做起来卡在**往哪个键里塞**：

- 每个功能的路由都有自己的参数结构体，路径字段的**线协议名各不相同**——`SplitParams.file`、`MergeParams.files`、`CsvPlanParams.roots`（**不是 root**）、`ManifestParams.sourceRoot/targetRoot`、`ArchivePlanParams.sourceRoot/targetRoot`、`ZipPlanParams.archive`（解压）与 `sources`（压缩）、`F11ShellParams.list`＋`root`、`F12ShellParams.dir`；
- c09 根本没有路径字段，它的扫描根走 `planOperation` 的 `sourcePaths` 入参（`lib.rs:264`，且要求恰好一个）；
- 每个功能还分"输入必填"和"输出二选一"两类：`f06` 只要 `output`（`plan_csv.rs:215`），而 `f07`–`f09`/`c11`–`c14` 是 `output`／`outputDir` 二选一（`plan_csv.rs:115-134`、`plan_ops.rs:169-184`）。

按记忆写这张表我一开始就写错了四处（`c01` 写成 `file`、`f10` 写成 `root`、c10 两模式混为一谈、用"键名以 s 结尾即多值"当启发式），所以在真正落地前改成**逐条从结构体读**，出处写进代码注释。

## 2. 决策

1. **槽位表 `FEATURE_PATH_SLOTS`（`src-tauri/src/recipe_apply.rs`）**：每个功能两项——`required`（必填输入槽）与 `outputs`（输出互斥组，至少给一个），外加 `via_source_paths` 标记 c09 这类"路径不在 params 里"的通道。表里 24 项，每项的键都能在对应结构体里找到同名字段。
2. **每个槽位键必须同时是 `PATH_PARAM_KEYS` 的成员**（测试 `every_registered_feature_has_slots_and_no_duplicates` 断言）。理由：只有会被 `strip_paths` 剔除的键才需要用户在套用时重新给；否则说明它不是路径，被我错当槽位登记了。这条断言把"槽位＝路径"的口径钉住。
3. **`build_invocation` 只做三件事，且一律宁报错不猜**：缺必填槽→列出缺哪些；给了该功能没有的键→列出合法槽位；单/多值形状不符→拒（不做"取第一个"的宽容）。快照里**已经带**某个路径键时也报错——那是绕过保存通道手改出来的文件，静默覆盖等于把不变量 13 抹平。
4. **c10 按快照里的 `mode` 分流**，不要求调用方另传一个开关：`mode=="compress"` 用 `sources`＋`output`，否则 `archive`＋`outputDir`。判据取自路由自己的分支（`plan_manifest.rs:383`）。
5. **出口只有一个**：`loadRecipe` 组装完就调 `planOperation`，同一条采集通道、不另造第二套，也**不进队列**（方案 §15.2.5「载入后必须重新预览，不得直接 apply」）。
6. **防漂移断言而不是自洽测试**：`build_invocation` 自己成功不代表对——键名错了它照样把值写进一个没人读的键。所以断言做成「注入的键 → 解进真实结构体 → 字段值必须等于注入值」，逐功能覆盖（`split_and_csv_…`／`rewrite_and_ops_…`／`manifest_archive_and_zip_…`／`shell_only_routes_…`／`c09_slot_goes_to_source_paths_…`），再加一条自校 `every_slot_table_feature_has_a_landing_test`：新加功能却没补落地断言即红。

## 3. 这套断言立刻查出的真问题（本篇不解决，转 OQ-41）

`rewrite_and_ops_slots_land_on_the_route_struct_fields` 第一次跑就红，原因不在槽位，而在**快照与请求不是一回事**：

| 功能 | 事实 | 后果 |
|---|---|---|
| c01 | 路由 `TranscodeParams` 有必填 `sampleWindow`（无 serde 默认值），计划里不存 | 快照喂回路由 → 反序列化缺字段 |
| c02／c03 | 路由必填 `sampleBytes`，同样不在计划里 | 同上 |
| c03 | 路由字段叫 `replacement`，而内核 `replace_content.rs:349` 写进计划的键是 `replace` | **改名不同**，快照永远喂不进去 |

ADR-0072 定的是「参数快照＝计划自带的那份」，而 ① 的套用要的是**请求那份**。两者不是同一套形状，这不是工程侧能自行调和的（要么改快照口径、要么加一张同样会漂移的映射表、要么宣布部分功能不支持一键）。因此：

- c03 的失败**钉成断言**（现在断言的是"它确实失败"，将来裁决后这条要一起改，测试里写明了这句话）；
- 界面「套用」入口**本轮不接**——接了就是一按就报错的按钮，比没有按钮更糟（同 ADR-0072 的"不给假按钮"判断）；
- 待裁决内容登记为 [OQ-41](../OPEN-QUESTIONS.md)，含"采样窗口/采样字节算不算参数、要不要进模板"这一并决定项。

## 4. 验证

- 壳层：`cargo test --manifest-path src-tauri/Cargo.toml --offline` **43 → 53 项**。新增 `recipe_apply` 单测 10 项（同出口组装、c09 通道、缺/多槽按名列举、单复数形状、快照已带路径即拒、c10 双模式、未绑定与多步先拒、唯一映射表里的功能都有槽位）＋ 防漂移断言 7 项（含两条自校）。
- 全量：`cargo test --workspace` **647 项**、桌面壳 `cargo test` **53 项**、`cargo clippy`（含独立 workspace）exit 0。
- **CI 第一次判红（run 178，head `8ca3564`）**：lint 的「rustfmt（桌面壳独立 workspace）」——本地我在批次中段跑过一次壳层 rustfmt 是绿的，**但随后删掉一个死代码辅助函数时没再重跑**，格式又漂回去了。同一批里我还在 ADR 里写了「两个 workspace 显式跑了两遍」，这句当时并不成立——属于虚标，本段即为更正。
- **流程结论（写在这里供后续照做）**：桌面壳的 `fmt --check`／`clippy`／`test` 必须在**最后一次改动 `src-tauri` 之后**跑，而不是批次开始时跑一次；本批的修复批 `2c0a1ad`（纯格式）即为重跑后转绿的那一步。
- 前端未改，`pnpm test` **113 项**／`pnpm test:e2e` **17 项**／`check_locales.py`／`check_docs.py` 沿用并通过（本批改动为壳层命令＋文档）。
