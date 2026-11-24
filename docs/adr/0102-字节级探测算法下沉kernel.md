# 0102. 字节级探测算法下沉 kernel（Web 形态复用事实采集）

- 日期：2026-11-20
- 状态：**已采纳·五批＋A2-6/A2-7 补做全部落地**（2026-11-20 裁决「按五批全铺」；
  A2-1～A2-5 完成，A2-6 补 C04/C09/C13/F04，A2-7 补 C10 并把功能面收到 25/25）
- 相关：设计 05《Web 强制力边界契约》§2/§3；前置 [ADR-0098](./0098-WASM导出面骨架与零强制力feature.md)、[ADR-0099](./0099-Web形态的平台规则承载.md)、[ADR-0100](./0100-Web形态WASM构建链与前端装载.md)、[ADR-0101](./0101-Web配额QuotaBased口径.md)

## 背景

A1（C01 纵切）已打通「浏览器事实 → WASM 导出 → 内核计划」，但只覆盖 1/26 个功能。
A2 要把其余功能按同一模式推开时，撞上**分层约束**：

- kernel 的 26 个 `plan_*` 全是**纯函数**，输入是**事实 DTO**（`PlanC01File{path,
  canonical_path, file_size, sample}` 等），共 22 种；
- 事实的**采集**依赖 platform 的 FS 探测（`sample_bytes`、`csv_split_layout`、
  `inspect_line_endings`、`inspect_file`…）；
- **kernel 不依赖 platform**（现有架构，正确：内核保持纯函数、可单测、可 WASM 化）。

因此 22 种 DTO 中有 **15 种**的字段（`source_encoding`/`delimiter`/`columns`/`data_rows`/
`dirty_rows`/`LineEndingCounts`/`SplitLayout`…）只能由 platform 产出，**WASM 里拿不到**。

实测这些探测函数的实现形态（`crates/platform/src/text.rs:51` 起）：

```rust
pub fn sample_bytes(path: &Path, spec: SampleSpec) -> io::Result<SampledBytes> {
    let mut file = File::open(fs_path(path))?;   // ← 唯一的 FS 依赖
    let file_size = file.metadata()?.len();      // ← 唯一的 FS 依赖
    ... 其余全是字节处理 ...
}
```

**算法是纯字节的，只有 `File::open`/`metadata` 碰 FS。** 而 kernel 里已有同构先例：
`encoding::detect(sample: &[u8], prior)`、`detect_bom(bytes: &[u8])` 都是纯字节算法。

## 问题

若不在分层上开一个口子，Web 形态只有两条路，都不接受：

1. **WASM 里重写这 15 套算法** → 桌面与 Web 会成为两套独立实现，同一份文件在两个形态
   可能算出不同的编码/分隔符/列数，直接违反方案 §13.5「同一套内核能力」；
2. **把 platform 拖进 WASM 依赖** → platform 大量使用 `std::fs`/`windows-sys`，
   与 WASM 目标不兼容，且破坏「内核纯函数」这条现有架构。

## 决策（提案）

### 1. 分层原则：按「是否碰 FS」切，不按 crate 切

把 platform 的探测函数拆成两段：

```
┌─ 字节级算法（下沉 kernel）─────────────┐   ┌─ FS 读取（留在适配层）──────┐
│ sample_from_bytes(bytes, spec)         │   │ 桌面：std::fs 读字节        │
│ detect_encoding(bytes, prior)（已有）  │   │ Web：File API slice 读字节  │
│ csv_split_layout_from_bytes(...)       │   └────────────────────────────┘
│ inspect_line_endings_from_bytes(...)   │
└────────────────────────────────────────┘
```

- **kernel 新增**「字节进、事实出」的探测入口，签名只依赖 `&[u8]` 与 `u64`（大小），
  不出现 `Path`/`File`；
- **platform 保留**原函数签名（`&Path`），内部改为「读字节 → 调 kernel 算法」，
  **桌面侧行为与输出完全不变**（同一算法、同一代码路径）；
- **WASM** 侧由 web 事实采集层读 `Blob` 字节，调同一 kernel 入口。

### 2. 下沉顺序：按 A2 功能批次，不按算法数量

不一次性下沉全部 15 个。每批以「一个功能的端到端可用」为单位：

| 批次 | 功能 | 需下沉的算法 |
|---|---|---|
| A2-1 | C01（已完成） | 编码检测（已有） |
| A2-2 | C03 / C02 | `sample_from_bytes`＋内容结构判定、换行统计 |
| A2-3 | F06–F09 / C11–C14 | CSV 嗅探（分隔符/表头/列/数据行/脏行） |
| A2-4 | F01–F05 | 分割布局（按字节/行/份数） |
| A2-5 | C07 / C08 / F10 / F11 / F12 / F13 | 目录遍历类（**前端直接可采，无需下沉**） |

