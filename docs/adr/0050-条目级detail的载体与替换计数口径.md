# 0050. 条目级 detail 的载体：ApplyEntryReport 上的一行文本 ＋ 替换计数按 U+FFFD 个数计

- **状态**：已采纳（2026-11-02，开发代理按台账既定方向实施）
- **日期**：2026-11-02
- **决策者**：开发代理
- **关联**：方案 §5.2（"解码遇非法字节→按替换字符/中止/换候选三策略可配，**默认替换并计数上报**"）、§9.5；T §3.2 C01 行（"替换字符计数为 0"）；设计 01 v1.41、设计 13 v0.25；TC-C01-EXE-05；ADR-0014（encoding 纯函数层）、ADR-0049（同日另一处"看得见的报告"缺口）

---

## 1. 问题背景

C01（编码转换）的执行链是 `plan_c01` → 通用执行器的 `edit-content` 分支 → 由调用方注入的 `content_editor` 闭包做转码。`kernel::encoding::transcode` 一直返回 `Transcoded.replacement_count`，但：

1. **计数被丢掉**：CLI 的编辑器写的是 `transcode(...).map(|r| r.bytes)`，执行报告里没有任何计数；映射表 `TC-C01-EXE-05` 的登记缺口之一就是"替换计数上报未做"。
2. **计数本身不是"计数"**：实现是 `if had_errors { 1 } else { 0 }`——把"这一份文件有没有非法序列"当成计数。一个含 4 处非法序列的文件报 1，方案 §5.2 的"计数上报"与 T 的"替换字符计数"都对不上。

通用执行器此前没有"条目级明细"的通道：`ApplyEntryReport` 只有 `entryId/status/error`，而 §5.2 的计数、将来的匹配计数（C03）等都是**成功条目身上的信息**，不属于 `error`。

## 2. 候选与结论

| 议题 | 候选 | 结论 |
|---|---|---|
| detail 放哪 | a) 新增壳层命令/事件把明细单独传；b) 在 `ApplyEntryReport` 上加可选字段；c) 塞进 `error.message` | **b**：明细属于"这条目执行结果的一部分"，与 `error` 正交（成功条目也可带）；复用既有报告通道＝CLI JSON、壳层任务详情、`task://done` 的 summary 一次全都拿到，无需扩 IPC 命令面（01 §4） |
| detail 形状 | a) 结构化 `BTreeMap<String, u64>`；b) 一行已成型文本 `Option<String>` | **b**：执行器是通用的，键的语义只有注入方知道；结构化映射会把"谁来本地化/怎么排版"的未定问题提前写进契约（09 的 i18n 与 06 的文案表都还没给这类明细定条目）。文本由**产出方**（kernel 的编辑器闭包）生成，前端不拼文案，不违反 06"前端不自造文案" |
| 计数怎么算 | a) 保持 0/1；b) 数解码产生的 U+FFFD 个数；c) 逐字节驱动 `Decoder` 自算非法序列数 | **b**：encoding_rs 按 WHATWG 实现，每处非法序列产出恰好一个 U+FFFD，数它即"替换字符计数"（T 的措辞）。实测：`a\xff\xffb\xfe\xc3c` → 4，纯 ASCII → 0，NUL 不被替换 → 0（无混淆） |
| 源文本自带 U+FFFD | a) 声称精确；b) 记为口径边界 | **b**：无法区分"解码产生的替换"与"源里本来就有的 U+FFFD"，两者会被一并计入。这一条写进 `transcode` 的文档注释与本 ADR §4；c) 留作推翻路径 |
| 流式路径（C02/C03 的 `ContentRewriter`） | a) 一并加 detail；b) 暂不加 | **b**：`rewrite(entry, reader, writer)` 的签名里没有可返回明细的位置，强行改造要动 trait 与两个 rewriter 实现，而本次需求只落在 c01 的整段变换路径；通道已通用，将来按需扩 |

## 3. 选择理由

- **改动半径与契约稳定性**：`detail` 用 `#[serde(default, skip_serializing_if = "Option::is_none")]`，旧报告（无该字段）仍可反序列化，缺省时不产出字段 → web/壳层零强制改动；前端 TS 类型同步加可选字段，保持 01 §2.1 的 wire 一致性。
- **强制力与判定位置不变**：计数由 kernel 纯函数算出，执行器只搬运，入口层（CLI）只渲染——不变量 1／2／3 未动；`Abort` 策略仍返回 E3002 不产出计数（既有语义不变）。
- **可断言且防空转**：kernel 用例断"两条条目各带自己的计数"（`abc`→3、`xy`→2，防串用）＋"注入方不产出明细时保持 `None`"（防凭空造行）；CLI 端到端断真实文件的 JSON 报告 `detail="invalid sequences replaced: 2"`、合法文件 `=0`（不是缺字段），并断人读输出同样显示。

## 4. 后果与已接受的不完美

- **文本型明细的本地化欠账**：detail 是英文一行文本，与 `reason` 同源风格。若 09/06 之后要为这类明细出正式文案，需要把 detail 升级为"码位＋参数"形态——届时新开 ADR，不在本篇里预支。
- **UI 消费面尚未接**：桌面壳目前对 c01 计划传入 `content_editor: None`（编辑类计划在桌面上不可执行），所以 detail 目前只有 CLI 与"报告契约"两层可见。任务列表 UI 建起来时要把 `detail` 一并显示，这一条记在开发计划 §5 的 C01 行剩余缺口里。
- **U+FFFD 口径的边界**（见 §2 第三/四行）：源文本自带的 U+FFFD 会被计入。推翻条件：C01 验收语料里出现"合法输入却计数非零"的实际样本，或 encoding_rs 暴露逐序列错误计数能力时改为逐序列计。
- **`Performed`/`ApplyEntryReport` 的字面量补全是一次机械改动**（39＋17 处 `detail: None`），编译期即可保证不漏；本篇记录它，是为了让"为什么报告里多了个字段"有处可查。

## 5. 验证（2026-11-02 当次实测）

- `cargo test --workspace --all-features` **569 项**（kernel 376＝374＋2、语料断言 22、platform 97、CLI 6、CLI 集成 68）；新用例：`encoding::tests::transcode_counts_every_replacement_character`、`generic_apply::tests::edit_content_carries_detail_per_entry_and_keeps_it_absent_when_not_produced`、`apply_flow::c01_reports_invalid_sequence_replacement_counts_per_entry`。
- 壳层 `cargo test --manifest-path src-tauri/Cargo.toml` **18 项**（本批无壳层改动，沿用同会话绿灯）。
- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo check --manifest-path src-tauri/Cargo.toml`、`python scripts/check_docs.py`、`python fixtures/corpus/check_scope.py` 均 exit 0。
- **先红是实测的**：把执行器里 `detail,` 临时改回 `detail: None,` 后，kernel 用例与 CLI 用例分别失败（CLI 报"<detail 字段缺失>"），恢复后通过——证明两条用例都绑在真实管线上，不是只测纯函数。
- 计数值来自实测：`b"a\xff\xffb\xfe\xc3c"` 在 UTF-8 源下得 4 处替换；两处相邻 `0xff` 的输入得 2；纯 ASCII 得 0；`a\x00b` 得 0（NUL 不被替换，故与 U+FFFD 口径不冲突）。
