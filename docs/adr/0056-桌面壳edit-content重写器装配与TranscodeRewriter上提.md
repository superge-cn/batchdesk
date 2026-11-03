# 0056. 桌面壳 edit-content 重写器装配与 `TranscodeRewriter` 上提

- **状态**：已采纳（2026-11-03，P0 缺陷修复）
- **日期**：2026-11-03
- **决策者**：开发代理（工程实现决策；产品行为、错误码与阈值不变）
- **关联**：ADR-0055（C01 流式转码与 `ContentRewriter` 明细位）、ADR-0050（条目级 detail 口径）、ADR-0053（`read_stream`/`StreamTranscoder`）；设计 01 §5（执行上下文与注入端口）、设计 12 §3.1（kind 路由）；`crates/kernel/src/{encoding,line_endings,replace_content}.rs`、`crates/cli/src/main.rs`、`src-tauri/src/tasks.rs`

---

## 1. 问题

桌面壳 `run_apply` 给非 c09/c10 计划构造 `GenericApplyContext` 时把 `content_editor` 与 `content_rewriter` 恒写为 `None`。内核执行器对**含 `edit-content` 条目**的计划有装配校验（`generic_apply.rs`）：两者皆空即整单 `E1002` 失败。而 C01（转码，`plan.rs` 的 `plan_c01`）、C02（换行，`line_endings.rs` 的 `plan_c02`）、C03（替换，`replace_content.rs` 的 `plan_c03`）**恒产 `edit-content` 条目**——三个功能在桌面 UI 上能选参数、能生成计划、能预览，一点"执行"就失败；`tasks.rs` 里也没有这三条 kind 的执行测试，因此问题长期未被门禁发现。

同时 C01 的流式重写器实现（`TranscodeRewriter`）当时只存在于 CLI 二进制内（ADR-0055 §3）——桌面壳即便想注入也无实现可复用。

## 2. 候选与结论

| 议题 | 候选 | 结论 |
|---|---|---|
| C01 重写器载体 | a) 壳层照抄一份转码流式逻辑；b) 把 `TranscodeRewriter` 上提到 `kernel::encoding`；c) 壳层退回整段 `content_editor` | **b**：一份实现两处前端（CLI/桌面），行为不可能分叉；c 会把 C01 拉回 O(文件) 内存，直接推翻 ADR-0055 |
| 计划参数解析归属 | a) 壳层自己读 `plan.params` 拼参数；b) kernel 提供 `from_plan` 构造器 | **b**：`target/invalid/platformPrior/source` 的语义属于领域，壳层只做装配；两处各解一遍迟早口径漂移（CLI 原先的 `c02_rewriter_from_plan`/`c03_rewriter_from_plan` 正是这种重复） |
| 装配失败处理 | a) 不注入，让内核报 `E1002`；b) 壳层按计划错误直接失败任务 | **b**：走与其他 apply 错误同一条路径（任务 Failed＋message），不静默降级成"绿色完成但文件没变" |
| `content_editor` 是否保留注入 | a) 两个端口都注入；b) 只注入 `content_rewriter` | **b**：三类计划的执行形态都是"读源→写回同文件"，流式端口已覆盖；`content_editor` 保留字段但桌面传 `None` |

## 3. 实现

- `kernel::encoding::TranscodeRewriter`：自 CLI 平移（逐行等价），新增 `new(...)` 与 `from_plan(&PlanDocument)`；`rewrite` 行为不变——源编码已知时目标 BOM 只写一次→64 KiB 块 `read`→`StreamTranscoder`→`write`→`finish`，明细 `invalid sequences replaced: {n}`；源编码缺失走整段检测回退。
- `kernel::line_endings::LineEndingRewriter::from_plan`、`kernel::replace_content::ReplaceRewriter::from_plan`：把 CLI 的同名参数解析函数移入 kernel（错误码与文案不变）。
- CLI：删除本地 `TranscodeRewriter`、`rewrite_io_error`、`c02_rewriter_from_plan`、`c03_rewriter_from_plan`，改调 kernel 的 `from_plan`；`generic_plan_encoding` 仅剩"整段编辑器回退"一处使用，行为不变。
- 桌面壳：新增 `edit_content_rewriter(plan)`——按 `kind` 返回对应重写器（c01/c02/c03，其余 `None`）；`run_apply` 在**计划含 `edit-content` 条目**时装配 `content_rewriter`，装配失败即 `failed_result`。
- 测试（先红后绿）：`src-tauri/src/tasks.rs` 新增三条队列用例（c01 转码写 BOM＋detail、c02 LF→CRLF、c03 字面替换），每条都做 apply→undo 往返；注入前三条均以 `E1002` 失败，注入后全绿。kernel 侧新增 `transcode_rewriter_from_plan_reads_the_c01_param_snapshot` 钉参数名与 E1001 行为。

## 4. 已接受的坑与边界

- **计数口径不变**：C02/C03 的计数仍在计划期上报，执行期 `detail` 为空（ADR-0050 边界）；C01 的替换计数继续走条目级 `detail`。
- **未知 kind + `edit-content` 条目**：壳层不猜测语义（返回 `None`），仍由内核装配校验以 `E1002` 拦下——这是刻意的"宁可失败也不静默跳过"。
- **`generic_plan_encoding` 留在 CLI**：它只服务"有 `edit-content` 但非 c01/c02/c03"的整段编辑器回退；按当前计划生成器该路径不可达（只有这三类产 `edit-content`），保留而非删除属于最小改动半径，清理留给后续独立变更。
- **未覆盖**：桌面侧仍无 >10 MB 大文件的实机验收、无 Windows 实机长路径/BOM 复核；这些属人工台账，不在本次自动化范围。
- **前端零强制力不变**：本决策只涉及壳层装配，Web/WASM 不获得任何判定入口。
