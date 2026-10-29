//! 配置、界面状态与处理模板的落盘（设计 03 §3／§6／§7／§11）。
//!
//! 本层是壳层适配：读旧文件、原子写新文件、损坏兜底与"UiState 只重置自己"
//! 都在这里完成；kernel 侧的迁移与校验保持纯函数。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kernel::config::{ConfigDocument, CURRENT_CONFIG_VERSION};
use kernel::error::KernelError;
use kernel::fs::FsAdapter;
use kernel::recipe::{migrate_recipe_value, RecipeDocumentV3, CURRENT_RECIPE_SCHEMA_VERSION};
use kernel::ui_state::UiState;
use serde::Serialize;
use serde_json::Value;

use crate::fs::{now_rfc3339, CliFsAdapter};

/// `getConfig` 的结果：文档 + 兜底警告 + 是否只读（高版本降级）。
#[derive(Clone, Debug)]
pub struct ConfigLoad {
    pub document: ConfigDocument,
    pub warning: Option<KernelError>,
    pub read_only: bool,
}

#[derive(Clone, Debug, Default)]
pub struct RecipeList {
    pub recipes: Vec<RecipeDocumentV3>,
    pub warnings: Vec<KernelError>,
}

/// 配置目录下的三个对象：`config.json`、`ui-state.json`、`recipes/`（§3 拓扑）。
#[derive(Clone, Debug)]
pub struct ConfigStore {
    root: PathBuf,
}

