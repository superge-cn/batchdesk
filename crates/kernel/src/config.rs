//! 普通用户配置（ConfigDocument）的类型、默认值与来源追踪（设计 03 §2–§4）。
//!
//! 纯函数层：不读写磁盘、不取时钟；`updatedAt` 由调用方传入（不变量 1）。
//! 字段存在性与默认值以设计 03 §4.1／§4.2 为权威；本模块只落地类型与清单，
//! 凡涉及产品阈值的字段一律留空（省略＝按基线取值），不在此写第二份数值。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::KernelError;

pub const CURRENT_CONFIG_VERSION: u32 = 1;

pub type MigrationFn = fn(ConfigDocument) -> Result<ConfigDocument, KernelError>;

/// 线性迁移链（设计 03 §6.2）：`[(1, m1_to_2), (2, m2_to_3), ...]`。
///
/// v1 是 schema 起点，链为空；新增版本时在此登记 `N → N+1` 的纯函数，
/// 禁止跳版本合并迁移（可测性要求，§6.1-1）。
pub fn migration_chain() -> &'static [(u32, MigrationFn)] {
    &[]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LaunchBehavior {
    Normal,
    Minimized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicySetting {
    Skip,
    Sequence,
    Overwrite,
    Ask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EncodingPolicy {
    Auto,
    StrictUtf8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NewlinePolicy {
    Keep,
    Crlf,
    Lf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeletionMode {
    MoveToQuarantine,
    Permanent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlaceholderPolicy {
    Skip,
    DownloadThenProcess,
}

/// `"auto"` 或显式数值（设计 03 §4.1 的 `number | "auto"`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoOrNumber {
    Auto,
    Number(u64),
}

impl Serialize for AutoOrNumber {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Number(value) => serializer.serialize_u64(*value),
        }
    }
}

impl<'de> Deserialize<'de> for AutoOrNumber {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = AutoOrNumber;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a non-negative integer or the string \"auto\"")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value == "auto" {
                    Ok(AutoOrNumber::Auto)
                } else {
                    Err(E::invalid_value(serde::de::Unexpected::Str(value), &self))
                }
            }

            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(AutoOrNumber::Number(value))
            }

            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                u64::try_from(value)
                    .map(AutoOrNumber::Number)
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Signed(value), &self))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingSource {
    Default,
    User,
    Recipe,
    Task,
}

impl SettingSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::User => "user",
            Self::Recipe => "recipe",
            Self::Task => "task",
        }
    }
}

/// 设置页"为什么是这个值"的数据支撑（设计 03 §2）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingValue {
    pub value: Value,
    pub effective_source: SettingSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_value: Option<Value>,
    pub default_value: Value,
}

/// 单 scope 的逐项来源视图（设计 03 §2.1）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeView {
    pub scope: String,
    pub values: BTreeMap<String, SettingValue>,
}

/// 三档优先级解析（设计 03 §2）：`recipe/task 覆盖 > user > default`，纯函数。
pub fn resolve_effective(
    default_value: Value,
    user_value: Option<Value>,
    override_value: Option<Value>,
) -> SettingValue {
    let (value, effective_source) = match (override_value.as_ref(), user_value.as_ref()) {
        (Some(over), _) => (over.clone(), SettingSource::Task),
        (None, Some(user)) => (user.clone(), SettingSource::User),
        (None, None) => (default_value.clone(), SettingSource::Default),
    };
    SettingValue {
        value,
        effective_source,
        user_value,
        default_value,
    }
}

macro_rules! scope_default {
    ($ty:ty, $($key:literal => $default:expr),+ $(,)?) => {
        impl $ty {
            pub const KEYS: &'static [&'static str] = &[$($key),+];

            /// "未设置"档：字段全为 `None`，有效值由 `default_value` 提供的安全默认。
            pub fn defaults() -> Self {
                Self::default()
            }

            fn default_value(key: &str) -> Value {
                match key {
                    $($key => $default,)+
                    _ => Value::Null,
                }
            }

            /// 白名单 + 类型/枚举校验，全部走 serde（未知 key 由
            /// `deny_unknown_fields` 拒绝，非法枚举值由反序列化拒绝）。
            pub fn apply_patch(&mut self, patch: &Map<String, Value>) -> Result<(), KernelError> {
                if patch.is_empty() {
                    return Ok(());
                }
                let mut current = serde_json::to_value(&*self).map_err(|error| {
                    KernelError::internal("config scope is not serializable", error.to_string())
                })?;
                let object = current.as_object_mut().ok_or_else(|| {
                    KernelError::internal("config scope is not an object", "scope value")
                })?;
                for (key, value) in patch {
                    object.insert(key.clone(), value.clone());
                }
                myself_replace(self, current)
            }

            /// 逐项来源视图：有效值 = 用户档（若显式）否则默认。
            pub fn view(&self) -> BTreeMap<String, SettingValue> {
                let mut values = BTreeMap::new();
                for key in Self::KEYS {
                    let default_value = Self::default_value(key);
                    let current = serde_json::to_value(&*self).unwrap_or(Value::Null);
                    let user_value = current.get(*key).cloned().filter(|value| !value.is_null());
                    values.insert(
                        (*key).to_string(),
                        resolve_effective(default_value, user_value, None),
                    );
                }
                values
            }
        }
    };
}

