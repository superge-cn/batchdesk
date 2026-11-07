//! 处理模板/配方（C37）的类型、迁移与导入校验（设计 03 §5／§6）。
//!
//! 纯函数层：不读写磁盘、不取时钟；`createdAt`／`updatedAt` 由调用方传入。
//! 模板只保存声明式参数与适用扩展名，不保存脚本/宏/凭据/文件内容/绝对源路径
//! （方案 §15、AGENTS 不变量 13）。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::KernelError;
use crate::plan::PlanAction;

pub const CURRENT_RECIPE_SCHEMA_VERSION: u32 = 5;

/// 多步骤上限沿用 F15 单束上限（方案 §15.1，不新增数值）。
pub const MAX_RECIPE_STEPS: usize = 10;

/// 导入/保存时必须剔除的路径类参数（模板不得保存源路径/输出目录，方案 §15.2.4）。
/// 键名统一按小写登记，匹配时先把参数键小写化（大小写不敏感）。
///
/// 名单按**本仓库 IPC 参数名的真实语义**登记（2026-11-05 校正）：
/// - `source` 曾是名单成员，但它在 F04/F06/F07/F08/C01/C11–C14 里是**源编码**
///   （CLI 的 `--source`），删掉它等于让导出的模板丢了编码；同理 `target` 是**目标编码**，
///   不能因为名字像路径就进表。路径在那些功能里叫 `output`／`files`。
/// - 反过来，UI 与 CLI 真正用到的 `file`／`files`／`sources`／`roots`／`manifest`／
///   `list`／`archive`／`dir` 过去**不在**表里——于是导入一份自家 UI 存的模板时，
///   绝对路径会原样留在参数里，违反不变量 13。
///
/// 只做精确（小写后）匹配，所以 `targetBytes`／`targetEncoding`／`sourcesColumn`
/// 这类含路径词的正常参数不会被误伤。
pub const PATH_PARAM_KEYS: &[&str] = &[
    // 单文件源
    "file",
    "path",
    "sourcepath",
    // 多文件源
    "files",
    "paths",
    "sourcepaths",
    "sources",
    // 目录/根
    "dir",
    "directory",
    "folder",
    "root",
    "roots",
    "sourceroot",
    "targetroot",
    "unclassifieddir",
    // 输出
    "output",
    "outputdir",
    "outputdirectory",
    // 清单/压缩包（路径而非内容）
    "list",
    "manifest",
    "archive",
];

