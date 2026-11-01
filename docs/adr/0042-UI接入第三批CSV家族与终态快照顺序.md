# 0042. UI 接入第三批：CSV 家族与目录清单的壳层参数面与终态快照顺序

- **状态**：已采纳（2026-11-01，开发代理按台账既定方向实施）
- **日期**：2026-11-01
- **决策者**：开发代理
- **关联**：ADR-0040（首批 F05/F11/F12）、ADR-0041（批量接入与 `plan_kinds.rs`）；设计 01 §4.1/§4.3；设计 12 §3.1；设计 06（错误码）

---

## 1. 问题背景

阶段 0 出口标准（方案 §10.2）要求逐功能执行 T 的三类用例，而映射表中大量条目为"🔶 e2e 待 UI"。ADR-0041 把壳层路由铺到 11 个视图后，剩余缺口最集中的一组是 CSV 家族（F06 合并／F07 抽取列／F08 前后 N 行／F09 位置行区间）与 F10 目录清单：它们共用同一套"采样→表头/列宽事实→kernel 计划"的采集管道（`platform::csv_merge::csv_merge_inspect`、`platform::csv_range::csv_range_layout`、`platform::listing::inspect_tree`），逐功能另写一份参数结构会产生 5 份近似样板。

接入过程中还暴露一个可观测顺序问题：既有壳层测试断言"终态任务不得留在恢复快照里"，在并行负载下偶发失败——`finish()` 先在内存置终态并推送 `task://state`/`task://done`，之后才写快照。

## 2. 候选与结论

| 议题 | 候选 | 结论 |
|---|---|---|
| 五个 kind 的参数结构 | a) 每 kind 一个 struct；b) 单一 `CsvPlanParams` 超集＋按 kind 取用 | **b**：F06–F09 字段高度重叠（文件列表／目标编码／分隔符／表头／采样），单 struct 免去 4 份镜像；kind 专属字段（`window`/`count`、`startRow`/`endRow`、`columns`/`columnIndexes`、`strict`/`sourceColumn*`）缺失时按 E1001/E1002 明确拒绝，不静默取默认 |
| 新模块位置 | a) 继续塞 `plan_kinds.rs`；b) 独立 `src-tauri/src/plan_csv.rs` | **b**：按家族分模块（kinds=文本/编码、csv=CSV 与清单），路由仍在 `lib.rs` 单处 match 可见 |
| 输出目标推导 | a) 壳层自定义命名；b) 与 CLI 同源：`output` 单输入专用、`outputDir` 按同名落盘 | **b**：与 `run_f07/f08/f09/f10` 一致，避免同一功能两种产物路径 |
| 列选择输入 | a) 结构化数组 `[{kind,value}]`（JSON textarea）；b) 名称与序号两个逗号分隔字段 | **b**：表单可用优先；顺序语义定为"名称在前、序号在后"，kernel 的重复列 E1002/缺列 E3004 判定不变 |
| 表头开关 | a) 复选框布尔；b) 文本"有/无"→`hasHeader=false` | **b**：与现有注册表的 text 字段规格一致，不新增控件形态；缺省视为有表头 |
| 终态与快照顺序 | a) 改测试为有界等待；b) `finish()` 内先 `persist()` 再推 state/done；c) 持锁写盘 | **b＋a**：先剪枝快照再让终态对外可观测（生产语义修正），同时把测试断言改为有界等待以覆盖"内存终态已可见、写盘尚未收敛"的极窄窗口；c 需非重入锁嵌套，风险大于收益，否决 |

## 3. 选择理由

- **单 struct 超集**：`CsvPlanParams` 同时容纳 F06–F10 字段（`roots` 与 `files` 并存），缺省一律显式报错，避免"参数没传→按某个默认跑"的静默行为；采样窗口无产品默认值，仍由调用方传入（方案 §5.2 无数值，AGENTS"数值阈值只引用基线"）。
- **判定仍留 kernel**：壳层只做事实采集与参数装配，E3001（编码未确认）／E3004（缺列、多行表头）／E1002（区间非法、目标重名）由 `plan_f06…plan_f10` 产生，壳层不复制判定分支——与不变量 1/3 一致。
- **F10 走既有流式通道**：计划只存摘要计数，CSV 写出由通用执行器的 Listing 分支经 `DirectoryLister` 完成；桌面新增测试断言 UTF-8 BOM、文件/目录行与撤销后产物进隔离目录（reverse=`Quarantine`）。
- **快照顺序**：崩溃恢复的判据是"快照里只应有非终态任务"。原顺序下，读到 `task://done` 的 UI 若紧接着读存储信息，可能看到仍含该任务的快照；先写盘后推事件把这个窗口从"事件之后"移到"事件之前"。

## 4. 后果与退路

- **后果**：`CsvPlanParams` 是超集，字段与 kind 的合法组合由 `plan_csv_shell` 的分臂保证；新增 CSV 类功能时优先复用该 struct，而不是再造参数面。壳层测试断言变为有界等待（最长 5 s），失败信息仍指向同一条不变量。
- **退路**：若超集结构在后续家族（C11/C12/C13/C14）中变得难以阅读，可按家族再拆 `CsvRowParams`/`CsvColumnParams`；`plan_csv_shell` 的分臂已是按 kind 隔离的，拆分不影响路由与前端。

## 5. 关联

- 代码：`src-tauri/src/plan_csv.rs`（f06–f09 与 f10）、`src-tauri/src/lib.rs`（`planOperation` 路由）、`src-tauri/src/tasks.rs`（`finish()` 顺序＋`wait_snapshot_pruned`）、`web/src/FeaturePages.tsx`（注册表 5 新页）、`web/src/App.tsx`（导航至 15 视图）；
- 契约：设计 01 v1.37（壳层路由追加）、设计 12 v0.9（视图注册表）；
- 后续：c04/c11/c12/c13/c14 与 c07/c08/c10 桌面执行按同模式接入。