fn myself_replace<T: for<'de> Deserialize<'de>>(
    target: &mut T,
    value: Value,
) -> Result<(), KernelError> {
    *target = serde_json::from_value(value)
        .map_err(|error| KernelError::invalid_params(format!("invalid config value: {error}")))?;
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct GeneralScope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_output_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_behavior: Option<LaunchBehavior>,
}

scope_default!(GeneralScope,
    "language" => Value::String("zh-CN".into()),
    "theme" => Value::String("light".into()),
    "defaultOutputDir" => Value::Null,
    "launchBehavior" => Value::String("normal".into()),
);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ProcessingScope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_conflict: Option<ConflictPolicySetting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding_policy: Option<EncodingPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newline_policy: Option<NewlinePolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion_mode: Option<DeletionMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_backup: Option<bool>,
}

scope_default!(ProcessingScope,
    "onConflict" => Value::String("skip".into()),
    "encodingPolicy" => Value::String("auto".into()),
    "newlinePolicy" => Value::String("keep".into()),
    "deletionMode" => Value::String("move-to-quarantine".into()),
    "keepBackup" => Value::Bool(true),
);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct PerformanceScope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<AutoOrNumber>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_budget: Option<AutoOrNumber>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub large_file_threshold: Option<AutoOrNumber>,
}

scope_default!(PerformanceScope,
    "concurrency" => Value::String("auto".into()),
    "memoryBudget" => Value::String("auto".into()),
    "largeFileThreshold" => Value::String("auto".into()),
);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct SecurityScope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protected_dirs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quarantine_dir: Option<String>,
    /// 与撤销窗口同一口径（方案 §7.8）；默认值与区间引用基线，此处不设数值。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quarantine_retention_days: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder_policy: Option<PlaceholderPolicy>,
    /// 工程默认取基线磁盘余量下限（11.6-12）；数值以基线为准，此处不设。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_space_margin_floor_bytes: Option<u64>,
    /// 工程默认取基线同一行的比例（11.6-12）；数值以基线为准，此处不设。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_space_margin_ratio: Option<f64>,
}

scope_default!(SecurityScope,
    "protectedDirs" => Value::Array(Vec::new()),
    "quarantineDir" => Value::Null,
    "quarantineRetentionDays" => Value::Null,
    "placeholderPolicy" => Value::String("skip".into()),
    "diskSpaceMarginFloorBytes" => Value::Null,
    "diskSpaceMarginRatio" => Value::Null,
);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct LoggingScope {
    /// 仅指操作日志保留天数；默认/区间引用方案 §9.6，此处不设数值。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_full_paths: Option<bool>,
}

scope_default!(LoggingScope,
    "retentionDays" => Value::Null,
    "logFullPaths" => Value::Bool(false),
);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ConfigScopes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub general: Option<GeneralScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performance: Option<PerformanceScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<SecurityScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingScope>,
}

/// 五个可写 scope 的键（授权组只读展示走 `getLicenseState`，不落本文档）。
pub const CONFIG_SCOPE_KEYS: &[&str] = &[
    "general",
    "processing",
    "performance",
    "security",
    "logging",
];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ConfigDocument {
    pub config_version: u32,
    pub updated_at: String,
    pub scopes: ConfigScopes,
}

impl ConfigDocument {
    /// 全默认档（文件不存在或首次启动时使用）：只写版本锚点，scopes 省略。
    pub fn defaults() -> Self {
        Self {
            config_version: CURRENT_CONFIG_VERSION,
            updated_at: String::new(),
            scopes: ConfigScopes::default(),
        }
    }

    /// 校验版本锚点：高于当前版本 → E1007（只读模式，不猜测字段语义）。
    pub fn check_version(&self) -> Result<(), KernelError> {
        if self.config_version > CURRENT_CONFIG_VERSION {
            return Err(KernelError::new(
                "E1007",
                format!(
                    "config version {} is newer than the supported {}",
                    self.config_version, CURRENT_CONFIG_VERSION
                ),
                None,
                false,
            ));
        }
        Ok(())
    }

