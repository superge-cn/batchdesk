//! BatchDesk Web 形态的 WASM 导出面（Wave 5 首块骨架，设计 05 §2）。
//!
//! 三条边界（设计 05 §0 结论）：
//! 1. Web 版强制力只存在于云端服务端；本地模式对授权/配额**零强制力**——不拦截、不放行、不判档。
//! 2. `web/` 不含判定逻辑，只按内核/服务端已给的结果做显示。
//! 3. **本导出面不得出现授权/配额判定入口**（许可证校验、层级判定、`consume`/`release` 公开绑定）。
//!
//! 因此这里只导出两类东西：
//! - **处理入口**：`plan_*`（只读产出计划，不落盘、不执行）；
//! - **能力查询**：`browser_capabilities`（只返回环境事实）。
//!
//! 导出面由 `crates/wasm/tests/export_surface.rs` 断言（出现禁止符号即失败），
//! 编译期 feature `web-zero-force` 裁掉 kernel 的授权阻断路径。

use wasm_bindgen::prelude::*;

/// 环境能力事实（只读，JSON 字符串）。
///
/// 字段刻意只含环境事实；**不含授权/配额**（那些由服务端下发，不经此入口）。
#[wasm_bindgen]
pub fn browser_capabilities() -> String {
    let capabilities = serde_json::json!({
        "form": "web",
        "localMode": true,
        "exportVersion": env!("CARGO_PKG_VERSION"),
    });
    capabilities.to_string()
}

/// **线协议形态**的 C01 入参（与桌面壳层同构）。
///
/// 为什么不能直接用 `kernel::plan::PlanC01Input` 反序列化：内核的 `TargetEncoding`
/// 等枚举的 serde 是**内部结构体变体**（`{"utf8":{"bom":false}}`），而 CLI/桌面/浏览器
/// 三方共用的是 `as_str()`/`parse()` 的**字符串形态**（`"utf-8"`）。实测对拍见
/// `crates/wasm/tests/export_surface.rs::c01_facts_from_browser_shape_feed_the_kernel_plan`。
/// 这层转换是本边界的职责，不是前端或内核的。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebC01Params {
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub invalid: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    /// 事实数组：`path`/`canonicalPath`/`fileSize`/`sample`（由 web 侧采集层产出）。
    pub files: Vec<WebC01File>,
    #[serde(default)]
    pub sample_window: Option<u64>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebC01File {
    pub path: String,
    #[serde(default)]
    pub canonical_path: Option<String>,
    pub file_size: u64,
    pub sample: Vec<u8>,
}

fn parse_or_error<T>(
    value: Option<&str>,
    parse: impl FnOnce(&str) -> Option<T>,
    field: &str,
) -> Result<Option<T>, String> {
    match value {
        None => Ok(None),
        Some(raw) => parse(raw)
            .map(Some)
            .ok_or_else(|| format!("E1002: unknown {field}: {raw}")),
    }
}