/// 导入时直接拒绝的脚本类参数（模板不保存脚本/宏/命令，AGENTS 不变量 13）。
pub const SCRIPT_PARAM_KEYS: &[&str] = &["script", "macro", "command", "exec"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepSource {
    Original,
    Previous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecipeFormat {
    Desktop,
    Web,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateStep {
    pub step_id: String,
    pub kind: PlanAction,
    pub params_snapshot: Map<String, Value>,
    pub source: StepSource,
    /// 该步绑定的**功能码**（`f01`／`c09`／`c10`…），v4 新增（OQ-36 裁决①）。
    ///
    /// 为什么必须有它：`kind` 是 `PlanAction`，而壳层 `planOperation` 按功能码路由，
    /// 一个动作对多个功能码（`split`＝f01/f02/f03/f05、`edit-content`＝c01/c02/c03、
    /// `rename`＝f12/f13/c04、`merge`＝f04/f06、`csv-rows`＝f08/f09、`move`＝c07/c08），
    /// 25 个功能码里 16 个反推不出来——没有这个字段，模板永远不知道该回填到哪一页。
    /// `None`＝该步**不可套用**（旧 v3 里一对多动作迁移后的状态），不是"绑定到了通用出口"。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
    /// 采样类技术口径（v5 新增，OQ-41 裁决②的附带决定：**进模板、不进快照**）。
    ///
    /// `params_snapshot` 保持「计划自带那份」（ADR-0072）不动——检测采样窗口/字节是
    /// **请求侧**的技术值，计划参数里从不携带（实测 plan_c01/c02/c03 只写 target/invalid/
    /// platformPrior 等），套用时路由又必填它们。v5 起保存时把界面当前值存进这块；
    /// `None`（v4 及更早模板）＝套用通道按映射表的缺省补（见壳层 `FEATURE_PARAM_MAP`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling: Option<RecipeSampling>,
}

/// 采样类技术口径的可选存档（v5，OQ-41 裁决）。三个键都可选：只存该功能
/// 实际用到的那几个（c01 记 `sampleWindow`、c02/c03 记 `sampleBytes`），
/// 不为没用的键造 null。值必须 ≥1（0 无意义，validate 拒绝）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct RecipeSampling {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_blocks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_bytes: Option<u64>,
}

/// 动作→功能码**一对一**的表：只登记唯一对应项，迁移时用来自动补 `feature`。
///
/// 依据（2026-11-06 逐模块实测）：`crates/kernel/src` 里各动作的计划生成模块与
/// `src-tauri/src/lib.rs` 的 `planOperation` 路由。`archive` 与 `extract` 都只出自
/// `archive.rs`（c10），`copy` 只出自 `c07.rs`；`transcode` 没有任何计划生成方使用，
/// 故不登记。**故意不完整**：六个一对多动作族必须留在 `None`，补全＝猜功能。
pub fn feature_for_action(action: PlanAction) -> Option<&'static str> {
    match action {
        PlanAction::Delete => Some("c09"),
        PlanAction::CsvExtract => Some("f07"),
        PlanAction::Listing => Some("f10"),
        PlanAction::CreateDir => Some("f11"),
        PlanAction::Archive => Some("c10"),
        PlanAction::Extract => Some("c10"),
        PlanAction::CsvDedup => Some("c11"),
        PlanAction::CsvFilter => Some("c12"),
        PlanAction::CsvSort => Some("c13"),
        PlanAction::CsvColumns => Some("c14"),
        PlanAction::Copy => Some("c07"),
        _ => None,
    }
}

impl TemplateStep {
    /// 该步能否被「一键套用」：有功能码绑定才算能——判据只有一条，
    /// 不做参数键猜测（方案 §15.2.5：应用模板只回填参数并调用 `planOperation`）。
    pub fn is_applicable(&self) -> bool {
        self.feature
            .as_deref()
            .map(|feature| !feature.trim().is_empty())
            .unwrap_or(false)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct RecipeMatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipeDocumentV2 {
    pub recipe_id: String,
    pub schema_version: u32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub kind: PlanAction,
    pub params_snapshot: Map<String, Value>,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub match_: Option<RecipeMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_format: Option<RecipeFormat>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipeDocument {
    pub recipe_id: String,
    pub schema_version: u32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub steps: Vec<TemplateStep>,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub match_: Option<RecipeMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_format: Option<RecipeFormat>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
}

impl RecipeDocument {
    /// 单步模板的入口（Phase A 兼容：kind + paramsSnapshot → steps[0]）。
    pub fn from_single_step(
        recipe_id: impl Into<String>,
        name: impl Into<String>,
        kind: PlanAction,
        params_snapshot: Map<String, Value>,
        created_at: impl Into<String>,
        feature: Option<String>,
    ) -> Self {
        Self {
            recipe_id: recipe_id.into(),
            schema_version: CURRENT_RECIPE_SCHEMA_VERSION,
            name: name.into(),
            description: None,
            steps: vec![TemplateStep {
                step_id: "step-1".to_string(),
                kind,
                params_snapshot,
                source: StepSource::Original,
                feature,
                sampling: None,
            }],
            match_: None,
            source_format: None,
            created_at: created_at.into(),
            updated_at: None,
            last_used_at: None,
            usage_count: None,
            pinned: None,
        }
    }

    /// 校验结构约束（步数上限、stepId 唯一、参数不含脚本/路径）。
    pub fn validate(&self) -> Result<(), KernelError> {
        if self.recipe_id.trim().is_empty() {
            return Err(KernelError::missing_param("recipeId is required"));
        }
        if self.name.trim().is_empty() {
            return Err(KernelError::missing_param("recipe name is required"));
        }
        if self.steps.is_empty() {
            return Err(KernelError::invalid_params(
                "recipe must have at least one step",
            ));
        }
        if self.steps.len() > MAX_RECIPE_STEPS {
            return Err(KernelError::invalid_params(format!(
                "recipe has {} steps; the limit is {MAX_RECIPE_STEPS}",
                self.steps.len()
            )));
        }
        let mut seen = std::collections::BTreeSet::new();
        for step in &self.steps {
            if step.step_id.trim().is_empty() {
                return Err(KernelError::missing_param("stepId is required"));
            }
            if !seen.insert(step.step_id.clone()) {
                return Err(KernelError::invalid_params(format!(
                    "duplicate stepId: {}",
                    step.step_id
                )));
            }
            if let Some(feature) = &step.feature {
                if !is_feature_code(feature) {
                    return Err(KernelError::invalid_params(format!(
                        "invalid feature code for step {}: {feature}",
                        step.step_id
                    )));
                }
            }
            if let Some(key) = script_key(&step.params_snapshot) {
                return Err(KernelError::invalid_params(format!(
                    "template parameters must not contain scripts (found {key})"
                )));
            }
            if let Some(sampling) = &step.sampling {
                for (name, value) in [
                    ("sampleWindow", sampling.sample_window),
                    ("sampleBlocks", sampling.sample_blocks),
                    ("sampleBytes", sampling.sample_bytes),
                ] {
                    if value == Some(0) {
                        return Err(KernelError::invalid_params(format!(
                            "sampling {name} must be at least 1 for step {}",
                            step.step_id
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// 逐个步骤剔除路径类参数，返回被剔除的键（去重后按出现顺序）。
    ///
    /// 保存与导入两条路都走它：**模板永不保存路径**是不变量 13 的硬要求，
    /// 只靠调用方（界面）自觉会把"导入一份别人存的模板"变成路径入口。
    pub fn strip_paths(&mut self) -> Vec<String> {
        let mut removed = Vec::new();
        for step in &mut self.steps {
            for key in strip_path_params(&mut step.params_snapshot) {
                if !removed.contains(&key) {
                    removed.push(key);
                }
            }
        }
        removed
    }

    /// v3→v4：把**能唯一确定**的功能码补进步骤；一对多动作保持 `None`。
    /// 已经带 `feature` 的步骤不动它——显式值优先于推断。
    pub fn bind_features_from_actions(&mut self) {
        for step in &mut self.steps {
            if step.feature.is_none() {
                step.feature = feature_for_action(step.kind).map(str::to_string);
            }
        }
    }

    /// 「一键套用」的入口判据：**单步**且该步有功能码（方案 §15.2.5、设计 03 §命令表）。
    /// 不满足就返回 E1002 并说明原因，界面据此不给按钮，而不是给一个按不动的按钮。
    pub fn single_step_binding(&self) -> Result<(String, &TemplateStep), KernelError> {
        let step = self
            .steps
            .first()
            .ok_or_else(|| KernelError::invalid_params("recipe has no step to apply"))?;
        if self.steps.len() > 1 {
            return Err(KernelError::invalid_params(format!(
                "multi-step recipe is not applied in one click ({} steps); plan each step separately",
                self.steps.len()
            )));
        }
        let feature = step.feature.clone().ok_or_else(|| {
            KernelError::invalid_params(format!(
                "recipe step {} has no feature binding; action {} maps to several features, re-save it from that feature page",
                step.step_id, step.kind.as_str()
            ))
        })?;
        if !is_feature_code(&feature) {
            return Err(KernelError::invalid_params(format!(
                "recipe step {} has an invalid feature code",
                step.step_id
            )));
        }
        Ok((feature, step))
    }
}

/// 功能码形态：小写字母＋数字，2–4 字符（`c09`／`f12`／`c14`）。只判形态，
/// 「这个码在不在路由表里」由壳层 `planOperation` 的 `other =>` 分支把关。
fn is_feature_code(value: &str) -> bool {
    let bytes = value.as_bytes();
    (2..=4).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn script_key(params: &Map<String, Value>) -> Option<&str> {
    params
        .keys()
        .find(|key| {
            let lowered = key.to_ascii_lowercase();
            SCRIPT_PARAM_KEYS.contains(&lowered.as_str())
        })
        .map(|key| key.as_str())
}

/// 导入前的路径剔除：移除源路径/输出目录类参数，返回被剔除的键。
pub fn strip_path_params(params: &mut Map<String, Value>) -> Vec<String> {
    let mut removed = Vec::new();
    let keys: Vec<String> = params.keys().cloned().collect();
    for key in keys {
        let lowered = key.to_ascii_lowercase();
        if PATH_PARAM_KEYS.contains(&lowered.as_str()) {
            params.remove(&key);
            removed.push(key);
        }
    }
    removed
}

/// 读取任意版本的模板 JSON 并迁移到当前版本（设计 03 §6.1-5）。
///
/// v3→v4 的迁移是**补 `feature`**：一对多动作留 `None`（不可套用），
/// 一对一动作按 `feature_for_action` 自动绑定——迁移不猜功能码。
///
/// v4→v5 没有可补的字段：`sampling` 从 v5 起才在保存时采集，旧模板留 `None`，
/// 套用时由壳层映射表按该功能的缺省补（迁移不猜采样值）。
///
/// 低版本（1/2）迁移；高于 `CURRENT_RECIPE_SCHEMA_VERSION` → E1007（不猜测语义）。
pub fn migrate_recipe_value(payload: Value) -> Result<RecipeDocument, KernelError> {
    let version = payload
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| KernelError::missing_param("schemaVersion is required"))?;
    let version = version as u32;
    if version > CURRENT_RECIPE_SCHEMA_VERSION {
        return Err(KernelError::new(
            "E1007",
            format!(
                "recipe schemaVersion {version} is newer than the supported {CURRENT_RECIPE_SCHEMA_VERSION}"
            ),
            None,
            false,
        ));
    }
    match version {
        1 => {
            let v1: RecipeV1 = serde_json::from_value(payload).map_err(recipe_parse_error)?;
            Ok(v1.into_current())
        }
        2 => {
            let v2: RecipeDocumentV2 =
                serde_json::from_value(payload).map_err(recipe_parse_error)?;
            Ok(v2.into_current())
        }
        3 => {
            let mut v3: RecipeDocument =
                serde_json::from_value(payload).map_err(recipe_parse_error)?;
            v3.strip_paths();
            v3.bind_features_from_actions();
            v3.schema_version = CURRENT_RECIPE_SCHEMA_VERSION;
            Ok(v3)
        }
        4 => {
            let mut v4: RecipeDocument =
                serde_json::from_value(payload).map_err(recipe_parse_error)?;
            v4.strip_paths();
            v4.schema_version = CURRENT_RECIPE_SCHEMA_VERSION;
            Ok(v4)
        }
        CURRENT_RECIPE_SCHEMA_VERSION => {
            let mut current: RecipeDocument =
                serde_json::from_value(payload).map_err(recipe_parse_error)?;
            current.strip_paths();
            Ok(current)
        }
        other => Err(KernelError::new(
            "E1007",
            format!("unsupported recipe schemaVersion {other}"),
            None,
            false,
        )),
    }
}

fn recipe_parse_error(error: serde_json::Error) -> KernelError {
    KernelError::invalid_params(format!("invalid recipe payload: {error}"))
}

/// v1 的最小字段集（迁移输入；迁移只升元数据，不猜测未知字段）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecipeV1 {
    recipe_id: String,
    schema_version: u32,
    name: String,
    kind: PlanAction,
    #[serde(default)]
    params_snapshot: Map<String, Value>,
    created_at: String,
}

impl RecipeV1 {
    fn into_current(mut self) -> RecipeDocument {
        let removed = strip_path_params(&mut self.params_snapshot);
        let _ = removed;
        RecipeDocument {
            recipe_id: self.recipe_id,
            schema_version: CURRENT_RECIPE_SCHEMA_VERSION,
            name: self.name,
            description: None,
            steps: vec![TemplateStep {
                step_id: "step-1".to_string(),
                kind: self.kind,
                params_snapshot: self.params_snapshot,
                source: StepSource::Original,
                feature: feature_for_action(self.kind).map(str::to_string),
                sampling: None,
            }],
            match_: Some(RecipeMatch::default()),
            source_format: None,
            created_at: self.created_at,
            updated_at: None,
            last_used_at: None,
            usage_count: None,
            pinned: None,
        }
    }
}

impl RecipeDocumentV2 {
    fn into_current(self) -> RecipeDocument {
        let mut params = self.params_snapshot;
        strip_path_params(&mut params);
        RecipeDocument {
            recipe_id: self.recipe_id,
            schema_version: CURRENT_RECIPE_SCHEMA_VERSION,
            name: self.name,
            description: self.description,
            steps: vec![TemplateStep {
                step_id: "step-1".to_string(),
                kind: self.kind,
                params_snapshot: params,
                source: StepSource::Original,
                feature: feature_for_action(self.kind).map(str::to_string),
                sampling: None,
            }],
            match_: self.match_,
            source_format: self.source_format,
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_used_at: self.last_used_at,
            usage_count: self.usage_count,
            pinned: self.pinned,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 路径键表的口径：真路径删掉，**编码参数不能跟着删**。
    ///
    /// 2026-11-05 校正：旧表把 `source` 当路径，而它是 F04/F06/C01/C11–C14 的源编码；
    /// 同时 `file`/`files`/`output`/`manifest` 这些 UI 真用的路径键又不在表里。
    #[test]
    fn strip_path_params_drops_real_paths_and_keeps_encodings() {
        let mut params = Map::new();
        params.insert("source".to_string(), json!("gb18030"));
        params.insert("target".to_string(), json!("utf-8-bom"));
        params.insert("targetBytes".to_string(), json!(1024));
        params.insert("delimiter".to_string(), json!(","));
        params.insert("files".to_string(), json!(["/src/a.csv", "/src/b.csv"]));
        params.insert("output".to_string(), json!("/out/merged.csv"));
        params.insert("manifest".to_string(), json!("/src/list.csv"));
        params.insert("unclassifiedDir".to_string(), json!("/src/other"));
        let removed = strip_path_params(&mut params);
        assert_eq!(
            removed,
            vec![
                "files".to_string(),
                "manifest".to_string(),
                "output".to_string(),
                "unclassifiedDir".to_string(),
            ]
        );
        for key in ["source", "target", "targetBytes", "delimiter"] {
            assert!(params.contains_key(key), "{key} 不该被当成路径删掉");
        }
    }

    #[test]
    fn strip_paths_covers_every_step_and_reports_each_key_once() {
        let mut first = Map::new();
        first.insert("file".to_string(), json!("/src/a.txt"));
        first.insert("targetBytes".to_string(), json!(1024));
        let mut last = Map::new();
        last.insert("file".to_string(), json!("/src/b.txt"));
        last.insert("outputDir".to_string(), json!("/out"));
        let mut recipe = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "两步模板",
            PlanAction::Split,
            first,
            "2026-11-05T00:00:00Z",
            None,
        );
        recipe.steps.push(TemplateStep {
            step_id: "step-2".to_string(),
            kind: PlanAction::Transcode,
            params_snapshot: last,
            source: StepSource::Previous,
            feature: None,
            sampling: None,
        });
        assert_eq!(
            recipe.strip_paths(),
            vec!["file".to_string(), "outputDir".to_string()]
        );
        for step in &recipe.steps {
            assert!(step.params_snapshot.get("file").is_none());
            assert!(step.params_snapshot.get("outputDir").is_none());
        }
        assert_eq!(recipe.steps[0].params_snapshot["targetBytes"], json!(1024));
        assert!(recipe.strip_paths().is_empty(), "重复调用不应再报被删键");
    }

    #[test]
    fn v1_recipe_is_wrapped_into_a_single_step() {
        let payload = json!({
            "recipeId": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "schemaVersion": 1,
            "name": "去重",
            "kind": "delete",
            "paramsSnapshot": {"keep": "earliest"},
            "createdAt": "2026-10-29T00:00:00Z"
        });
        let recipe = migrate_recipe_value(payload).unwrap();
        assert_eq!(recipe.schema_version, CURRENT_RECIPE_SCHEMA_VERSION);
        assert_eq!(recipe.steps.len(), 1);
        assert_eq!(recipe.steps[0].source, StepSource::Original);
        assert_eq!(recipe.steps[0].params_snapshot["keep"], json!("earliest"));
        assert_eq!(recipe.match_, Some(RecipeMatch::default()));
    }

    #[test]
    fn v2_recipe_keeps_metadata_and_wraps_params() {
        let payload = json!({
            "recipeId": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "schemaVersion": 2,
            "name": "改名",
            "description": "去掉前缀",
            "kind": "rename",
            "paramsSnapshot": {"pattern": "new-{n}"},
            "match": {"extensions": ["txt"], "tags": ["常用"]},
            "sourceFormat": "desktop",
            "createdAt": "2026-10-29T00:00:00Z",
            "usageCount": 3,
            "pinned": true
        });
        let recipe = migrate_recipe_value(payload).unwrap();
        assert_eq!(recipe.steps.len(), 1);
        assert_eq!(recipe.steps[0].kind, PlanAction::Rename);
        assert_eq!(recipe.steps[0].params_snapshot["pattern"], json!("new-{n}"));
        assert_eq!(recipe.description.as_deref(), Some("去掉前缀"));
        assert_eq!(recipe.usage_count, Some(3));
        assert_eq!(recipe.pinned, Some(true));
        assert_eq!(
            recipe.match_.as_ref().unwrap().extensions.as_deref(),
            Some(&["txt".to_string()][..])
        );
    }

    #[test]
    fn v3_recipe_round_trips_and_strips_paths() {
        let mut step_params = Map::new();
        step_params.insert("pattern".to_string(), json!("x-{n}"));
        step_params.insert("outputDir".to_string(), json!("/tmp/out"));
        let mut recipe = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "模板",
            PlanAction::Rename,
            step_params,
            "2026-10-29T00:00:00Z",
            None,
        );
        let payload = serde_json::to_value(&recipe).unwrap();
        let migrated = migrate_recipe_value(payload).unwrap();
        assert!(migrated.steps[0].params_snapshot.get("outputDir").is_none());
        assert_eq!(migrated.steps[0].params_snapshot["pattern"], json!("x-{n}"));
        recipe.steps[0].step_id = "step-1".to_string();
        assert_eq!(
            strip_path_params(&mut recipe.steps[0].params_snapshot),
            vec!["outputDir".to_string()]
        );
    }

    #[test]
    fn newer_schema_version_is_rejected_with_e1007() {
        let payload = json!({
            "recipeId": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "schemaVersion": CURRENT_RECIPE_SCHEMA_VERSION + 1,
            "name": "x",
            "steps": []
        });
        let error = migrate_recipe_value(payload).unwrap_err();
        assert_eq!(error.code, "E1007");
    }

    #[test]
    fn missing_schema_version_is_e1001() {
        let payload = json!({
            "recipeId": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "name": "x"
        });
        let error = migrate_recipe_value(payload).unwrap_err();
        assert_eq!(error.code, "E1001");
    }

    #[test]
    fn script_parameters_are_rejected() {
        let mut recipe = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "危险模板",
            PlanAction::Rename,
            Map::new(),
            "2026-10-29T00:00:00Z",
            None,
        );
        recipe.steps[0]
            .params_snapshot
            .insert("script".to_string(), json!("rm -rf /"));
        let error = recipe.validate().unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(error.message.contains("script"));
    }

    #[test]
    fn step_count_and_duplicate_ids_are_enforced() {
        let mut recipe = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "十一步",
            PlanAction::Rename,
            Map::new(),
            "2026-10-29T00:00:00Z",
            None,
        );
        let template = recipe.steps[0].clone();
        while recipe.steps.len() < MAX_RECIPE_STEPS {
            let mut step = template.clone();
            step.step_id = format!("step-{}", recipe.steps.len() + 1);
            recipe.steps.push(step);
        }
        recipe.validate().unwrap();
        let mut extra = template.clone();
        extra.step_id = "step-extra".to_string();
        recipe.steps.push(extra);
        let error = recipe.validate().unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(error.message.contains("limit"));

        let mut duplicated = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "重复 id",
            PlanAction::Rename,
            Map::new(),
            "2026-10-29T00:00:00Z",
            None,
        );
        let step = duplicated.steps[0].clone();
        duplicated.steps.push(step);
        let error = duplicated.validate().unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(error.message.contains("duplicate stepId"));
    }

    #[test]
    fn empty_steps_are_rejected() {
        let mut recipe = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "空模板",
            PlanAction::Rename,
            Map::new(),
            "2026-10-29T00:00:00Z",
            None,
        );
        recipe.steps.clear();
        let error = recipe.validate().unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn migration_from_v3_binds_unique_features_and_leaves_many_to_many_unbound() {
        // v3 只带动作：一对一动作自动补功能码，一对多保持 None＝不可套用（不许猜）。
        let payload = json!({
            "recipeId": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "schemaVersion": 3,
            "name": "旧的分片模板",
            "steps": [{
                "stepId": "step-1",
                "kind": "split",
                "paramsSnapshot": {"targetBytes": 1048576},
                "source": "original"
            }],
            "createdAt": "2026-11-05T00:00:00Z"
        });
        let migrated = migrate_recipe_value(payload).unwrap();
        assert_eq!(migrated.schema_version, CURRENT_RECIPE_SCHEMA_VERSION);
        assert_eq!(migrated.steps[0].feature, None);
        assert!(!migrated.steps[0].is_applicable());

        let unique = json!({
            "recipeId": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q9",
            "schemaVersion": 3,
            "name": "旧的清单模板",
            "steps": [{
                "stepId": "step-1",
                "kind": "listing",
                "paramsSnapshot": {},
                "source": "original"
            }],
            "createdAt": "2026-11-05T00:00:00Z"
        });
        assert_eq!(
            migrate_recipe_value(unique).unwrap().steps[0]
                .feature
                .as_deref(),
            Some("f10")
        );
    }

    #[test]
    fn migration_never_overwrites_an_explicit_feature() {
        let payload = json!({
            "recipeId": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8QA",
            "schemaVersion": 3,
            "name": "已经绑好功能的旧模板",
            "steps": [{
                "stepId": "step-1",
                "kind": "delete",
                "paramsSnapshot": {},
                "source": "original",
                "feature": "c09"
            }],
            "createdAt": "2026-11-05T00:00:00Z"
        });
        let doc = migrate_recipe_value(payload).unwrap();
        assert_eq!(doc.steps[0].feature.as_deref(), Some("c09"));
    }

    #[test]
    fn v4_round_trip_keeps_feature_and_omits_the_key_when_absent() {
        let mut params = Map::new();
        params.insert("targetBytes".to_string(), json!(2048));
        let bound = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8QB",
            "按大小分片",
            PlanAction::Split,
            params.clone(),
            "2026-11-06T00:00:00Z",
            Some("f01".to_string()),
        );
        let json = serde_json::to_value(&bound).unwrap();
        assert_eq!(json["steps"][0]["feature"], json!("f01"));
        let back: RecipeDocument = serde_json::from_value(json).unwrap();
        assert_eq!(back, bound);

        let unbound = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8QC",
            "没绑功能的分片",
            PlanAction::Split,
            params,
            "2026-11-06T00:00:00Z",
            None,
        );
        let json = serde_json::to_value(&unbound).unwrap();
        assert!(json["steps"][0].get("feature").is_none());
    }

    #[test]
    fn single_step_binding_is_the_apply_gate() {
        let mut params = Map::new();
        params.insert("find".to_string(), json!("a"));
        let bound = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8QD",
            "改名（已绑定）",
            PlanAction::Rename,
            params.clone(),
            "2026-11-06T00:00:00Z",
            Some("f12".to_string()),
        );
        let (feature, step) = bound.single_step_binding().unwrap();
        assert_eq!(feature, "f12");
        assert_eq!(step.kind, PlanAction::Rename);

        // 一对多动作没绑定 → 报的是「该重存了」，不是含糊的失败。
        let unbound = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8QE",
            "改名（未绑定）",
            PlanAction::Rename,
            params.clone(),
            "2026-11-06T00:00:00Z",
            None,
        );
        let error = unbound.single_step_binding().unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(error.message.contains("re-save"));

        // 多步模板不走一键（方案 §15.2.5 的一键只给单步）。
        let mut multi = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8QF",
            "两步",
            PlanAction::Rename,
            params.clone(),
            "2026-11-06T00:00:00Z",
            Some("f12".to_string()),
        );
        multi.steps.push(TemplateStep {
            step_id: "step-2".to_string(),
            kind: PlanAction::CsvDedup,
            params_snapshot: params,
            source: StepSource::Previous,
            feature: Some("c11".to_string()),
            sampling: None,
        });
        let error = multi.single_step_binding().unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(error.message.contains("multi-step"));
    }

    #[test]
    fn validate_rejects_malformed_feature_codes() {
        let mut recipe = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8QG",
            "坏功能码",
            PlanAction::Split,
            Map::new(),
            "2026-11-06T00:00:00Z",
            Some("F01".to_string()),
        );
        assert_eq!(recipe.validate().unwrap_err().code, "E1002");
        recipe.steps[0].feature = Some("f0123".to_string());
        assert_eq!(recipe.validate().unwrap_err().code, "E1002");
        recipe.steps[0].feature = Some("c09 ".to_string());
        assert_eq!(recipe.validate().unwrap_err().code, "E1002");
        recipe.steps[0].feature = Some("f01".to_string());
        recipe.validate().unwrap();
    }

    #[test]
    fn v5_round_trips_sampling_and_omits_the_block_when_absent() {
        let mut params = Map::new();
        params.insert("target".to_string(), json!("utf-8"));
        let mut bound = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8H",
            "转码模板（带采样）",
            PlanAction::Transcode,
            params.clone(),
            "2026-11-07T00:00:00Z",
            Some("c01".to_string()),
        );
        bound.steps[0].sampling = Some(RecipeSampling {
            sample_window: Some(65536),
            sample_blocks: None,
            sample_bytes: None,
        });
        bound.validate().unwrap();
        let json = serde_json::to_value(&bound).unwrap();
        assert_eq!(json["steps"][0]["sampling"]["sampleWindow"], json!(65536));
        assert!(json["steps"][0]["sampling"].get("sampleBlocks").is_none());
        let back: RecipeDocument = serde_json::from_value(json).unwrap();
        assert_eq!(back, bound);

        // 没存采样的步骤不写块（同 feature 的省略口径）
        let plain = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8I",
            "转码模板（无采样）",
            PlanAction::Transcode,
            params,
            "2026-11-07T00:00:00Z",
            Some("c01".to_string()),
        );
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json["steps"][0].get("sampling").is_none());
    }

    #[test]
    fn v4_payload_migrates_to_v5_with_sampling_absent() {
        // v4 模板没有 sampling 键：serde default → None，版本号升 5。
        // 采样值不猜——套用时由壳层映射表按缺省补（ADR-0080 §2）。
        let payload = json!({
            "recipeId": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8J",
            "schemaVersion": 4,
            "name": "v4 的转码模板",
            "steps": [{
                "stepId": "step-1",
                "kind": "transcode",
                "paramsSnapshot": {"target": "utf-8"},
                "source": "original",
                "feature": "c01"
            }],
            "createdAt": "2026-11-06T00:00:00Z"
        });
        let migrated = migrate_recipe_value(payload).unwrap();
        assert_eq!(migrated.schema_version, CURRENT_RECIPE_SCHEMA_VERSION);
        assert_eq!(migrated.steps[0].sampling, None);
        assert!(migrated.validate().is_ok());
    }

    #[test]
    fn sampling_zero_is_rejected_and_unknown_keys_are_denied() {
        let mut recipe = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8K",
            "零采样",
            PlanAction::Transcode,
            Map::new(),
            "2026-11-07T00:00:00Z",
            Some("c01".to_string()),
        );
        recipe.steps[0].sampling = Some(RecipeSampling {
            sample_window: Some(0),
            ..RecipeSampling::default()
        });
        let error = recipe.validate().unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(error.message.contains("sampleWindow"), "{error:?}");

        // 采样块的键集是封闭的：塞进未知键（deny_unknown_fields）直接解析失败，
        // 不静默丢——否则保存侧新加的键会被导入通道悄悄抹掉。
        let payload = json!({
            "recipeId": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8L",
            "schemaVersion": 5,
            "name": "未知采样键",
            "steps": [{
                "stepId": "step-1",
                "kind": "transcode",
                "paramsSnapshot": {},
                "source": "original",
                "feature": "c01",
                "sampling": {"window": 4096}
            }],
            "createdAt": "2026-11-07T00:00:00Z"
        });
        assert!(migrate_recipe_value(payload).is_err());
    }
}