    /// 逐项视图；未知 scope → E1002。
    pub fn scope_view(&self, scope: &str) -> Result<ScopeView, KernelError> {
        let values = match scope {
            "general" => self.scopes.general.clone().unwrap_or_default().view(),
            "processing" => self.scopes.processing.clone().unwrap_or_default().view(),
            "performance" => self.scopes.performance.clone().unwrap_or_default().view(),
            "security" => self.scopes.security.clone().unwrap_or_default().view(),
            "logging" => self.scopes.logging.clone().unwrap_or_default().view(),
            other => {
                return Err(KernelError::invalid_params(format!(
                    "unknown config scope: {other}"
                )))
            }
        };
        Ok(ScopeView {
            scope: scope.to_string(),
            values,
        })
    }

    /// 应用 patch（未知 key/非法枚举 → E1002；不写时钟，`updatedAt` 由调用方回填）。
    pub fn apply_patch(
        &mut self,
        scope: &str,
        patch: &Map<String, Value>,
    ) -> Result<(), KernelError> {
        self.check_version()?;
        match scope {
            "general" => self
                .scopes
                .general
                .get_or_insert_with(GeneralScope::defaults)
                .apply_patch(patch)?,
            "processing" => self
                .scopes
                .processing
                .get_or_insert_with(ProcessingScope::defaults)
                .apply_patch(patch)?,
            "performance" => self
                .scopes
                .performance
                .get_or_insert_with(PerformanceScope::defaults)
                .apply_patch(patch)?,
            "security" => self
                .scopes
                .security
                .get_or_insert_with(SecurityScope::defaults)
                .apply_patch(patch)?,
            "logging" => self
                .scopes
                .logging
                .get_or_insert_with(LoggingScope::defaults)
                .apply_patch(patch)?,
            other => {
                return Err(KernelError::invalid_params(format!(
                    "unknown config scope: {other}"
                )))
            }
        }
        Ok(())
    }