impl ConfigStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }

    pub fn ui_state_path(&self) -> PathBuf {
        self.root.join("ui-state.json")
    }

    pub fn recipes_dir(&self) -> PathBuf {
        self.root.join("recipes")
    }

    fn write_json<T: Serialize>(&self, path: &Path, value: &T) -> Result<(), KernelError> {
        let data = serde_json::to_vec_pretty(value).map_err(|error| {
            KernelError::internal("config value is not serializable", error.to_string())
        })?;
        let adapter = CliFsAdapter::new(None);
        adapter
            .write_atomic(&path.to_string_lossy(), &data)
            .map_err(write_failure)
    }

    /// 读取配置。
    ///
    /// - 文件不存在 → 全默认档（不写盘，避免把"未设置"固化成显式值）；
    /// - 文件损坏 → 留存 `config.corrupt.<ts>.json`、回默认档并报 E5009（§6.1-3）；
    /// - 版本高于当前 → 只读展示 + E1007（§6.1-4）。
    pub fn load_config(&self) -> Result<ConfigLoad, KernelError> {
        let path = self.config_path();
        if !path.exists() {
            let mut document = ConfigDocument::defaults();
            document.updated_at = now_rfc3339();
            return Ok(ConfigLoad {
                document,
                warning: None,
                read_only: false,
            });
        }
        let text = fs::read_to_string(&path).map_err(read_failure)?;
        match serde_json::from_str::<ConfigDocument>(&text) {
            Ok(document) => {
                let read_only = document.check_version().is_err();
                let warning = document.check_version().err();
                Ok(ConfigLoad {
                    document,
                    warning,
                    read_only,
                })
            }
            Err(error) => {
                let backup = self.backup_corrupt(&path)?;
                let mut document = ConfigDocument::defaults();
                document.updated_at = now_rfc3339();
                Ok(ConfigLoad {
                    document,
                    warning: Some(KernelError::new(
                        "E5009",
                        "config file is corrupt; a backup was kept and defaults were restored",
                        Some(format!(
                            "{}: {error}; backup: {}",
                            path.display(),
                            backup.display()
                        )),
                        false,
                    )),
                    read_only: false,
                })
            }
        }
    }

    /// 原子写配置（临时文件 + 改名）；`updatedAt` 在此回填（kernel 不取时钟）。
    pub fn save_config(&self, document: &mut ConfigDocument) -> Result<(), KernelError> {
        document.check_version()?;
        if document.config_version == 0 {
            document.config_version = CURRENT_CONFIG_VERSION;
        }
        document.updated_at = now_rfc3339();
        self.write_json(&self.config_path(), document)
    }

    /// 读取界面状态：损坏或版本不兼容**只重置 UiState**（§6.1-6、§11.3）。
    pub fn load_ui_state(&self) -> UiState {
        let path = self.ui_state_path();
        let Ok(text) = fs::read_to_string(&path) else {
            return UiState::defaults();
        };
        match serde_json::from_str::<UiState>(&text) {
            Ok(state) if state.is_compatible() => state,
            _ => {
                let mut fresh = UiState::defaults();
                fresh.updated_at = now_rfc3339();
                let _ = self.write_json(&path, &fresh);
                fresh
            }
        }
    }

    pub fn save_ui_state(&self, state: &mut UiState) -> Result<(), KernelError> {
        state.ui_state_version = kernel::ui_state::CURRENT_UI_STATE_VERSION;
        state.updated_at = now_rfc3339();
        self.write_json(&self.ui_state_path(), state)
    }

    pub fn reset_ui_state(&self) -> Result<UiState, KernelError> {
        let mut fresh = UiState::defaults();
        self.save_ui_state(&mut fresh)?;
        Ok(fresh)
    }

    fn recipe_path(&self, recipe_id: &str) -> Result<PathBuf, KernelError> {
        safe_recipe_id(recipe_id)?;
        Ok(self.recipes_dir().join(format!("{recipe_id}.json")))
    }

    /// 保存模板：校验结构约束后原子写（`schemaVersion` 固定为当前版本）。
    pub fn save_recipe(&self, recipe: &mut RecipeDocumentV3) -> Result<(), KernelError> {
        recipe.validate()?;
        recipe.schema_version = CURRENT_RECIPE_SCHEMA_VERSION;
        let path = self.recipe_path(&recipe.recipe_id)?;
        self.write_json(&path, recipe)
    }

    /// 读取单个模板（低版本按需迁移；高版本 E1007）。
    pub fn get_recipe(&self, recipe_id: &str) -> Result<RecipeDocumentV3, KernelError> {
        let path = self.recipe_path(recipe_id)?;
        if !path.exists() {
            return Err(KernelError::new(
                "E2001",
                format!("recipe not found: {recipe_id}"),
                None,
                false,
            ));
        }
        let text = fs::read_to_string(&path).map_err(read_failure)?;
        let payload: Value = serde_json::from_str(&text).map_err(|error| {
            KernelError::invalid_params(format!("invalid recipe file: {error}"))
        })?;
        migrate_recipe_value(payload)
    }

    /// 列出全部模板（坏文件单独计入 warnings，不阻断其余条目）。
    pub fn list_recipes(&self) -> RecipeList {
        let mut list = RecipeList::default();
        let Ok(entries) = fs::read_dir(self.recipes_dir()) else {
            return list;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .collect();
        paths.sort();
        for path in paths {
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            match self.get_recipe(stem) {
                Ok(recipe) => list.recipes.push(recipe),
                Err(error) => list.warnings.push(error),
            }
        }
        list
    }

    /// 删除模板（幂等，设计 03 §7）。
    pub fn delete_recipe(&self, recipe_id: &str) -> Result<(), KernelError> {
        let path = self.recipe_path(recipe_id)?;
        if path.exists() {
            fs::remove_file(&path).map_err(io_failure)?;
        }
        Ok(())
    }

    /// 导入模板：解析 → 迁移（低版本升级、高版本 E1007）→ 校验 → 落盘（§13.8）。
    pub fn import_recipe(&self, payload: &str) -> Result<RecipeDocumentV3, KernelError> {
        let value: Value = serde_json::from_str(payload).map_err(|error| {
            KernelError::invalid_params(format!("invalid recipe payload: {error}"))
        })?;
        let mut recipe = migrate_recipe_value(value)?;
        self.save_recipe(&mut recipe)?;
        Ok(recipe)
    }

    /// 导出模板：返回 v3 JSON 字符串（落盘位置由壳层另存对话框决定）。
    pub fn export_recipe(&self, recipe_id: &str) -> Result<String, KernelError> {
        let recipe = self.get_recipe(recipe_id)?;
        serde_json::to_string_pretty(&recipe)
            .map_err(|error| KernelError::internal("recipe is not serializable", error.to_string()))
    }

    fn backup_corrupt(&self, path: &Path) -> Result<PathBuf, KernelError> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("config");
        let backup = path.with_file_name(format!("{stem}.corrupt.{stamp}.json"));
        fs::rename(path, &backup).map_err(io_failure)?;
        Ok(backup)
    }
}

