//! 处理模板/配方（C37）的类型、迁移与导入校验（设计 03 §5／§6）。
//!
//! 纯函数层：不读写磁盘、不取时钟；`createdAt`／`updatedAt` 由调用方传入。
//! 模板只保存声明式参数与适用扩展名，不保存脚本/宏/凭据/文件内容/绝对源路径
//! （方案 §15、AGENTS 不变量 13）。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::KernelError;
use crate::plan::PlanAction;

pub const CURRENT_RECIPE_SCHEMA_VERSION: u32 = 3;

/// 多步骤上限沿用 F15 单束上限（方案 §15.1，不新增数值）。
pub const MAX_RECIPE_STEPS: usize = 10;

/// 导入时必须剔除的路径类参数（模板不得保存源路径/输出目录，方案 §15.2.4）。
/// 键名统一按小写登记，匹配时先把参数键小写化（大小写不敏感）。
pub const PATH_PARAM_KEYS: &[&str] = &[
    "source",
    "sourcepath",
    "sourcepaths",
    "outputdir",
    "outputdirectory",
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
pub struct RecipeDocumentV3 {
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

impl RecipeDocumentV3 {
    /// 单步模板的入口（Phase A 兼容：kind + paramsSnapshot → steps[0]）。
    pub fn from_single_step(
        recipe_id: impl Into<String>,
        name: impl Into<String>,
        kind: PlanAction,
        params_snapshot: Map<String, Value>,
        created_at: impl Into<String>,
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
            if let Some(key) = script_key(&step.params_snapshot) {
                return Err(KernelError::invalid_params(format!(
                    "template parameters must not contain scripts (found {key})"
                )));
            }
        }
        Ok(())
    }
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

/// 读取任意版本的模板 JSON 并迁移到 v3（设计 03 §6.1-5）。
///
/// 低版本（1/2）迁移；高于 `CURRENT_RECIPE_SCHEMA_VERSION` → E1007（不猜测语义）。
pub fn migrate_recipe_value(payload: Value) -> Result<RecipeDocumentV3, KernelError> {
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
            Ok(v1.into_v3())
        }
        2 => {
            let v2: RecipeDocumentV2 =
                serde_json::from_value(payload).map_err(recipe_parse_error)?;
            Ok(v2.into_v3())
        }
        3 => {
            let mut v3: RecipeDocumentV3 =
                serde_json::from_value(payload).map_err(recipe_parse_error)?;
            for step in &mut v3.steps {
                strip_path_params(&mut step.params_snapshot);
            }
            Ok(v3)
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
    fn into_v3(mut self) -> RecipeDocumentV3 {
        let removed = strip_path_params(&mut self.params_snapshot);
        let _ = removed;
        RecipeDocumentV3 {
            recipe_id: self.recipe_id,
            schema_version: CURRENT_RECIPE_SCHEMA_VERSION,
            name: self.name,
            description: None,
            steps: vec![TemplateStep {
                step_id: "step-1".to_string(),
                kind: self.kind,
                params_snapshot: self.params_snapshot,
                source: StepSource::Original,
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
    fn into_v3(self) -> RecipeDocumentV3 {
        let mut params = self.params_snapshot;
        strip_path_params(&mut params);
        RecipeDocumentV3 {
            recipe_id: self.recipe_id,
            schema_version: CURRENT_RECIPE_SCHEMA_VERSION,
            name: self.name,
            description: self.description,
            steps: vec![TemplateStep {
                step_id: "step-1".to_string(),
                kind: self.kind,
                params_snapshot: params,
                source: StepSource::Original,
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
        let mut recipe = RecipeDocumentV3::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "模板",
            PlanAction::Rename,
            step_params,
            "2026-10-29T00:00:00Z",
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
            "schemaVersion": 4,
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
        let mut recipe = RecipeDocumentV3::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "危险模板",
            PlanAction::Rename,
            Map::new(),
            "2026-10-29T00:00:00Z",
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
        let mut recipe = RecipeDocumentV3::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "十一步",
            PlanAction::Rename,
            Map::new(),
            "2026-10-29T00:00:00Z",
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

        let mut duplicated = RecipeDocumentV3::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "重复 id",
            PlanAction::Rename,
            Map::new(),
            "2026-10-29T00:00:00Z",
        );
        let step = duplicated.steps[0].clone();
        duplicated.steps.push(step);
        let error = duplicated.validate().unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(error.message.contains("duplicate stepId"));
    }

    #[test]
    fn empty_steps_are_rejected() {
        let mut recipe = RecipeDocumentV3::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "空模板",
            PlanAction::Rename,
            Map::new(),
            "2026-10-29T00:00:00Z",
        );
        recipe.steps.clear();
        let error = recipe.validate().unwrap_err();
        assert_eq!(error.code, "E1002");
    }
}