/// C01 转码计划（只读）：入参/出参均为 JSON 字符串。
///
/// 失败时返回 `Err(JsValue)`，错误码沿用 kernel 的 `E1xxx–E9xxx`（设计 06 码位）。
pub fn plan_c01_core(json: &str) -> Result<String, String> {
    let params: WebC01Params =
        serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let target = parse_or_error(
        params.target.as_deref(),
        kernel::encoding::TargetEncoding::parse,
        "target encoding",
    )?
    .ok_or_else(|| "E1002: target is required".to_string())?;
    let invalid = parse_or_error(
        params.invalid.as_deref(),
        kernel::encoding::InvalidSequencePolicy::parse,
        "invalid-sequence policy",
    )?
    .unwrap_or(kernel::encoding::InvalidSequencePolicy::ReplaceAndCount);
    let confirmed_source = parse_or_error(
        params.source.as_deref(),
        kernel::encoding::SourceEncoding::parse,
        "source encoding",
    )?;
    let input = kernel::plan::PlanC01Input {
        plan_id: ulid_like_id(),
        target,
        invalid,
        prior: kernel::encoding::PlatformPrior::Other,
        confirmed_source,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        files: params
            .files
            .into_iter()
            .map(|file| kernel::plan::PlanC01File {
                path: file.path,
                canonical_path: file.canonical_path,
                file_size: file.file_size,
                sample: file.sample,
            })
            .collect(),
    };
    let plan = kernel::plan::plan_c01(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

/// C01 转码计划（只读）：入参/出参均为 JSON 字符串。
#[wasm_bindgen]
pub fn plan_c01(json: &str) -> Result<String, JsValue> {
    plan_c01_core(json).map_err(|error| JsValue::from_str(&error))
}

/// 当前时间（Unix 毫秒）。
///
/// **wasm32-unknown-unknown 上不能用 `std::time::SystemTime::now()`**——std 在该
/// 目标没有时钟实现，调用即 panic（`unreachable`）。这条路径覆盖全部 28 个 plan
/// 入口，native 测试与 vitest（mock 掉 WASM）都发现不了，只有真实浏览器跑真 WASM
/// 才暴露（A3-15 的 e2e 用例就是那道防线）。wasm 侧改走 JS 的 `Date.now()`。
#[cfg(target_arch = "wasm32")]
fn epoch_millis() -> u128 {
    js_sys::Date::now() as u128
}

#[cfg(not(target_arch = "wasm32"))]
fn epoch_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

/// 计划 id（ULID 形态，与 `platform::ids::new_ulid` 同口径）。
///
/// kernel 保持纯函数、不碰时间/熵；由适配层提供——桌面是 `platform::ids`，
/// Web 是本实现。此前这里是「时间戳＋计数器拼串」的占位写法，**不满足 ULID 的
/// 首字符约束**（必须 ≤ '7'），直到 F09 计划入口校验 `is_ulid` 才暴露出来。
/// 现按 RFC 时间戳＋blake3 熵生成，保证 `is_ulid` 通过。
fn ulid_like_id() -> String {
    let since_epoch = epoch_millis();
    let millis = (since_epoch as u64) & 0xffff_ffff_ffff;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let entropy_source = format!("{since_epoch}:{counter}:wasm-plan");
    let digest = blake3::hash(entropy_source.as_bytes());
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
    bytes[6..].copy_from_slice(&digest.as_bytes()[..10]);
    let mut number = u128::from_be_bytes(bytes);
    let mut output = [0u8; 26];
    for slot in output.iter_mut().rev() {
        *slot = CROCKFORD[(number & 0x1f) as usize];
        number >>= 5;
    }
    // 首字符必须 ≤ '7'（ULID 的时间戳上界约束）。
    if output[0] > b'7' {
        output[0] = b'7';
    }
    String::from_utf8(output.to_vec()).expect("Crockford alphabet is ASCII")
}

/// Web 执行沙箱的缺省输出目录（A3-8）。
///
/// 与 F01/F02 计划里既有的 `/out/<name>` 同口径。**不能**回落到源文件路径：那样计划
/// 里 `target == source`，执行时目标已存在——`skip` 会自我跳过、`overwrite` 等于
/// 原地覆盖源文件（A3-7 台账记的 `target-in-place`）。
const WEB_DEFAULT_OUTPUT_DIR: &str = "/out";

/// 把用户填的输出目录与源文件名拼成计划里的目标路径。
///
/// 目录口径与 `MemoryFs` 的键空间一致：反斜杠统一成正斜杠、去掉尾部 `/`、缺前导
/// `/` 时补上（`out` 与 `/out` 是同一个目录）。文件名只取源路径最后一段——与桌面
/// CLI `--output-dir` 的派生规则相同（设计 01 §5 的 `output-dir` 分支）。
fn web_output_path(output_dir: Option<&str>, source: &str) -> String {
    let raw = output_dir
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(WEB_DEFAULT_OUTPUT_DIR)
        .replace('\\', "/");
    let trimmed = raw.trim_end_matches('/');
    let dir = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    let name = kernel::conflict::file_name_of(source);
    // `""`（输入全是斜杠）与 `/`（沙箱根目录）都落在根下，不留 `//`。
    if dir.is_empty() || dir == "/" {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// F12 改名计划（只读）：入参/出参均为 JSON 字符串。
///
/// 目标名合法性规则由调用方（服务端下发或按目标卷选择）传入；**缺省＝POSIX 语义**
/// （保守：只拦 `/` 与 NUL，不额外推断平台保留名）。这是**平台事实**而非授权判定
/// （不变量 11、设计 05 §2）。
pub fn plan_f12_core(json: &str) -> Result<String, String> {
    let input: kernel::rename::WebPlanF12Input =
        serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let input: kernel::rename::PlanF12Input = input
        .try_into()
        .map_err(|error| format!("E1002: {error}"))?;
    let plan = kernel::rename::plan_f12(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_f12(json: &str) -> Result<String, JsValue> {
    plan_f12_core(json).map_err(|error| JsValue::from_str(&error))
}

// ── A2-2：C03 查找替换 / C02 换行转换（ADR-0102）─────────────────────────
//
// 这两个功能的**算法本体已在 kernel**（`inspect_bytes`、`LineEndingProcessor`），
// 下沉进来的只是「整段字节 → 事实」的入口（`counts_from_bytes`）。platform 侧保留
// `&Path` 签名并改调同一算法，故桌面行为不变。

/// C03 的线协议入参（camelCase）。事实仍由 web 采集层从 File API 读字节。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebC03File {
    pub path: String,
    #[serde(default)]
    pub canonical_path: Option<String>,
    pub file_size: u64,
    /// **整段内容**：C03 的匹配计数与上下文按整段语义判定；Web 侧只对
    /// 中小文件启用（大文件走流式需要 FsAdapter，见 ADR-0102 §4 未做项）。
    pub data: Vec<u8>,
    pub readonly: bool,
    #[serde(default)]
    pub locked: bool,
}

/// C02 的线协议入参：只需头部样本＋整段统计。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebC02File {
    pub path: String,
    #[serde(default)]
    pub canonical_path: Option<String>,
    pub file_size: u64,
    pub sample: Vec<u8>,
    pub data: Vec<u8>,
}

/// 头部窗口（与壳层 `read_head` 同口径：文件更短即全文）。
fn head_of(data: &[u8], window: usize) -> Vec<u8> {
    let end = window.min(data.len());
    data[..end].to_vec()
}

fn parse_mode(raw: &str, field: &str) -> Result<kernel::replace_content::ReplaceMode, String> {
    kernel::replace_content::ReplaceMode::parse(raw)
        .ok_or_else(|| format!("E1002: unknown {field}: {raw}"))
}

/// C03 的纯转换（**非** `#[wasm_bindgen]`）：线协议 JSON → 计划 JSON。
///
/// 抽成独立函数是为了让 native 测试可直接调用——`#[wasm_bindgen]` 会生成
/// `__wbindgen_describe` 描述符号，在 native 下调用即 abort（实测 C03 触发、
/// C01/C02 不触发，这种「有的能测有的不能」最易被误当成通过）。
pub fn plan_c03_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        mode: String,
        find: String,
        replacement: String,
        #[serde(default = "default_csv_mode")]
        csv_mode: String,
        /// 采样窗口字节数；缺省 4096（与 CLI/桌面同一缺省）。
        #[serde(default = "default_sample_bytes")]
        sample_bytes: usize,
        files: Vec<WebC03File>,
    }
    fn default_csv_mode() -> String {
        "plain".to_string()
    }
    fn default_sample_bytes() -> usize {
        4096
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    if params.find.is_empty() {
        return Err("E1002: find must not be empty".to_string());
    }
    let mode = parse_mode(&params.mode, "replace mode")?;
    let csv_mode = kernel::replace_content::CsvMode::parse(&params.csv_mode)
        .ok_or_else(|| format!("E1002: unknown csv mode: {}", params.csv_mode))?;
    let mut facts = Vec::with_capacity(params.files.len());
    for file in params.files {
        let structure = kernel::replace_content::ContentStructure::resolve(&file.path);
        let inspect = kernel::replace_content::inspect_bytes(
            &file.data,
            mode,
            &params.find,
            &params.replacement,
            csv_mode,
            structure,
            None,
        )
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
        facts.push(kernel::replace_content::PlanC03File {
            path: file.path,
            canonical_path: file.canonical_path,
            file_size: file.file_size,
            sample: head_of(&file.data, params.sample_bytes),
            structure,
            inspect,
            readonly: file.readonly,
            locked: file.locked,
        });
    }
    let input = kernel::replace_content::PlanC03Input {
        plan_id: ulid_like_id(),
        mode,
        find: params.find,
        replacement: params.replacement,
        csv_mode,
        confirmed_source: None,
        prior: kernel::encoding::PlatformPrior::Other,
        delimiter: None,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        files: facts,
    };
    let plan = kernel::replace_content::plan_c03(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

/// C03 查找替换计划（只读）：入参/出参均为 JSON 字符串。
#[wasm_bindgen]
pub fn plan_c03(json: &str) -> Result<String, JsValue> {
    plan_c03_core(json).map_err(|error| JsValue::from_str(&error))
}

/// C02 的纯转换（非 `#[wasm_bindgen]`，理由见 `plan_c03_core`）。
/// 换行统计走 ADR-0102 下沉的 `counts_from_bytes`。
pub fn plan_c02_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        target: String,
        #[serde(default = "default_structure")]
        structure: String,
        files: Vec<WebC02File>,
    }
    fn default_structure() -> String {
        "auto".to_string()
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let target = kernel::line_endings::LineEnding::parse(&params.target)
        .ok_or_else(|| format!("E1002: unknown target: {}", params.target))?;
    let mode = kernel::line_endings::LineEndingStructureMode::parse(&params.structure)
        .ok_or_else(|| format!("E1002: unknown structure: {}", params.structure))?;
    // auto → 按扩展名解析为具体结构（与 platform 同口径：`mode.resolve(path)`）。
    let mut facts = Vec::with_capacity(params.files.len());
    for file in params.files {
        let resolved = mode.resolve(&file.path);
        let counts = kernel::line_endings::counts_from_bytes(&file.data, resolved)
            .map_err(|error| format!("{}: {}", error.code, error.message))?;
        facts.push(kernel::line_endings::PlanC02File {
            path: file.path,
            canonical_path: file.canonical_path,
            file_size: file.file_size,
            sample: file.sample,
            counts,
            structure: resolved,
        });
    }
    let input = kernel::line_endings::PlanC02Input {
        plan_id: ulid_like_id(),
        target,
        structure: mode,
        files: facts,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        confirmed_source: None,
        prior: kernel::encoding::PlatformPrior::Other,
    };
    let plan = kernel::line_endings::plan_c02(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

/// C02 换行转换计划（只读）：入参/出参均为 JSON 字符串。
#[wasm_bindgen]
pub fn plan_c02(json: &str) -> Result<String, JsValue> {
    plan_c02_core(json).map_err(|error| JsValue::from_str(&error))
}

// ── A2-3：CSV 家族（F06–F09／C11–C14，ADR-0102）───────────────────────────
//
// 八个功能共用同一组事实（`kernel::csv_merge::CsvInspection`）。Web 侧把文件读成
// 整段字节交给 WASM，嗅探/列数/脏行全部由 kernel 的字节级入口算——桌面 platform
// 走的是同一条路径（对拍见 `platform::csv_merge::tests`）。

/// CSV 文件的线协议事实（camelCase）。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebCsvFile {
    pub path: String,
    #[serde(default)]
    pub canonical_path: Option<String>,
    pub file_size: u64,
    /// **头部采样**：用于编码判定与分隔符嗅探。
    pub sample: Vec<u8>,
    /// **整段 UTF-8 字节**：用于行数/列信息。浏览器读到的即原始字节。
    pub data: Vec<u8>,
    #[serde(default)]
    pub has_header: bool,
    #[serde(default = "default_true")]
    pub target_occupied: bool,
}

fn default_true() -> bool {
    true
}

impl WebCsvFile {
    /// 由字节算出共享事实（唯一来源＝kernel）。
    fn inspect(
        &self,
        explicit_delimiter: Option<kernel::csv::Delimiter>,
    ) -> Result<kernel::csv_merge::CsvInspection, String> {
        let mut inspection = kernel::csv_merge::inspect_head(
            &self.sample,
            self.file_size,
            kernel::encoding::PlatformPrior::Other,
            explicit_delimiter,
        );
        if inspection.source_encoding.is_none() {
            // 编码未确认时计划层会按 E3001 阻断；此处不猜（方案 §5.2）。
            return Ok(inspection);
        }
        kernel::csv_merge::complete_records(&mut inspection, &self.data, self.has_header);
        Ok(inspection)
    }
}

/// 列选择的线协议形态：`{"name": "..."}` 或 `{"index": n}`。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebColumnPick {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub index: Option<u64>,
}

impl WebColumnPick {
    fn into_kernel(self) -> Result<kernel::csv_extract::ColumnPick, String> {
        match (self.name, self.index) {
            (Some(name), None) => Ok(kernel::csv_extract::ColumnPick::Name(name)),
            (None, Some(index)) => Ok(kernel::csv_extract::ColumnPick::Index(index)),
            _ => Err("E1002: column pick needs exactly one of name or index".to_string()),
        }
    }
}

/// C11 CSV 去重计划（只读，ADR-0102 A2-3）。
pub fn plan_c11_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        target: String,
        invalid: Option<String>,
        #[serde(default)]
        output_dir: Option<String>,
        key: Vec<WebColumnPick>,
        files: Vec<WebCsvFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let target = kernel::encoding::TargetEncoding::parse(&params.target)
        .ok_or_else(|| format!("E1002: unknown target: {}", params.target))?;
    let invalid = parse_or_error(
        params.invalid.as_deref(),
        kernel::encoding::InvalidSequencePolicy::parse,
        "invalid-sequence policy",
    )?
    .unwrap_or(kernel::encoding::InvalidSequencePolicy::ReplaceAndCount);
    let key = params
        .key
        .into_iter()
        .map(WebColumnPick::into_kernel)
        .collect::<Result<Vec<_>, _>>()?;
    let mut files = Vec::with_capacity(params.files.len());
    let output_dir = params.output_dir;
    for file in params.files {
        let inspection = file.inspect(None)?;
        if inspection.source_encoding.is_none() {
            return Err("E1002: source encoding must be confirmed before planning".to_string());
        }
        let records = kernel::csv_merge::parse_records(&file.data, inspection.delimiter.as_byte());
        let output = web_output_path(output_dir.as_deref(), &file.path);
        files.push(kernel::csv_dedup::PlanC11File {
            path: file.path,
            canonical_path: file.canonical_path,
            output,
            file_size: inspection.file_size,
            source_encoding: inspection.source_encoding,
            delimiter: inspection.delimiter,
            has_header: inspection.has_header,
            columns: inspection.columns,
            column_count: inspection.column_count,
            dirty_rows: inspection.dirty_rows,
            records,
            target_occupied: file.target_occupied,
        });
    }
    let input = kernel::csv_dedup::PlanC11Input {
        plan_id: ulid_like_id(),
        target,
        invalid,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        key,
        files,
    };
    let plan = kernel::csv_dedup::plan_c11(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_c11(json: &str) -> Result<String, JsValue> {
    plan_c11_core(json).map_err(|error| JsValue::from_str(&error))
}

/// C12 CSV 过滤计划（只读，ADR-0102 A2-3）。
pub fn plan_c12_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        target: String,
        invalid: Option<String>,
        #[serde(default)]
        output_dir: Option<String>,
        mode_keep: bool,
        condition_column: WebColumnPick,
        condition_op: String,
        condition_value: String,
        files: Vec<WebCsvFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let target = kernel::encoding::TargetEncoding::parse(&params.target)
        .ok_or_else(|| format!("E1002: unknown target: {}", params.target))?;
    let invalid = parse_or_error(
        params.invalid.as_deref(),
        kernel::encoding::InvalidSequencePolicy::parse,
        "invalid-sequence policy",
    )?
    .unwrap_or(kernel::encoding::InvalidSequencePolicy::ReplaceAndCount);
    let op = kernel::csv_filter::FilterOp::parse(&params.condition_op)
        .ok_or_else(|| format!("E1002: unknown filter op: {}", params.condition_op))?;
    let column = params.condition_column.into_kernel()?;
    let mut files = Vec::with_capacity(params.files.len());
    let output_dir = params.output_dir;
    for file in params.files {
        let inspection = file.inspect(None)?;
        if inspection.source_encoding.is_none() {
            return Err("E1002: source encoding must be confirmed before planning".to_string());
        }
        let records = kernel::csv_merge::parse_records(&file.data, inspection.delimiter.as_byte());
        let output = web_output_path(output_dir.as_deref(), &file.path);
        files.push(kernel::csv_filter::PlanC12File {
            path: file.path,
            canonical_path: file.canonical_path,
            output,
            file_size: inspection.file_size,
            source_encoding: inspection.source_encoding,
            delimiter: inspection.delimiter,
            has_header: inspection.has_header,
            columns: inspection.columns,
            column_count: inspection.column_count,
            dirty_rows: inspection.dirty_rows,
            records,
            target_occupied: file.target_occupied,
        });
    }
    let input = kernel::csv_filter::PlanC12Input {
        plan_id: ulid_like_id(),
        target,
        invalid,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        mode_keep: params.mode_keep,
        condition_column: column,
        condition_op: op,
        condition_value: params.condition_value,
        files,
    };
    let plan = kernel::csv_filter::plan_c12(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_c12(json: &str) -> Result<String, JsValue> {
    plan_c12_core(json).map_err(|error| JsValue::from_str(&error))
}

/// F06 CSV 合并计划（只读，ADR-0102 A2-3）。
pub fn plan_f06_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        target: String,
        target_encoding: String,
        invalid: Option<String>,
        output_delimiter: Option<String>,
        add_source_column: bool,
        source_column_name: String,
        strict: bool,
        files: Vec<WebCsvFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let target = kernel::encoding::TargetEncoding::parse(&params.target_encoding)
        .ok_or_else(|| format!("E1002: unknown target encoding: {}", params.target_encoding))?;
    let invalid = parse_or_error(
        params.invalid.as_deref(),
        kernel::encoding::InvalidSequencePolicy::parse,
        "invalid-sequence policy",
    )?
    .unwrap_or(kernel::encoding::InvalidSequencePolicy::ReplaceAndCount);
    let explicit = match params.output_delimiter {
        Some(raw) => Some(
            kernel::csv::Delimiter::parse(&raw)
                .ok_or_else(|| format!("E1002: unknown output delimiter: {raw}"))?,
        ),
        None => None,
    };
    let mut files = Vec::with_capacity(params.files.len());
    for file in params.files {
        let inspection = file.inspect(explicit)?;
        if inspection.source_encoding.is_none() {
            return Err("E1002: source encoding must be confirmed before planning".to_string());
        }
        let label = if params.add_source_column {
            Some(file.path.clone())
        } else {
            None
        };
        files.push(kernel::csv_merge::PlanF06File {
            path: file.path,
            canonical_path: file.canonical_path,
            file_size: inspection.file_size,
            source_encoding: inspection.source_encoding,
            delimiter: inspection.delimiter,
            has_header: inspection.has_header,
            columns: inspection.columns,
            column_count: inspection.column_count,
            data_rows: inspection.data_rows,
            dirty_rows: inspection.dirty_rows,
            source_label: label,
        });
    }
    let input = kernel::csv_merge::PlanF06Input {
        plan_id: ulid_like_id(),
        target: params.target,
        target_encoding: target,
        invalid,
        output_delimiter: explicit,
        add_source_column: params.add_source_column,
        source_column_name: params.source_column_name,
        strict: params.strict,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        target_occupied: false,
        files,
    };
    let plan = kernel::csv_merge::plan_f06(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_f06(json: &str) -> Result<String, JsValue> {
    plan_f06_core(json).map_err(|error| JsValue::from_str(&error))
}

/// F07 CSV 列提取计划（只读，ADR-0102 A2-3）。
pub fn plan_f07_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        target_encoding: String,
        invalid: Option<String>,
        selections: Vec<WebColumnPick>,
        #[serde(default)]
        output_dir: Option<String>,
        files: Vec<WebCsvFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let target = kernel::encoding::TargetEncoding::parse(&params.target_encoding)
        .ok_or_else(|| format!("E1002: unknown target encoding: {}", params.target_encoding))?;
    let invalid = parse_or_error(
        params.invalid.as_deref(),
        kernel::encoding::InvalidSequencePolicy::parse,
        "invalid-sequence policy",
    )?
    .unwrap_or(kernel::encoding::InvalidSequencePolicy::ReplaceAndCount);
    let selections = params
        .selections
        .into_iter()
        .map(WebColumnPick::into_kernel)
        .collect::<Result<Vec<_>, _>>()?;
    let mut files = Vec::with_capacity(params.files.len());
    let output_dir = params.output_dir;
    for file in params.files {
        let inspection = file.inspect(None)?;
        if inspection.source_encoding.is_none() {
            return Err("E1002: source encoding must be confirmed before planning".to_string());
        }
        let target_path = web_output_path(output_dir.as_deref(), &file.path);
        files.push(kernel::csv_extract::PlanF07File {
            path: file.path,
            canonical_path: file.canonical_path,
            file_size: inspection.file_size,
            source_encoding: inspection.source_encoding,
            delimiter: inspection.delimiter,
            has_header: inspection.has_header,
            columns: inspection.columns,
            column_count: inspection.column_count,
            data_rows: inspection.data_rows,
            dirty_rows: inspection.dirty_rows,
            target: target_path,
            target_occupied: file.target_occupied,
        });
    }
    let input = kernel::csv_extract::PlanF07Input {
        plan_id: ulid_like_id(),
        target_encoding: target,
        invalid,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        selections,
        files,
    };
    let plan = kernel::csv_extract::plan_f07(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_f07(json: &str) -> Result<String, JsValue> {
    plan_f07_core(json).map_err(|error| JsValue::from_str(&error))
}

/// F08 CSV 行提取计划（只读，ADR-0102 A2-3）。
pub fn plan_f08_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        target_encoding: String,
        invalid: Option<String>,
        window: String,
        count: u64,
        #[serde(default)]
        start_row: u64,
        #[serde(default)]
        end_row: u64,
        #[serde(default)]
        output_dir: Option<String>,
        files: Vec<WebCsvFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let target = kernel::encoding::TargetEncoding::parse(&params.target_encoding)
        .ok_or_else(|| format!("E1002: unknown target encoding: {}", params.target_encoding))?;
    let invalid = parse_or_error(
        params.invalid.as_deref(),
        kernel::encoding::InvalidSequencePolicy::parse,
        "invalid-sequence policy",
    )?
    .unwrap_or(kernel::encoding::InvalidSequencePolicy::ReplaceAndCount);
    let window = kernel::csv_rows::RowWindow::parse(&params.window)
        .ok_or_else(|| format!("E1002: unknown window: {}", params.window))?;
    let mut files = Vec::with_capacity(params.files.len());
    let output_dir = params.output_dir;
    for file in params.files {
        let inspection = file.inspect(None)?;
        if inspection.source_encoding.is_none() {
            return Err("E1002: source encoding must be confirmed before planning".to_string());
        }
        let target_path = web_output_path(output_dir.as_deref(), &file.path);
        files.push(kernel::csv_rows::PlanF08File {
            path: file.path,
            canonical_path: file.canonical_path,
            file_size: inspection.file_size,
            source_encoding: inspection.source_encoding,
            delimiter: inspection.delimiter,
            has_header: inspection.has_header,
            columns: inspection.columns,
            column_count: inspection.column_count,
            data_rows: inspection.data_rows,
            dirty_rows: inspection.dirty_rows,
            target: target_path,
            target_occupied: file.target_occupied,
        });
    }
    let input = kernel::csv_rows::PlanF08Input {
        plan_id: ulid_like_id(),
        target_encoding: target,
        invalid,
        window,
        count: params.count,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        files,
    };
    let _ = (params.start_row, params.end_row);
    let plan = kernel::csv_rows::plan_f08(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_f08(json: &str) -> Result<String, JsValue> {
    plan_f08_core(json).map_err(|error| JsValue::from_str(&error))
}

/// F09 CSV 区间读取计划（只读，ADR-0102 A2-3 收尾）。
pub fn plan_f09_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        target_encoding: String,
        invalid: Option<String>,
        start_row: u64,
        end_row: u64,
        #[serde(default)]
        output_dir: Option<String>,
        files: Vec<WebCsvFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let target = kernel::encoding::TargetEncoding::parse(&params.target_encoding)
        .ok_or_else(|| format!("E1002: unknown target encoding: {}", params.target_encoding))?;
    let invalid = parse_or_error(
        params.invalid.as_deref(),
        kernel::encoding::InvalidSequencePolicy::parse,
        "invalid-sequence policy",
    )?
    .unwrap_or(kernel::encoding::InvalidSequencePolicy::ReplaceAndCount);
    let mut files = Vec::with_capacity(params.files.len());
    let output_dir = params.output_dir;
    for file in params.files {
        let inspection = file.inspect(None)?;
        if inspection.source_encoding.is_none() {
            return Err("E1002: source encoding must be confirmed before planning".to_string());
        }
        let facts = kernel::csv_range::range_from_bytes(
            &file.data,
            file.has_header,
            params.start_row,
            params.end_row,
            inspection.delimiter,
        )
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
        // unclosed_quote：整段扫描的尾部状态（提前终止时按 platform 同口径记 false）。
        let unclosed_quote = if facts.total_known {
            let mut scanner = kernel::csv::CsvRecordScanner::new(inspection.delimiter.as_byte());
            scanner.feed(&file.data, |_| {});
            scanner.finish().unclosed_quote
        } else {
            false
        };
        let column_count = if facts.header.is_some() {
            facts.header_columns
        } else {
            facts.max_selected_columns
        };
        let target_path = web_output_path(output_dir.as_deref(), &file.path);
        files.push(kernel::csv_range::PlanF09File {
            path: file.path,
            canonical_path: file.canonical_path,
            file_size: inspection.file_size,
            source_encoding: inspection.source_encoding,
            delimiter: inspection.delimiter,
            delimiter_detected: inspection.delimiter_detected,
            has_header: inspection.has_header,
            columns: inspection.columns,
            column_count,
            data_rows: facts.data_rows,
            total_known: facts.total_known,
            selected_rows: facts.selected_rows,
            header_range: facts.header.map(|span| (span.start, span.end)),
            data_range: facts.selected,
            dirty_rows: facts.dirty_rows,
            multi_line_header: facts.multi_line_header,
            unclosed_quote,
            target: target_path,
            target_occupied: file.target_occupied,
        });
    }
    let input = kernel::csv_range::PlanF09Input {
        plan_id: ulid_like_id(),
        target_encoding: target,
        invalid,
        start_row: params.start_row,
        end_row: params.end_row,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        files,
    };
    let plan = kernel::csv_range::plan_f09(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_f09(json: &str) -> Result<String, JsValue> {
    plan_f09_core(json).map_err(|error| JsValue::from_str(&error))
}

/// C14 的切分规则（线协议形态）。`by` 为 `{"separator":"-"}` 或 `{"regex":"\s+"}`。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSplitSpec {
    pub source: WebColumnPick,
    #[serde(default)]
    pub separator: Option<String>,
    #[serde(default)]
    pub regex: Option<String>,
}

impl WebSplitSpec {
    fn into_kernel(self) -> Result<kernel::csv_columns::SplitSpec, String> {
        let by = match (self.separator, self.regex) {
            (Some(separator), None) => kernel::csv_columns::SplitBy::Separator(separator),
            (None, Some(pattern)) => kernel::csv_columns::SplitBy::Regex(pattern),
            _ => {
                return Err("E1002: split rule needs exactly one of separator or regex".to_string())
            }
        };
        Ok(kernel::csv_columns::SplitSpec {
            source: self.source.into_kernel()?,
            by,
        })
    }
}

/// C14 CSV 列拆分计划（只读，ADR-0102 A2-3 收尾）。当前支持拆分模式；
/// 合并模式的 `parts` 恒为 1（计划层按规则合并，不需要段数评估）。
pub fn plan_c14_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        target_encoding: String,
        invalid: Option<String>,
        mode: String,
        #[serde(default)]
        split: Option<WebSplitSpec>,
        #[serde(default)]
        merge_picks: Vec<WebColumnPick>,
        #[serde(default)]
        joiner: String,
        #[serde(default)]
        target_name: Option<String>,
        #[serde(default)]
        output_dir: Option<String>,
        files: Vec<WebCsvFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let target = kernel::encoding::TargetEncoding::parse(&params.target_encoding)
        .ok_or_else(|| format!("E1002: unknown target encoding: {}", params.target_encoding))?;
    let invalid = parse_or_error(
        params.invalid.as_deref(),
        kernel::encoding::InvalidSequencePolicy::parse,
        "invalid-sequence policy",
    )?
    .unwrap_or(kernel::encoding::InvalidSequencePolicy::ReplaceAndCount);
    let mode = match params.mode.as_str() {
        "split" => kernel::csv_columns::ColumnsMode::Split(
            params
                .split
                .ok_or_else(|| "E1002: split rule is required in split mode".to_string())?
                .into_kernel()?,
        ),
        "merge" => kernel::csv_columns::ColumnsMode::Merge(kernel::csv_columns::MergeSpec {
            picks: params
                .merge_picks
                .into_iter()
                .map(WebColumnPick::into_kernel)
                .collect::<Result<Vec<_>, _>>()?,
            joiner: params.joiner,
            target_name: params.target_name,
        }),
        other => return Err(format!("E1002: unknown c14 mode: {other}")),
    };
    let mut files = Vec::with_capacity(params.files.len());
    let output_dir = params.output_dir;
    for file in params.files {
        let inspection = file.inspect(None)?;
        if inspection.source_encoding.is_none() {
            return Err("E1002: source encoding must be confirmed before planning".to_string());
        }
        let records = kernel::csv_merge::parse_records(&file.data, inspection.delimiter.as_byte());
        // parts 由 kernel 的字节级入口评估（原在壳层，见 ADR-0102）。
        let parts = match &mode {
            kernel::csv_columns::ColumnsMode::Split(spec) => kernel::csv_columns::parts_from_bytes(
                file.has_header,
                &records,
                &spec.source,
                &spec.by,
            )
            .map_err(|error| format!("{}: {}", error.code, error.message))?,
            kernel::csv_columns::ColumnsMode::Merge(_) => 1,
        };
        let target_path = web_output_path(output_dir.as_deref(), &file.path);
        files.push(kernel::csv_columns::PlanC14File {
            path: file.path,
            canonical_path: file.canonical_path,
            file_size: inspection.file_size,
            source_encoding: inspection.source_encoding,
            delimiter: inspection.delimiter,
            has_header: inspection.has_header,
            columns: inspection.columns,
            column_count: inspection.column_count,
            data_rows: inspection.data_rows,
            dirty_rows: inspection.dirty_rows,
            parts,
            target: target_path,
            target_occupied: file.target_occupied,
        });
    }
    let input = kernel::csv_columns::PlanC14Input {
        plan_id: ulid_like_id(),
        mode,
        target_encoding: target,
        invalid,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        files,
    };
    let plan = kernel::csv_columns::plan_c14(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_c14(json: &str) -> Result<String, JsValue> {
    plan_c14_core(json).map_err(|error| JsValue::from_str(&error))
}

// ── A2-4：分割布局（F01–F05，ADR-0102）────────────────────────────────────
//
// 五个功能共用 `kernel::split::layout_from_bytes` / `kernel::csv::csv_split_from_bytes`
// 两个字节级入口；platform 的流式路径已改调同一实现（对拍见 platform 测试）。

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// ULID 形态校验（与 `platform::ids::is_ulid` 同口径）。
///
/// kernel 的 `is_ulid` 是 `pub(crate)`，WASM 侧不能调；此处本地实现同一规则：
/// 26 字符、首字符 ≤ '7'、字符全在 Crockford 字母表内。
fn is_ulid(value: &str) -> bool {
    value.len() == 26
        && value.as_bytes()[0] <= b'7'
        && value.bytes().all(|b| CROCKFORD.contains(&b))
}

/// 分组标识：**合法则用，否则生成**。
///
/// 计划层要求该字段为 26 字符 ULID，而 Web 形态的调用方（浏览器脚本）不保证
/// 生成合法形态——与其把失败抛给界面，不如在此兜底（该标识不承载用户语义）。
fn valid_or_generated(value: &str) -> String {
    if is_ulid(value) {
        value.to_string()
    } else {
        ulid_like_id()
    }
}

/// 分割类文件的线协议事实（camelCase）。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSplitFile {
    pub path: String,
    #[serde(default)]
    pub canonical_path: Option<String>,
    /// 同组文件共享的标识（由调用方给；Web 侧按选中顺序生成）。
    #[serde(default)]
    pub group_id: String,
    pub file_size: u64,
    pub data: Vec<u8>,
}

/// F01/F02/F03 共用：由整段字节算出分片布局。
fn split_layout_for(
    file: &WebSplitFile,
    accumulator: kernel::split::SplitAccumulator,
) -> kernel::split::SplitLayout {
    kernel::split::layout_from_bytes(&file.data, file.file_size, accumulator)
}

/// F01 按字节分片计划（只读）。
pub fn plan_f01_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        target_bytes: u64,
        #[serde(default)]
        output_dir: String,
        files: Vec<WebSplitFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let mut files = Vec::with_capacity(params.files.len());
    for file in params.files {
        let accumulator = kernel::split::SplitAccumulator::new(params.target_bytes)
            .map_err(|error| format!("E1002: {}", error.message))?;
        let layout = split_layout_for(&file, accumulator);
        files.push(kernel::split::PlanF01File {
            path: file.path,
            canonical_path: file.canonical_path,
            // 分组标识由调用方指定；Web 侧未指定时自动生成（计划层要求 ULID 形态）。
            group_id: valid_or_generated(&file.group_id),
            layout,
        });
    }
    let input = kernel::split::PlanF01Input {
        plan_id: ulid_like_id(),
        target_bytes: params.target_bytes,
        output_dir: params.output_dir,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        existing_targets: Vec::new(),
        files,
    };
    let plan = kernel::split::plan_f01(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_f01(json: &str) -> Result<String, JsValue> {
    plan_f01_core(json).map_err(|error| JsValue::from_str(&error))
}

/// F02 按行分片计划（只读）。
pub fn plan_f02_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        lines_per_part: u64,
        #[serde(default)]
        output_dir: String,
        files: Vec<WebSplitFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let mut files = Vec::with_capacity(params.files.len());
    for file in params.files {
        let accumulator = kernel::split::SplitAccumulator::new_lines(params.lines_per_part)
            .map_err(|error| format!("E1002: {}", error.message))?;
        let layout = split_layout_for(&file, accumulator);
        files.push(kernel::split::PlanF01File {
            path: file.path,
            canonical_path: file.canonical_path,
            group_id: valid_or_generated(&file.group_id),
            layout,
        });
    }
    let input = kernel::split::PlanF02Input {
        plan_id: ulid_like_id(),
        lines_per_part: params.lines_per_part,
        output_dir: params.output_dir,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        existing_targets: Vec::new(),
        files,
    };
    let plan = kernel::split::plan_f02(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_f02(json: &str) -> Result<String, JsValue> {
    plan_f02_core(json).map_err(|error| JsValue::from_str(&error))
}

/// F03 按份数分片计划（只读）：先计行数再按配额分配。
pub fn plan_f03_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        parts: u64,
        #[serde(default)]
        output_dir: String,
        files: Vec<WebSplitFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    if params.parts == 0 {
        return Err("E1002: parts must be greater than zero".to_string());
    }
    let mut files = Vec::with_capacity(params.files.len());
    for file in params.files {
        let total_lines = kernel::split::count_lines(&file.data);
        let quotas = kernel::split::part_line_quotas(total_lines, params.parts);
        let accumulator = kernel::split::SplitAccumulator::new_quotas(quotas)
            .map_err(|error| format!("E1002: {}", error.message))?;
        let layout = split_layout_for(&file, accumulator);
        files.push(kernel::split::PlanF01File {
            path: file.path,
            canonical_path: file.canonical_path,
            group_id: valid_or_generated(&file.group_id),
            layout,
        });
    }
    let input = kernel::split::PlanF03Input {
        plan_id: ulid_like_id(),
        parts: params.parts,
        output_dir: params.output_dir,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        existing_targets: Vec::new(),
        files,
    };
    let plan = kernel::split::plan_f03(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_f03(json: &str) -> Result<String, JsValue> {
    plan_f03_core(json).map_err(|error| JsValue::from_str(&error))
}

/// F05 CSV 分片计划（只读）。
pub fn plan_f05_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        basis: serde_json::Value,
        keep_header: bool,
        #[serde(default)]
        delimiter: Option<String>,
        #[serde(default)]
        output_dir: String,
        files: Vec<WebCsvFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let basis = parse_basis(&params.basis)?;
    let explicit = match params.delimiter {
        Some(raw) => Some(
            kernel::csv::Delimiter::parse(&raw)
                .ok_or_else(|| format!("E1002: unknown delimiter: {raw}"))?,
        ),
        None => None,
    };
    let mut files = Vec::with_capacity(params.files.len());
    for file in params.files {
        let inspection = file.inspect(explicit)?;
        if inspection.source_encoding.is_none() {
            return Err("E1002: source encoding must be confirmed before planning".to_string());
        }
        let accumulator = match basis {
            kernel::csv::CsvSplitBasis::Bytes(bytes) => {
                kernel::csv::CsvSplitAccumulator::new_bytes(bytes, params.keep_header)
            }
            kernel::csv::CsvSplitBasis::Rows(rows) => {
                kernel::csv::CsvSplitAccumulator::new_rows(rows, params.keep_header)
            }
        }
        .map_err(|error| format!("E1002: {}", error.message))?;
        let layout = kernel::csv::csv_split_from_bytes(
            &file.data,
            inspection.file_size,
            inspection.delimiter,
            inspection.delimiter_detected,
            inspection.source_encoding,
            None,
            accumulator,
        );
        files.push(kernel::csv::PlanF05File {
            path: file.path,
            canonical_path: file.canonical_path,
            group_id: ulid_like_id(),
            layout,
        });
    }
    let input = kernel::csv::PlanF05Input {
        plan_id: ulid_like_id(),
        basis,
        keep_header: params.keep_header,
        delimiter: explicit,
        output_dir: params.output_dir,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        existing_targets: Vec::new(),
        files,
    };
    let plan = kernel::csv::plan_f05(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_f05(json: &str) -> Result<String, JsValue> {
    plan_f05_core(json).map_err(|error| JsValue::from_str(&error))
}

/// `basis` 的线协议形态：`{"bytes": n}` 或 `{"rows": n}`。
fn parse_basis(value: &serde_json::Value) -> Result<kernel::csv::CsvSplitBasis, String> {
    if let Some(bytes) = value.get("bytes").and_then(serde_json::Value::as_u64) {
        return Ok(kernel::csv::CsvSplitBasis::Bytes(bytes));
    }
    if let Some(rows) = value.get("rows").and_then(serde_json::Value::as_u64) {
        return Ok(kernel::csv::CsvSplitBasis::Rows(rows));
    }
    Err("E1002: basis needs exactly one of bytes or rows".to_string())
}

// ── A2-5：C07/C08/F10/F11/F13（ADR-0102 第五批·收尾）───────────────────────
//
// 这五个功能的事实**不依赖 kernel 算法下沉**——它们要么由调用方直接给定
// （C07 的清单行、F11 的目录行、F13 的规则），要么前端可从 File API 采集
// （C08 的 name/size/lastModified）。采不到的字段（目录遍历计数、目标占用）
// 如实置空，不伪造。

/// 清单同步的逐行事实（C07）。Web 侧只能从已选文件推断 exists/size；
/// `target_occupied` 无目标 FS 可查，恒 false。
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebC07Fact {
    pub exists: bool,
    pub size: u64,
}

/// C07 清单同步计划（只读）。
pub fn plan_c07_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        operation: String,
        missing: String,
        source_root: String,
        target_root: String,
        rows: Vec<WebC07Row>,
        #[serde(default)]
        facts: Vec<WebC07Fact>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let operation = match params.operation.as_str() {
        "move" => kernel::c07::C07Operation::Move,
        "copy" => kernel::c07::C07Operation::Copy,
        other => return Err(format!("E1002: unknown operation: {other}")),
    };
    let missing = match params.missing.as_str() {
        "skip" => kernel::c07::MissingPolicy::Skip,
        "error" => kernel::c07::MissingPolicy::Error,
        other => return Err(format!("E1002: unknown missing policy: {other}")),
    };
    let facts = params
        .rows
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let fact = params.facts.get(index).copied().unwrap_or(WebC07Fact {
                exists: false,
                size: 0,
            });
            kernel::c07::PlanC07Fact {
                exists: fact.exists,
                size: fact.size,
                // 浏览器内无目标文件系统可查（不写用户磁盘）。
                target_occupied: false,
            }
        })
        .collect();
    let input = kernel::c07::PlanC07Input {
        plan_id: ulid_like_id(),
        operation,
        missing,
        source_root: params.source_root,
        target_root: params.target_root,
        rows: params
            .rows
            .into_iter()
            .map(|row| kernel::c07::PlanC07Row {
                name: row.name,
                sub_dir: row.sub_dir,
            })
            .collect(),
        facts,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        long_path_supported: true,
    };
    let plan = kernel::c07::plan_c07(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebC07Row {
    pub name: String,
    pub sub_dir: String,
}

#[wasm_bindgen]
pub fn plan_c07(json: &str) -> Result<String, JsValue> {
    plan_c07_core(json).map_err(|error| JsValue::from_str(&error))
}

/// C07 清单解析（编码感知）——浏览器只读得到字节，解析必须走内核。
///
/// 三个形态共用 `kernel::c07::parse_manifest`：编码检测/显式编码、分隔符嗅探、
/// 引号与换行处理、两列校验都在内核里，前端另写一套 CSV 与编码处理必然漂移
/// （GB18030、BOM、引号内换行任一处不一致，用户就会看到「桌面能识别、浏览器不能」）。
///
/// `source` 不只是可选优化：中英混排的文件名清单会被统计检测猜成候选集外的编码
/// （E3001），用户必须能指定编码。
pub fn parse_c07_manifest_core(
    data: &[u8],
    delimiter: Option<&str>,
    source: Option<&str>,
) -> Result<String, String> {
    let delimiter = match delimiter {
        Some(value) => Some(
            kernel::csv::Delimiter::parse(value)
                .ok_or_else(|| format!("E1002: unknown delimiter: {value}"))?,
        ),
        None => None,
    };
    let source = match source {
        Some(value) => Some(
            kernel::encoding::SourceEncoding::parse(value)
                .ok_or_else(|| format!("E1002: unknown source encoding: {value}"))?,
        ),
        None => None,
    };
    let rows = kernel::c07::parse_manifest(data, delimiter, source)
        .map_err(|error| format!("{}: {}", error.code, error.message))?
        .into_iter()
        .map(|row| WebC07Row {
            name: row.name,
            sub_dir: row.sub_dir,
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&rows).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn parse_c07_manifest(
    data: &[u8],
    delimiter: Option<String>,
    source: Option<String>,
) -> Result<String, JsValue> {
    parse_c07_manifest_core(data, delimiter.as_deref(), source.as_deref())
        .map_err(|error| JsValue::from_str(&error))
}

/// C08 归档条件（线协议形态，与内核 serde 同形）。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "kind", content = "value")]
pub enum WebArchiveCondition {
    Extensions(Vec<String>),
    ModifiedDateRange {
        #[serde(default)]
        from: Option<String>,
        #[serde(default)]
        to: Option<String>,
    },
    SizeRange {
        #[serde(default)]
        min: Option<u64>,
        #[serde(default)]
        max: Option<u64>,
    },
    NameKeyword(String),
}

impl From<WebArchiveCondition> for kernel::c08::ArchiveCondition {
    fn from(value: WebArchiveCondition) -> Self {
        match value {
            WebArchiveCondition::Extensions(list) => Self::Extensions(list),
            WebArchiveCondition::ModifiedDateRange { from, to } => {
                Self::ModifiedDateRange { from, to }
            }
            WebArchiveCondition::SizeRange { min, max } => Self::SizeRange { min, max },
            WebArchiveCondition::NameKeyword(keyword) => Self::NameKeyword(keyword),
        }
    }
}

/// C08 归档计划（只读）。文件事实来自 `platform::listing` 同形的字段。
pub fn plan_c08_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        source_root: String,
        target_root: String,
        rules: Vec<WebArchiveRule>,
        unmatched: String,
        files: Vec<WebC08File>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let unmatched = match params.unmatched.as_str() {
        "keep" => kernel::c08::UnmatchedPolicy::Keep,
        other => {
            // `move:<dir>` 形态：未命中项落入指定子目录。
            if let Some(dir) = other.strip_prefix("move:") {
                kernel::c08::UnmatchedPolicy::MoveTo(dir.to_string())
            } else {
                return Err(format!("E1002: unknown unmatched policy: {other}"));
            }
        }
    };
    let input = kernel::c08::PlanC08Input {
        plan_id: ulid_like_id(),
        source_root: params.source_root,
        target_root: params.target_root,
        rules: params
            .rules
            .into_iter()
            .map(|rule| kernel::c08::ArchiveRule {
                sub_dir: rule.sub_dir,
                condition: rule.condition.into(),
            })
            .collect(),
        unmatched,
        files: params
            .files
            .into_iter()
            .map(|file| kernel::c08::PlanC08File {
                rel_path: file.rel_path,
                size: file.size,
                modified_date: file.modified_date,
                exists: file.exists,
            })
            .collect(),
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        long_path_supported: true,
    };
    let plan = kernel::c08::plan_c08(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebArchiveRule {
    pub sub_dir: String,
    pub condition: WebArchiveCondition,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebC08File {
    pub rel_path: String,
    pub size: u64,
    #[serde(default)]
    pub modified_date: Option<String>,
    #[serde(default = "default_true")]
    pub exists: bool,
}

#[wasm_bindgen]
pub fn plan_c08(json: &str) -> Result<String, JsValue> {
    plan_c08_core(json).map_err(|error| JsValue::from_str(&error))
}

/// F10 目录清单计划（只读）。
///
/// **能力边界**：`files/dirs/placeholders/skipped/max_depth` 来自**目录遍历**，
/// 浏览器 File API 只给选中文件，采不到目录树。Web 侧如实置 0／由调用方给，
/// 计划仍可按根目录产出条目（预览口径与桌面不同，已在 ADR 记明）。
pub fn plan_f10_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        #[serde(default)]
        roots: Vec<WebF10Root>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let input = kernel::listing::PlanF10Input {
        plan_id: ulid_like_id(),
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        files: params
            .roots
            .into_iter()
            .map(|root| kernel::listing::PlanF10File {
                root: root.path,
                canonical_root: root.canonical_path,
                target: root.target,
                target_occupied: false,
                files: root.files,
                dirs: root.dirs,
                placeholders: root.placeholders,
                skipped: root.skipped,
                max_depth: root.max_depth,
            })
            .collect(),
    };
    let plan = kernel::listing::plan_f10(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebF10Root {
    pub path: String,
    #[serde(default)]
    pub canonical_path: Option<String>,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub dirs: u64,
    #[serde(default)]
    pub placeholders: u64,
    #[serde(default)]
    pub skipped: u64,
    #[serde(default)]
    pub max_depth: u32,
}

#[wasm_bindgen]
pub fn plan_f10(json: &str) -> Result<String, JsValue> {
    plan_f10_core(json).map_err(|error| JsValue::from_str(&error))
}

/// F11 批量建目录计划（只读）。
pub fn plan_f11_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        root: String,
        #[serde(default)]
        canonical_root: Option<String>,
        rows: Vec<WebF11Row>,
        /// 目标名合法性规则缺省＝POSIX（与 F12/F13 同口径）。
        #[serde(default)]
        name_rules: Option<kernel::conflict::OwnedNameRules>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let input = kernel::create_dirs::PlanF11Input {
        plan_id: ulid_like_id(),
        root: params.root,
        canonical_root: params.canonical_root,
        rows: params
            .rows
            .into_iter()
            .map(|row| kernel::create_dirs::PlanF11Row {
                line: row.line,
                path: row.path,
            })
            .collect(),
        // 浏览器内无目标 FS 可探测；冲突判定因此退化为「本批次内比较」。
        existing_targets: Vec::new(),
        existing_files: Vec::new(),
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        name_rules: params
            .name_rules
            .unwrap_or_else(kernel::conflict::OwnedNameRules::posix)
            .into(),
    };
    let plan = kernel::create_dirs::plan_f11(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebF11Row {
    pub line: u64,
    pub path: String,
}

#[wasm_bindgen]
pub fn plan_f11(json: &str) -> Result<String, JsValue> {
    plan_f11_core(json).map_err(|error| JsValue::from_str(&error))
}

/// F13 正则改名计划（只读）。复用 F12 的项形态（路径＋当前名）。
pub fn plan_f13_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        items: Vec<kernel::rename::WebPlanF12Item>,
        rule: WebF13RulePayload,
        #[serde(default)]
        name_rules: Option<kernel::conflict::OwnedNameRules>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let items = params
        .items
        .into_iter()
        .map(|item| {
            let name = std::path::Path::new(&item.path)
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            kernel::rename::PlanF12Item {
                path: item.path,
                name,
            }
        })
        .collect();
    let input = kernel::regex_rename::PlanF13Input {
        plan_id: ulid_like_id(),
        items,
        existing_targets: Vec::new(),
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        long_path_supported: true,
        access_facts: Vec::new(),
        name_rules: params
            .name_rules
            .unwrap_or_else(kernel::conflict::OwnedNameRules::posix)
            .into(),
        rule: kernel::regex_rename::PlanF13Rule {
            pattern: params.rule.pattern,
            replacement: params.rule.replacement,
            scope: kernel::rename::RenameScope::parse(&params.rule.scope)
                .ok_or_else(|| format!("E1002: unknown rename scope: {}", params.rule.scope))?,
        },
    };
    let plan = kernel::regex_rename::plan_f13(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebF13RulePayload {
    pub pattern: String,
    pub replacement: String,
    pub scope: String,
}

#[wasm_bindgen]
pub fn plan_f13(json: &str) -> Result<String, JsValue> {
    plan_f13_core(json).map_err(|error| JsValue::from_str(&error))
}

// ── A2-6：C04 / F04 / C13 / C09（ADR-0102 五批外补做）───────────────────────
//
// C10 不在此列：其输入是**压缩包内条目**，浏览器无解压能力，采不到（见台账）。

/// C04 名称替换计划（只读）。复用 F12 的项形态。
pub fn plan_c04_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        items: Vec<kernel::rename::WebPlanF12Item>,
        rule: WebC04Rule,
        #[serde(default)]
        name_rules: Option<kernel::conflict::OwnedNameRules>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let items = params
        .items
        .into_iter()
        .map(|item| {
            let name = std::path::Path::new(&item.path)
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            kernel::rename::PlanF12Item {
                path: item.path,
                name,
            }
        })
        .collect();
    let scope = kernel::rename::RenameScope::parse(&params.rule.scope)
        .ok_or_else(|| format!("E1002: unknown rename scope: {}", params.rule.scope))?;
    let input = kernel::name_replace::PlanC04Input {
        plan_id: ulid_like_id(),
        items,
        existing_targets: Vec::new(),
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        long_path_supported: true,
        access_facts: Vec::new(),
        name_rules: params
            .name_rules
            .unwrap_or_else(kernel::conflict::OwnedNameRules::posix)
            .into(),
        rule: kernel::name_replace::PlanC04Rule {
            mode: kernel::name_replace::C04Mode::parse(&params.rule.mode)
                .ok_or_else(|| format!("E1002: unknown c04 mode: {}", params.rule.mode))?,
            find: params.rule.find,
            replace: params.rule.replace,
            scope,
        },
    };
    let plan = kernel::name_replace::plan_c04(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebC04Rule {
    pub mode: String,
    pub find: String,
    pub replace: String,
    pub scope: String,
}

#[wasm_bindgen]
pub fn plan_c04(json: &str) -> Result<String, JsValue> {
    plan_c04_core(json).map_err(|error| JsValue::from_str(&error))
}

/// F04 文本合并计划（只读）。与 F06 同族，只是 N:1 单输出。
pub fn plan_f04_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        target: String,
        target_encoding: String,
        invalid: Option<String>,
        files: Vec<WebMergeFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let target = kernel::encoding::TargetEncoding::parse(&params.target_encoding)
        .ok_or_else(|| format!("E1002: unknown target encoding: {}", params.target_encoding))?;
    let invalid = parse_or_error(
        params.invalid.as_deref(),
        kernel::encoding::InvalidSequencePolicy::parse,
        "invalid-sequence policy",
    )?
    .unwrap_or(kernel::encoding::InvalidSequencePolicy::ReplaceAndCount);
    let mut files = Vec::with_capacity(params.files.len());
    for file in params.files {
        // 采样用于编码判定（Web 侧读到的是原始字节）。
        let sample: Vec<u8> = file.data.iter().take(file.data.len()).copied().collect();
        let detection = kernel::encoding::detect(&sample, kernel::encoding::PlatformPrior::Other);
        let source_encoding = detection.require_encoding().ok();
        files.push(kernel::merge::PlanF04File {
            path: file.path,
            canonical_path: file.canonical_path,
            file_size: file.file_size,
            source_encoding,
        });
    }
    let input = kernel::merge::PlanF04Input {
        plan_id: ulid_like_id(),
        target: params.target,
        target_encoding: target,
        invalid,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        target_occupied: false,
        files,
    };
    let plan = kernel::merge::plan_f04(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebMergeFile {
    pub path: String,
    #[serde(default)]
    pub canonical_path: Option<String>,
    pub file_size: u64,
    pub data: Vec<u8>,
}

#[wasm_bindgen]
pub fn plan_f04(json: &str) -> Result<String, JsValue> {
    plan_f04_core(json).map_err(|error| JsValue::from_str(&error))
}

/// C13 CSV 排序计划（只读）。键列取值走下沉后的 `kernel::csv_sort::key_values`。
pub fn plan_c13_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        keys: Vec<WebColumnPick>,
        order: String,
        #[serde(default)]
        external_chunk_rows: Option<u64>,
        target_encoding: String,
        invalid: Option<String>,
        #[serde(default)]
        output_dir: Option<String>,
        files: Vec<WebCsvFile>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let keys = params
        .keys
        .into_iter()
        .map(WebColumnPick::into_kernel)
        .collect::<Result<Vec<_>, _>>()?;
    let order = kernel::csv_sort::SortOrder::parse(&params.order)
        .ok_or_else(|| format!("E1002: unknown sort order: {}", params.order))?;
    let target = kernel::encoding::TargetEncoding::parse(&params.target_encoding)
        .ok_or_else(|| format!("E1002: unknown target encoding: {}", params.target_encoding))?;
    let invalid = parse_or_error(
        params.invalid.as_deref(),
        kernel::encoding::InvalidSequencePolicy::parse,
        "invalid-sequence policy",
    )?
    .unwrap_or(kernel::encoding::InvalidSequencePolicy::ReplaceAndCount);
    let mut files = Vec::with_capacity(params.files.len());
    let output_dir = params.output_dir;
    for file in params.files {
        let inspection = file.inspect(None)?;
        if inspection.source_encoding.is_none() {
            return Err("E1002: source encoding must be confirmed before planning".to_string());
        }
        let records = kernel::csv_merge::parse_records(&file.data, inspection.delimiter.as_byte());
        let values =
            kernel::csv_sort::key_values(&keys, &inspection.columns, &records, file.has_header);
        let target_path = web_output_path(output_dir.as_deref(), &file.path);
        files.push(kernel::csv_sort::PlanC13File {
            path: file.path,
            canonical_path: file.canonical_path,
            file_size: inspection.file_size,
            source_encoding: inspection.source_encoding,
            delimiter: inspection.delimiter,
            has_header: inspection.has_header,
            columns: inspection.columns,
            column_count: inspection.column_count,
            data_rows: inspection.data_rows,
            dirty_rows: inspection.dirty_rows,
            key_values: values,
            target: target_path,
            target_occupied: file.target_occupied,
        });
    }
    let input = kernel::csv_sort::PlanC13Input {
        plan_id: ulid_like_id(),
        keys,
        order,
        external_chunk_rows: params.external_chunk_rows,
        target_encoding: target,
        invalid,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        files,
    };
    let plan = kernel::csv_sort::plan_c13(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_c13(json: &str) -> Result<String, JsValue> {
    plan_c13_core(json).map_err(|error| JsValue::from_str(&error))
}

/// C09 重复文件清理计划（只读）。
///
/// **能力边界**：`PlanFile.full_hash` 需对文件内容做完整哈希。Web 侧可由调用方
/// 预算后传入（哈希算法须与桌面一致＝blake3）；本入口只做计划，不算哈希。
/// `DuplicateReport` 同理由调用方提供（分组来自三阶段扫描）。
pub fn plan_c09_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        keep: String,
        mode: String,
        hash_algorithm: String,
        #[serde(default)]
        quick_window_bytes: Option<u64>,
        #[serde(default)]
        include_zero_byte: bool,
        files: Vec<WebC09File>,
        report: WebDuplicateReport,
        #[serde(default)]
        skipped_paths: Vec<String>,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let keep = match params.keep.as_str() {
        "earliest" => kernel::plan::KeepPolicy::Earliest,
        "shortest-path" => kernel::plan::KeepPolicy::ShortestPath,
        "newest" => kernel::plan::KeepPolicy::Newest,
        other => return Err(format!("E1002: unknown keep policy: {other}")),
    };
    let mode = match params.mode.as_str() {
        "full" => kernel::duplicate::GroupingMode::Full,
        "fast" => kernel::duplicate::GroupingMode::Fast,
        other => return Err(format!("E1002: unknown grouping mode: {other}")),
    };
    let input = kernel::plan::PlanC09Input {
        plan_id: ulid_like_id(),
        keep,
        mode,
        hash_algorithm: params.hash_algorithm,
        quick_window_bytes: params.quick_window_bytes,
        include_zero_byte: params.include_zero_byte,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        files: params
            .files
            .into_iter()
            .map(|file| kernel::plan::PlanFile {
                id: file.id,
                path: file.path,
                canonical_path: file.canonical_path,
                mtime_nanos: file.mtime_nanos,
                full_hash: file.full_hash,
            })
            .collect(),
        report: kernel::duplicate::DuplicateReport {
            groups: params
                .report
                .groups
                .into_iter()
                .map(|group| kernel::duplicate::DuplicateGroup {
                    size: group.size,
                    hash: group.hash,
                    members: group.members,
                    confidence: if group.confidence == "confirmed" {
                        kernel::duplicate::Confidence::Confirmed
                    } else {
                        kernel::duplicate::Confidence::Candidate
                    },
                    needs_manual_review: group.needs_manual_review,
                })
                .collect(),
            zero_byte_ids: params.report.zero_byte_ids,
            hardlink_skipped: params.report.hardlink_skipped,
            incomplete: params.report.incomplete,
        },
        skipped_paths: params.skipped_paths,
    };
    let plan = kernel::plan::plan_c09(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebC09File {
    pub id: u64,
    pub path: String,
    #[serde(default)]
    pub canonical_path: Option<String>,
    pub mtime_nanos: i128,
    #[serde(default)]
    pub full_hash: Option<Vec<u8>>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDuplicateReport {
    #[serde(default)]
    pub groups: Vec<WebDuplicateGroup>,
    #[serde(default)]
    pub zero_byte_ids: Vec<u64>,
    #[serde(default)]
    pub hardlink_skipped: Vec<u64>,
    #[serde(default)]
    pub incomplete: Vec<u64>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDuplicateGroup {
    pub size: u64,
    #[serde(default)]
    pub hash: Option<Vec<u8>>,
    pub members: Vec<u64>,
    pub confidence: String,
    #[serde(default)]
    pub needs_manual_review: bool,
}

#[wasm_bindgen]
pub fn plan_c09(json: &str) -> Result<String, JsValue> {
    plan_c09_core(json).map_err(|error| JsValue::from_str(&error))
}

/// C10 ZIP 解压计划（只读）。
///
/// **输入事实来自浏览器侧的中央目录读取**（`web/src/lib/web/zipIndex.ts`，ADR-0103）：
/// 只读元数据、不解压任何条目。`entries` 与桌面 `ZipArchiveService::inspect` 逐字段
/// 同口径（`index`/`name`/`kind`/`size`/`compressedSize`/`gbkFallback`/`encrypted`/`supported`）。
///
/// **能力边界**（与桌面的差异，记入 TRACEABILITY）：
/// - `existingTargets` 由调用方提供；Web 侧 OPFS 目录列举尚未实现，缺省为空。
///   于是「目标已存在」不会在计划期预判（桌面会扫目标目录），执行期才会撞；
/// - `protectedDirs` 恒空（浏览器沙箱没有受保护目录概念）；
/// - 目标名规则缺省 POSIX（与 F11/F12/F13 同口径），可由调用方覆盖；
/// - 解压执行（`apply`）在 Web 形态尚不存在（OPFS FsAdapter 未实现，设计 02 §3.1），
///   本入口只出计划，不落盘。
///
/// `maxTotalBytes`/`maxRatio` 与 CLI、桌面同口径**必填**：两者是用户可见的限额，
/// 给隐式默认值等于替用户放行未知解压量。
pub fn plan_c10_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        archive: String,
        archive_size: u64,
        entries: Vec<WebArchiveEntry>,
        output_dir: String,
        max_total_bytes: u64,
        max_ratio: f64,
        #[serde(default)]
        existing_targets: Vec<String>,
        #[serde(default)]
        name_rules: Option<kernel::conflict::OwnedNameRules>,
        /// 执行期是否会提供密码（不是密码本身，ADR-0074）。
        #[serde(default)]
        password_provided: bool,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let mut entries = Vec::with_capacity(params.entries.len());
    for entry in params.entries {
        // `supported` 刻意不给默认值：缺字段时静默当成「不支持」会把整包条目悄悄跳过，
        // 产出一个看起来正常、实际什么都没选中的计划。
        let kind = kernel::archive::ArchiveEntryKind::parse(&entry.kind)
            .ok_or_else(|| format!("E1002: unknown archive entry kind: {}", entry.kind))?;
        entries.push(kernel::archive::ArchiveEntryInfo {
            index: entry.index,
            name: entry.name,
            kind,
            size: entry.size,
            compressed_size: entry.compressed_size,
            gbk_fallback: entry.gbk_fallback,
            encrypted: entry.encrypted,
            supported: entry.supported,
        });
    }
    let input = kernel::archive::PlanC10Input {
        plan_id: ulid_like_id(),
        archive: params.archive,
        archive_size: params.archive_size,
        entries,
        output_dir: params.output_dir,
        max_total_bytes: params.max_total_bytes,
        max_ratio: params.max_ratio,
        existing_targets: params.existing_targets,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
        name_rules: params
            .name_rules
            .unwrap_or_else(kernel::conflict::OwnedNameRules::posix)
            .into(),
        password_provided: params.password_provided,
    };
    let plan = kernel::archive::plan_c10(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

/// C10 ZIP **压缩**计划（只读）。
///
/// 与解压侧的分工：解压的条目事实来自中央目录（`zipIndex.ts`），压缩的条目事实
/// 来自**用户选中的文件**（`sources` ＋ `sourceFileCount`/`sourceBytes` 由浏览器
/// 统计）。内核只按这些事实出一份计划（单条 `archive` 条目），**不打包任何内容**：
/// `apply` 在 Web 形态仍不存在，本入口也不例外。
///
/// **能力边界**（记入 TRACEABILITY）：
/// - `targetExists` 恒 false——浏览器没有产物文件系统可查，计划因此不会预判
///   「目标已存在」；执行前须由用户自行确认（界面在输出字段给提示）；
/// - `sources` 是选中文件的相对路径（单文件选择时即文件名）。浏览器拿不到目录树，
///   所以「整目录压缩」不在 Web 能力内，只有逐个选中的文件可作源；
/// - `protectedDirs` 恒空、`caseSense` 取 POSIX（与解压侧同口径）。
///
/// `sourceFileCount`/`sourceBytes` **必填**：缺省成 0 会让计划显示「压缩 0 个文件、
/// 0 字节」——看着正常、实际与用户选的文件无关（同 `supported` 不加默认值的理由）。
pub fn plan_c10_compress_core(json: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        output: String,
        #[serde(default)]
        sources: Vec<String>,
        #[serde(default)]
        compression: Option<String>,
        #[serde(default)]
        source_file_count: Option<u64>,
        #[serde(default)]
        source_bytes: Option<u64>,
        #[serde(default)]
        target_exists: bool,
    }
    let params: Params = serde_json::from_str(json).map_err(|error| format!("E1002: {error}"))?;
    let compression = match params.compression.as_deref() {
        // 与 CLI 缺省一致（`--compression` 默认 deflate）。
        None => kernel::archive::ArchiveCompression::Deflate,
        Some(value) => kernel::archive::ArchiveCompression::parse(value)
            .ok_or_else(|| format!("E1002: unknown compression: {value}"))?,
    };
    let (Some(source_file_count), Some(source_bytes)) =
        (params.source_file_count, params.source_bytes)
    else {
        return Err("E1001: c10 compression needs sourceFileCount and sourceBytes".to_string());
    };
    let input = kernel::archive::PlanC10CompressInput {
        plan_id: ulid_like_id(),
        output: params.output,
        sources: params.sources,
        compression,
        target_exists: params.target_exists,
        source_file_count,
        source_bytes,
        protected_dirs: Vec::new(),
        case_sense: kernel::fs::CaseSense::Sensitive,
    };
    let plan = kernel::archive::plan_c10_compress(input)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    serde_json::to_string(&plan).map_err(|error| format!("E1002: {error}"))
}

#[wasm_bindgen]
pub fn plan_c10_compress(json: &str) -> Result<String, JsValue> {
    plan_c10_compress_core(json).map_err(|error| JsValue::from_str(&error))
}

/// 中央目录条目事实（线协议，与 `ZipEntryFact` 同形）。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebArchiveEntry {
    pub index: u64,
    /// 解码失败（含浏览器有损解码出 U+FFFD）时为 `null`，内核会记入 `unsafeNames`。
    #[serde(default)]
    pub name: Option<String>,
    pub kind: String,
    pub size: u64,
    pub compressed_size: u64,
    pub gbk_fallback: bool,
    pub encrypted: bool,
    pub supported: bool,
}

#[wasm_bindgen]
pub fn plan_c10(json: &str) -> Result<String, JsValue> {
    plan_c10_core(json).map_err(|error| JsValue::from_str(&error))
}

// ── Web 执行面（ADR-0108）────────────────────────────────────────────────────
//
// 与 `plan_*` 的区别：这些入口**会改变沙箱状态**（登记输入、落盘产物、撤销），
// 但仍然不碰用户磁盘——产物以字节形式交回主线程下载。设计 05 §2 允许导出
// apply/undo 与产物读取入口；沙箱自身不含任何授权/配额判定。

pub mod archive_mem;
pub mod exec;
pub mod listing_mem;

use std::sync::Mutex;

use kernel::error::KernelError;

/// 会话沙箱：Worker 生命周期内一份；`web_exec_reset` 或换输入时清空。
static SANDBOX: Mutex<Option<exec::Sandbox>> = Mutex::new(None);

fn with_sandbox<T>(
    run: impl FnOnce(&mut exec::Sandbox) -> Result<T, KernelError>,
) -> Result<T, String> {
    let mut guard = match SANDBOX.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let sandbox = guard.get_or_insert_with(exec::Sandbox::new);
    run(sandbox).map_err(|error| format!("{}: {}", error.code, error.message))
}

/// 登记输入字节（`manifest` = `[{path,length}]`，`data` 为同序拼接的字节）。
#[wasm_bindgen]
pub fn web_exec_stage(manifest: &str, data: &[u8]) -> Result<String, JsValue> {
    with_sandbox(|sandbox| sandbox.stage(manifest, data)).map_err(|error| JsValue::from_str(&error))
}

/// 执行计划（请求 JSON = `{plan, options?, now?, password?}`）。
///
/// `password` 只对 C10 有意义，且是**执行期秘密**（ADR-0074）：计划里只有
/// `passwordProvided` 这个事实，密码本身只随这一跳进来。
#[wasm_bindgen]
pub fn web_exec_apply(request: &str) -> Result<String, JsValue> {
    with_sandbox(|sandbox| sandbox.apply(request)).map_err(|error| JsValue::from_str(&error))
}

/// 只登记目录（F10 的清单根；`manifest` = `[{path}]`，无字节）。
#[wasm_bindgen]
pub fn web_exec_stage_dirs(manifest: &str) -> Result<String, JsValue> {
    with_sandbox(|sandbox| sandbox.stage_dirs(manifest)).map_err(|error| JsValue::from_str(&error))
}

/// 登记 F10 的目录事实（`[{root, files:[{path,size,mtime}]}]`）。
#[wasm_bindgen]
pub fn web_exec_stage_listing(json: &str) -> Result<String, JsValue> {
    with_sandbox(|sandbox| sandbox.stage_listing(json)).map_err(|error| JsValue::from_str(&error))
}

/// 撤销计划（只回滚沙箱内产物）。
#[wasm_bindgen]
pub fn web_exec_undo(plan_id: &str) -> Result<String, JsValue> {
    with_sandbox(|sandbox| {
        let outcome = sandbox.undo(plan_id)?;
        serde_json::to_string(&outcome)
            .map_err(|error| KernelError::new("E9001", error.to_string(), None, false))
    })
    .map_err(|error| JsValue::from_str(&error))
}

/// 读产物字节（`content` 是 `Uint8Array`）。
#[wasm_bindgen]
pub fn web_exec_product(path: &str) -> Result<Vec<u8>, JsValue> {
    with_sandbox(|sandbox| sandbox.product(path)).map_err(|error| JsValue::from_str(&error))
}

/// 清空沙箱（换视图／换输入时调用）。
#[wasm_bindgen]
pub fn web_exec_reset() -> String {
    if let Ok(mut guard) = SANDBOX.lock() {
        if let Some(sandbox) = guard.as_mut() {
            sandbox.reset();
        }
    }
    "{\"reset\":true}".to_string()
}
