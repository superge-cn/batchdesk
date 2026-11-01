# 0043. UI 接入第四批：CSV 处理族（C11–C14）与名称替换（C04）的参数面归一

- **状态**：已采纳（2026-11-01，开发代理按台账既定方向实施）
- **日期**：2026-11-01
- **决策者**：开发代理
- **关联**：ADR-0040/0041/0042（UI 接入前三批）；设计 01 §4.1；设计 12 §3.1；设计 06（错误码）

---

## 1. 问题背景

阶段 0 剩余未接页面中，C11（CSV 去重）、C12（CSV 过滤）、C13（CSV 排序）、C14（CSV 列拆分/合并）与 C04（名称查找替换）是同一批"参数→事实采集→计划"结构：前四者的输入事实均来自 `csv_dedup_inspect`／`csv_merge_inspect`（表头、列宽、脏行、完整记录），C04 与 F12 共用目录扫描事实。若每个 kind 各写一份参数结构与采集代码，将在壳层累积 5 份近似样板，偏离 ADR-0041/0042 已确立的注册表化方向。

## 2. 候选与结论

| 议题 | 候选 | 结论 |
|---|---|---|
| 参数面 | a) 每 kind 一个 struct；b) 单一 `OpPlanParams` 超集（沿用 ADR-0042 的 `CsvPlanParams` 做法） | **b**：新增 `src-tauri/src/plan_ops.rs` 与 `plan_op_shell(state, kind, params)`，按 kind 分臂取用；缺必填参数一律 E1001/E1002 明确报错 |
| 列选择输入 | a) 结构化数组；b) 名称与序号两个逗号分隔字段（`picks()` 合成，名称在前、序号在后） | **b**：与 ADR-0042 一致，kernel 的 `ColumnPick` 语义与 E3004/E1002 判定不变 |
| C13 键列取值 | a) 壳层重新实现类型推断；b) 与 CLI 同源：`csv_dedup_inspect` 取整表记录，剔除表头行后按首个可解析列抽取键值 | **b**：表头值会污染 numeric/date 推断（集成测试曾抓到此 bug），壳层必须复刻"数据行"边界 |
| C14 拆分件数 `parts` | a) 壳层直接填 1；b) 与 CLI 同源：`resolve_column`＋`split_value` 在采样记录上取最大段数 | **b**：kernel 把 parts 定义为**采样评估值**（`split-parts-estimate` 警告），壳层填错会让预览与执行报告背离 |
| C04 目录扫描 | a) 复用 `plan_f12_shell`；b) 在 `plan_ops.rs` 内按 CLI `run_c04` 口径自采（符号链接跳过、`existing_targets` 同步收集、按名称排序、`long_path_supported` 探测） | **b**：F12 与 C04 的规则载荷不同（`PlanF12Rule` vs `PlanC04Rule`），共用采集函数会引入分支；两处共享的是 `RenameScope` 与冲突/长路径判定，留在 kernel |
| 事实采集与 CLI 的重复 | 接受受控重复 | **沿用 ADR-0041 结论**：同源 platform 函数，漂移由 apply_flow/壳层测试报警 |

## 3. 选择理由

- **判定留 kernel**：C11 整行/键列判重、C12 非数值单元格保守不匹配、C13 类型感知与外排、C14 段数超出保留/不足补空，都由 `plan_c11…plan_c14` 决定；壳层只搬运事实（不变量 1/2）。
- **无产品默认值**：采样窗口、外排块行数均为调用方显式传入（方案 §5.2/§5.15 无数值，OQ-28 同口径）；web 表单的 `sampleWindow` 默认值只是输入提示，壳层缺失即报错。
- **测试锚点**：新增壳层测试 `csv_dedup_plan_executes_through_the_desktop_queue_and_undoes`，用与壳层同一套 `csv_dedup_inspect` 事实构造计划，断言桌面队列 apply 产物行数（表头＋去重后 2 行）与 undo 后产物进隔离目录——覆盖"壳层事实采集 → kernel 计划 → 通用执行器"整条链，而不只是路由可达。

## 4. 后果与退路

- **后果**：`OpPlanParams` 字段数继续增长（约 30 个），可读性依赖"按 kind 分臂"这一约束；新增 CSV 类 kind 时先扩该 struct，不得另造参数面。web 注册表增至 20 个功能视图，导航项全部复用同一按钮模板，无新增控件形态。
- **退路**：若 struct 膨胀到难以审阅，按家族拆 `CsvRowParams`（C11/C12/C13）与 `CsvColumnParams`（C14），`plan_op_shell` 的分臂已是隔离边界，拆分不影响 `planOperation` 路由与前端字段注册表。

## 5. 关联

- 代码：`src-tauri/src/plan_ops.rs`（c04/c11/c12/c13/c14 壳层计划）、`src-tauri/src/lib.rs`（路由）、`src-tauri/src/tasks.rs`（c11 队列测试）、`web/src/FeaturePages.tsx`（5 新页字段注册表）、`web/src/App.tsx`（导航至 20 视图）；
- 契约：设计 01 v1.38（追加 `planOperation` kind 面）、设计 12 v0.10（视图注册表）；
- 后续：剩余 c07/c08/c10（ZIP 桌面执行）按同模式接入。