    /// 恢复默认档（含 `performance` 的"恢复自动"，设计 03 §7）。
    pub fn reset_scope(&mut self, scope: &str) -> Result<(), KernelError> {
        self.check_version()?;
        match scope {
            "general" => self.scopes.general = Some(GeneralScope::defaults()),
            "processing" => self.scopes.processing = Some(ProcessingScope::defaults()),
            "performance" => self.scopes.performance = Some(PerformanceScope::defaults()),
            "security" => self.scopes.security = Some(SecurityScope::defaults()),
            "logging" => self.scopes.logging = Some(LoggingScope::defaults()),
            other => {
                return Err(KernelError::invalid_params(format!(
                    "unknown config scope: {other}"
                )))
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(entries: &[(&str, Value)]) -> Map<String, Value> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn defaults_match_the_safety_registry() {
        let document = ConfigDocument::defaults();
        assert_eq!(document.config_version, CURRENT_CONFIG_VERSION);

        let processing = document.scope_view("processing").unwrap();
        assert_eq!(
            processing.values["onConflict"].value,
            Value::String("skip".into())
        );
        assert_eq!(
            processing.values["deletionMode"].value,
            Value::String("move-to-quarantine".into())
        );
        assert_eq!(processing.values["keepBackup"].value, Value::Bool(true));
        assert!(processing
            .values
            .values()
            .all(|setting| setting.effective_source == SettingSource::Default));

        let security = document.scope_view("security").unwrap();
        assert_eq!(
            security.values["placeholderPolicy"].value,
            Value::String("skip".into())
        );
        assert_eq!(
            security.values["protectedDirs"].value,
            Value::Array(Vec::new())
        );
        assert_eq!(
            security.values["quarantineRetentionDays"].value,
            Value::Null,
            "retention comes from the baseline, not a second copy"
        );

        let general = document.scope_view("general").unwrap();
        assert_eq!(
            general.values["language"].value,
            Value::String("zh-CN".into())
        );
        assert_eq!(general.values["theme"].value, Value::String("light".into()));
        assert_eq!(
            general.values["defaultOutputDir"].value,
            Value::Null,
            "no output location is presolved"
        );

        let logging = document.scope_view("logging").unwrap();
        assert_eq!(logging.values["logFullPaths"].value, Value::Bool(false));
        assert_eq!(logging.values["retentionDays"].value, Value::Null);

        let performance = document.scope_view("performance").unwrap();
        assert_eq!(
            performance.values["concurrency"].value,
            Value::String("auto".into())
        );
    }

    #[test]
    fn scope_view_reports_sources_and_defaults() {
        let mut document = ConfigDocument::defaults();
        document
            .apply_patch(
                "general",
                &patch(&[("theme", Value::String("dark".into()))]),
            )
            .unwrap();
        let view = document.scope_view("general").unwrap();
        let theme = &view.values["theme"];
        assert_eq!(theme.value, Value::String("dark".into()));
        assert_eq!(theme.effective_source, SettingSource::User);
        assert_eq!(theme.user_value, Some(Value::String("dark".into())));
        assert_eq!(theme.default_value, Value::String("light".into()));
        let language = &view.values["language"];
        assert_eq!(language.effective_source, SettingSource::Default);
        assert_eq!(language.user_value, None);
    }

    #[test]
    fn unknown_key_and_unknown_scope_are_rejected() {
        let mut document = ConfigDocument::defaults();
        let error = document
            .apply_patch("general", &patch(&[("nope", Value::Bool(true))]))
            .unwrap_err();
        assert_eq!(error.code, "E1002");
        let error = document.scope_view("license").unwrap_err();
        assert_eq!(error.code, "E1002");
        let error = document.reset_scope("license").unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn invalid_enum_value_is_rejected() {
        let mut document = ConfigDocument::defaults();
        let error = document
            .apply_patch(
                "general",
                &patch(&[("theme", Value::String("sepia".into()))]),
            )
            .unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn performance_setting_accepts_auto_or_numbers_only() {
        let mut document = ConfigDocument::defaults();
        document
            .apply_patch(
                "performance",
                &patch(&[("concurrency", serde_json::json!(4))]),
            )
            .unwrap();
        assert_eq!(
            document.scopes.performance.as_ref().unwrap().concurrency,
            Some(AutoOrNumber::Number(4))
        );
        document
            .apply_patch(
                "performance",
                &patch(&[("concurrency", Value::String("auto".into()))]),
            )
            .unwrap();
        assert_eq!(
            document.scopes.performance.as_ref().unwrap().concurrency,
            Some(AutoOrNumber::Auto)
        );
        let error = document
            .apply_patch(
                "performance",
                &patch(&[("concurrency", Value::String("max".into()))]),
            )
            .unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn reset_scope_restores_defaults() {
        let mut document = ConfigDocument::defaults();
        document
            .apply_patch("processing", &patch(&[("keepBackup", Value::Bool(false))]))
            .unwrap();
        let view = document.scope_view("processing").unwrap();
        assert_eq!(view.values["keepBackup"].value, Value::Bool(false));
        assert_eq!(
            view.values["keepBackup"].effective_source,
            SettingSource::User
        );
        document.reset_scope("processing").unwrap();
        let view = document.scope_view("processing").unwrap();
        assert_eq!(view.values["keepBackup"].value, Value::Bool(true));
        assert_eq!(
            view.values["keepBackup"].effective_source,
            SettingSource::Default
        );
        assert_eq!(view.values["keepBackup"].user_value, None);
    }

    #[test]
    fn higher_config_version_is_read_only() {
        let mut document = ConfigDocument::defaults();
        document.config_version = CURRENT_CONFIG_VERSION + 1;
        let error = document
            .apply_patch(
                "general",
                &patch(&[("language", Value::String("en-US".into()))]),
            )
            .unwrap_err();
        assert_eq!(error.code, "E1007");
        let error = document.reset_scope("general").unwrap_err();
        assert_eq!(error.code, "E1007");
    }

    #[test]
    fn config_round_trips_through_json_and_rejects_unknown_fields() {
        let document = ConfigDocument::defaults();
        let json = serde_json::to_string(&document).unwrap();
        assert!(
            !json.contains("\"defaultOutputDir\":null"),
            "None fields are omitted"
        );
        let decoded: ConfigDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, document);
        let error = serde_json::from_str::<ConfigDocument>(
            r#"{"configVersion":1,"updatedAt":"","scopes":{},"license":{}}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn migration_chain_is_empty_at_v1() {
        assert!(migration_chain().is_empty());
        assert_eq!(CURRENT_CONFIG_VERSION, 1);
    }

    #[test]
    fn resolve_effective_prefers_override_then_user_then_default() {
        let default_value = Value::String("skip".into());
        let user = Some(Value::String("sequence".into()));
        let over = Some(Value::String("overwrite".into()));
        let resolved = resolve_effective(default_value.clone(), user.clone(), over);
        assert_eq!(resolved.value, Value::String("overwrite".into()));
        assert_eq!(resolved.effective_source, SettingSource::Task);
        let resolved = resolve_effective(default_value.clone(), user.clone(), None);
        assert_eq!(resolved.value, Value::String("sequence".into()));
        assert_eq!(resolved.effective_source, SettingSource::User);
        assert_eq!(resolved.user_value, user);
        let resolved = resolve_effective(default_value.clone(), None, None);
        assert_eq!(resolved.value, default_value.clone());
        assert_eq!(resolved.effective_source, SettingSource::Default);
        assert_eq!(resolved.default_value, default_value);
    }
}
