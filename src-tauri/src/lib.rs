//! BatchDesk desktop shell for the C09 vertical slice.
//!
//! The shell owns filesystem paths, plan retention in memory and command
//! registration; domain rules stay in `kernel` and shell adapters in
//! `platform`. Apply/undo are synchronous in this slice; task queue/progress
//! events are the next step.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use kernel::apply::ApplyOptions;
use kernel::csv::Delimiter;
use kernel::duplicate::{group_duplicates, DuplicateCandidate, GroupingMode, GroupingOptions};
use kernel::error::KernelError;
use kernel::fs::FsAdapter;
use kernel::path::ProtectedDir;
use kernel::plan::{
    plan_c09, KeepPolicy, PlanAction, PlanC09Input, PlanDocument, PlanFile, PlanResolution,
};
use kernel::preflight::{run_preflight, DiskSpaceCheck, PreflightInput, PreflightReport};
use kernel::recipe::{RecipeDocument, RecipeMatch, TemplateStep};
use kernel::ui_state::UiState;
use platform::config_store::ConfigStore;
use platform::fs::CliFsAdapter;
use platform::ids::{new_plan_id, new_ulid};
use platform::journal::FileJournal;
use platform::oplog::SqliteOperationLog;
use platform::preflight::{
    available_space, long_path_supported, path_check, probe_unc, same_volume, unc_share_root,
    DEFAULT_DISK_MARGIN_FLOOR_BYTES, DEFAULT_DISK_MARGIN_RATIO,
};
use platform::protected::{make_absolute, protected_dirs_from_strings};
use platform::retention::{
    cleanup, validate_retention_days, CleanupOutcome, DEFAULT_RETENTION_DAYS,
};
use platform::scan::{scan_path, ScanMode, ScanOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{Emitter, Manager, State};

mod plan_csv;
mod plan_kinds;
mod plan_manifest;
mod plan_ops;
mod recipe_apply;
mod tasks;
mod window_state;

use tasks::{
    ProtectedSource, RecoveryAction, StoredPlan, TaskDetail, TaskPage, TaskPaths, TaskQueue,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

impl IpcError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            detail: None,
            retryable: false,
            task_id: None,
        }
    }
}

impl From<KernelError> for IpcError {
    fn from(error: KernelError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            detail: error.detail,
            retryable: error.retryable,
            task_id: None,
        }
    }
}

fn io_error(error: std::io::Error) -> IpcError {
    let code = if error.kind() == std::io::ErrorKind::NotFound {
        "E2001"
    } else {
        "E9001"
    };
    IpcError {
        code: code.to_string(),
        message: error.to_string(),
        detail: None,
        retryable: false,
        task_id: None,
    }
}

#[derive(Clone)]
struct DataPaths {
    quarantine_dir: PathBuf,
    journal_path: PathBuf,
    queue_path: PathBuf,
    /// C17a 操作日志（审计轨，ADR-0013）。
    oplog_path: PathBuf,
    config_dir: PathBuf,
}

impl DataPaths {
    fn from_app(app: &tauri::App) -> Result<Self, Box<dyn std::error::Error>> {
        let base = app.path().app_data_dir()?;
        std::fs::create_dir_all(&base)?;
        let quarantine_dir = base.join("quarantine");
        std::fs::create_dir_all(&quarantine_dir)?;
        // 配置与容器数据分离（设计 03 §3）：配置走平台惯例配置目录。
        let config_dir = app.path().app_config_dir()?;
        std::fs::create_dir_all(&config_dir)?;
        Ok(Self {
            quarantine_dir,
            journal_path: base.join("journal.jsonl"),
            queue_path: base.join("tasks.json"),
            oplog_path: base.join("oplog.sqlite3"),
            config_dir,
        })
    }
}

struct AppState {
    plans: Mutex<HashMap<String, StoredPlan>>,
    /// C10 解密密码（ADR-0074）：按 planId 暂存，`applyPlan` 取用后即删。
    ///
    /// 只活在内存里——`StoredPlan`／`StoredJob` 会被 `PersistedQueue` 序列化写盘，
    /// 所以密码不进它们；进程重启或崩溃恢复后这张表是空的，加密包任务将以 E3008
    /// 失败并提示重新预览。
    plan_secrets: Mutex<HashMap<String, String>>,
    paths: DataPaths,
    tasks: TaskQueue,
}