### 3. 不变量

- **桌面零行为变化**：每批必须以「platform 侧既有测试全绿」＋「新增 kernel 字节级测试」
  双重验证；同一段字节在 platform 路径与 kernel 路径必须产出**同一事实**（对拍测试）。
- **WASM 导出面仍受 ADR-0098 白名单约束**：只加处理入口，不得加判定入口。
- **FS 读取在 Web 侧只有 File API**：不引 OPFS 写入、不引目录句柄（A1 裁决）。

### 4. 明确不做

- 不把 `std::fs` 带进 kernel（哪怕 `cfg` 包裹）——字节级入口是 kernel 里唯一的探测形态；
- 不为 Web 单独维护一份算法实现（那等于方案 §13.5 明排除的双实现）；
- 不在本批次接 apply/undo（设计 02 §3.1 的 OPFS FsAdapter 未实现，见 ADR 后续）。

## 代价与取舍

| 项 | 说明 |
|---|---|
| **动 kernel 公共 API** | kernel 新增探测模块属公共面变化，需同步设计 04/05 的溯源（TRACEABILITY）。 |
| **桌面侧要回归** | 每批都要跑 platform 全量测试；CSV/换行类算法复杂，回归面比「接个导出」大得多。 |
| **分批交付** | 26 个功能不可能一次做完；每批结束时的中间态是「部分功能 Web 可用」，需在台账如实标注。 |
| **已知不完美** | `canonical_path` 在 Web 侧恒 null（File API 不暴露真实路径）；采样只取头部窗口（等距窗口待后续）。这两条是**能力差异**而非缺陷，须在台账声明。 |

## 验收口径（每批）

1. kernel 新增字节级入口有单测（含边界：空文件、小于窗口、含 BOM、CRLF 混合）；
2. platform 既有测试全绿，且**新增对拍**：同一文件分别走 platform 原路径与 kernel 字节路径，事实逐字段相等；
3. WASM 侧端到端：web 事实 → 导出 → 真实计划（形如 A1 的 `browser_sampled_*_produce_a_real_plan`）；
4. `cargo test --workspace` 全绿；`check_docs.py` 不出现新的溯源缺口。

## 裁决记录（2026-11-20）

- **批次范围**：产品所有者裁决「按五批全铺」（非先做单批验证）；
- **溯源更新**：设计 04 已追加 v1.21 说明（字节级探测入口）。

## 落地进度

| 批次 | 功能 | 状态 |
|---|---|---|
| A2-1 | C01 | 已落地（事实采集 + WASM 边界转换） |
| A2-2 | C02 / C03 | 已落地（`counts_from_bytes` 下沉 + 分块/整段对拍 + 两端到端出计划） |
| A2-3 | F06–F09 / C11–C14 | **已落地（8/8）**：下沉 `CsvInspection`/`inspect_head`/`complete_records`、`RangeFacts`/`range_from_bytes`（F09 区间状态机）、`parts_from_bytes`（C14 段数评估），八个功能全部接入 WASM 导出并端到端出计划 |
| A2-4 | F01–F05 | **已落地（5/5）**：`kernel::split::{layout_from_bytes, count_lines}` 与 `kernel::csv::csv_split_from_bytes`，platform 改调并删除本地扫描循环 |
| A2-5 | C07/C08/F10/F11/F12/F13 | **已落地**：五个导出（C07/C08/F10/F11/F13；F12 先此已接）接入，事实由前端采集或调用方给定；**无需下沉 kernel** |

**A2-2 实测发现的额外约束**（值得后续批次复用）：

- `#[wasm_bindgen]` 函数在 **native 测试里不可直接调用**（会走 `__wbindgen_describe`
  占位符并 abort）——C01/C02 因入参类型简单恰好不触发，C03 的 `Vec<u8>` 会触发。
  故一律拆成 `plan_xxx_core()`（普通 Rust 函数）＋薄 bindgen 包装，测试调 core。
- 内核枚举的 serde 形态与线协议字符串形态**不是同一套**（`TargetEncoding` 的
  结构体变体名是 `utf8` 而非 `utf-8`），转换必须在 WASM 边界完成。


## A2-3 收尾补充（同日）

- **F09**：区间状态机（原 platform 私有 `RangeState`）整体下沉为
  `kernel::csv_range::{RangeSpan, RangeFacts, range_from_bytes}`，platform 改调它
  并删除本地状态机；新增「桌面流式 vs kernel 整段」对拍（4 组数据 × 3 种区间参数，
  含只有表头／空文件）。反向验证：去掉「扫到 endRow 即停」的 `done` 标记 → 对拍失败。
