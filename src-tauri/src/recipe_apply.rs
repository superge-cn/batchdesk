//! `loadRecipe` 的路径槽位表（OQ-40 裁决①：模板 id ＋**调用方交进来的路径参数**）。
//!
//! 为什么需要这张表：模板按不变量 13 **不保存任何路径**（保存与导入两条路都会按
//! `PATH_PARAM_KEYS` 剔除），所以「把模板变成计划」缺的正是路径那一段。用户裁的是①——
//! `loadRecipe` 的入参带上文件集，壳层再走与 `planOperation` **完全相同**的那条路由（同出口）。
//!
//! 键名不靠猜：每个功能的槽位都取自该路由**参数结构体的必填字段**，出处逐条标在下面。
//! 线协议名是 serde `camelCase` 后的形态（`output_dir` → `outputDir`、`source_root` → `sourceRoot`）。
//! 没登记的功能直接报错，不退化成"试几个键名看哪个通过"，也不按位置猜哪个是输入哪个是输出。
//!
//! 两类槽位：
//! - `required`：**每个都必须给**（输入侧，f11 的 `root`、c07 的 `sourceRoot` 也在这一类，
//!   因为它们在各自路由里就是必填字段，语义上属于"从哪儿读"）；
//! - `outputs`：**至少给其中一个**（输出侧的互斥组，如 `output`／`outputDir`；空组＝该功能没有输出路径）。

use kernel::plan::PlanAction;
use kernel::recipe::{RecipeDocument, RecipeSampling, StepSource};
use serde_json::{Map, Value};

/// 一个路径槽位：线协议键 ＋ 是否多值。
#[derive(Clone, Copy, Debug)]
pub struct PathSlot {
    pub key: &'static str,
    pub many: bool,
}

const fn one(key: &'static str) -> PathSlot {
    PathSlot { key, many: false }
}

const fn many(key: &'static str) -> PathSlot {
    PathSlot { key, many: true }
}

/// 一个功能的路径槽位。
#[derive(Clone, Copy, Debug)]
pub struct FeaturePaths {
    pub feature: &'static str,
    pub required: &'static [PathSlot],
    /// 输出互斥组：给其中任意一个即可（空切片＝无输出路径）。
    pub outputs: &'static [PathSlot],
    /// c09 的扫描根走 `planOperation` 的 `sourcePaths` 入参而不是 params
    /// （`lib.rs:264` 用 `source_paths[0]` 当根，且要求恰好一个）。
    pub via_source_paths: bool,
}

