//! BatchDesk desktop shell for the C09 vertical slice.
//!
//! The shell owns filesystem paths, plan retention in memory and command
//! registration; domain rules stay in `kernel` and shell adapters in
//! `platform`. Apply/undo are synchronous in this slice; task queue/progress
//! events are the next step.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use kernel::apply::ApplyOptions;
use kernel::duplicate::{group_duplicates, DuplicateCandidate, GroupingMode, GroupingOptions};
use kernel::error::KernelError;
use kernel::fs::FsAdapter;
use kernel::plan::{plan_c09, KeepPolicy, PlanAction, PlanC09Input, PlanDocument, PlanFile};
use kernel::recipe::{
    migrate_recipe_value, RecipeDocumentV3, RecipeMatch, TemplateStep,
};
use kernel::ui_state::UiState;
use platform::config_store::{ConfigStore, RecipeList};
use platform::fs::CliFsAdapter;
use platform::ids::{new_plan_id, new_ulid};
use platform::journal::FileJournal;
use platform::protected::{make_absolute, protected_dirs_from_strings};
use platform::retention::{
    cleanup, validate_retention_days, CleanupOutcome, DEFAULT_RETENTION_DAYS,
};
use platform::scan::{scan_path, ScanMode, ScanOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{Emitter, Manager, State};

mod tasks;

use tasks::{StoredPlan, TaskDetail, TaskPage, TaskPaths, TaskQueue};

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
            config_dir,
        })
    }
}

struct AppState {
    plans: Mutex<HashMap<String, StoredPlan>>,
    paths: DataPaths,
    tasks: TaskQueue,
}

impl AppState {
    fn new(paths: DataPaths, tasks: TaskQueue) -> Self {
        Self {
            plans: Mutex::new(HashMap::new()),
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
    if kind != "c09" {
        return Err(IpcError::new(
            "E1002",
            "this shell slice supports only c09 plans",
        ));
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
    let protected_dirs =
        protected_dirs_from_strings(params.protected_dirs.as_deref().unwrap_or(&[]))
            .map_err(IpcError::from)?;
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
                std::fs::canonicalize(&file.path)
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned())
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
    let stored = state
        .plans
        .lock()
        .unwrap()
        .remove(&plan_id)
        .ok_or_else(|| IpcError::new("E1001", "plan not found; generate a fresh preview"))?;
    let task_id = state
        .tasks
        .enqueue_apply(stored, options.unwrap_or_default());
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
    cleanup(
        &mut journal,
        &state.paths.quarantine_dir,
        days,
        apply.unwrap_or(false),
    )
    .map_err(IpcError::from)
}

// ── 配置 / 界面状态 / 处理模板（设计 03 §7）──────────────────────────

fn config_store(state: &State<'_, AppState>) -> ConfigStore {
    ConfigStore::new(state.paths.config_dir.clone())
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
fn setConfig(
    state: State<'_, AppState>,
    scope: String,
    patch: Value,
) -> Result<Value, IpcError> {
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
    store.save_config(&mut load.document).map_err(IpcError::from)?;
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
    store.save_config(&mut load.document).map_err(IpcError::from)?;
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
    config_store(&state).reset_ui_state().map_err(IpcError::from)
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
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn saveRecipe(state: State<'_, AppState>, recipe: Value) -> Result<RecipeDocumentV3, IpcError> {
    let input: SaveRecipeInput = serde_json::from_value(recipe)
        .map_err(|error| IpcError::new("E1002", format!("invalid recipe input: {error}")))?;
    let recipe_id = input.recipe_id.unwrap_or_else(|| new_ulid(&input.name));
    let created_at = platform::fs::now_rfc3339();
    let mut document = match input.steps {
        Some(steps) => RecipeDocumentV3 {
            recipe_id,
            schema_version: kernel::recipe::CURRENT_RECIPE_SCHEMA_VERSION,
            name: input.name,
            description: input.description,
            steps,
            match_: input.match_,
            source_format: None,
            created_at,
            updated_at: None,
            last_used_at: None,
            usage_count: None,
            pinned: None,
        },
        None => {
            let kind = input.kind.ok_or_else(|| {
                IpcError::new("E1001", "saveRecipe requires kind or steps")
            })?;
            let params = input.params_snapshot.unwrap_or_default();
            let mut document =
                RecipeDocumentV3::from_single_step(recipe_id, input.name, kind, params, created_at);
            document.description = input.description;
            document.match_ = input.match_;
            document
        }
    };
    let store = config_store(&state);
    store.save_recipe(&mut document).map_err(IpcError::from)?;
    Ok(document)
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn getRecipe(state: State<'_, AppState>, recipe_id: String) -> Result<RecipeDocumentV3, IpcError> {
    config_store(&state)
        .get_recipe(&recipe_id)
        .map_err(IpcError::from)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecipePage {
    items: Vec<RecipeDocumentV3>,
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
fn exportRecipe(
    state: State<'_, AppState>,
    recipe_id: String,
) -> Result<ExportPayload, IpcError> {
    let payload = config_store(&state)
        .export_recipe(&recipe_id)
        .map_err(IpcError::from)?;
    Ok(ExportPayload { payload })
}

#[tauri::command(rename_all = "camelCase")]
#[allow(non_snake_case)]
fn importRecipe(
    state: State<'_, AppState>,
    payload: String,
) -> Result<RecipeDocumentV3, IpcError> {
    // 导入即低版本迁移、高版本 E1007；导入后必须重新预览（不变量 13）。
    config_store(&state)
        .import_recipe(&payload)
        .map_err(IpcError::from)
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
            let tasks = TaskQueue::new(
                TaskPaths {
                    quarantine_dir: paths.quarantine_dir.clone(),
                    journal_path: paths.journal_path.clone(),
                },
                Arc::new(move |event, payload| {
                    if let Err(error) = handle.emit(event, payload) {
                        eprintln!("E9001: failed to emit {event}: {error}");
                    }
                }),
            );
            app.manage(AppState::new(paths, tasks));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            getIpcMeta,
            getStorageInfo,
            planOperation,
            applyPlan,
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