- **C14**：`parts` 评估由壳层下沉为 `kernel::csv_columns::parts_from_bytes`
  （复用已在 kernel 的 `split_value`/`resolve_column`）；线协议补 `WebSplitSpec`
  （separator/regex 二选一）与 merge 三字段。

**顺带修掉的既有限量实现缺陷**：`ulid_like_id()` 原为「时间戳＋计数器拼串」，
不满足 ULID 首字符 ≤ '7' 的约束，被 F09 计划入口的 `is_ulid` 校验暴露；现改为与
`platform::ids::new_ulid` 同口径的时间戳＋blake3 熵生成（wasm crate 增 blake3 依赖，
`cargo deny check` 通过）。

**新增不变量提醒**：`kernel` 的测试模块必须带 `#[cfg(test)]`——本次曾漏掉，
导致测试代码被编译进生产库（clippy 在非 test 构建里报 unused imports 才暴露）。


## A2-4 补充（同日）

- **F01/F02/F03 共用** `kernel::split::layout_from_bytes`（累加器本就在 kernel）；
  F03 的行数预扫描下沉为 `count_lines`。platform 的 `scan_layout` 改为读整段字节调它。
- **F05** 下沉 `kernel::csv::csv_split_from_bytes`（累加器＋扫描器均在 kernel）。
- **两处 platform 对拍**：`split_layouts_agree_between_stream_and_bytes`（5 组数据 × 3 种基准，
  含末行无换行/空文件/单行）、`csv_split_layouts_agree_between_stream_and_bytes`（5 组 × 4 种基准组合）。

**两处门禁口径修正（值得记住）**：

1. **导出面黑名单误判**：词表用裸子串 `quota`，把 kernel 的**分片行配额**
   （`part_line_quotas`/`new_quotas`，与授权无关）也拦了。已收窄为授权语义符号
   （`QuotaBased`/`quota_exhausted`）——门禁要拦的是「授权配额判定入口」，
   不是字面词。
2. **`groupId`/`plan_id` 的 ULID 约束**：Web 侧调用方不保证生成合法 ULID 形态，
   故 wasm 侧 `valid_or_generated()`：合法则用、否则本地生成（同口径 blake3 实现）。
   计划层要求 ULID 的字段，调用方传入非法值时不应把失败抛给界面。


## A2-5 补充（同日·五批收官）

五个功能**不含算法下沉**——事实要么由调用方直接给定（C07 清单行、F11 目录行、
F13 规则），要么前端可从 File API 采集（C08 的 name/size/lastModified）：

- **C07**：`exists/size` 由已选文件表推断；`target_occupied` 无目标 FS 可查，恒 false。
- **C08**：`rel_path/size/modified_date` 从 File API 采（`lastModified` → `yyyy-mm-dd`）。
- **F10**：`files/dirs/placeholders/skipped/max_depth` 来自**目录遍历**，浏览器采不到
  → 由调用方给定／置 0；预览口径与桌面不同（已在 TRACEABILITY 记明）。
- **F11**：`rows` 由调用方给；`existing_targets` 采不到 → 冲突判定退化为批次内比较。
- **F13**：复用 F12 的项形态与 `OwnedNameRules`（缺省 POSIX）。

**五批收官时状态**：功能全集 23 个，产物导出 `plan_*` 20 个，未接入 5 个
（c04、c09、c10、c13、f04）。**A2-6 已补做其中 4 个**（见下），仅 **c10 仍缺**。

## A2-6 补做（同日）

- **C04**（名称替换）：复用 F12 项形态＋`OwnedNameRules`，直接接入。
- **F04**（文本合并）：与 F06 同族（字节＋编码采样），直接接入。
- **C13**（CSV 排序）：壳层的 `key_values`（键列取值）**下沉** 为
  `kernel::csv_sort::key_values`，壳层改调同一路径；附 3 项口径测试
  （跳缺失行、BOM/空白容忍、1-based 下标）。
- **C09**（重复文件清理）：接入；**能力边界**——`full_hash` 与 `DuplicateReport`
  由调用方提供（Web 侧需与桌面一致的 blake3 实现，本入口只出计划、不算哈希）。

## C10 仍缺（阻塞，非本批遗漏）

C10 的输入是**压缩包内条目**（`ArchiveEntryInfo`：名称/类型/大小/路径安全性）。
浏览器没有解压能力，File API 也拿不到包内清单——**采不到事实就出不了真计划**。
需先定「Web 侧压缩包读取」方案（如引 JS 解压库或服务端预解析）再排期；
**不得**用占位条目冒充计划（那会产出不存在的解压结果）。

> **2026-11-21 更新**：该阻塞已由 [ADR-0103](./0103-C10-Web侧条目清单零依赖中央目录读取.md) 解除——结论是「计划只需要中央目录，不需要解压器」，零依赖读取器已落地并完成四夹具对拍；WASM 导出待接。