/// recipeId 会拼接进文件名，必须限制字符集以防路径穿越（安全默认值）。
fn safe_recipe_id(recipe_id: &str) -> Result<(), KernelError> {
    let valid = !recipe_id.is_empty()
        && recipe_id.len() <= 64
        && recipe_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(KernelError::invalid_params(format!(
            "invalid recipeId: {recipe_id}"
        )))
    }
}

fn write_failure(error: KernelError) -> KernelError {
    KernelError::new(
        "E5011",
        "configuration storage is not writable",
        Some(format!("{}: {}", error.code, error.message)),
        false,
    )
}

fn io_failure(error: std::io::Error) -> KernelError {
    KernelError::new(
        "E5011",
        "configuration storage is not writable",
        Some(error.to_string()),
        false,
    )
}

fn read_failure(error: std::io::Error) -> KernelError {
    KernelError::new(
        "E5011",
        "configuration storage is not readable",
        Some(error.to_string()),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::config::CURRENT_CONFIG_VERSION;
    use kernel::plan::PlanAction;
    use serde_json::{json, Map};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("batchdesk-config-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn store() -> (TempDir, ConfigStore) {
        let dir = TempDir::new();
        let store = ConfigStore::new(dir.0.clone());
        (dir, store)
    }

    fn recipe(recipe_id: &str) -> RecipeDocumentV3 {
        let mut params = Map::new();
        params.insert("keep".to_string(), json!("earliest"));
        RecipeDocumentV3::from_single_step(
            recipe_id,
            "去重模板",
            PlanAction::Delete,
            params,
            "2026-10-29T00:00:00Z",
        )
    }

    #[test]
    fn missing_config_yields_defaults_without_writing() {
        let (dir, store) = store();
        let load = store.load_config().unwrap();
        assert_eq!(load.document.config_version, CURRENT_CONFIG_VERSION);
        assert!(load.warning.is_none());
        assert!(!load.read_only);
        assert!(!dir.0.join("config.json").exists());
    }

    #[test]
    fn saved_config_is_read_back() {
        let (_dir, store) = store();
        let mut document = ConfigDocument::defaults();
        let mut patch = Map::new();
        patch.insert("theme".to_string(), json!("dark"));
        document.apply_patch("general", &patch).unwrap();
        store.save_config(&mut document).unwrap();
        assert!(!document.updated_at.is_empty());

        let load = store.load_config().unwrap();
        assert_eq!(
            load.document.scopes.general.unwrap().theme,
            Some(kernel::config::Theme::Dark)
        );
        assert!(load.warning.is_none());
    }

    #[test]
    fn corrupt_config_is_backed_up_and_defaults_restored() {
        let (dir, store) = store();
        fs::write(store.config_path(), b"{ not json").unwrap();
        let load = store.load_config().unwrap();
        let warning = load.warning.unwrap();
        assert_eq!(warning.code, "E5009");
        assert!(!load.read_only);
        assert_eq!(load.document.config_version, CURRENT_CONFIG_VERSION);
        let backups: Vec<_> = fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config.corrupt.")
            })
            .collect();
        assert_eq!(backups.len(), 1, "corrupt copy must be kept");
    }

    #[test]
    fn newer_config_version_is_read_only() {
        let (_dir, store) = store();
        let payload = json!({
            "configVersion": CURRENT_CONFIG_VERSION + 1,
            "updatedAt": "2026-10-29T00:00:00Z",
            "scopes": {}
        });
        fs::write(store.config_path(), payload.to_string()).unwrap();
        let load = store.load_config().unwrap();
        assert!(load.read_only);
        assert_eq!(load.warning.unwrap().code, "E1007");

        let mut document = load.document;
        let error = store.save_config(&mut document).unwrap_err();
        assert_eq!(error.code, "E1007");
    }

    #[test]
    fn corrupt_ui_state_is_reset_without_touching_config() {
        let (_dir, store) = store();
        let mut document = ConfigDocument::defaults();
        store.save_config(&mut document).unwrap();
        fs::write(store.ui_state_path(), b"<broken>").unwrap();

        let state = store.load_ui_state();
        assert!(state.is_compatible());
        assert!(state.window.is_none());
        let config_text = fs::read_to_string(store.config_path()).unwrap();
        assert!(
            config_text.contains("configVersion"),
            "config must stay intact"
        );
        let rewritten = fs::read_to_string(store.ui_state_path()).unwrap();
        assert!(rewritten.contains("uiStateVersion"));
    }

    #[test]
    fn ui_state_round_trips_and_reset() {
        let (_dir, store) = store();
        let mut state = UiState::defaults();
        state.remember_directory("/data");
        store.save_ui_state(&mut state).unwrap();
        let loaded = store.load_ui_state();
        assert_eq!(
            loaded.navigation.as_ref().unwrap().recent_directories,
            Some(vec!["/data".to_string()])
        );
        let reset = store.reset_ui_state().unwrap();
        assert!(reset.navigation.is_none());
        assert!(store.load_ui_state().navigation.is_none());
    }

    #[test]
    fn recipe_save_get_list_and_delete() {
        let (_dir, store) = store();
        let mut template = recipe("01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8");
        store.save_recipe(&mut template).unwrap();
        let loaded = store.get_recipe(&template.recipe_id).unwrap();
        assert_eq!(loaded.steps.len(), 1);
        assert_eq!(loaded.steps[0].params_snapshot["keep"], json!("earliest"));

        let list = store.list_recipes();
        assert_eq!(list.recipes.len(), 1);
        assert!(list.warnings.is_empty());

        store.delete_recipe(&template.recipe_id).unwrap();
        assert!(store.list_recipes().recipes.is_empty());
        store.delete_recipe(&template.recipe_id).unwrap();
    }

    #[test]
    fn import_migrates_low_versions_and_rejects_high_versions() {
        let (_dir, store) = store();
        let payload = json!({
            "recipeId": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "schemaVersion": 1,
            "name": "旧模板",
            "kind": "delete",
            "paramsSnapshot": {"keep": "earliest"},
            "createdAt": "2026-10-29T00:00:00Z"
        })
        .to_string();
        let imported = store.import_recipe(&payload).unwrap();
        assert_eq!(imported.schema_version, CURRENT_RECIPE_SCHEMA_VERSION);
        assert!(store.get_recipe(&imported.recipe_id).is_ok());

        let high = json!({
            "recipeId": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8B",
            "schemaVersion": 9,
            "name": "未来模板",
            "steps": []
        })
        .to_string();
        let error = store.import_recipe(&high).unwrap_err();
        assert_eq!(error.code, "E1007");
    }

    #[test]
    fn export_returns_v3_json() {
        let (_dir, store) = store();
        let mut template = recipe("01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8");
        store.save_recipe(&mut template).unwrap();
        let payload = store.export_recipe(&template.recipe_id).unwrap();
        let decoded: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(decoded["schemaVersion"], json!(3));
        assert!(decoded.get("steps").is_some());
    }

    #[test]
    fn recipe_ids_with_path_separators_are_rejected() {
        let (_dir, store) = store();
        assert_eq!(store.get_recipe("../evil").unwrap_err().code, "E1002");
        assert_eq!(store.delete_recipe("a/b").unwrap_err().code, "E1002");
        assert_eq!(
            store.save_recipe(&mut recipe("..")).unwrap_err().code,
            "E1002"
        );
    }

    #[test]
    fn recipe_files_are_not_written_for_invalid_templates() {
        let (_dir, store) = store();
        let mut template = recipe("01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8");
        template.steps[0]
            .params_snapshot
            .insert("script".to_string(), json!("rm -rf /"));
        let error = store.save_recipe(&mut template).unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(!store
            .recipes_dir()
            .join("01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8.json")
            .exists());
    }

    #[test]
    fn list_recipes_reports_broken_files_without_aborting() {
        let (_dir, store) = store();
        let mut template = recipe("01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8");
        store.save_recipe(&mut template).unwrap();
        fs::create_dir_all(store.recipes_dir()).unwrap();
        fs::write(store.recipes_dir().join("broken.json"), b"oops").unwrap();
        let list = store.list_recipes();
        assert_eq!(list.recipes.len(), 1);
        assert_eq!(list.warnings.len(), 1);
    }
}