/// 逐条出处（2026-11-06 实测自各路由结构体）：
/// - f01／f02／f03：`plan_kinds.rs:62` `SplitParams{file, outputDir}` 两个都必填
/// - f04：`plan_kinds.rs:186` `MergeParams{files, output}` 必填
/// - f05：`lib.rs:887` `F05ShellParams{file, outputDir}` 必填
/// - f06：`plan_csv.rs:215` 明确 `requires output`；输入 `files`
/// - f07／f08／f09：`plan_csv.rs:115-134` `requires output or outputDir`；输入 `files`
/// - f10：`plan_csv.rs:31` `CsvPlanParams{roots}`（清单类功能按根目录扫描）；输出同上二选一
/// - f11：`lib.rs:989` `F11ShellParams{list, root}` 两个都必填（清单文件＋建档根）
/// - f12：`lib.rs:1081` `F12ShellParams{dir}` 必填，无输出路径
/// - c04：`plan_ops.rs:263` 该分支只用 `dir`
/// - c01／c02／c03：`plan_kinds.rs:280/357/430` 输入 `files`；`target` 是**编码**不是路径，
///   留在参数快照里，不进槽位
/// - c07：`plan_manifest.rs:58` `ManifestParams{manifest, sourceRoot, targetRoot}`
/// - c08：`plan_manifest.rs:208` `ArchivePlanParams{sourceRoot, targetRoot, rules}`；
///   `rules` 是规则文本，留在快照里
/// - c10：`plan_manifest.rs:344/383` 按 `mode` 分流——解压 `archive`＋`outputDir`、
///   压缩 `sources`＋`output`
/// - c11…c14：`plan_ops.rs:35` 输入 `files`；输出 `output`／`outputDir` 二选一
///   （`output` 单值还要求**恰好一个输入**，见 `plan_ops.rs:171`，这条由路由自己把关）
/// - c09：`C09Params`（`lib.rs:158`）没有任何路径字段，根来自 `sourcePaths`
pub const FEATURE_PATH_SLOTS: &[FeaturePaths] = &[
    FeaturePaths {
        feature: "c09",
        required: &[many("sourcePaths")],
        outputs: &[],
        via_source_paths: true,
    },
    FeaturePaths {
        feature: "f01",
        required: &[one("file")],
        outputs: &[one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f02",
        required: &[one("file")],
        outputs: &[one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f03",
        required: &[one("file")],
        outputs: &[one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f04",
        required: &[many("files")],
        outputs: &[one("output")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f05",
        required: &[one("file")],
        outputs: &[one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f06",
        required: &[many("files")],
        outputs: &[one("output")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f07",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f08",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f09",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f10",
        required: &[many("roots")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f11",
        required: &[one("list"), one("root")],
        outputs: &[],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f12",
        required: &[one("dir")],
        outputs: &[],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c04",
        required: &[one("dir")],
        outputs: &[],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c01",
        required: &[many("files")],
        outputs: &[],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c02",
        required: &[many("files")],
        outputs: &[],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c03",
        required: &[many("files")],
        outputs: &[],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c07",
        required: &[one("manifest"), one("sourceRoot")],
        outputs: &[one("targetRoot")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c08",
        required: &[one("sourceRoot")],
        outputs: &[one("targetRoot")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c10",
        required: &[one("archive")],
        outputs: &[one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c11",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c12",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c13",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c14",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
];

/// c10 压缩模式的槽位（解压态在上面的表里，因为快照里的 `mode` 才是分流依据）。
pub const C10_COMPRESS_SLOTS: &[PathSlot] = &[many("sources")];
pub const C10_COMPRESS_OUTPUTS: &[PathSlot] = &[one("output")];

pub fn slots_for(feature: &str) -> Option<&'static FeaturePaths> {
    FEATURE_PATH_SLOTS
        .iter()
        .find(|entry| entry.feature == feature)
}

/// 模板采样块的哪个字段供这个路由键取值。
#[derive(Clone, Copy, Debug)]
pub enum SamplingField {
    Window,
    Blocks,
    Bytes,
}

impl SamplingField {
    fn take(self, sampling: &RecipeSampling) -> Option<u64> {
        match self {
            SamplingField::Window => sampling.sample_window,
            SamplingField::Blocks => sampling.sample_blocks,
            SamplingField::Bytes => sampling.sample_bytes,
        }
    }
}

/// 正文块的一个键：模板 `content` 里允许出现的键与必填性（v6，OQ-43 裁决②）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentReq {
    /// 缺了就拒绝（模板没有正文＝回放不了该功能的变换语义，提示重存）。
    Required,
    /// 有就注入、没有由快照/路由缺省兜底。
    Optional,
}

/// 一个功能的「快照→请求」参数映射（OQ-41 裁决②＋OQ-43 裁决②，[ADR-0080](../docs/adr/0080-*.md)／[ADR-0081](../docs/adr/0081-*.md)）。
///
/// 快照保持「计划自带那份」（ADR-0072）不动；这张表负责把它变成路由要的请求形状。
/// 各类处理全部**声明式**，逐类有防漂移断言（见 `lib.rs` 的
/// `*_slots_land_on_the_route_struct_fields`），依据逐条实测自内核计划生成器与壳层路由结构体：
/// - `renames`：计划键 → 路由键。同名不同义的键（f04/f06 旧 `target`）已在内核侧改名
///   （ADR-0080），这里只剩纯改名；
/// - `unset_if`：计划把「自动」写成字面量（`delimiter: "auto"`），路由的
///   `Delimiter::parse` 不认、只认具体分隔符或缺省——剔除即回到"自动"语义；
/// - `parse_f64`：计划把数值格式化成串（c10 的 `maxRatio`），路由要 `f64`；
///   解析失败就报错，不猜；
/// - `drop`：计划展示键与路由字段**类型冲突**（f06/f07 的 `columns` 展示串对路由
///   的 `Vec`、f06 的 `sourceColumn` 展示串对路由的 `bool`）——喂回去就是反序列化
///   硬失败，剔除后由 `content` 注入请求侧真值；
/// - `content`：v6 正文键白名单（`Required`＝缺了拒绝并提示重存；`Optional`＝有就
///   注入且**覆盖**快照同名键）。未知键拒绝（不静默丢，方案 §15.2.4）；
/// - `content_any`：「至少一个」组（f07 的列名或列序号……）；
/// - `sampling`：路由必填但计划不携带的采样键（`plan_c01/c02/c03`、
///   `scan_options` 都不写它们）。`Some(缺省)`＝必填，模板 `sampling` 块优先、
///   否则用缺省；`None`＝可选转发（路由自己有 serde 默认）。缺省值 4096 取自
///   界面既有占位（web `FeaturePages` 的 `|| 4096`），**不是**方案 §5.2 的数值
///   （§5.2 明写不设数值窗口）。
///
/// OQ-43② 之前的静态 `refuse` 已被 content 机制取代：v5 及更早模板没有正文，
/// 在 assess 里按 `content`/`content_any` 的必填性拒绝并提示重存。
#[derive(Clone, Copy, Debug)]
pub struct FeatureParamMap {
    pub feature: &'static str,
    pub renames: &'static [(&'static str, &'static str)],
    pub unset_if: &'static [(&'static str, &'static str)],
    pub parse_f64: &'static [&'static str],
    pub drop: &'static [&'static str],
    pub content: &'static [(&'static str, ContentReq)],
    pub content_any: &'static [&'static [&'static str]],
    pub sampling: &'static [(&'static str, SamplingField, Option<u64>)],
}

const fn m(feature: &'static str) -> FeatureParamMap {
    FeatureParamMap {
        feature,
        renames: &[],
        unset_if: &[],
        parse_f64: &[],
        drop: &[],
        content: &[],
        content_any: &[],
        sampling: &[],
    }
}

/// 采样缺省 4096 的出处见 `FeatureParamMap` 文档：界面既有占位，非方案数值。
const UI_SAMPLING_DEFAULT: u64 = 4096;

/// 逐条出处（2026-11-07 逐模块实测，内核 plan 生成器 → 壳层路由结构体）：
/// - c01：`plan.rs:899` 写 `target/invalid/platformPrior`（确认过编码还有 `source`）；
///   路由 `TranscodeParams`（plan_kinds.rs:280）必填 `sampleWindow`，字段名 `prior`。
/// - c02：`line_endings.rs:395` 写 `target/structure/platformPrior`；路由必填 `sampleBytes`。
/// - c03：`replace_content.rs:714` 写 `mode/find/replace/csvMode/delimiter/platformPrior`；
///   路由 `ReplaceParams` 要 `replacement`（计划键叫 `replace`）＋必填 `sampleBytes`，
///   `delimiter="auto"` 会被 `Delimiter::parse` 拒。
/// - f04：`merge.rs:161` 写 `output/targetEncoding/invalid/fileCount`（`output` 为
///   ADR-0080 改名后）；路由 `MergeParams.target` 是**编码**，计划键 `targetEncoding`。
/// - f05：`csv.rs:686` 写 `targetBytes|rowsPerPart/outputDir/keepHeader/delimiter`；
///   路由 `F05ShellParams.rows`，`delimiter="auto"` 要剔除。
/// - f06：`csv_merge.rs:754` 写 `output/targetEncoding/invalid/delimiter/hasHeader/
///   columns(列数 Number)/strict/sourceColumn(解析结果 String|Null)/fileCount`；
///   路由 `CsvPlanParams.sourceColumn` 是 **bool**（请求侧开关）——计划展示串必须
///   drop 后由 content 注入；`sourceColumnRelative=true` 需要 `baseDir`（路径），
///   模板不存路径 ⇒ 该形态拒绝（ADR-0081 §4）。
/// - f07：`csv_extract.rs:232` 写 `columns`（**逗号连接的展示串**）；路由要
///   `Vec<String>`/`Vec<u64>` ⇒ drop 后由 content 注入列名或列序号。
/// - f08/f09：`csv_rows.rs:225`／`csv_range.rs:310` 写 `targetEncoding/...`；
///   路由 `CsvPlanParams.target`（编码）＋运行时必填 `sampleWindow`（plan_csv.rs:145）。
/// - f10：`listing.rs:207` 写 `format/includeFiles/includeDirs/targetEncoding`；
///   路由 f10 分支不读 `target`，改名只为键面对齐（无害）。
/// - f11：路由 `F11ShellParams{list, root}` 都是路径槽位——清单**文件**由调用方
///   现选、路由重读其内容，快照里没有正文不影响回放（ADR-0081 §2 纠偏：
///   OQ-43 登记时把 f11 误列为"正文缺失"，实测为可套用）。
/// - c08：`c08.rs:295` 只写 `ruleCount/fileCount/unmatchedKept`；路由
///   `ArchivePlanParams.rules`（serde 必填）只能来自 content。
/// - c11：`csv_dedup.rs:197` 写 `key`（"row"/"columns"）；按键列去重需要
///   `keyColumns`，本轮不在 OQ-43② 范围 ⇒ 整行去重可套用、按键列拒绝（build 里按快照判）。
/// - c12：`csv_filter.rs:313` 写 `target/invalid/op/mode(keep|drop)`；路由要
///   `column|columnIndex`＋`value`（计划不存）与 `filterMode`（计划的 `mode` 改名）。
/// - c13：`csv_sort.rs:335` 写 `order/targetEncoding/invalid/hasHeader`；路由要
///   `sortColumns|sortColumnIndexes`（计划不存），`targetEncoding→target` 改名。
/// - c14：`csv_columns.rs:430` 写 `mode(split|merge)/targetEncoding/invalid/hasHeader`；
///   路由要拆分规格（`splitColumn|splitColumnIndex`＋`splitSeparator` XOR
///   `splitRegex`）或合并规格（`mergeColumns|mergeColumnIndexes`＋`mergeJoinWith`），
///   全部来自 content。
/// - c10：`archive.rs:427` 写 `maxTotalBytes/maxRatio(String)`；路由要 `f64`。
///   压缩计划的 `mode` 已由 ADR-0080 补进内核。
pub const FEATURE_PARAM_MAP: &[FeatureParamMap] = &[
    m("c09"),
    m("f01"),
    m("f02"),
    m("f03"),
    FeatureParamMap {
        renames: &[("targetEncoding", "target")],
        ..m("f04")
    },
    FeatureParamMap {
        renames: &[("rowsPerPart", "rows")],
        unset_if: &[("delimiter", "auto")],
        ..m("f05")
    },
    FeatureParamMap {
        renames: &[("targetEncoding", "target")],
        drop: &["columns", "sourceColumn"],
        content: &[
            ("sourceColumn", ContentReq::Required),
            ("sourceColumnName", ContentReq::Optional),
            ("sourceColumnRelative", ContentReq::Optional),
        ],
        sampling: &[(
            "sampleWindow",
            SamplingField::Window,
            Some(UI_SAMPLING_DEFAULT),
        )],
        ..m("f06")
    },
    FeatureParamMap {
        drop: &["columns"],
        content: &[
            ("columns", ContentReq::Optional),
            ("columnIndexes", ContentReq::Optional),
        ],
        content_any: &[&["columns", "columnIndexes"]],
        sampling: &[(
            "sampleWindow",
            SamplingField::Window,
            Some(UI_SAMPLING_DEFAULT),
        )],
        ..m("f07")
    },
    FeatureParamMap {
        renames: &[("targetEncoding", "target")],
        sampling: &[(
            "sampleWindow",
            SamplingField::Window,
            Some(UI_SAMPLING_DEFAULT),
        )],
        ..m("f08")
    },
    FeatureParamMap {
        renames: &[("targetEncoding", "target")],
        sampling: &[(
            "sampleWindow",
            SamplingField::Window,
            Some(UI_SAMPLING_DEFAULT),
        )],
        ..m("f09")
    },
    FeatureParamMap {
        renames: &[("targetEncoding", "target")],
        ..m("f10")
    },
    m("f11"),
    m("f12"),
    FeatureParamMap {
        renames: &[("platformPrior", "prior")],
        sampling: &[
            (
                "sampleWindow",
                SamplingField::Window,
                Some(UI_SAMPLING_DEFAULT),
            ),
            ("sampleBlocks", SamplingField::Blocks, None),
        ],
        ..m("c01")
    },
    FeatureParamMap {
        renames: &[("platformPrior", "prior")],
        sampling: &[(
            "sampleBytes",
            SamplingField::Bytes,
            Some(UI_SAMPLING_DEFAULT),
        )],
        ..m("c02")
    },
    FeatureParamMap {
        renames: &[("replace", "replacement"), ("platformPrior", "prior")],
        unset_if: &[("delimiter", "auto")],
        sampling: &[(
            "sampleBytes",
            SamplingField::Bytes,
            Some(UI_SAMPLING_DEFAULT),
        )],
        ..m("c03")
    },
    m("c04"),
    m("c07"),
    FeatureParamMap {
        content: &[
            ("rules", ContentReq::Required),
            // recursive 是请求侧开关，计划参数里没有（c08.rs 只写 ruleCount/fileCount）
            ("recursive", ContentReq::Optional),
        ],
        ..m("c08")
    },
    FeatureParamMap {
        parse_f64: &["maxRatio"],
        ..m("c10")
    },
    FeatureParamMap {
        sampling: &[(
            "sampleWindow",
            SamplingField::Window,
            Some(UI_SAMPLING_DEFAULT),
        )],
        ..m("c11")
    },
    FeatureParamMap {
        renames: &[("mode", "filterMode")],
        content: &[
            ("column", ContentReq::Optional),
            ("columnIndex", ContentReq::Optional),
            ("op", ContentReq::Optional),
            ("value", ContentReq::Required),
            ("filterMode", ContentReq::Optional),
        ],
        content_any: &[&["column", "columnIndex"]],
        sampling: &[(
            "sampleWindow",
            SamplingField::Window,
            Some(UI_SAMPLING_DEFAULT),
        )],
        ..m("c12")
    },
    FeatureParamMap {
        renames: &[("targetEncoding", "target")],
        content: &[
            ("sortColumns", ContentReq::Optional),
            ("sortColumnIndexes", ContentReq::Optional),
            ("order", ContentReq::Optional),
            // 外排分片行数是请求侧口径，计划参数里没有
            ("externalChunkRows", ContentReq::Optional),
        ],
        content_any: &[&["sortColumns", "sortColumnIndexes"]],
        sampling: &[(
            "sampleWindow",
            SamplingField::Window,
            Some(UI_SAMPLING_DEFAULT),
        )],
        ..m("c13")
    },
    FeatureParamMap {
        renames: &[("targetEncoding", "target")],
        content: &[
            ("splitColumn", ContentReq::Optional),
            ("splitColumnIndex", ContentReq::Optional),
            ("splitSeparator", ContentReq::Optional),
            ("splitRegex", ContentReq::Optional),
            ("mergeColumns", ContentReq::Optional),
            ("mergeColumnIndexes", ContentReq::Optional),
            ("mergeJoinWith", ContentReq::Optional),
            ("mergeName", ContentReq::Optional),
        ],
        content_any: &[&["splitColumn", "mergeColumns"]],
        sampling: &[(
            "sampleWindow",
            SamplingField::Window,
            Some(UI_SAMPLING_DEFAULT),
        )],
        ..m("c14")
    },
];

pub fn param_map_for(feature: &str) -> Option<&'static FeatureParamMap> {
    FEATURE_PARAM_MAP
        .iter()
        .find(|entry| entry.feature == feature)
}

/// 一次 `loadRecipe` 的结果：交给 `planOperation` 同一条路由的三个入参。
#[derive(Debug)]
pub struct RecipeInvocation {
    pub kind: String,
    pub params: Value,
    pub source_paths: Vec<String>,
}

/// 正文键判定：存在且非 null 才算有（Null 与缺失等价，Required 不被 Null 骗过）。
fn content_has(content: Option<&Map<String, Value>>, key: &str) -> bool {
    content
        .and_then(|c| c.get(key))
        .map(|value| !value.is_null())
        .unwrap_or(false)
}

fn is_compress(snapshot: &Map<String, Value>) -> bool {
    snapshot
        .get("mode")
        .and_then(Value::as_str)
        .map(|mode| mode == "compress")
        .unwrap_or(false)
}

/// 一次套用的**静态评估**：槽位选择、守卫与拒绝原因。
///
/// `describeRecipe`（界面据此决定给不给「套用」按钮）与 `build_invocation_step`
/// （真正组装请求）共用同一份判定——写两遍必然漂移。
pub struct ApplyAssessment {
    pub feature: String,
    pub via_source_paths: bool,
    pub required: &'static [PathSlot],
    pub outputs: &'static [PathSlot],
    pub param_map: &'static FeatureParamMap,
}

/// 第 `step_index` 步的静态评估（Phase B 多步执行：每步单独判定）。
///
/// 一套守卫（正文必填/正文任一项/f06 baseDir/c10 旧压缩/c11 按键列）同时服务
/// describe 与 build——不限制步数，多步模板的每一步都必须能单独评估，
/// 任一步不可套用就如实报出。
pub fn assess_step(
    document: &RecipeDocument,
    step_index: usize,
) -> Result<ApplyAssessment, String> {
    let (feature, step) = document
        .step_binding(step_index)
        .map_err(|error| error.message)?;
    let entry = slots_for(&feature)
        .ok_or_else(|| format!("feature {feature} has no registered path slots in this shell"))?;
    let param_map = param_map_for(&feature)
        .ok_or_else(|| format!("feature {feature} has no registered param map in this shell"))?;
    // v6 正文（OQ-43 裁决②）：需要正文的功能在模板没有对应键时如实拒绝——
    // v5 及更早模板没有正文块，重存即可修（保存通道现在采集正文，ADR-0081）。
    let content = step.content.as_ref();
    for (key, req) in param_map.content {
        if *req == ContentReq::Required && !content_has(content, key) {
            return Err(format!(
                "transformation content ({key}) is required for {feature} and is not stored in this template; re-save it from the feature page (schema v6)"
            ));
        }
    }
    for group in param_map.content_any {
        if !group.iter().any(|key| content_has(content, key)) {
            return Err(format!(
                "transformation content for {feature} needs one of ({}); re-save the template from the feature page (schema v6)",
                group.join(", ")
            ));
        }
    }
    // f06 相对来源列需要 baseDir（路径）：模板按不变量 13 不存路径 ⇒ 该形态
    // 套不了。保存侧界面会提示；这里按正文如实拒绝，不喂给路由一个含糊报错。
    if feature == "f06"
        && content
            .and_then(|c| c.get("sourceColumnRelative"))
            .map(|value| value == &Value::Bool(true))
            .unwrap_or(false)
    {
        return Err(
            "relative source-column labels need baseDir, and templates never store paths; re-save without sourceColumnRelative or apply from the feature page"
                .to_string(),
        );
    }
    // c10 旧压缩模板：`mode` 是 ADR-0080 之后才随压缩计划保存的。没有 mode 但带
    // `compression` 的快照是旧版压缩模板，槽位分流会把它当解压处理——拒绝并让
    // 用户重存（新计划带 mode，重存即可修；这不是永久拒绝）。
    if feature == "c10"
        && !is_compress(&step.params_snapshot)
        && step.params_snapshot.contains_key("compression")
    {
        return Err(
            "older compress template saved before mode was recorded; re-save it from the feature page"
                .to_string(),
        );
    }
    // c11 按键列去重：键列正文不在计划参数里，空键列会被路由当"按整行去重"——
    // 静默改变语义，必须拒绝。整行去重（key=="row"，路由的空 picks 缺省）可套用。
    if feature == "c11"
        && step.params_snapshot.get("key").and_then(Value::as_str) == Some("columns")
    {
        return Err("dedup key columns are not carried in plan params; \
             by-column dedup templates cannot be applied"
            .to_string());
    }
    // c10 有两个模式，槽位不同：判据取快照里的 mode，不靠调用方另传一个开关
    let (required, outputs) = if feature == "c10" && is_compress(&step.params_snapshot) {
        (C10_COMPRESS_SLOTS, C10_COMPRESS_OUTPUTS)
    } else {
        (entry.required, entry.outputs)
    };
    Ok(ApplyAssessment {
        feature,
        via_source_paths: entry.via_source_paths,
        required,
        outputs,
        param_map,
    })
}

/// 把映射表套到注入完路径的参数上：改名 → 剔"自动"字面量 → 字符串数值化 → 补采样。
///
/// 快照**已经带**采样键时原样保留（手改模板多给的值，类型正确与否交给路由
/// serde 把关）；映射只补"计划从不携带"的键。
fn apply_param_map(
    params: &mut Map<String, Value>,
    map: &FeatureParamMap,
    sampling: Option<&RecipeSampling>,
    content: Option<&Map<String, Value>>,
) -> Result<(), String> {
    for (from, to) in map.renames {
        if let Some(value) = params.remove(*from) {
            params.entry((*to).to_string()).or_insert(value);
        }
    }
    for (key, literal) in map.unset_if {
        if params.get(*key).and_then(Value::as_str) == Some(*literal) {
            params.remove(*key);
        }
    }
    for key in map.drop {
        params.remove(*key);
    }
    // v6 正文注入：白名单外的键拒绝（不静默丢，方案 §15.2.4）；注入**覆盖**
    // 快照同名键——正文是保存时刻的请求侧真值。
    if let Some(content) = content {
        for key in content.keys() {
            if !map.content.iter().any(|(declared, _)| declared == key) {
                let declared: Vec<&str> = map.content.iter().map(|(k, _)| *k).collect();
                return Err(format!(
                    "{} has no content key {key}; declared keys are {}",
                    map.feature,
                    declared.join(", ")
                ));
            }
        }
        for (key, value) in content {
            params.insert(key.clone(), value.clone());
        }
    }
    for key in map.parse_f64 {
        let parsed = match params.get(*key) {
            Some(Value::String(text)) => Some(
                text.parse::<f64>()
                    .map_err(|_| format!("plan param {key} = {text:?} is not a number"))?,
            ),
            Some(Value::Number(_)) | None => None,
            Some(_) => return Err(format!("plan param {key} is not a number")),
        };
        if let Some(value) = parsed {
            let number = serde_json::Number::from_f64(value)
                .ok_or_else(|| format!("plan param {key} is not a finite number"))?;
            params.insert((*key).to_string(), Value::Number(number));
        }
    }
    for (key, field, default) in map.sampling {
        if params.contains_key(*key) {
            continue;
        }
        let value = sampling.and_then(|block| field.take(block)).or(*default);
        if let Some(value) = value {
            params.insert((*key).to_string(), Value::from(value));
        }
    }
    Ok(())
}

/// 单步模板的套用入口（Phase A 兼容，**仅测试用**：生产路径走
/// `build_invocation_step`，`loadRecipe` 按 stepIndex 逐步生成）。
///
/// 多步模板**不走这里**：每一步要单独评估/生成计划（见 `build_invocation_step`），
/// 第 2 步起的输入在第 1 步执行前不存在（§15.2.3）。这里如实拒绝，不让调用方
/// 把多步模板当成"第 0 步"悄悄套出去。
#[cfg(test)]
pub fn build_invocation(
    document: &RecipeDocument,
    path_params: &Map<String, Value>,
) -> Result<RecipeInvocation, String> {
    if document.steps.len() > 1 {
        return Err(format!(
            "multi-step recipe has {} steps; plan each step separately",
            document.steps.len()
        ));
    }
    build_invocation_step(document, 0, path_params)
}

/// 第 `step_index` 步的套用（Phase B：每步单独生成计划，输入由调用方给）。
///
/// 把调用方给的路径参数按槽位校验后注入参数快照，再按映射表补齐请求形状。
/// 四条硬规则，都是为了"宁可报错也不猜"：
/// 1. 快照里**已经带**某个路径键 → 报错（不变量 13 说模板不该存路径；真存了说明这份
///    模板绕过了保存通道，静默覆盖等于把这条边界抹掉）；
/// 2. 必填槽缺失 → 报错并列出缺哪些；
/// 3. 给了该功能**没有的槽位键** → 报错，不静默忽略（忽略了就会让界面上填了却没生效变成静默失败）；
/// 4. 评估（拒绝原因）与槽位选择全部来自 `assess_step`，与本命令的 describe 输出同源。
pub fn build_invocation_step(
    document: &RecipeDocument,
    step_index: usize,
    path_params: &Map<String, Value>,
) -> Result<RecipeInvocation, String> {
    let assessment = assess_step(document, step_index)?;
    let feature = assessment.feature.as_str();
    let step = &document.steps[step_index];

    let (required, outputs) = (assessment.required, assessment.outputs);
    let mut declared: Vec<&str> = required.iter().map(|slot| slot.key).collect();
    declared.extend(outputs.iter().map(|slot| slot.key));
    for key in path_params.keys() {
        if !declared.contains(&key.as_str()) {
            return Err(format!(
                "{feature} has no path slot {key}; declared slots are {}",
                declared.join(", ")
            ));
        }
    }

    let mut missing: Vec<&str> = Vec::new();
    for slot in required {
        if !path_params.contains_key(slot.key) {
            missing.push(slot.key);
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "{feature} needs the path slots: {}",
            missing.join(", ")
        ));
    }
    if !outputs.is_empty()
        && !outputs
            .iter()
            .any(|slot| path_params.contains_key(slot.key))
    {
        let options: Vec<&str> = outputs.iter().map(|slot| slot.key).collect();
        return Err(format!(
            "{feature} needs one of the output slots: {}",
            options.join(", ")
        ));
    }

    let mut params = step.params_snapshot.clone();
    let mut source_paths: Vec<String> = Vec::new();
    for slot in required.iter().chain(outputs.iter()) {
        let Some(value) = path_params.get(slot.key) else {
            continue;
        };
        // 单数槽收 1 个、多值槽收 ≥1 个：形状不对直接拒，不做"取第一个"的宽容处理
        let paths: Vec<String> = match value {
            Value::Array(items) => {
                if !slot.many {
                    return Err(format!(
                        "path slot {} of {feature} takes one value",
                        slot.key
                    ));
                }
                items
                    .iter()
                    .map(|item| {
                        item.as_str().map(|text| text.to_string()).ok_or_else(|| {
                            format!("path slot {} of {feature} is not a string", slot.key)
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
            Value::String(text) => {
                if slot.many {
                    return Err(format!("path slot {} of {feature} takes a list", slot.key));
                }
                vec![text.clone()]
            }
            _ => {
                return Err(format!(
                    "path slot {} of {feature} is empty or wrong type",
                    slot.key
                ))
            }
        };
        if paths.is_empty() || paths.iter().any(|path| path.trim().is_empty()) {
            return Err(format!(
                "path slot {} of {feature} must not be empty",
                slot.key
            ));
        }
        if assessment.via_source_paths {
            if paths.len() != 1 {
                return Err("c09 requires exactly one source directory".to_string());
            }
            source_paths = paths;
            continue;
        }
        if params.contains_key(slot.key) {
            return Err(format!(
                "template params already carry {}; it must not store paths",
                slot.key
            ));
        }
        let injected = if slot.many {
            Value::Array(paths.into_iter().map(Value::String).collect())
        } else {
            Value::String(paths[0].clone())
        };
        params.insert(slot.key.to_string(), injected);
    }

    if assessment.via_source_paths && source_paths.is_empty() {
        return Err("c09 requires exactly one source directory".to_string());
    }

    apply_param_map(
        &mut params,
        assessment.param_map,
        step.sampling.as_ref(),
        step.content.as_ref(),
    )?;

    Ok(RecipeInvocation {
        kind: assessment.feature,
        params: Value::Object(params),
        source_paths,
    })
}

/// `describeRecipe` 的返回体：界面据此决定给不给「套用」按钮、渲染哪些输入框。
/// 不可套用时 `reason` 说明原因（拒绝是静态评估的一部分，同 build 一套判定）。
///
/// Phase B：`steps` 逐段给出评估（多步模板每一步的槽位/拒绝原因/高风险），
/// `oneClickAllowed` 决定能否一键执行；`applicable`/`feature`/`required`/`outputs`
/// 保持单步语义（多步模板为 false／None／空，由 `steps` 承担）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeApplySpec {
    pub recipe_id: String,
    pub applicable: bool,
    pub reason: Option<String>,
    pub feature: Option<String>,
    pub kind: Option<String>,
    pub required: Vec<SlotSpec>,
    pub outputs: Vec<SlotSpec>,
    pub steps: Vec<StepApplySpec>,
    pub one_click_allowed: bool,
}

/// 多步模板的一步评估（Phase B，§15.2.3）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepApplySpec {
    pub step_id: String,
    pub index: usize,
    pub feature: Option<String>,
    pub kind: Option<String>,
    /// `"original"`（原始选择）或 `"previous"`（上一步实际产物）。
    pub source: String,
    pub applicable: bool,
    pub reason: Option<String>,
    pub required: Vec<SlotSpec>,
    pub outputs: Vec<SlotSpec>,
    /// 高风险步骤（删除／覆盖／不可逆，§7.11）：含高风险步骤的模板禁用一键。
    pub high_risk: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotSpec {
    pub key: String,
    pub many: bool,
}

fn slot_spec(slot: &PathSlot) -> SlotSpec {
    SlotSpec {
        key: slot.key.to_string(),
        many: slot.many,
    }
}

/// 静态评估一份模板的套用面：能套＝给出槽位；不能套＝给出原因。
pub fn describe(document: &RecipeDocument) -> RecipeApplySpec {
    let mut steps = Vec::with_capacity(document.steps.len());
    for (index, step) in document.steps.iter().enumerate() {
        let source = match step.source {
            StepSource::Original => "original",
            StepSource::Previous => "previous",
        };
        let high_risk = step.kind == PlanAction::Delete;
        match assess_step(document, index) {
            Ok(assessment) => steps.push(StepApplySpec {
                step_id: step.step_id.clone(),
                index,
                feature: Some(assessment.feature.clone()),
                kind: Some(step.kind.as_str().to_string()),
                source: source.to_string(),
                applicable: true,
                reason: None,
                required: assessment.required.iter().map(slot_spec).collect(),
                outputs: assessment.outputs.iter().map(slot_spec).collect(),
                high_risk,
            }),
            Err(reason) => steps.push(StepApplySpec {
                step_id: step.step_id.clone(),
                index,
                feature: step.feature.clone(),
                kind: Some(step.kind.as_str().to_string()),
                source: source.to_string(),
                applicable: false,
                reason: Some(reason),
                required: Vec::new(),
                outputs: Vec::new(),
                high_risk,
            }),
        }
    }
    let one_click_allowed =
        steps.iter().all(|step| step.applicable) && !steps.iter().any(|step| step.high_risk);
    let first = steps.first();
    let single = steps.len() == 1;
    let applicable = single && first.is_some_and(|step| step.applicable);
    let reason = if applicable {
        None
    } else if single {
        first.and_then(|step| step.reason.clone())
    } else {
        Some(format!(
            "multi-step recipe has {} steps; apply step by step",
            steps.len()
        ))
    };
    RecipeApplySpec {
        recipe_id: document.recipe_id.clone(),
        applicable,
        reason,
        feature: if applicable {
            first.and_then(|step| step.feature.clone())
        } else {
            None
        },
        kind: if applicable {
            first.and_then(|step| step.kind.clone())
        } else {
            None
        },
        required: if applicable {
            first.map(|step| step.required.clone()).unwrap_or_default()
        } else {
            Vec::new()
        },
        outputs: if applicable {
            first.map(|step| step.outputs.clone()).unwrap_or_default()
        } else {
            Vec::new()
        },
        steps,
        one_click_allowed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::plan::PlanAction;
    use kernel::recipe::CURRENT_RECIPE_SCHEMA_VERSION;
    use serde_json::json;

    fn recipe(feature: &str, kind: PlanAction, snapshot: Map<String, Value>) -> RecipeDocument {
        RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "模板",
            kind,
            snapshot,
            "2026-11-06T00:00:00Z",
            Some(feature.to_string()),
        )
    }

    fn object(items: &[(&str, Value)]) -> Map<String, Value> {
        let mut map = Map::new();
        for (key, value) in items {
            map.insert((*key).to_string(), value.clone());
        }
        map
    }

    #[test]
    fn every_registered_feature_has_slots_and_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for entry in FEATURE_PATH_SLOTS {
            assert!(
                seen.insert(entry.feature),
                "功能码 {} 在槽位表里出现两次",
                entry.feature
            );
            assert!(!entry.required.is_empty(), "{} 没有必填槽位", entry.feature);
            for slot in entry.required.iter().chain(entry.outputs) {
                assert!(
                    kernel::recipe::PATH_PARAM_KEYS
                        .iter()
                        .any(|key| *key == slot.key.to_ascii_lowercase()),
                    "槽位 {} 不在路径键名单里＝它不是路径（会被快照带着走）",
                    slot.key
                );
            }
        }
        // 自校：槽位表覆盖的是壳层真能路由的功能码，缩水说明写法变了
        assert!(
            seen.len() >= 24,
            "槽位表只剩 {} 项，解析或写法变了（核对失效＝没有正例）",
            seen.len()
        );
    }

    #[test]
    fn slots_come_from_the_route_not_from_names() {
        // 这三条正是"凭记忆写会写错"的地方：c01 的输入是 files（不是 file）、
        // f10 是 roots（不是 root）、c08 的输出是 targetRoot。
        let entry = slots_for("c01").unwrap();
        assert_eq!(entry.required[0].key, "files");
        assert!(entry.outputs.is_empty());
        assert_eq!(slots_for("f10").unwrap().required[0].key, "roots");
        assert_eq!(slots_for("c08").unwrap().outputs[0].key, "targetRoot");
    }

    #[test]
    fn load_recipe_builds_the_same_route_call_as_plan_operation() {
        let invocation = build_invocation(
            &recipe(
                "f01",
                PlanAction::Split,
                object(&[("targetBytes", json!(1024))]),
            ),
            &object(&[("file", json!("/in/a.txt")), ("outputDir", json!("/out"))]),
        )
        .unwrap();
        assert_eq!(invocation.kind, "f01");
        assert_eq!(invocation.params["targetBytes"], json!(1024));
        assert_eq!(invocation.params["file"], json!("/in/a.txt"));
        assert_eq!(invocation.params["outputDir"], json!("/out"));
        assert!(invocation.source_paths.is_empty());
    }

    #[test]
    fn c09_uses_the_source_paths_channel_instead_of_params() {
        let invocation = build_invocation(
            &recipe(
                "c09",
                PlanAction::Delete,
                object(&[("keep", json!("earliest"))]),
            ),
            &object(&[("sourcePaths", json!(["/data"]))]),
        )
        .unwrap();
        assert_eq!(invocation.kind, "c09");
        assert_eq!(invocation.source_paths, vec!["/data".to_string()]);
        // 根不进 params：c09 的路由只读 source_paths[0]
        assert!(!invocation
            .params
            .as_object()
            .unwrap()
            .contains_key("sourcePaths"));

        let too_many = build_invocation(
            &recipe("c09", PlanAction::Delete, Map::new()),
            &object(&[("sourcePaths", json!(["/a", "/b"]))]),
        );
        assert!(too_many.unwrap_err().contains("exactly one"));
    }

    #[test]
    fn missing_and_extra_slots_are_both_rejected_by_name() {
        // 缺输出槽：报的是缺哪一个，不是含糊的"参数错误"
        let error = build_invocation(
            &recipe("f01", PlanAction::Split, Map::new()),
            &object(&[("file", json!("/in/a.txt"))]),
        )
        .unwrap_err();
        assert!(error.contains("outputDir"), "{error}");

        // 给了该功能没有的键：不静默忽略（否则界面上填了却没生效就是静默失败）
        let error = build_invocation(
            &recipe("f01", PlanAction::Split, Map::new()),
            &object(&[
                ("file", json!("/in/a.txt")),
                ("outputDir", json!("/out")),
                ("roots", json!(["/x"])),
            ]),
        )
        .unwrap_err();
        assert!(error.contains("no path slot roots"), "{error}");

        // 输出互斥组一个都没给
        let error = build_invocation(
            &recipe("c11", PlanAction::CsvDedup, Map::new()),
            &object(&[("files", json!(["/a.csv"]))]),
        )
        .unwrap_err();
        assert!(error.contains("one of the output slots"), "{error}");
    }

    #[test]
    fn slot_shapes_are_strict_about_single_and_list_values() {
        // 单数槽给数组、多值槽给字符串都要拒
        let error = build_invocation(
            &recipe("f01", PlanAction::Split, Map::new()),
            &object(&[("file", json!(["/a", "/b"])), ("outputDir", json!("/out"))]),
        )
        .unwrap_err();
        assert!(error.contains("takes one value"), "{error}");

        let error = build_invocation(
            &recipe("c01", PlanAction::EditContent, Map::new()),
            &object(&[("files", json!("/a.txt"))]),
        )
        .unwrap_err();
        assert!(error.contains("takes a list"), "{error}");

        let error = build_invocation(
            &recipe("c01", PlanAction::EditContent, Map::new()),
            &object(&[("files", json!(["  "]))]),
        )
        .unwrap_err();
        assert!(error.contains("must not be empty"), "{error}");
    }

    #[test]
    fn a_template_that_still_carries_paths_is_not_silently_overwritten() {
        // 不变量 13：模板不该带路径。真带了（绕过保存通道的手改文件）要报错，
        // 而不是让调用方给的值悄悄盖掉它——那等于把这条边界抹平。
        let snapshot = object(&[("file", json!("/leaked/a.txt"))]);
        let error = build_invocation(
            &recipe("f01", PlanAction::Split, snapshot),
            &object(&[("file", json!("/in/a.txt")), ("outputDir", json!("/out"))]),
        )
        .unwrap_err();
        assert!(error.contains("already carry file"), "{error}");
    }

    #[test]
    fn c10_slots_follow_the_mode_in_the_snapshot() {
        let extract = build_invocation(
            &recipe("c10", PlanAction::Extract, Map::new()),
            &object(&[("archive", json!("/a.zip")), ("outputDir", json!("/out"))]),
        )
        .unwrap();
        assert_eq!(extract.params["archive"], json!("/a.zip"));

        let compress = build_invocation(
            &recipe(
                "c10",
                PlanAction::Archive,
                object(&[("mode", json!("compress"))]),
            ),
            &object(&[
                ("sources", json!(["/a.txt"])),
                ("output", json!("/out.zip")),
            ]),
        )
        .unwrap();
        assert_eq!(compress.params["sources"], json!(["/a.txt"]));
        assert_eq!(compress.params["output"], json!("/out.zip"));

        // 压缩态误给解压键 → 拒
        let error = build_invocation(
            &recipe(
                "c10",
                PlanAction::Archive,
                object(&[("mode", json!("compress"))]),
            ),
            &object(&[("archive", json!("/a.zip")), ("output", json!("/o.zip"))]),
        )
        .unwrap_err();
        assert!(error.contains("no path slot archive"), "{error}");
    }

    #[test]
    fn unbound_and_multi_step_recipes_are_rejected_before_paths() {
        // 未绑定（一对多动作）：报的是"该重存"，不是"路径没给"
        let unbound = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "未绑定",
            PlanAction::Split,
            Map::new(),
            "2026-11-06T00:00:00Z",
            None,
        );
        let error = build_invocation(&unbound, &Map::new()).unwrap_err();
        assert!(error.contains("re-save"), "{error}");

        let mut multi = recipe("f01", PlanAction::Split, Map::new());
        multi.schema_version = CURRENT_RECIPE_SCHEMA_VERSION;
        multi.steps.push(multi.steps[0].clone());
        let error = build_invocation(&multi, &Map::new()).unwrap_err();
        assert!(error.contains("multi-step"), "{error}");
    }

    #[test]
    fn describe_assesses_each_step_and_the_one_click_gate() {
        // Phase B（§15.2.3）：多步模板逐段评估；单步语义的 applicable 保持 false。
        let mut document = recipe(
            "f01",
            PlanAction::Split,
            object(&[("targetBytes", json!(1024))]),
        );
        document.steps.push(kernel::recipe::TemplateStep {
            step_id: "step-2".to_string(),
            kind: PlanAction::Rename,
            params_snapshot: object(&[("prefix", json!("2026_"))]),
            source: StepSource::Previous,
            feature: Some("f12".to_string()),
            sampling: None,
            content: None,
        });
        let spec = describe(&document);
        assert!(!spec.applicable, "多步模板不走单步一键");
        assert_eq!(spec.steps.len(), 2);
        assert_eq!(spec.steps[0].source, "original");
        assert_eq!(spec.steps[1].source, "previous");
        assert!(spec.steps.iter().all(|step| step.applicable));
        assert!(spec.one_click_allowed);

        // 含高风险步骤（删除）→ 一键禁用，且该步如实标 high_risk。
        document.steps.push(kernel::recipe::TemplateStep {
            step_id: "step-3".to_string(),
            kind: PlanAction::Delete,
            params_snapshot: Map::new(),
            source: StepSource::Original,
            feature: Some("c09".to_string()),
            sampling: None,
            content: None,
        });
        let spec = describe(&document);
        assert!(!spec.one_click_allowed);
        assert!(spec.steps[2].high_risk);
        assert!(spec.steps[2].applicable, "c09 本身仍可套用，只是高风险");
    }

    #[test]
    fn build_invocation_step_selects_that_steps_slots() {
        // 每步单独生成计划：第 2 步的输入由调用方给（不预生成所有步骤）。
        let mut document = recipe(
            "f01",
            PlanAction::Split,
            object(&[("targetBytes", json!(1024))]),
        );
        document.steps.push(kernel::recipe::TemplateStep {
            step_id: "step-2".to_string(),
            kind: PlanAction::Rename,
            params_snapshot: object(&[("prefix", json!("2026_"))]),
            source: StepSource::Previous,
            feature: Some("f12".to_string()),
            sampling: None,
            content: None,
        });
        let first = build_invocation_step(
            &document,
            0,
            &object(&[("file", json!("/in/a.txt")), ("outputDir", json!("/out"))]),
        )
        .unwrap();
        assert_eq!(first.kind, "f01");
        assert_eq!(first.params["targetBytes"], json!(1024));

        let second =
            build_invocation_step(&document, 1, &object(&[("dir", json!("/out"))])).unwrap();
        assert_eq!(second.kind, "f12");
        assert_eq!(second.params["dir"], json!("/out"));
        assert_eq!(second.params["prefix"], json!("2026_"));

        let error = build_invocation_step(&document, 2, &Map::new()).unwrap_err();
        assert!(error.contains("no step 2"), "{error}");
    }

    #[test]
    fn every_feature_in_the_feature_table_has_path_slots() {
        // 内核那张「动作→功能码唯一」表里出现的功能码，都必须在这张槽位表里有位置——
        // 否则旧模板一迁移就变成"能套用但没有通道"的空承诺。
        for feature in [
            "c09", "f07", "f10", "f11", "c10", "c11", "c12", "c13", "c14", "c07",
        ] {
            assert!(
                slots_for(feature).is_some(),
                "功能码 {feature} 在唯一映射表里，却没有路径槽位"
            );
        }
    }

    #[test]
    fn path_slots_and_param_maps_cover_the_same_features() {
        // 两张表必须同面：少一边，功能就会"有槽位没参数映射"（评估时才炸）
        // 或"有参数映射没槽位"（永远评估不过）。
        for entry in FEATURE_PATH_SLOTS {
            assert!(
                param_map_for(entry.feature).is_some(),
                "功能码 {} 有槽位却没有参数映射",
                entry.feature
            );
        }
        for map in FEATURE_PARAM_MAP {
            assert!(
                slots_for(map.feature).is_some(),
                "功能码 {} 有参数映射却没有槽位",
                map.feature
            );
        }
        assert_eq!(
            FEATURE_PATH_SLOTS.len(),
            FEATURE_PARAM_MAP.len(),
            "两张表行数不一致＝有功能码只登了一边"
        );
    }

    #[test]
    fn sampling_defaults_fill_and_template_values_win() {
        // c01 路由必填 sampleWindow：模板没存 → 映射表补界面缺省；
        // 模板存了 → 用模板的；可选键（sampleBlocks）只在模板带了才转发。
        let invocation = build_invocation(
            &recipe(
                "c01",
                PlanAction::EditContent,
                object(&[
                    ("target", json!("utf-8")),
                    ("platformPrior", json!("windows")),
                ]),
            ),
            &object(&[("files", json!(["/in/a.txt"]))]),
        )
        .unwrap();
        assert_eq!(invocation.params["sampleWindow"], json!(4096));
        assert_eq!(invocation.params["prior"], json!("windows"));
        assert!(invocation.params.get("platformPrior").is_none());
        assert!(invocation.params.get("sampleBlocks").is_none());

        let mut document = recipe(
            "c01",
            PlanAction::EditContent,
            object(&[("target", json!("utf-8"))]),
        );
        document.steps[0].sampling = Some(kernel::recipe::RecipeSampling {
            sample_window: Some(65536),
            sample_blocks: Some(4),
            sample_bytes: None,
        });
        let invocation =
            build_invocation(&document, &object(&[("files", json!(["/in/a.txt"]))])).unwrap();
        assert_eq!(invocation.params["sampleWindow"], json!(65536));
        assert_eq!(invocation.params["sampleBlocks"], json!(4));

        // 快照自己带了采样键（手改模板）：原样保留，映射不覆盖。
        let invocation = build_invocation(
            &recipe(
                "c01",
                PlanAction::EditContent,
                object(&[("target", json!("utf-8")), ("sampleWindow", json!(1024))]),
            ),
            &object(&[("files", json!(["/in/a.txt"]))]),
        )
        .unwrap();
        assert_eq!(invocation.params["sampleWindow"], json!(1024));
    }

    #[test]
    fn c03_snapshot_becomes_a_valid_request_after_mapping() {
        // 这正是 OQ-41 钉住的三类差异：replace→replacement 改名、缺 sampleBytes、
        // platformPrior→prior 改名。裁决②后一次套用全部补齐。
        let invocation = build_invocation(
            &recipe(
                "c03",
                PlanAction::EditContent,
                object(&[
                    ("mode", json!("plain")),
                    ("find", json!("a")),
                    ("replace", json!("b")),
                    ("platformPrior", json!("windows")),
                    ("delimiter", json!("auto")),
                ]),
            ),
            &object(&[("files", json!(["/in/a.txt"]))]),
        )
        .unwrap();
        assert_eq!(invocation.params["replacement"], json!("b"));
        assert_eq!(invocation.params["sampleBytes"], json!(4096));
        assert_eq!(invocation.params["prior"], json!("windows"));
        assert!(invocation.params.get("replace").is_none());
        assert!(invocation.params.get("delimiter").is_none());
    }

    #[test]
    fn c10_max_ratio_is_parsed_into_a_number() {
        let invocation = build_invocation(
            &recipe(
                "c10",
                PlanAction::Extract,
                object(&[
                    ("maxTotalBytes", json!(1073741824u64)),
                    ("maxRatio", json!("2.5")),
                ]),
            ),
            &object(&[("archive", json!("/a.zip")), ("outputDir", json!("/out"))]),
        )
        .unwrap();
        assert_eq!(invocation.params["maxRatio"], json!(2.5));

        // 不是数字的 maxRatio：报错不猜（路由要 f64，喂错类型就是静默失败）。
        let error = build_invocation(
            &recipe(
                "c10",
                PlanAction::Extract,
                object(&[("maxRatio", json!("wide-open"))]),
            ),
            &object(&[("archive", json!("/a.zip")), ("outputDir", json!("/out"))]),
        )
        .unwrap_err();
        assert!(error.contains("maxRatio"), "{error}");
    }

    #[test]
    fn f05_rows_rename_and_auto_delimiter_are_normalized() {
        let invocation = build_invocation(
            &recipe(
                "f05",
                PlanAction::Split,
                object(&[("rowsPerPart", json!(100)), ("delimiter", json!("auto"))]),
            ),
            &object(&[("file", json!("/in/a.csv")), ("outputDir", json!("/out"))]),
        )
        .unwrap();
        assert_eq!(invocation.params["rows"], json!(100));
        assert!(invocation.params.get("rowsPerPart").is_none());
        assert!(invocation.params.get("delimiter").is_none());
    }

    #[test]
    fn content_features_refuse_without_content_and_apply_with_it() {
        // OQ-43 裁决②：六个功能需要 v6 正文——v5 及更早模板（无正文块）拒绝并
        // 提示重存（重存即可修：保存通道现在采集正文）；带正文则套用，注入的键
        // 落进请求（「真的落在路由结构体字段上」由 lib.rs 的防漂移断言逐功能验证）。
        let cases: &[(&str, PlanAction, serde_json::Value)] = &[
            ("f06", PlanAction::Merge, json!({"sourceColumn": true})),
            ("f07", PlanAction::CsvExtract, json!({"columns": ["name"]})),
            ("c08", PlanAction::Move, json!({"rules": ["docs|ext:pdf"]})),
            (
                "c12",
                PlanAction::CsvFilter,
                json!({"column": "qty", "value": "10", "op": "eq"}),
            ),
            ("c13", PlanAction::CsvSort, json!({"sortColumns": ["id"]})),
            (
                "c14",
                PlanAction::CsvColumns,
                json!({"splitColumn": "tags", "splitSeparator": ";"}),
            ),
        ];
        // 各功能最小路径槽位（正文用例只证"正文齐了就能套"，槽位落地在 lib.rs）
        let paths: &[(&str, serde_json::Value)] = &[
            (
                "f06",
                json!({"files": ["/in/a.csv"], "output": "/out/m.csv"}),
            ),
            ("f07", json!({"files": ["/in/a.csv"], "outputDir": "/out"})),
            ("c08", json!({"sourceRoot": "/src", "targetRoot": "/dst"})),
            (
                "c12",
                json!({"files": ["/in/a.csv"], "output": "/out/f.csv"}),
            ),
            ("c13", json!({"files": ["/in/a.csv"], "outputDir": "/out"})),
            ("c14", json!({"files": ["/in/a.csv"], "outputDir": "/out"})),
        ];
        for (feature, kind, content) in cases {
            let without = recipe(feature, *kind, Map::new());
            let error = build_invocation(&without, &Map::new()).unwrap_err();
            assert!(
                error.contains("re-save"),
                "{feature} 缺正文时应当提示重存：{error}"
            );

            let mut content_map = Map::new();
            for (key, value) in content.as_object().unwrap() {
                content_map.insert(key.clone(), value.clone());
            }
            let mut with = recipe(feature, *kind, Map::new());
            with.steps[0].content = Some(content_map);
            let path_object: Map<String, Value> =
                serde_json::from_value(paths.iter().find(|(f, _)| f == feature).unwrap().1.clone())
                    .unwrap();
            build_invocation(&with, &path_object)
                .unwrap_or_else(|error| panic!("{feature} 带正文应当可套用：{error}"));
        }

        // f11 纠偏（ADR-0081 §2）：清单**文件**是路径槽位、路由重读其内容，
        // 快照没有正文不影响回放——OQ-43 登记时误列为"正文缺失"，现更正为可套用。
        let f11 = recipe("f11", PlanAction::CreateDir, Map::new());
        let invocation = build_invocation(
            &f11,
            &object(&[("list", json!("/lists/dirs.csv")), ("root", json!("/data"))]),
        )
        .unwrap();
        assert_eq!(invocation.params["list"], json!("/lists/dirs.csv"));

        // f06 相对来源列需要 baseDir（路径）：模板不存路径 ⇒ 该形态拒绝。
        let mut relative = recipe("f06", PlanAction::Merge, Map::new());
        relative.steps[0].content = Some(object(&[
            ("sourceColumn", json!(true)),
            ("sourceColumnRelative", json!(true)),
        ]));
        let error = build_invocation(&relative, &Map::new()).unwrap_err();
        assert!(error.contains("baseDir"), "{error}");

        // 正文白名单外的键拒绝（不静默丢，方案 §15.2.4）
        let mut stray = recipe("c08", PlanAction::Move, Map::new());
        stray.steps[0].content = Some(object(&[("rules", json!(["a"])), ("oops", json!(1))]));
        let error = build_invocation(
            &stray,
            &object(&[("sourceRoot", json!("/src")), ("targetRoot", json!("/dst"))]),
        )
        .unwrap_err();
        assert!(error.contains("no content key oops"), "{error}");

        // c11 按键列去重仍拒绝（键列不在本轮 OQ-43② 的六功能清单里）
        let by_columns = recipe(
            "c11",
            PlanAction::CsvDedup,
            object(&[("key", json!("columns"))]),
        );
        let error = build_invocation(&by_columns, &Map::new()).unwrap_err();
        assert!(error.contains("key columns"), "{error}");
    }

    #[test]
    fn f04_encoding_lands_on_target_after_rename() {
        // f04 的同名陷阱：路由 `target` 是编码，输出路径是槽位。计划键
        // `targetEncoding` 改名后喂给 `target`；输出路径来自 pathParams 的 `output`。
        let invocation = build_invocation(
            &recipe(
                "f04",
                PlanAction::Merge,
                object(&[("targetEncoding", json!("utf-8"))]),
            ),
            &object(&[
                ("files", json!(["/in/a.txt", "/in/b.txt"])),
                ("output", json!("/out/all.txt")),
            ]),
        )
        .unwrap();
        assert_eq!(invocation.params["target"], json!("utf-8"));
        assert_eq!(invocation.params["output"], json!("/out/all.txt"));
        assert!(invocation.params.get("targetEncoding").is_none());
    }

    #[test]
    fn older_c10_compress_template_is_refused_until_re_saved() {
        // mode 是 ADR-0080 之后才随压缩计划保存的；旧压缩模板没有 mode 但带
        // compression——不拒绝就会被槽位分流当解压处理。
        let old = recipe(
            "c10",
            PlanAction::Archive,
            object(&[("compression", json!("deflate"))]),
        );
        let error = build_invocation(&old, &Map::new()).unwrap_err();
        assert!(error.contains("re-save"), "{error}");

        // 新计划带 mode：正常走压缩槽位。
        let fresh = recipe(
            "c10",
            PlanAction::Archive,
            object(&[
                ("mode", json!("compress")),
                ("compression", json!("deflate")),
            ]),
        );
        let invocation = build_invocation(
            &fresh,
            &object(&[
                ("sources", json!(["/a.txt"])),
                ("output", json!("/out.zip")),
            ]),
        )
        .unwrap();
        assert_eq!(invocation.params["mode"], json!("compress"));
    }
}
