//! 本机界面状态（UiState）的类型与白名单校验（设计 03 §11）。
//!
//! UiState 与 ConfigDocument 分离：不导出、不跨设备、不参与配方迁移；
//! 丢失或版本不兼容只重置布局（方案 §15.3）。本模块为纯函数，
//! 读写磁盘与"损坏即重置"的兜底由壳层（`batchdesk-platform`）负责。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::KernelError;

pub const CURRENT_UI_STATE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TemplateRunMode {
    Stepwise,
    OneClick,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct WindowState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximized: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct LayoutState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_collapsed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_panel_width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_panel_open: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_panel_open: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct NavigationState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_section: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_feature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_directories: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct SortState {
    pub column: String,
    pub direction: SortDirection,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct TableState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column_widths: Option<BTreeMap<String, f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<SortState>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct TemplatePrefs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_mode: Option<TemplateRunMode>,
}

/// 本机界面状态（设计 03 §11.2）。**不含**已选文件、搜索/筛选、草稿、AI 对话、
/// 任务状态或许可证数据（方案 §15.3，`deny_unknown_fields` 会拒绝这些字段）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct UiState {
    pub ui_state_version: u32,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<LayoutState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub navigation: Option<NavigationState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tables: Option<BTreeMap<String, TableState>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_prefs: Option<BTreeMap<String, TemplatePrefs>>,
}

impl UiState {
    pub fn defaults() -> Self {
        Self {
            ui_state_version: CURRENT_UI_STATE_VERSION,
            updated_at: String::new(),
            ..Self::default()
        }
    }

    /// 版本不兼容（过高或未定版本）→ 只重置 UiState（设计 03 §6.1-6）。
    pub fn is_compatible(&self) -> bool {
        self.ui_state_version == CURRENT_UI_STATE_VERSION
    }

    /// 白名单 patch：未知字段 → E1002；非法类型由 serde 拒绝。
    pub fn apply_patch(&mut self, patch: &Map<String, Value>) -> Result<(), KernelError> {
        if patch.is_empty() {
            return Ok(());
        }
        if patch.contains_key("uiStateVersion") || patch.contains_key("updatedAt") {
            return Err(KernelError::invalid_params(
                "uiStateVersion/updatedAt are managed by the shell",
            ));
        }
        let mut current = serde_json::to_value(&*self).map_err(|error| {
            KernelError::internal("ui state is not serializable", error.to_string())
        })?;
        let object = current
            .as_object_mut()
            .ok_or_else(|| KernelError::internal("ui state is not an object", "ui state value"))?;
        for (key, value) in patch {
            object.insert(key.clone(), value.clone());
        }
        *self = serde_json::from_value(current).map_err(|error| {
            KernelError::invalid_params(format!("invalid ui state value: {error}"))
        })?;
        Ok(())
    }

    /// 最近目录按顺序去重并保留本次置顶项（设计 03 §11.2 的 `recentDirectories`）。
    pub fn remember_directory(&mut self, directory: &str) {
        let navigation = self.navigation.get_or_insert_with(NavigationState::default);
        let mut recent = navigation.recent_directories.take().unwrap_or_default();
        recent.retain(|entry| entry != directory);
        recent.insert(0, directory.to_string());
        navigation.recent_directories = Some(recent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn patch(entries: &[(&str, Value)]) -> Map<String, Value> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn defaults_use_the_current_version_and_are_empty() {
        let state = UiState::defaults();
        assert_eq!(state.ui_state_version, CURRENT_UI_STATE_VERSION);
        assert!(state.is_compatible());
        assert!(state.window.is_none());
        assert!(state.layout.is_none());
    }

    #[test]
    fn patch_accepts_whitelisted_sections() {
        let mut state = UiState::defaults();
        state
            .apply_patch(&patch(&[
                ("window", json!({"width": 1280.0, "height": 800.0})),
                ("layout", json!({"sidebarWidth": 320.0})),
            ]))
            .unwrap();
        assert_eq!(state.window.as_ref().unwrap().width, Some(1280.0));
        assert_eq!(state.layout.as_ref().unwrap().sidebar_width, Some(320.0));
    }

    #[test]
    fn patch_rejects_selected_files_search_and_drafts() {
        let mut state = UiState::defaults();
        for key in [
            "selectedFiles",
            "search",
            "filter",
            "draft",
            "aiConversation",
            "tasks",
        ] {
            let error = state.apply_patch(&patch(&[(key, json!("x"))])).unwrap_err();
            assert_eq!(error.code, "E1002", "must reject {key}");
            assert!(error.message.contains("invalid ui state value"));
        }
    }

    #[test]
    fn patch_rejects_unknown_nested_fields_and_managed_versions() {
        let mut state = UiState::defaults();
        let error = state
            .apply_patch(&patch(&[("window", json!({"width": 1.0, "zoom": 2.0}))]))
            .unwrap_err();
        assert_eq!(error.code, "E1002");
        let error = state
            .apply_patch(&patch(&[("uiStateVersion", json!(9))]))
            .unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn incompatible_version_is_detected_for_reset() {
        let mut state = UiState::defaults();
        state.ui_state_version = CURRENT_UI_STATE_VERSION + 1;
        assert!(!state.is_compatible());
        state.ui_state_version = 0;
        assert!(!state.is_compatible());
    }

    #[test]
    fn recent_directories_are_deduplicated_and_move_to_front() {
        let mut state = UiState::defaults();
        state.remember_directory("/a");
        state.remember_directory("/b");
        state.remember_directory("/a");
        assert_eq!(
            state.navigation.unwrap().recent_directories,
            Some(vec!["/a".to_string(), "/b".to_string()])
        );
    }

    #[test]
    fn state_round_trips_through_json() {
        let mut state = UiState::defaults();
        state
            .apply_patch(&patch(&[(
                "templatePrefs",
                json!({"tpl": {"runMode": "one-click"}}),
            )]))
            .unwrap();
        let json = serde_json::to_string(&state).unwrap();
        let decoded: UiState = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, state);
    }
}
