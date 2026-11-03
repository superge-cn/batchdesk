# 0055. C01 流式转码编辑器与 ContentRewriter 明细返回位

- **状态**：已采纳（2026-11-03，A 类流式改造第 3 批）
- **日期**：2026-11-03
- **决策者**：开发代理（工程实现决策；产品行为与阈值不变）
- **关联**：方案 v1.19 §5.2（转码与计数上报）、§9.1（内存指标）；设计 01 v1.45 §5；ADR-0014（编码层）、ADR-0050（条目级 detail 与替换计数口径）、ADR-0053（`read_stream`＋`StreamTranscoder`）、ADR-0054（`RecordFeeder`）；`crates/kernel/src/generic_apply.rs`、`crates/cli/src/main.rs`

---

## 1. 问题

A 类剩余四臂中，C01（转码）与 C14（列拆分/合并）可直接复用 ADR-0053/0054 的底座；C03 与 C13 各有一道**底座解决不了**的结构性障碍（见 §4）。本批把能复用底座的两臂收掉。

- **C01**：CLI 的转码编辑器是整段 `ContentEditor`（`Fn(&[u8]) -> EditedContent`）——源文件全量驻留，峰值 O(文件)。而转码本质是逐块流：`read`→`StreamTranscoder`→`write`（底座已有）。内核的 edit-content 臂已有流式通道 `ContentRewriter`（C02/C03 在用），但它的返回型是 `Result<(), _>`——**没有放条目级明细的位置**（ADR-0050 的已登记边界），直接把 C01 挪过去会丢"替换计数上报"。
- **C14**：`perform_csv_columns` 是"整文件读入→全量解析→内存构建输出"，与 F07 同构，逐行拆分/合并即读即写即可。

## 2. 候选与结论

| 议题 | 候选 | 结论 |
|---|---|---|
| C01 载体 | a) 给 `ContentEditor` 加流式变体端口；b) 复用 `ContentRewriter` | **b**：端口只有一个（少一个概念），且 C01 的执行形态（读源→写回同文件）与 C02/C03 完全一致。代价是 trait 返回型要扩 |
| 明细返回位 | a) trait 加 `&mut` 收集器参数；b) 返回型改 `EditOutcome { detail }`；c) 明细丢弃（C01 桌面化后再说） | **b**：a 给每个实现加仪式感且无人需要中途增量；c 违反 §5.2"默认替换并计数上报"。`EditOutcome` 带默认值，既有实现改一行 |
| 源编码未确认的回退 | a) 流式边读边检测；b) 保留整段检测回退 | **b**：`plan_c01` 对不确定编码本就 E3001 阻断，非阻断条目必带源编码快照；整段回退只在"防御性违规计划"下走到，不值得为它设计流式检测 |
| C14 拆分方式解析时机 | a) 保持原顺序（读源之后才报"缺 separator/regex"）；b) 提前到读源之前 | **b**：错误码与消息不变，且计划期已校验，无观察面；提前避免"必然失败的执行仍读完全源" |
| C03 | — | **不在本批**：regex 模式的流式替换需要重叠窗口设计（任意模式的最大匹配长度不可知，缓冲切边会漏跨块匹配），plain 模式单独流式会造成双路径分叉——留给专门的 ADR |
| C13 | — | **不在本批**：外排的归并相把所有块整块读回内存（`cursors: Vec<(Vec<Vec<String>>, usize)>`），只流式输入侧无净收益；真正的有界归并需要**拉取式**记录读取端口（`read_stream` 是 push 型回调，无法惰性逐条）——属设计 04 新端口，另立 ADR |

## 3. 实现

- `ContentRewriter::rewrite` 返回 `Result<EditOutcome, KernelError>`；edit-content 臂经闭包局部变量把 detail 带出（`rewrite_atomic_with` 的回调签名固定为 `Result<(), _>`）。既有实现 `LineEndingRewriter`/`ReplaceRewriter` 返回 `EditOutcome::default()`（C02/C03 的计数在计划期已上报，无执行期明细）。
- CLI `TranscodeRewriter`：源编码已知时 `bom_prefix` 一次 → 64 KiB 块 `read`→`StreamTranscoder`→`write`→`finish`；detail＝`invalid sequences replaced: {n}`（与整段编辑器同文案，`Abort` 仍 E3002）。未知源编码走整段检测回退。
- C14 `perform_csv_columns`：`stream_csv_records`＋`CsvStreamWriter`（ADR-0054 同一管道）；表头行按模式派生新列名，数据行逐行拆分/合并；空源有表头仍 E2010。
- 顺带补齐测试桩：`TestFs` 此前**未实现** `rewrite_atomic_with`（缺省 E5001）——既有的 ContentRewriter 取消回滚断言在这一臂是空转通过的；桩补真实现后该取消断言首次真正走到重写路径。

## 4. 已接受的坑与边界

- **C03 未动**：`ReplaceRewriter` 仍整段变换（内存 O(文件)），映射表缺口保留；流式化需重叠窗口设计。
- **C13 未动**：输入侧与归并相均整文件/整块驻留；流式化需拉取式记录读取端口（设计 04 新契约）。
- **detail 口径**：流式转码的替换计数与整段 `transcode` 同一 U+FFFD 口径（ADR-0050 边界不变：源文本自带的 U+FFFD 一并计入）。
- **跨块等价性**由 CLI 单测钉住：>64 KiB 的 GB18030 输入（双字节字符被读缓冲切开）流式输出与整段 `transcode` 逐字节一致，BOM 目标同样一致。
- **桌面壳**：C01 在桌面仍不可执行（壳层对 edit-content 无编辑器注入，缺口原样保留，见开发计划 §3.2 平台核心行）。