impl AppState {
    fn new(paths: DataPaths, tasks: TaskQueue) -> Self {
        Self {
            plans: Mutex::new(HashMap::new()),
            plan_secrets: Mutex::new(HashMap::new()),
            paths,
            tasks,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct C09Params {
    keep: Option<String>,
    quick_window_bytes: Option<u64>,
    protected_dirs: Option<Vec<String>>,
    include_zero_byte: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IpcMeta {
    contract_version: u32,
    kernel_version: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageInfo {
    quarantine_dir: String,
    journal_file: String,
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn getIpcMeta() -> IpcMeta {
    IpcMeta {
        contract_version: 1,
        kernel_version: kernel::KERNEL_VERSION.to_string(),
    }
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn getStorageInfo(state: State<'_, AppState>) -> StorageInfo {
    StorageInfo {
        quarantine_dir: state.paths.quarantine_dir.display().to_string(),
        journal_file: state.paths.journal_path.display().to_string(),
    }
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn planOperation(
    state: State<'_, AppState>,
    kind: String,
    params: Value,
    source_paths: Vec<String>,
) -> Result<PlanDocument, IpcError> {
    // kind 路由（UI 接入切片，设计 12 §3.1）：c09 既有路径；
    // f05/f11/f12 主流程在此采集事实并生成计划；其余 kind E1002 明确拒绝。
    match kind.as_str() {
        "c09" => {}
        "f05" => return plan_f05_shell(&state, params, source_paths),
        "f11" => return plan_f11_shell(&state, params, source_paths),
        "f12" => return plan_f12_shell(&state, params, source_paths),
        "f01" | "f02" | "f03" => {
            return plan_kinds::plan_split_shell(&state, &kind, params).map_err(IpcError::from)
        }
        "f04" => return plan_kinds::plan_merge_shell(&state, params).map_err(IpcError::from),
        "c01" => return plan_kinds::plan_transcode_shell(&state, params).map_err(IpcError::from),
        "c02" => {
            return plan_kinds::plan_line_endings_shell(&state, params).map_err(IpcError::from)
        }
        "c03" => return plan_kinds::plan_replace_shell(&state, params).map_err(IpcError::from),
        "f06" | "f07" | "f08" | "f09" => {
            return plan_csv::plan_csv_shell(&state, &kind, params).map_err(IpcError::from)
        }
        "f10" => return plan_csv::plan_listing_shell(&state, params).map_err(IpcError::from),
        "c04" | "c11" | "c12" | "c13" | "c14" => {
            return plan_ops::plan_op_shell(&state, &kind, params).map_err(IpcError::from)
        }
        "c07" => return plan_manifest::plan_manifest_shell(&state, params).map_err(IpcError::from),
        "c08" => {
            return plan_manifest::plan_rule_archive_shell(&state, params).map_err(IpcError::from)
        }
        "c10" => return plan_manifest::plan_zip_shell(&state, params).map_err(IpcError::from),
        other => {
            return Err(IpcError::new(
                "E1002",
                format!("this shell slice does not support plan kind: {other}"),
            ))
        }
    }
    if source_paths.len() != 1 {
        return Err(IpcError::new(
            "E1001",
            "c09 requires exactly one source directory",
        ));
    }
    let params = if params.is_null() {
        C09Params::default()
    } else {
        serde_json::from_value::<C09Params>(params)
            .map_err(|error| IpcError::new("E1002", format!("invalid c09 params: {error}")))?
    };
    let keep = match params.keep.as_deref() {
        None | Some("earliest") => KeepPolicy::Earliest,
        Some("shortest-path") => KeepPolicy::ShortestPath,
        Some("newest") => KeepPolicy::Newest,
        Some(other) => {
            return Err(IpcError::new(
                "E1002",
                format!("invalid keep policy: {other}"),
            ))
        }
    };
    let scan = ScanOptions {
        mode: ScanMode::Full,
        quick_window: params.quick_window_bytes,
    };
    scan.validate()
        .map_err(|message| IpcError::new("E1002", message))?;
    let root = make_absolute(Path::new(&source_paths[0]));
    let outcome = scan_path(&root, scan).map_err(io_error)?;
    let candidates = build_candidates(&outcome);
    let include_zero_byte = params.include_zero_byte.unwrap_or(false);
    let report = group_duplicates(
        &candidates,
        GroupingOptions {
            mode: GroupingMode::Full,
            include_zero_byte,
        },
    );
    let (config_protected, _, _) = security_settings(&state);
    let (protected_list, protected_source) = match params.protected_dirs.as_deref() {
        Some(list) => (list.to_vec(), ProtectedSource::Params),
        None => (config_protected, ProtectedSource::Config),
    };
    let protected_dirs = protected_dirs_from_strings(&protected_list).map_err(IpcError::from)?;
    let case_sense = CliFsAdapter::new(None).case_sensitivity();
    let files = outcome
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| PlanFile {
            id: index as u64,
            path: file.path.to_string_lossy().into_owned(),
            canonical_path: if protected_dirs.is_empty() {
                None
            } else {
                // Windows canonicalize 恒带 \\?\ 前缀，由适配层剥回显示形式。
                platform::fs::canonicalize_display(&file.path)
            },
            mtime_nanos: file.mtime_nanos,
            full_hash: file.full_hash.clone(),
        })
        .collect();
    let plan = plan_c09(PlanC09Input {
        plan_id: new_plan_id(&root),
        keep,
        mode: GroupingMode::Full,
        hash_algorithm: "blake3".to_string(),
        quick_window_bytes: params.quick_window_bytes,
        include_zero_byte,
        protected_dirs: protected_dirs.clone(),
        case_sense,
        files,
        report,
        skipped_paths: outcome
            .skipped
            .iter()
            .map(|(path, _)| path.to_string_lossy().into_owned())
            .collect(),
    })
    .map_err(IpcError::from)?;

    state.plans.lock().unwrap().insert(
        plan.plan_id.clone(),
        StoredPlan {
            plan: plan.clone(),
            protected_dirs,
            case_sense,
            protected_source,
        },
    );
    Ok(plan)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskHandle {
    task_id: String,
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn applyPlan(
    state: State<'_, AppState>,
    plan_id: String,
    options: Option<ApplyOptions>,
) -> Result<TaskHandle, IpcError> {
    let stored = {
        let plans = state.plans.lock().unwrap();
        plans
            .get(&plan_id)
            .cloned()
            .ok_or_else(|| IpcError::new("E1001", "plan not found; generate a fresh preview"))?
    };
    // S02：配置来源的保护名单在 plan→apply 间变化 → 旧预览失效，必须重新预览。
    if stored.protected_source == ProtectedSource::Config {
        let (current_list, _, _) = security_settings(&state);
        let current = protected_dirs_from_strings(&current_list).map_err(IpcError::from)?;
        if !same_protected_dirs(&stored.protected_dirs, &current) {
            return Err(IpcError::new(
                "E1002",
                "protectedDirs changed after preview; regenerate preview",
            ));
        }
    }
    // S01：执行前预检；磁盘空间不足整任务阻断，零副作用，且不提供跳过入口。
    let report = preflight_stored_plan(&state, &stored).map_err(IpcError::from)?;
    if let Some(error) = apply_gate_block(&report) {
        return Err(error);
    }
    state.plans.lock().unwrap().remove(&plan_id);
    // 一次性秘密：取走即从内存表删除（ADR-0074 §3.6）。
    let secret = state.plan_secrets.lock().unwrap().remove(&plan_id);
    // 方案 §7.7：桌面壳在取消后要问"已完成 N 项，是否一并撤销？"，
    // 因此执行期一律保留已应用条目（Journal 记账不动），答复为"撤销"时再走 undoPlan。
    let mut options = options.unwrap_or_default();
    options.cancel_policy = kernel::apply::CancelPolicy::KeepApplied;
    let task_id = state.tasks.enqueue_apply(stored, options, secret);
    Ok(TaskHandle { task_id })
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn preflightPlan(state: State<'_, AppState>, plan_id: String) -> Result<PreflightReport, IpcError> {
    let stored = {
        let plans = state.plans.lock().unwrap();
        plans
            .get(&plan_id)
            .cloned()
            .ok_or_else(|| IpcError::new("E1001", "plan not found; generate a fresh preview"))?
    };
    preflight_stored_plan(&state, &stored).map_err(IpcError::from)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn recoverTask(
    state: State<'_, AppState>,
    task_id: String,
    action: RecoveryAction,
) -> Result<TaskHandle, IpcError> {
    let task_id = state
        .tasks
        .recover(&task_id, action)
        .map_err(IpcError::from)?;
    Ok(TaskHandle { task_id })
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn retryTask(
    state: State<'_, AppState>,
    task_id: String,
    entry_ids: Option<Vec<String>>,
) -> Result<TaskHandle, IpcError> {
    let task_id = state
        .tasks
        .retry(&task_id, entry_ids)
        .map_err(IpcError::from)?;
    Ok(TaskHandle { task_id })
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn undoPlan(state: State<'_, AppState>, plan_id: String) -> Result<TaskHandle, IpcError> {
    let task_id = state.tasks.enqueue_undo(plan_id);
    Ok(TaskHandle { task_id })
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn cancelTask(state: State<'_, AppState>, task_id: String) {
    state.tasks.cancel(&task_id);
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn pauseTask(state: State<'_, AppState>, task_id: String) {
    state.tasks.pause(&task_id);
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn resumeTask(state: State<'_, AppState>, task_id: String) {
    state.tasks.resume(&task_id);
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn listTasks(state: State<'_, AppState>, offset: Option<usize>, limit: Option<usize>) -> TaskPage {
    state.tasks.list(offset.unwrap_or(0), limit.unwrap_or(50))
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn getTask(state: State<'_, AppState>, task_id: String) -> Result<TaskDetail, IpcError> {
    state
        .tasks
        .get(&task_id)
        .ok_or_else(|| IpcError::new("E1001", "task not found"))
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn cleanupRetention(
    state: State<'_, AppState>,
    retention_days: Option<u64>,
    apply: Option<bool>,
) -> Result<CleanupOutcome, IpcError> {
    let days = retention_days.unwrap_or(DEFAULT_RETENTION_DAYS);
    validate_retention_days(days).map_err(IpcError::from)?;
    let mut journal =
        FileJournal::open(state.paths.journal_path.clone()).map_err(IpcError::from)?;
    let mut outcome = cleanup(
        &mut journal,
        &state.paths.quarantine_dir,
        days,
        apply.unwrap_or(false),
    )
    .map_err(IpcError::from)?;
    // C17a 操作日志随保留期清理（设计 10 §4：天数即 logging.retentionDays，
    // 不把日志保留期当撤销窗口）；清理失败降级诊断输出，不影响演练结果。
    if apply.unwrap_or(false) {
        match SqliteOperationLog::open(state.paths.oplog_path.clone()) {
            Ok((mut oplog, warning)) => {
                if let Some(warning) = warning {
                    eprintln!("{}: {}", warning.code, warning.message);
                }
                match oplog.purge_before(&outcome.cutoff) {
                    Ok(removed) => outcome.removed_log_records = removed,
                    Err(error) => eprintln!("{}: {}", error.code, error.message),
                }
            }
            Err(error) => eprintln!("{}: {}", error.code, error.message),
        }
    }
    Ok(outcome)
}

// ── 配置 / 界面状态 / 处理模板（设计 03 §7）──────────────────────────

fn config_store(state: &State<'_, AppState>) -> ConfigStore {
    ConfigStore::new(state.paths.config_dir.clone())
}

/// 读取安全 scope 的生效值；缺失时回落到基线默认（方案 §9.1、§3.3）。
fn security_settings(state: &State<'_, AppState>) -> (Vec<String>, u64, f64) {
    let store = config_store(state);
    match store.load_config() {
        Ok(load) => {
            if let Some(warning) = &load.warning {
                eprintln!("{}: {}", warning.code, warning.message);
            }
            let security = load.document.scopes.security.unwrap_or_default();
            (
                security.protected_dirs.unwrap_or_default(),
                security
                    .disk_space_margin_floor_bytes
                    .unwrap_or(DEFAULT_DISK_MARGIN_FLOOR_BYTES),
                security
                    .disk_space_margin_ratio
                    .unwrap_or(DEFAULT_DISK_MARGIN_RATIO),
            )
        }
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            (
                Vec::new(),
                DEFAULT_DISK_MARGIN_FLOOR_BYTES,
                DEFAULT_DISK_MARGIN_RATIO,
            )
        }
    }
}

fn same_protected_dirs(left: &[ProtectedDir], right: &[ProtectedDir]) -> bool {
    let mut left: Vec<(String, Option<String>)> = left
        .iter()
        .map(|entry| (entry.literal.clone(), entry.canonical.clone()))
        .collect();
    let mut right: Vec<(String, Option<String>)> = right
        .iter()
        .map(|entry| (entry.literal.clone(), entry.canonical.clone()))
        .collect();
    left.sort();
    right.sort();
    left.dedup();
    right.dedup();
    left == right
}

/// S01：采集事实并交给 kernel 纯函数判定；不产生任何写操作。
/// `State` 只负责提供数据目录与当前设置，判定本体在 `preflight_gate`（可单测）。
fn preflight_stored_plan(
    state: &State<'_, AppState>,
    stored: &StoredPlan,
) -> Result<PreflightReport, KernelError> {
    let (_, margin_floor, margin_ratio) = security_settings(state);
    preflight_gate(
        stored,
        &state.paths.quarantine_dir,
        margin_floor,
        margin_ratio,
    )
}

/// 预检闸：每次调用都重新采集磁盘余量与路径事实，**不缓存预览期的结论**
/// （TC-S01-PRE-04「环境变化重算」）。零写入：本函数只 stat/读空间，
/// 由 `preflight_gate_writes_nothing` 钉住。
pub(crate) fn preflight_gate(
    stored: &StoredPlan,
    quarantine_dir: &Path,
    margin_floor_bytes: u64,
    margin_ratio: f64,
) -> Result<PreflightReport, KernelError> {
    let plan = &stored.plan;
    let mut input = PreflightInput {
        protected_dirs: stored.protected_dirs.clone(),
        case_sense: stored.case_sense,
        ..Default::default()
    };
    let long_paths = long_path_supported();
    let mut unc_roots = BTreeSet::new();
    // 网络路径要按**共享根**探测（不是 probe_root 的"最近存在目录"，见 unc_share_root 注释）。
    if let Some(share) = unc_share_root(quarantine_dir) {
        unc_roots.insert(share.to_string_lossy().into_owned());
    }

    if plan.kind == "c09" {
        let mut required = 0u64;
        for entry in &plan.entries {
            if entry.blocked.is_some() || entry.resolution == Some(PlanResolution::Skip) {
                continue;
            }
            let size = entry
                .expected
                .as_ref()
                .map(|expected| expected.size)
                .unwrap_or(0);
            if !same_volume(Path::new(&entry.source), quarantine_dir) {
                required = required.saturating_add(size);
            }
        }
        input.disk = Some(DiskSpaceCheck {
            required_bytes: required,
            available_bytes: available_space(quarantine_dir)?,
            temp_peak_bytes: 0,
            margin_floor_bytes,
            margin_ratio,
        });
    }

    for entry in &plan.entries {
        if entry.blocked.is_some() || entry.resolution == Some(PlanResolution::Skip) {
            continue;
        }
        let source = Path::new(&entry.source);
        input.paths.push(path_check(source, long_paths));
        if let Some(target) = &entry.target {
            input.paths.push(path_check(Path::new(target), long_paths));
        }
        if let Some(share) = unc_share_root(source) {
            unc_roots.insert(share.to_string_lossy().into_owned());
        }
    }
    for root in unc_roots {
        if let Some(check) = probe_unc(Path::new(&root)) {
            input.unc.push(check);
        }
    }
    Ok(run_preflight(&input))
}

/// 预检结论 → 派发前的阻断错误（TC-S01-PRE-03／RBK-01）：返回 `Some` 时
/// `applyPlan` 既没出队、也没写 Journal，任务零副作用。
pub(crate) fn apply_gate_block(report: &PreflightReport) -> Option<IpcError> {
    if !report.is_task_blocked() {
        return None;
    }
    Some(IpcError {
        code: "E2004".to_string(),
        message: "preflight blocked: insufficient disk space".to_string(),
        detail: serde_json::to_string(report).ok(),
        retryable: true,
        task_id: None,
    })
}

fn to_value<T: Serialize>(value: &T) -> Result<Value, IpcError> {
    serde_json::to_value(value).map_err(|error| IpcError::new("E9001", error.to_string()))
}

fn patch_object(patch: Value) -> Result<Map<String, Value>, IpcError> {
    patch
        .as_object()
        .cloned()
        .ok_or_else(|| IpcError::new("E1002", "patch must be a JSON object"))
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn getConfig(state: State<'_, AppState>, scope: Option<String>) -> Result<Value, IpcError> {
    let store = config_store(&state);
    let load = store.load_config().map_err(IpcError::from)?;
    if let Some(warning) = &load.warning {
        // E5009/E1007 的用户提示待日志与 Toast 接入；此处先落诊断输出。
        eprintln!("{}: {}", warning.code, warning.message);
    }
    match scope {
        Some(scope) => to_value(&load.document.scope_view(&scope).map_err(IpcError::from)?),
        None => to_value(&load.document),
    }
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn setConfig(state: State<'_, AppState>, scope: String, patch: Value) -> Result<Value, IpcError> {
    let store = config_store(&state);
    let mut load = store.load_config().map_err(IpcError::from)?;
    if load.read_only {
        return Err(IpcError::new(
            "E1007",
            "config was written by a newer version; this app is read-only",
        ));
    }
    load.document
        .apply_patch(&scope, &patch_object(patch)?)
        .map_err(IpcError::from)?;
    store
        .save_config(&mut load.document)
        .map_err(IpcError::from)?;
    to_value(&load.document.scope_view(&scope).map_err(IpcError::from)?)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn resetScope(state: State<'_, AppState>, scope: String) -> Result<Value, IpcError> {
    let store = config_store(&state);
    let mut load = store.load_config().map_err(IpcError::from)?;
    if load.read_only {
        return Err(IpcError::new(
            "E1007",
            "config was written by a newer version; this app is read-only",
        ));
    }
    load.document.reset_scope(&scope).map_err(IpcError::from)?;
    store
        .save_config(&mut load.document)
        .map_err(IpcError::from)?;
    to_value(&load.document.scope_view(&scope).map_err(IpcError::from)?)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn getUiState(state: State<'_, AppState>) -> UiState {
    config_store(&state).load_ui_state()
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn setUiState(state: State<'_, AppState>, patch: Value) -> Result<UiState, IpcError> {
    let store = config_store(&state);
    let mut current = store.load_ui_state();
    current
        .apply_patch(&patch_object(patch)?)
        .map_err(IpcError::from)?;
    store.save_ui_state(&mut current).map_err(IpcError::from)?;
    Ok(current)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn resetUiState(state: State<'_, AppState>) -> Result<UiState, IpcError> {
    config_store(&state)
        .reset_ui_state()
        .map_err(IpcError::from)
}

/// 把模板变成**预览计划**（设计 03 §命令表；OQ-40 裁决①＝入参加文件集）。
///
/// 三条不变式写在这里：
/// - **同出口**：槽位注入后直接走 `planOperation` 用的那条路由，不另造第二套采集；
/// - **只出计划不执行**：本命令不碰队列（方案 §15.2.5「载入后必须重新预览，不得直接 apply」）；
/// - **路径不来自模板**：模板按不变量 13 不存路径，路径全部来自 `pathParams` 入参，
///   键位由 `recipe_apply` 的槽位表校验（缺、多、形状不符都报错，不猜）。
#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn loadRecipe(
    state: State<'_, AppState>,
    recipe_id: String,
    path_params: Value,
) -> Result<PlanDocument, IpcError> {
    let document = config_store(&state)
        .get_recipe(&recipe_id)
        .map_err(IpcError::from)?;
    let object = match path_params {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        _ => {
            return Err(IpcError::new(
                "E1002",
                "loadRecipe: pathParams must be an object of path slots",
            ))
        }
    };
    let invocation = recipe_apply::build_invocation(&document, &object)
        .map_err(|message| IpcError::new("E1002", format!("loadRecipe: {message}")))?;
    // c09 的根走 sourcePaths 通道、其余走 params——差别已经在槽位表里表达完，
    // 这里只有一条出口：同一次 planOperation 调用。
    planOperation(
        state,
        invocation.kind,
        invocation.params,
        invocation.source_paths,
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveRecipeInput {
    #[serde(default)]
    recipe_id: Option<String>,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "match")]
    match_: Option<RecipeMatch>,
    #[serde(default)]
    kind: Option<PlanAction>,
    #[serde(default)]
    params_snapshot: Option<Map<String, Value>>,
    #[serde(default)]
    steps: Option<Vec<TemplateStep>>,
    /// 单步模板绑定的功能码（OQ-36 裁决①：`TemplateStep.feature`）。
    /// 界面保存时传的就是当前功能页的 id；没传就按动作推断，**只在动作唯一时**才补得上。
    #[serde(default)]
    feature: Option<String>,
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn saveRecipe(state: State<'_, AppState>, recipe: Value) -> Result<RecipeDocument, IpcError> {
    let input: SaveRecipeInput = serde_json::from_value(recipe)
        .map_err(|error| IpcError::new("E1002", format!("invalid recipe input: {error}")))?;
    let recipe_id = input.recipe_id.unwrap_or_else(|| new_ulid(&input.name));
    let created_at = platform::fs::now_rfc3339();
    let mut document = match input.steps {
        Some(steps) => RecipeDocument {
            recipe_id,
            schema_version: kernel::recipe::CURRENT_RECIPE_SCHEMA_VERSION,
            name: input.name,
            description: input.description,
            steps: steps
                .into_iter()
                .map(|step| {
                    let feature = step.feature.clone().or_else(|| {
                        kernel::recipe::feature_for_action(step.kind).map(str::to_string)
                    });
                    TemplateStep { feature, ..step }
                })
                .collect(),
            match_: input.match_,
            source_format: None,
            created_at,
            updated_at: None,
            last_used_at: None,
            usage_count: None,
            pinned: None,
        },
        None => {
            let kind = input
                .kind
                .ok_or_else(|| IpcError::new("E1001", "saveRecipe requires kind or steps"))?;
            let params = input.params_snapshot.unwrap_or_default();
            let feature = input
                .feature
                .or_else(|| kernel::recipe::feature_for_action(kind).map(str::to_string));
            let mut document = RecipeDocument::from_single_step(
                recipe_id, input.name, kind, params, created_at, feature,
            );
            document.description = input.description;
            document.match_ = input.match_;
            document
        }
    };
    let store = config_store(&state);
    // 模板不保存路径（方案 §15.2.1、不变量 13）：界面已经把源/输出移出快照，这里再兜一层——
    // 保存入口只有一个，但调用方不止一个（将来的 Web 形态、脚本化保存），
    // 只靠"调用方自觉"迟早会有一条路径漏进模板。
    // 返回的被删键清单在这条路上刻意丢掉：这里没有可写的日志轨（诊断日志轨仍待 ADR），
    // 而它真正的用处是让调用方/测试能断言"删了哪些"，不是给用户看的。
    let _removed = document.strip_paths();
    store.save_recipe(&mut document).map_err(IpcError::from)?;
    Ok(document)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn getRecipe(state: State<'_, AppState>, recipe_id: String) -> Result<RecipeDocument, IpcError> {
    config_store(&state)
        .get_recipe(&recipe_id)
        .map_err(IpcError::from)
}

/// 静态评估一份模板的「一键套用」面（OQ-41 裁决②落地后的入口）。
///
/// 界面据此决定给不给「套用」按钮：能套＝拿到槽位表去渲染输入框；
/// 不能套（未绑定/多步/正文不在计划参数里/旧压缩模板）＝拿到原因如实展示，
/// **不渲染按钮**——同 ADR-0072 的「不给假按钮」。判定与 `loadRecipe` 的
/// `build_invocation` 同源（`recipe_apply::assess`），不会出现 describe 说能套、
/// loadRecipe 却报错的两套口径。
#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn describeRecipe(
    state: State<'_, AppState>,
    recipe_id: String,
) -> Result<recipe_apply::RecipeApplySpec, IpcError> {
    let document = config_store(&state)
        .get_recipe(&recipe_id)
        .map_err(IpcError::from)?;
    Ok(recipe_apply::describe(&document))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecipePage {
    items: Vec<RecipeDocument>,
    total: usize,
}
#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn listRecipes(
    state: State<'_, AppState>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> RecipePage {
    let list = config_store(&state).list_recipes();
    let total = list.recipes.len();
    let items = list
        .recipes
        .into_iter()
        .skip(offset.unwrap_or(0))
        .take(limit.unwrap_or(usize::MAX))
        .collect();
    RecipePage { items, total }
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn deleteRecipe(state: State<'_, AppState>, recipe_id: String) -> Result<(), IpcError> {
    config_store(&state)
        .delete_recipe(&recipe_id)
        .map_err(IpcError::from)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportPayload {
    payload: String,
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn exportRecipe(state: State<'_, AppState>, recipe_id: String) -> Result<ExportPayload, IpcError> {
    let payload = config_store(&state)
        .export_recipe(&recipe_id)
        .map_err(IpcError::from)?;
    Ok(ExportPayload { payload })
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn importRecipe(state: State<'_, AppState>, payload: String) -> Result<RecipeDocument, IpcError> {
    // 导入即低版本迁移、高版本 E1007；导入后必须重新预览（不变量 13）。
    config_store(&state)
        .import_recipe(&payload)
        .map_err(IpcError::from)
}

// ── f05/f11/f12 主流程计划（UI 接入切片，设计 12 §3.1）──────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct F05ShellParams {
    file: String,
    #[serde(default)]
    target_bytes: Option<u64>,
    #[serde(default)]
    rows: Option<u64>,
    output_dir: String,
    #[serde(default)]
    delimiter: Option<String>,
    #[serde(default = "default_true")]
    keep_header: bool,
    #[serde(default)]
    plan_id: Option<String>,
}

fn default_true() -> bool {
    true
}

fn plan_f05_shell(
    state: &State<'_, AppState>,
    params: Value,
    _source_paths: Vec<String>,
) -> Result<PlanDocument, IpcError> {
    let params: F05ShellParams = serde_json::from_value(params)
        .map_err(|error| IpcError::new("E1002", format!("invalid f05 params: {error}")))?;
    let basis = match (params.target_bytes, params.rows) {
        (Some(bytes), None) => kernel::csv::CsvSplitBasis::Bytes(bytes),
        (None, Some(rows)) => kernel::csv::CsvSplitBasis::Rows(rows),
        _ => {
            return Err(IpcError::new(
                "E1002",
                "f05 requires exactly one of targetBytes or rows",
            ))
        }
    };
    let delimiter = match params.delimiter.as_deref() {
        Some(value) => Some(
            Delimiter::parse(value)
                .ok_or_else(|| IpcError::new("E1002", format!("invalid delimiter: {value}")))?,
        ),
        None => None,
    };
    let absolute = make_absolute(Path::new(&params.file));
    let metadata = std::fs::metadata(&absolute)
        .map_err(|error| IpcError::new("E2001", format!("{}: {error}", absolute.display())))?;
    if !metadata.is_file() {
        return Err(IpcError::new("E1002", "f05 requires a regular file"));
    }
    let output_dir = make_absolute(Path::new(&params.output_dir));
    std::fs::create_dir_all(&output_dir)
        .map_err(|error| IpcError::new("E1004", format!("{}: {error}", output_dir.display())))?;
    let sample_window = metadata.len().max(4096);
    let layout = platform::csv::csv_split_layout(
        &absolute,
        platform::csv::CsvScanOptions {
            delimiter,
            keep_header: params.keep_header,
            basis,
            sample_window_bytes: sample_window,
        },
    )
    .map_err(|error| IpcError::new("E1004", format!("{}: {error}", absolute.display())))?;
    let plan_id = params.plan_id.unwrap_or_else(|| new_ulid("f05"));
    let target = output_dir.join(
        absolute
            .file_stem()
            .map(|stem| format!("{}.part-0001.csv", stem.to_string_lossy()))
            .unwrap_or_else(|| "part-0001.csv".into()),
    );
    let plan = kernel::csv::plan_f05(kernel::csv::PlanF05Input {
        plan_id: plan_id.clone(),
        basis,
        keep_header: params.keep_header,
        delimiter,
        output_dir: output_dir.to_string_lossy().into_owned(),
        protected_dirs: Vec::new(),
        case_sense: CliFsAdapter::new(None).case_sensitivity(),
        existing_targets: vec![target.to_string_lossy().into_owned()],
        files: vec![kernel::csv::PlanF05File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path: platform::fs::canonicalize_display(&absolute),
            // C18 计划幂等（ADR-0039）：groupId 确定性派生。
            group_id: platform::ids::derived_ulid(&format!("f05:{plan_id}")),
            layout,
        }],
    })
    .map_err(IpcError::from)?;
    state.plans.lock().unwrap().insert(
        plan.plan_id.clone(),
        StoredPlan {
            plan: plan.clone(),
            protected_dirs: Vec::new(),
            case_sense: CliFsAdapter::new(None).case_sensitivity(),
            protected_source: ProtectedSource::Params,
        },
    );
    Ok(plan)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct F11ShellParams {
    list: String,
    root: String,
    #[serde(default)]
    plan_id: Option<String>,
}

fn plan_f11_shell(
    state: &State<'_, AppState>,
    params: Value,
    _source_paths: Vec<String>,
) -> Result<PlanDocument, IpcError> {
    let params: F11ShellParams = serde_json::from_value(params)
        .map_err(|error| IpcError::new("E1002", format!("invalid f11 params: {error}")))?;
    let root = make_absolute(Path::new(&params.root));
    if !root.is_dir() {
        return Err(IpcError::new("E1003", "f11 root must be a directory"));
    }
    let rows: Vec<kernel::create_dirs::PlanF11Row> = params
        .list
        .lines()
        .enumerate()
        .map(|(index, line)| kernel::create_dirs::PlanF11Row {
            line: index as u64 + 1,
            path: line.trim().to_string(),
        })
        .filter(|row| !row.path.is_empty())
        .collect();
    // 与 CLI `run_f11` 同口径：按清单路径的**每一级前缀**探测现存目标并区分文件/目录。
    // 只探叶子路径会漏掉"父级位置是文件"——那种分支到 apply 期才失败。
    let root_text = root.to_string_lossy().into_owned();
    let (existing, existing_files) = f11_existing_facts(&root_text, &rows);
    let plan_id = params.plan_id.unwrap_or_else(|| new_ulid("f11"));
    let plan = kernel::create_dirs::plan_f11(kernel::create_dirs::PlanF11Input {
        plan_id: plan_id.clone(),
        root: root_text,
        canonical_root: platform::fs::canonicalize_display(&root),
        rows,
        existing_targets: existing,
        existing_files,
        protected_dirs: Vec::new(),
        case_sense: CliFsAdapter::new(None).case_sensitivity(),
        name_rules: platform::fs::name_rules(),
    })
    .map_err(IpcError::from)?;
    state.plans.lock().unwrap().insert(
        plan.plan_id.clone(),
        StoredPlan {
            plan: plan.clone(),
            protected_dirs: Vec::new(),
            case_sense: CliFsAdapter::new(None).case_sensitivity(),
            protected_source: ProtectedSource::Params,
        },
    );
    Ok(plan)
}

/// F11 事实采集：对清单每条有效路径的**每一级前缀**探测存在性，并把其中的文件单独列出。
/// kernel 需要这两份事实才能把"父级位置是文件"的分支在计划期跳过（方案 §5.15 的集中报告）。
fn f11_existing_facts(
    root_text: &str,
    rows: &[kernel::create_dirs::PlanF11Row],
) -> (Vec<String>, Vec<String>) {
    let mut existing: Vec<String> = Vec::new();
    let mut existing_files: Vec<String> = Vec::new();
    for row in rows {
        let Ok(normalized) =
            kernel::create_dirs::normalize_list_path(&row.path, platform::fs::name_rules())
        else {
            continue;
        };
        let mut prefix = String::new();
        for component in normalized.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            let target = kernel::create_dirs::target_for_list_path(root_text, &prefix);
            let Ok(metadata) = std::fs::metadata(&target) else {
                continue;
            };
            if metadata.is_file() {
                existing_files.push(target.clone());
            }
            existing.push(target);
        }
    }
    (existing, existing_files)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct F12ShellParams {
    dir: String,
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    suffix: Option<String>,
    #[serde(default)]
    find: Option<String>,
    #[serde(default = "default_empty")]
    replace: String,
    #[serde(default = "default_scope")]
    scope: String,
    #[serde(default)]
    plan_id: Option<String>,
}

fn default_empty() -> String {
    String::new()
}

fn default_scope() -> String {
    "full".to_string()
}

fn plan_f12_shell(
    state: &State<'_, AppState>,
    params: Value,
    _source_paths: Vec<String>,
) -> Result<PlanDocument, IpcError> {
    let params: F12ShellParams = serde_json::from_value(params)
        .map_err(|error| IpcError::new("E1002", format!("invalid f12 params: {error}")))?;
    let scope = kernel::rename::RenameScope::parse(&params.scope)
        .ok_or_else(|| IpcError::new("E1002", format!("invalid scope: {}", params.scope)))?;
    let dir = make_absolute(Path::new(&params.dir));
    if !dir.is_dir() {
        return Err(IpcError::new("E1003", "f12 dir must be a directory"));
    }
    let mut items = Vec::new();
    let mut existing = Vec::new();
    let long_path_supported = platform::preflight::long_path_supported();
    let mut access_facts = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|error| IpcError::new("E1004", format!("{}: {error}", dir.display())))?
    {
        let entry = entry.map_err(|error| IpcError::new("E1004", error.to_string()))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        existing.push(path.to_string_lossy().into_owned());
        // 只读在预览期点名（方案 §5.13）；占用属预检面，计划期不探测。
        access_facts.push(platform::preflight::path_check_light(
            &path,
            long_path_supported,
        ));
        items.push(kernel::rename::PlanF12Item {
            path: path.to_string_lossy().into_owned(),
            name,
        });
    }
    let plan_id = params.plan_id.unwrap_or_else(|| new_ulid("f12"));
    let plan = kernel::rename::plan_f12(kernel::rename::PlanF12Input {
        plan_id: plan_id.clone(),
        items,
        existing_targets: existing,
        protected_dirs: Vec::new(),
        case_sense: CliFsAdapter::new(None).case_sensitivity(),
        name_rules: platform::fs::name_rules(),
        long_path_supported,
        access_facts,
        rule: kernel::rename::PlanF12Rule {
            prefix: params.prefix,
            suffix: params.suffix,
            find: params.find,
            replace: params.replace,
            scope,
        },
    })
    .map_err(IpcError::from)?;
    state.plans.lock().unwrap().insert(
        plan.plan_id.clone(),
        StoredPlan {
            plan: plan.clone(),
            protected_dirs: Vec::new(),
            case_sense: CliFsAdapter::new(None).case_sensitivity(),
            protected_source: ProtectedSource::Params,
        },
    );
    Ok(plan)
}

/// C05 预览面板（§7.4）：对已生成的重命名族计划聚合对照行与计数，零副作用。
#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn renamePreview(state: State<'_, AppState>, plan_id: String) -> Result<Value, IpcError> {
    let plan = state
        .plans
        .lock()
        .unwrap()
        .get(&plan_id)
        .cloned()
        .ok_or_else(|| IpcError::new("E1001", "plan not found; generate a fresh preview"))?
        .plan;
    let preview = kernel::preview::rename_preview(&plan).map_err(IpcError::from)?;
    serde_json::to_value(preview).map_err(|error| IpcError::new("E9001", error.to_string()))
}

/// C06 回滚预演（§7.8"回滚前重新做冲突检测"）：零副作用。
#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn undoDryRun(state: State<'_, AppState>, plan_id: String) -> Result<Value, IpcError> {
    let mut journal =
        FileJournal::open(state.paths.journal_path.clone()).map_err(IpcError::from)?;
    let adapter = CliFsAdapter::new(None);
    let preview =
        kernel::apply::undo_dry_run(&plan_id, &mut journal, &adapter).map_err(IpcError::from)?;
    serde_json::to_value(preview).map_err(|error| IpcError::new("E9001", error.to_string()))
}

fn build_candidates(outcome: &platform::scan::ScanOutcome) -> Vec<DuplicateCandidate> {
    outcome
        .files
        .iter()
        .enumerate()
        .filter_map(|(index, file)| {
            if file.size == 0 {
                return Some(DuplicateCandidate {
                    id: index as u64,
                    size: 0,
                    quick_hash: None,
                    full_hash: None,
                    identity: file.identity,
                });
            }
            file.full_hash.as_ref().map(|hash| DuplicateCandidate {
                id: index as u64,
                size: file.size,
                quick_hash: file.quick_hash.clone(),
                full_hash: Some(hash.clone()),
                identity: file.identity,
            })
        })
        .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let paths = DataPaths::from_app(app)?;
            let handle = app.handle().clone();
            // LicenseService 装配点（方案 13.8：接口不变、只换实现）：
            // 单机版注入 AlwaysAllow；商业版/QuotaBased 只替换此处实现。
            let config_dir = paths.config_dir.clone();
            let tasks = TaskQueue::with_license(
                TaskPaths {
                    quarantine_dir: paths.quarantine_dir.clone(),
                    journal_path: paths.journal_path.clone(),
                    queue_path: paths.queue_path.clone(),
                    oplog_path: paths.oplog_path.clone(),
                },
                Arc::new(move |event, payload| {
                    if let Err(error) = handle.emit(event, payload) {
                        eprintln!("E9001: failed to emit {event}: {error}");
                    }
                }),
                Arc::new(kernel::license::AlwaysAllowLicense),
                // C17a：写入前实时读 `logging.logFullPaths`（03 §4.1，默认 false）。
                Arc::new(move || {
                    ConfigStore::new(config_dir.clone())
                        .load_config()
                        .ok()
                        .and_then(|load| load.document.scopes.logging)
                        .and_then(|logging| logging.log_full_paths)
                        .unwrap_or(false)
                }),
            );
            app.manage(AppState::new(paths, tasks));
            // 设计 03 §11.2／§11.3：把上次关闭窗口时的矩形摆回来；离屏或显示器已被拔掉
            // 就居中主屏并把尺寸压进该屏可用区域。规则本体是 window_state::decide_placement
            // 这个纯函数，真实窗口摆放属桌面实机项。
            if let Some(main) = app.get_webview_window("main") {
                let state = app.state::<AppState>();
                let saved = config_store(&state).load_ui_state().window.clone();
                let monitors: Vec<window_state::MonitorRect> = main
                    .available_monitors()
                    .unwrap_or_default()
                    .iter()
                    .map(|monitor| window_state::MonitorRect {
                        x: i64::from(monitor.position().x),
                        y: i64::from(monitor.position().y),
                        width: i64::from(monitor.size().width),
                        height: i64::from(monitor.size().height),
                    })
                    .collect();
                match window_state::decide_placement(saved.as_ref(), &monitors) {
                    window_state::Placement::Nothing => {}
                    window_state::Placement::SizeOnly { width, height } => {
                        let _ =
                            main.set_size(tauri::PhysicalSize::new(width as u32, height as u32));
                    }
                    window_state::Placement::Restore {
                        x,
                        y,
                        width,
                        height,
                    }
                    | window_state::Placement::Centered {
                        x,
                        y,
                        width,
                        height,
                    } => {
                        let _ =
                            main.set_size(tauri::PhysicalSize::new(width as u32, height as u32));
                        let _ = main.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
                    }
                }
                if saved.and_then(|saved| saved.maximized) == Some(true) {
                    let _ = main.maximize();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // 只在关闭请求时落盘：UiState 本就是"可丢失的本机状态"（03 §11.1），
            // 拖动/缩放过程中逐事件写盘会把配置目录当高频写文件用。
            if !matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                return;
            }
            let state = window.state::<AppState>();
            let Ok(size) = window.inner_size() else {
                return;
            };
            let position = window.outer_position().ok();
            let monitor_id = window
                .current_monitor()
                .ok()
                .flatten()
                .and_then(|monitor| monitor.name().map(|name| name.to_string()));
            let snapshot = window_state::snapshot(
                i64::from(size.width),
                i64::from(size.height),
                position.map_or(0, |position| i64::from(position.x)),
                position.map_or(0, |position| i64::from(position.y)),
                window.is_maximized().unwrap_or(false),
                monitor_id,
            );
            let store = config_store(&state);
            let mut ui_state = store.load_ui_state();
            ui_state.window = Some(snapshot);
            if let Err(error) = store.save_ui_state(&mut ui_state) {
                // UiState 写失败不影响关闭（下次回默认布局），但要在 stderr 留痕。
                eprintln!("{}: failed to persist window geometry", error.code);
            }
        })
        .invoke_handler(tauri::generate_handler![
            getIpcMeta,
            getStorageInfo,
            planOperation,
            applyPlan,
            preflightPlan,
            recoverTask,
            retryTask,
            renamePreview,
            undoDryRun,
            undoPlan,
            cancelTask,
            pauseTask,
            resumeTask,
            listTasks,
            getTask,
            cleanupRetention,
            getConfig,
            setConfig,
            resetScope,
            getUiState,
            setUiState,
            resetUiState,
            loadRecipe,
            describeRecipe,
            saveRecipe,
            getRecipe,
            listRecipes,
            deleteRecipe,
            exportRecipe,
            importRecipe
        ])
        .run(tauri::generate_context!())
        .expect("failed to run BatchDesk desktop");
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // ── loadRecipe 槽位表的防漂移断言（OQ-40 裁决①）──────────────────────────
    //
    // 为什么不能只测 `build_invocation` 自己：它把键写进 params，而**真正决定成败的是
    // 各路由结构体的字段名**。表里某个键一旦与结构体不符（改名、camelCase 形态写错、
    // 单复数搞混），`build_invocation` 照样成功、路由却会带着空输入去采集事实——
    // 那是最坏的一种漂移：命令不报错，计划是错的。
    //
    // 所以每条断言都是「表注入的键 → 解进真实结构体 → 字段值必须等于注入值」，
    // 键名不符时字段会留在默认值（空串/空列表），断言立刻红。

    fn slots_of(
        feature: &str,
        // 快照里带上该路由**必填的非路径字段**（如 `target` 编码、`rules` 规则），
        // 否则解结构体失败的是这些字段，而不是我要证的槽位键——断言就失去针对性了。
        // 采样键不在此列：计划快照从不带它们（OQ-41），由映射表补，见 slots_of_sampling。
        snapshot_pairs: &[(&str, serde_json::Value)],
        path_params: &[(&str, serde_json::Value)],
    ) -> serde_json::Value {
        slots_of_sampling(feature, snapshot_pairs, None, path_params)
    }

    fn slots_of_sampling(
        feature: &str,
        snapshot_pairs: &[(&str, serde_json::Value)],
        sampling: Option<kernel::recipe::RecipeSampling>,
        path_params: &[(&str, serde_json::Value)],
    ) -> serde_json::Value {
        use kernel::plan::PlanAction;
        use kernel::recipe::RecipeDocument;
        let mut snapshot = serde_json::Map::new();
        for (key, value) in snapshot_pairs {
            snapshot.insert((*key).to_string(), value.clone());
        }
        let mut object = serde_json::Map::new();
        for (key, value) in path_params {
            object.insert((*key).to_string(), value.clone());
        }
        let mut document = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "漂移核对",
            PlanAction::Split,
            snapshot,
            "2026-11-06T00:00:00Z",
            Some(feature.to_string()),
        );
        document.steps[0].sampling = sampling;
        let invocation = recipe_apply::build_invocation(&document, &object)
            .unwrap_or_else(|error| panic!("{feature} 槽位注入失败：{error}"));
        invocation.params
    }

    #[test]
    fn split_and_csv_slots_land_on_the_route_struct_fields() {
        // f01/f02/f03 → SplitParams{file, output_dir}
        for feature in ["f01", "f02", "f03"] {
            let params = slots_of(
                feature,
                &[],
                &[("file", json!("/in/a.txt")), ("outputDir", json!("/out"))],
            );
            let parsed: plan_kinds::SplitParams = serde_json::from_value(params).unwrap();
            assert_eq!(parsed.file, "/in/a.txt", "{feature} 的 file 槽没解进去");
            assert_eq!(
                parsed.output_dir, "/out",
                "{feature} 的 outputDir 槽没解进去"
            );
        }

        // f04 → MergeParams{files, output}
        let params = slots_of(
            "f04",
            &[("target", json!("utf-8"))],
            &[
                ("files", json!(["/in/a.txt", "/in/b.txt"])),
                ("output", json!("/out/all.txt")),
            ],
        );
        let parsed: plan_kinds::MergeParams = serde_json::from_value(params).unwrap();
        assert_eq!(
            parsed.files,
            vec!["/in/a.txt".to_string(), "/in/b.txt".to_string()]
        );
        assert_eq!(parsed.output, "/out/all.txt");

        // f06…f09 → CsvPlanParams{files, output / outputDir}
        // f06 只要 `output`（plan_csv.rs:215 明写 requires output），
        // f07–f09 才是 output/outputDir 二选一（plan_csv.rs:115-134）。
        // f06/f07 是「正文缺失」拒绝功能（OQ-41②），到不了槽位注入——
        // 它们的槽位表保留给 OQ-43，这里只断言 describe 如实预声明。
        for feature in ["f06", "f07"] {
            let document = RecipeDocument::from_single_step(
                "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
                "正文缺失",
                PlanAction::CsvExtract,
                serde_json::Map::new(),
                "2026-11-07T00:00:00Z",
                Some(feature.to_string()),
            );
            let spec = recipe_apply::describe(&document);
            assert!(!spec.applicable, "{feature} 应当被预声明为不可套用");
        }
        for feature in ["f08", "f09"] {
            let params = slots_of(
                feature,
                &[],
                &[
                    ("files", json!(["/in/a.csv"])),
                    ("outputDir", json!("/out")),
                ],
            );
            let parsed: plan_csv::CsvPlanParams = serde_json::from_value(params).unwrap();
            assert_eq!(
                parsed.files,
                vec!["/in/a.csv".to_string()],
                "{feature} 的 files 槽"
            );
            assert_eq!(
                parsed.output_dir.as_deref(),
                Some("/out"),
                "{feature} 的 outputDir 槽"
            );
        }

        // f10 → 同一个结构体的 roots（**不是 root**，写错就永远是空列表）
        let params = slots_of(
            "f10",
            &[],
            &[("roots", json!(["/data"])), ("outputDir", json!("/out"))],
        );
        let parsed: plan_csv::CsvPlanParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.roots, vec!["/data".to_string()], "f10 的 roots 槽");
    }

    #[test]
    fn rewrite_and_ops_slots_land_on_the_route_struct_fields() {
        // c01/c02 → files（**不是 file**）。采样键（sampleWindow/sampleBytes）计划快照
        // **从不携带**（OQ-41 实测），映射表按缺省补——这里不带任何采样键，正好证明
        // 补出来的键真的落在路由字段上。
        for feature in ["c01", "c02"] {
            let snapshot: &[(&str, serde_json::Value)] = if feature == "c01" {
                &[
                    ("target", json!("utf-8")),
                    ("platformPrior", json!("windows")),
                ]
            } else {
                &[("target", json!("crlf"))]
            };
            let params = slots_of(feature, snapshot, &[("files", json!(["/in/a.txt"]))]);
            let files = if feature == "c01" {
                let parsed: plan_kinds::TranscodeParams = serde_json::from_value(params).unwrap();
                // platformPrior→prior 的改名也要落地：先验丢了，检测就会偏离原计划
                assert_eq!(
                    parsed.prior.as_deref(),
                    Some("windows"),
                    "c01 的 platformPrior 没改名进 prior"
                );
                assert_eq!(parsed.sample_window, 4096, "c01 的 sampleWindow 缺省没补上");
                parsed.files
            } else {
                let parsed: plan_kinds::LineEndingsParams = serde_json::from_value(params).unwrap();
                assert_eq!(parsed.sample_bytes, 4096, "c02 的 sampleBytes 缺省没补上");
                parsed.files
            };
            assert_eq!(
                files,
                vec!["/in/a.txt".to_string()],
                "{feature} 的 files 槽"
            );
        }

        // c03：OQ-41 钉住的「计划形状 ≠ 请求形状」三类差异，裁决②后由映射表闭合——
        // replace→replacement 改名、sampleBytes 缺省、platformPrior→prior。
        // 这条断言曾经钉住"现在会失败"（OQ-41 未裁时），现改为钉住"映射后确实成立"。
        let snapshot = &[
            ("mode", json!("plain")),
            ("find", json!("a")),
            ("replace", json!("b")),
            ("platformPrior", json!("windows")),
            ("delimiter", json!("auto")),
        ];
        let params = slots_of("c03", snapshot, &[("files", json!(["/in/a.txt"]))]);
        let parsed: plan_kinds::ReplaceParams = serde_json::from_value(params).unwrap();
        assert_eq!(
            parsed.replacement, "b",
            "c03 的 replace 没改名进 replacement"
        );
        assert_eq!(parsed.sample_bytes, 4096, "c03 的 sampleBytes 缺省没补上");
        assert_eq!(
            parsed.prior.as_deref(),
            Some("windows"),
            "c03 的 platformPrior 没改名进 prior"
        );
        assert!(parsed.delimiter.is_none(), "c03 的 auto 分隔符没被剔除");

        // c04/c11…c14 → OpPlanParams.dir / files / output
        let params = slots_of("c04", &[], &[("dir", json!("/data"))]);
        assert_eq!(
            serde_json::from_value::<plan_ops::OpPlanParams>(params)
                .unwrap()
                .dir
                .as_deref(),
            Some("/data"),
            "c04 的 dir 槽"
        );
        let params = slots_of(
            "c11",
            &[],
            &[("files", json!(["/a.csv"])), ("output", json!("/b.csv"))],
        );
        let parsed: plan_ops::OpPlanParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.files, vec!["/a.csv".to_string()]);
        assert_eq!(parsed.output.as_deref(), Some("/b.csv"));

        // c14 原来在这里做列数槽断言；同 f06/f07——拆分/合并规格不随计划保存，
        // OQ-41② 后是「正文缺失」拒绝功能，到不了槽位注入（见
        // content_free_features_refuse_with_reason），OpPlanParams 的槽位键保留给 OQ-43。
        {
            let document = RecipeDocument::from_single_step(
                "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
                "c14 列拆分",
                PlanAction::CsvColumns,
                serde_json::Map::new(),
                "2026-11-07T00:00:00Z",
                Some("c14".to_string()),
            );
            let spec = recipe_apply::describe(&document);
            assert!(!spec.applicable, "c14 应当被预声明为不可套用");
        }
    }

    #[test]
    fn manifest_archive_and_zip_slots_land_on_the_route_struct_fields() {
        // c07 → manifest + sourceRoot（必填）与 targetRoot（输出）
        let params = slots_of(
            "c07",
            &[],
            &[
                ("manifest", json!("/list.csv")),
                ("sourceRoot", json!("/src")),
                ("targetRoot", json!("/dst")),
            ],
        );
        let parsed: plan_manifest::ManifestParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.manifest, "/list.csv");
        assert_eq!(parsed.source_root, "/src");
        assert_eq!(parsed.target_root, "/dst");

        // c08 原来在这里做槽位落地断言；OQ-41② 落地后它是「正文缺失」拒绝功能
        // （归档规则不随计划保存，映射表补不出），build_invocation 在槽位注入前就拒绝，
        // 槽位键落在哪已不影响行为——拒绝口径由 content_free_features_refuse_with_reason
        // 覆盖，槽位表保留是为 OQ-43（正文进模板）裁决后能直接接上。
        {
            let document = RecipeDocument::from_single_step(
                "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
                "c08 归档",
                PlanAction::Move,
                serde_json::Map::new(),
                "2026-11-07T00:00:00Z",
                Some("c08".to_string()),
            );
            let spec = recipe_apply::describe(&document);
            assert!(!spec.applicable, "c08 应当被预声明为不可套用");
        }

        // c10 解压 → archive + outputDir；压缩 → sources + output
        use kernel::plan::PlanAction;
        use kernel::recipe::RecipeDocument;
        let mut snapshot = serde_json::Map::new();
        snapshot.insert("mode".to_string(), json!("compress"));
        let compress_doc = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "压缩",
            PlanAction::Archive,
            snapshot,
            "2026-11-06T00:00:00Z",
            Some("c10".to_string()),
        );
        let mut object = serde_json::Map::new();
        object.insert("sources".to_string(), json!(["/a.txt"]));
        object.insert("output".to_string(), json!("/out.zip"));
        let invocation = recipe_apply::build_invocation(&compress_doc, &object).unwrap();
        let parsed: plan_manifest::ZipPlanParams =
            serde_json::from_value(invocation.params).unwrap();
        assert_eq!(
            parsed.sources,
            vec!["/a.txt".to_string()],
            "c10 压缩的 sources 槽"
        );
        assert_eq!(parsed.output.as_deref(), Some("/out.zip"));

        let extract_doc = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q9",
            "解压",
            PlanAction::Extract,
            serde_json::Map::new(),
            "2026-11-06T00:00:00Z",
            Some("c10".to_string()),
        );
        let mut object = serde_json::Map::new();
        object.insert("archive".to_string(), json!("/a.zip"));
        object.insert("outputDir".to_string(), json!("/out"));
        let invocation = recipe_apply::build_invocation(&extract_doc, &object).unwrap();
        let parsed: plan_manifest::ZipPlanParams =
            serde_json::from_value(invocation.params).unwrap();
        assert_eq!(
            parsed.archive.as_deref(),
            Some("/a.zip"),
            "c10 解压的 archive 槽"
        );
        assert_eq!(parsed.output_dir.as_deref(), Some("/out"));
    }

    #[test]
    fn shell_only_routes_keep_their_own_slots() {
        // f05/f11/f12 用的是壳层自己的参数结构体，不在各 plan_* 模块里，
        // 表也照样要能对上——这三条是"只改一处就漏掉"的高危项。
        let params = slots_of(
            "f05",
            &[],
            &[("file", json!("/a.csv")), ("outputDir", json!("/out"))],
        );
        let parsed: F05ShellParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.file, "/a.csv");
        assert_eq!(parsed.output_dir, "/out");

        // f11 原来在这里做槽位落地断言；同 c08——清单正文不随计划保存，现在是
        // 「正文缺失」拒绝功能（content_free_features_refuse_with_reason 覆盖），
        // F11ShellParams{list, root} 的槽位表保留给 OQ-43。
        {
            let document = RecipeDocument::from_single_step(
                "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
                "f11 建目录",
                PlanAction::CreateDir,
                serde_json::Map::new(),
                "2026-11-07T00:00:00Z",
                Some("f11".to_string()),
            );
            let spec = recipe_apply::describe(&document);
            assert!(!spec.applicable, "f11 应当被预声明为不可套用");
        }

        let params = slots_of("f12", &[], &[("dir", json!("/data"))]);
        let parsed: F12ShellParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.dir, "/data", "f12 的 dir 槽");
    }

    #[test]
    fn csv_and_ops_sampling_defaults_land_on_their_routes() {
        // f08/f09/c11 的路由在运行时必填 sampleWindow（scan_options），计划快照从不
        // 携带——映射表按缺省补的键必须真的落在字段上；f06/f07/c12/c13/c14 是拒绝
        // 功能（正文缺失），到不了这一步，其采样口径随 OQ-43 一并再议。
        for feature in ["f08", "f09"] {
            let params = slots_of(
                feature,
                &[],
                &[
                    ("files", json!(["/in/a.csv"])),
                    ("outputDir", json!("/out")),
                ],
            );
            let parsed: plan_csv::CsvPlanParams = serde_json::from_value(params).unwrap();
            assert_eq!(
                parsed.sample_window,
                Some(4096),
                "{feature} 的 sampleWindow 缺省没补上"
            );
        }
        let params = slots_of(
            "c11",
            &[("key", json!("row")), ("target", json!("utf-8"))],
            &[
                ("files", json!(["/in/a.csv"])),
                ("output", json!("/out/x.csv")),
            ],
        );
        let parsed: plan_ops::OpPlanParams = serde_json::from_value(params).unwrap();
        assert_eq!(
            parsed.sample_window,
            Some(4096),
            "c11 的 sampleWindow 缺省没补上"
        );
    }

    #[test]
    fn template_sampling_values_win_over_defaults() {
        // v5 模板存了采样值（OQ-41 附带裁决：进模板）→ 用模板的，不用缺省。
        let sampling = kernel::recipe::RecipeSampling {
            sample_window: Some(65536),
            sample_blocks: Some(2),
            sample_bytes: None,
        };
        let params = slots_of_sampling(
            "c01",
            &[("target", json!("utf-8"))],
            Some(sampling),
            &[("files", json!(["/in/a.txt"]))],
        );
        let parsed: plan_kinds::TranscodeParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.sample_window, 65536, "模板采样值没有盖过缺省");
        assert_eq!(
            parsed.sample_blocks, 2,
            "可选的 sampleBlocks 没有随模板转发"
        );
    }

    #[test]
    fn c09_slot_goes_to_source_paths_and_not_into_params() {
        use kernel::plan::PlanAction;
        use kernel::recipe::RecipeDocument;
        let document = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "去重",
            PlanAction::Delete,
            serde_json::Map::new(),
            "2026-11-06T00:00:00Z",
            Some("c09".to_string()),
        );
        let mut object = serde_json::Map::new();
        object.insert("sourcePaths".to_string(), json!(["/data"]));
        let invocation = recipe_apply::build_invocation(&document, &object).unwrap();
        assert_eq!(invocation.source_paths, vec!["/data".to_string()]);
        // c09 的 C09Params 没有路径字段：键若误进 params，这里会多出未知字段
        let parsed: C09Params = serde_json::from_value(invocation.params).unwrap();
        assert!(parsed.keep.is_none() && parsed.quick_window_bytes.is_none());
    }

    #[test]
    fn every_slot_table_feature_has_a_landing_test() {
        // 自校：槽位表里登记的功能必须**逐个**在上面这几条断言里出现。
        // 新加功能却没补落地断言＝表可以漂移而 CI 全绿，这条就是拦这个的。
        let covered: std::collections::BTreeSet<&str> = [
            "f01", "f02", "f03", "f04", "f05", "f06", "f07", "f08", "f09", "f10", "f11", "f12",
            "c01", "c02", "c03", "c04", "c07", "c08", "c09", "c10", "c11", "c12", "c13", "c14",
        ]
        .into_iter()
        .collect();
        for entry in recipe_apply::FEATURE_PATH_SLOTS {
            assert!(
                covered.contains(entry.feature),
                "功能 {} 进了槽位表却没有落地断言（表会漂，CI 不会红）",
                entry.feature
            );
        }
        assert_eq!(
            covered.len(),
            recipe_apply::FEATURE_PATH_SLOTS.len(),
            "落地断言覆盖的功能数与槽位表不一致"
        );
    }

    #[test]
    fn protected_snapshot_comparison_ignores_order_and_duplicates() {
        let left = vec![ProtectedDir::new("/a"), ProtectedDir::new("/b")];
        let right = vec![
            ProtectedDir::new("/b"),
            ProtectedDir::new("/a"),
            ProtectedDir::new("/a"),
        ];
        assert!(same_protected_dirs(&left, &right));
        assert!(!same_protected_dirs(&left, &[ProtectedDir::new("/a")]));
        assert!(!same_protected_dirs(
            &left,
            &[ProtectedDir::new("/a"), ProtectedDir::new("/c")]
        ));
    }

    #[test]
    fn protected_snapshot_comparison_sees_canonical_changes() {
        let left = vec![ProtectedDir::with_canonical("/a", Some("/real/a".into()))];
        let right = vec![ProtectedDir::with_canonical("/a", Some("/real/b".into()))];
        assert!(!same_protected_dirs(&left, &right));
    }

    #[test]
    fn f11_facts_probe_every_parent_position_and_flag_files() {
        // 事实采集面：清单 "holder/sub" 必须同时探到 holder 与 holder/sub，并把作为文件的
        // holder 单列出来——否则 kernel 无从把该分支判成 parent-is-file（计划期跳过）。
        use std::fs;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("batchdesk-f11-{}-{seq}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("holder"), b"not a directory").unwrap();
        fs::create_dir_all(root.join("dir")).unwrap();

        let rows = vec![
            kernel::create_dirs::PlanF11Row {
                line: 1,
                path: "holder/sub".to_string(),
            },
            kernel::create_dirs::PlanF11Row {
                line: 2,
                path: "dir/sub".to_string(),
            },
            kernel::create_dirs::PlanF11Row {
                line: 3,
                path: "../escape".to_string(),
            },
        ];
        let root_text = root.to_string_lossy().into_owned();
        let (existing, existing_files) = f11_existing_facts(&root_text, &rows);

        let holder = root.join("holder").to_string_lossy().into_owned();
        let holder_sub = root
            .join("holder")
            .join("sub")
            .to_string_lossy()
            .into_owned();
        let dir = root.join("dir").to_string_lossy().into_owned();
        assert!(existing.contains(&holder), "父级位置也要探测：{existing:?}");
        assert!(
            existing_files.contains(&holder),
            "文件须单列：{existing_files:?}"
        );
        assert!(existing.contains(&dir), "目录进存在清单：{existing:?}");
        assert!(
            !existing_files.contains(&dir),
            "目录不得混进文件事实：{existing_files:?}"
        );
        assert!(
            !existing.contains(&holder_sub),
            "不存在的位置不该进入事实清单"
        );
        assert_eq!(
            existing
                .iter()
                .filter(|path| path.contains("escape"))
                .count(),
            0,
            "无效行（.. 逃逸）不产生事实"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
