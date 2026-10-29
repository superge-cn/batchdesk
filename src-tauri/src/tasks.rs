//! Desktop task queue (design 01 §4.1/§4.3/§6, design 12 §3).
//!
//! One worker drains queued apply/undo jobs. The queue owns state transitions,
//! pause/cancel flags and `task://*` event delivery; domain execution stays in
//! `kernel`. Events are emitted through a callback so the queue can be tested
//! without a running Tauri application.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use kernel::apply::{
    apply_plan, undo_plan_with_ports, ApplyContext, ApplyEntryReport, ApplyOptions, ApplyReport,
    ApplyState, UndoPorts,
};
use kernel::error::KernelError;
use kernel::fs::CaseSense;
use kernel::journal::JournalStore;
use kernel::license::AlwaysAllowLicense;
use kernel::path::ProtectedDir;
use kernel::plan::PlanDocument;
use kernel::progress::{CancelToken, ProgressEvent, ProgressPhase, ProgressSink};
use platform::fs::{now_rfc3339, rfc3339_days_ago, CliFsAdapter};
use platform::ids::new_ulid;
use platform::journal::FileJournal;
use platform::retention::DEFAULT_RETENTION_DAYS;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub type EventEmitter = Arc<dyn Fn(&str, Value) + Send + Sync>;

pub const EVENT_PROGRESS: &str = "task://progress";
pub const EVENT_STATE: &str = "task://state";
pub const EVENT_DONE: &str = "task://done";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    Queued,
    Running,
    Paused,
    Cancelling,
    /// 应用异常退出后由启动恢复加载；必须由用户选择续跑/回滚/放弃，不自动执行。
    Interrupted,
    Completed,
    PartialFailed,
    Failed,
    Cancelled,
}

impl TaskState {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::PartialFailed | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub task_id: String,
    pub kind: String,
    pub state: TaskState,
    pub phase: ProgressPhase,
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPage {
    pub items: Vec<TaskSummary>,
    pub total: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetail {
    pub summary: TaskSummary,
    pub entries: Vec<ApplyEntryReport>,
}

#[derive(Clone)]
pub struct TaskPaths {
    pub quarantine_dir: PathBuf,
    pub journal_path: PathBuf,
    /// 未完成任务快照（S03）：只落非终态任务，重启后标记为 interrupted。
    pub queue_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtectedSource {
    /// protectedDirs 来自 planOperation 的显式参数，预览快照即权威。
    #[default]
    Params,
    /// protectedDirs 来自 ConfigDocument.security.protectedDirs；
    /// apply 前若配置变化则旧预览失效（S02 名单变更重算）。
    Config,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredPlan {
    pub plan: PlanDocument,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    #[serde(default)]
    pub protected_source: ProtectedSource,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "job", rename_all = "camelCase")]
pub enum StoredJob {
    Apply {
        plan: StoredPlan,
        #[serde(default)]
        options: ApplyOptions,
    },
    Undo {
        plan_id: String,
    },
}

impl StoredJob {
    fn plan_id(&self) -> String {
        match self {
            Self::Apply { plan, .. } => plan.plan.plan_id.clone(),
            Self::Undo { plan_id } => plan_id.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedTask {
    summary: TaskSummary,
    job: StoredJob,
    #[serde(default)]
    entries: Vec<ApplyEntryReport>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedQueue {
    #[serde(default)]
    tasks: Vec<PersistedTask>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecoveryAction {
    Resume,
    Rollback,
    Discard,
}

#[derive(Default)]
struct TaskControl {
    cancel: AtomicBool,
    pause: AtomicBool,
}

impl CancelToken for TaskControl {
    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

impl TaskControl {
    fn wait_while_paused(&self) {
        while self.pause.load(Ordering::SeqCst) && !self.cancel.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(20));
        }
    }
}

struct TaskRecord {
    summary: TaskSummary,
    plan_id: String,
    control: Arc<TaskControl>,
    entries: Vec<ApplyEntryReport>,
    job: StoredJob,
    /// 由启动恢复加载时记录崩溃前状态；`None` 表示当前进程新建的正常任务。
    recoverable_from: Option<TaskState>,
}

struct QueueState {
    tasks: HashMap<String, TaskRecord>,
    order: Vec<String>,
}

struct Inner {
    state: Mutex<QueueState>,
    /// 串行化"取快照 + 原子写"，避免 worker 与命令线程交错写入导致旧快照覆盖新快照。
    persist_lock: Mutex<()>,
    paths: TaskPaths,
    emit: EventEmitter,
}

struct Job {
    task_id: String,
    kind: StoredJob,
    /// 崩溃时处于 cancelling 的任务，续跑实际是继续回滚；成功后终态记 cancelled。
    cancel_after_rollback: bool,
}

pub struct TaskQueue {
    inner: Arc<Inner>,
    sender: mpsc::Sender<Job>,
}

struct JobSink {
    inner: Arc<Inner>,
    task_id: String,
    control: Arc<TaskControl>,
}

impl ProgressSink for JobSink {
    fn report(&self, event: ProgressEvent) {
        self.inner.record_progress(&self.task_id, &event);
        self.control.wait_while_paused();
    }
}

struct JobResult {
    state: TaskState,
    report: ApplyReport,
    message: Option<String>,
}

impl TaskQueue {
    pub fn new(paths: TaskPaths, emit: EventEmitter) -> Self {
        let _ = std::fs::create_dir_all(&paths.quarantine_dir);
        if let Some(parent) = paths.journal_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Some(parent) = paths.queue_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut loaded = load_queue(&paths.queue_path);
        let mut tasks = HashMap::new();
        let mut order = Vec::new();
        for persisted in loaded.tasks.drain(..) {
            let plan_id = persisted.job.plan_id();
            let mut summary = persisted.summary;
            let interrupted_from = summary.state;
            summary.state = TaskState::Interrupted;
            summary.finished_at = None;
            summary.message = Some(
                "application exited before completion; choose resume, rollback, or discard"
                    .to_string(),
            );
            order.push(summary.task_id.clone());
            tasks.insert(
                summary.task_id.clone(),
                TaskRecord {
                    summary,
                    plan_id,
                    control: Arc::new(TaskControl::default()),
                    entries: persisted.entries,
                    job: persisted.job,
                    recoverable_from: Some(interrupted_from),
                },
            );
        }
        let inner = Arc::new(Inner {
            state: Mutex::new(QueueState { tasks, order }),
            persist_lock: Mutex::new(()),
            paths,
            emit,
        });
        let (sender, receiver) = mpsc::channel::<Job>();
        let worker = Arc::clone(&inner);
        thread::Builder::new()
            .name("batchdesk-task-queue".to_string())
            .spawn(move || worker_loop(worker, receiver))
            .expect("failed to start task queue worker");
        Self { inner, sender }
    }

    pub fn enqueue_apply(&self, plan: StoredPlan, options: ApplyOptions) -> String {
        let task_id = new_ulid(&format!("task:apply:{}", plan.plan.plan_id));
        let job = StoredJob::Apply { plan, options };
        let total = match &job {
            StoredJob::Apply { plan, .. } => plan.plan.entries.len() as u64,
            StoredJob::Undo { .. } => 0,
        };
        self.insert(&task_id, job, total);
        task_id
    }

    pub fn enqueue_undo(&self, plan_id: String) -> String {
        let task_id = new_ulid(&format!("task:undo:{plan_id}"));
        let total = FileJournal::open(self.inner.paths.journal_path.clone())
            .and_then(|journal| journal.entries_for_plan(&plan_id))
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| entry.status == kernel::journal::JournalStatus::Applied)
                    .count() as u64
            })
            .unwrap_or(0);
        self.insert(
            &task_id,
            StoredJob::Undo {
                plan_id: plan_id.clone(),
            },
            total,
        );
        task_id
    }

    fn insert(&self, task_id: &str, job: StoredJob, total: u64) -> String {
        let plan_id = job.plan_id();
        let kind = match &job {
            StoredJob::Apply { plan, .. } => plan.plan.kind.clone(),
            StoredJob::Undo { plan_id } => journal_plan_kind(&self.inner.paths, plan_id),
        };
        let summary = TaskSummary {
            task_id: task_id.to_string(),
            kind,
            state: TaskState::Queued,
            phase: ProgressPhase::Queued,
            done: 0,
            total,
            started_at: None,
            finished_at: None,
            message: None,
        };
        {
            let mut state = self.inner.state.lock().unwrap();
            state.tasks.insert(
                task_id.to_string(),
                TaskRecord {
                    summary,
                    plan_id: plan_id.clone(),
                    control: Arc::new(TaskControl::default()),
                    entries: Vec::new(),
                    job: job.clone(),
                    recoverable_from: None,
                },
            );
            state.order.push(task_id.to_string());
        }
        self.inner.persist();
        self.inner.emit_state(task_id, TaskState::Queued);
        self.inner.emit_progress(ProgressEvent {
            task_id: task_id.to_string(),
            phase: ProgressPhase::Queued,
            done: 0,
            total,
            message: None,
        });
        if self
            .sender
            .send(Job {
                task_id: task_id.to_string(),
                kind: job,
                cancel_after_rollback: false,
            })
            .is_err()
        {
            let report = ApplyReport {
                plan_id,
                task_id: task_id.to_string(),
                entries: Vec::new(),
                undone_by: None,
            };
            self.inner.finish(
                task_id,
                TaskState::Failed,
                report,
                Some("task queue stopped".into()),
            );
        }
        task_id.to_string()
    }

    pub fn cancel(&self, task_id: &str) {
        let (state, plan_id) = {
            let state = self.inner.state.lock().unwrap();
            let Some(record) = state.tasks.get(task_id) else {
                return;
            };
            if record.summary.state.is_terminal() || record.summary.state == TaskState::Interrupted
            {
                return;
            }
            record.control.cancel.store(true, Ordering::SeqCst);
            (record.summary.state, record.plan_id.clone())
        };
        if state == TaskState::Queued {
            let report = ApplyReport {
                plan_id,
                task_id: task_id.to_string(),
                entries: Vec::new(),
                undone_by: None,
            };
            self.inner.finish(
                task_id,
                TaskState::Cancelled,
                report,
                Some("cancelled before start".to_string()),
            );
        } else {
            self.inner.set_state(task_id, TaskState::Cancelling);
        }
    }

    pub fn pause(&self, task_id: &str) {
        {
            let state = self.inner.state.lock().unwrap();
            let Some(record) = state.tasks.get(task_id) else {
                return;
            };
            if !matches!(record.summary.state, TaskState::Queued | TaskState::Running) {
                return;
            }
            record.control.pause.store(true, Ordering::SeqCst);
        }
        self.inner.set_state(task_id, TaskState::Paused);
    }

    pub fn resume(&self, task_id: &str) {
        {
            let state = self.inner.state.lock().unwrap();
            let Some(record) = state.tasks.get(task_id) else {
                return;
            };
            if record.summary.state != TaskState::Paused {
                return;
            }
            record.control.pause.store(false, Ordering::SeqCst);
        }
        self.inner.set_state(task_id, TaskState::Running);
    }

    /// S03 崩溃恢复：resume / rollback / discard。仅对 interrupted 任务有效，
    /// 不自动执行；返回需要观察的任务 ID（rollback 时是新 undo 任务）。
    pub fn recover(&self, task_id: &str, action: RecoveryAction) -> Result<String, KernelError> {
        let (job, interrupted_from, entries) = {
            let state = self.inner.state.lock().unwrap();
            let Some(record) = state.tasks.get(task_id) else {
                return Err(KernelError::new(
                    "E1001",
                    "task not found",
                    Some(task_id.to_string()),
                    false,
                ));
            };
            if record.summary.state != TaskState::Interrupted {
                return Err(KernelError::invalid_params(
                    "only interrupted tasks can be recovered",
                ));
            }
            (
                record.job.clone(),
                record.recoverable_from.unwrap_or(TaskState::Running),
                record.entries.clone(),
            )
        };
        let plan_id = job.plan_id();
        let report = ApplyReport {
            plan_id: plan_id.clone(),
            task_id: task_id.to_string(),
            entries,
            undone_by: None,
        };
        match action {
            RecoveryAction::Discard => {
                self.inner.finish(
                    task_id,
                    TaskState::Cancelled,
                    report,
                    Some(
                        "discarded after restart; applied entries remain undoable via the journal"
                            .to_string(),
                    ),
                );
                Ok(task_id.to_string())
            }
            RecoveryAction::Rollback => match &job {
                StoredJob::Undo { .. } => self.resume_interrupted(task_id, job, true),
                StoredJob::Apply { plan, .. } => {
                    let undo_id = self.enqueue_undo(plan.plan.plan_id.clone());
                    self.inner.finish(
                        task_id,
                        TaskState::Cancelled,
                        report,
                        Some(format!(
                            "rolled back after restart; follow undo task {undo_id}"
                        )),
                    );
                    Ok(undo_id)
                }
            },
            RecoveryAction::Resume => {
                if interrupted_from == TaskState::Cancelling {
                    let undo_job = StoredJob::Undo {
                        plan_id: plan_id.clone(),
                    };
                    self.resume_interrupted(task_id, undo_job, true)
                } else {
                    self.resume_interrupted(task_id, job, false)
                }
            }
        }
    }

    fn resume_interrupted(
        &self,
        task_id: &str,
        job: StoredJob,
        cancel_after_rollback: bool,
    ) -> Result<String, KernelError> {
        let (job, total) = match job {
            StoredJob::Apply { plan, options } => {
                let mut remaining = plan.clone();
                let applied = applied_entry_ids(&self.inner.paths, &plan.plan.plan_id)?;
                remaining
                    .plan
                    .entries
                    .retain(|entry| !applied.contains(&entry.entry_id));
                remaining.plan.stats.total = remaining.plan.entries.len();
                remaining.plan.stats.blocked = remaining
                    .plan
                    .entries
                    .iter()
                    .filter(|entry| entry.blocked.is_some())
                    .count();
                let total = remaining.plan.entries.len() as u64;
                if total == 0 {
                    let report = ApplyReport {
                        plan_id: plan.plan.plan_id.clone(),
                        task_id: task_id.to_string(),
                        entries: Vec::new(),
                        undone_by: None,
                    };
                    self.inner.finish(
                        task_id,
                        TaskState::Completed,
                        report,
                        Some("all entries were already completed before restart".to_string()),
                    );
                    return Ok(task_id.to_string());
                }
                (
                    StoredJob::Apply {
                        plan: remaining,
                        options,
                    },
                    total,
                )
            }
            StoredJob::Undo { plan_id } => {
                let total = applied_entry_count(&self.inner.paths, &plan_id).unwrap_or(0);
                (StoredJob::Undo { plan_id }, total)
            }
        };
        if !self.inner.mark_resumed(task_id, total) {
            return Err(KernelError::invalid_params("task is no longer interrupted"));
        }
        self.inner.persist();
        if self
            .sender
            .send(Job {
                task_id: task_id.to_string(),
                kind: job,
                cancel_after_rollback,
            })
            .is_err()
        {
            self.inner.finish(
                task_id,
                TaskState::Failed,
                ApplyReport {
                    plan_id: String::new(),
                    task_id: task_id.to_string(),
                    entries: Vec::new(),
                    undone_by: None,
                },
                Some("task queue stopped".into()),
            );
        }
        Ok(task_id.to_string())
    }

    pub fn list(&self, offset: usize, limit: usize) -> TaskPage {
        let state = self.inner.state.lock().unwrap();
        let total = state.order.len();
        let items = state
            .order
            .iter()
            .rev()
            .skip(offset)
            .take(limit)
            .filter_map(|task_id| state.tasks.get(task_id))
            .map(|record| record.summary.clone())
            .collect();
        TaskPage { items, total }
    }

    pub fn get(&self, task_id: &str) -> Option<TaskDetail> {
        let state = self.inner.state.lock().unwrap();
        state.tasks.get(task_id).map(|record| TaskDetail {
            summary: record.summary.clone(),
            entries: record.entries.clone(),
        })
    }
}

impl Inner {
    fn emit(&self, event: &str, payload: Value) {
        (self.emit)(event, payload);
    }

    fn emit_state(&self, task_id: &str, state: TaskState) {
        self.emit(EVENT_STATE, json!({ "taskId": task_id, "state": state }));
    }

    fn emit_progress(&self, event: ProgressEvent) {
        match serde_json::to_value(&event) {
            Ok(payload) => self.emit(EVENT_PROGRESS, payload),
            Err(_) => self.emit(
                EVENT_PROGRESS,
                json!({ "taskId": event.task_id, "phase": event.phase, "done": event.done, "total": event.total }),
            ),
        }
    }

    fn set_state(&self, task_id: &str, next: TaskState) {
        let changed = {
            let mut state = self.state.lock().unwrap();
            let Some(record) = state.tasks.get_mut(task_id) else {
                return;
            };
            if record.summary.state.is_terminal() || record.summary.state == next {
                false
            } else {
                record.summary.state = next;
                true
            }
        };
        if changed {
            self.emit_state(task_id, next);
        }
        self.persist();
    }

    fn mark_resumed(&self, task_id: &str, total: u64) -> bool {
        let changed = {
            let mut state = self.state.lock().unwrap();
            let Some(record) = state.tasks.get_mut(task_id) else {
                return false;
            };
            if record.summary.state != TaskState::Interrupted {
                false
            } else {
                record.summary.state = TaskState::Queued;
                record.summary.phase = ProgressPhase::Queued;
                record.summary.done = 0;
                record.summary.total = total;
                record.summary.finished_at = None;
                record.summary.message = Some("resumed after restart".to_string());
                record.recoverable_from = None;
                true
            }
        };
        if changed {
            self.emit_state(task_id, TaskState::Queued);
            self.emit_progress(ProgressEvent {
                task_id: task_id.to_string(),
                phase: ProgressPhase::Queued,
                done: 0,
                total,
                message: Some("resumed after restart".to_string()),
            });
        }
        changed
    }

    fn persist(&self) {
        let _guard = self.persist_lock.lock().unwrap();
        let snapshot = {
            let state = self.state.lock().unwrap();
            PersistedQueue {
                tasks: state
                    .order
                    .iter()
                    .filter_map(|task_id| state.tasks.get(task_id))
                    .filter(|record| !record.summary.state.is_terminal())
                    .map(|record| PersistedTask {
                        summary: record.summary.clone(),
                        job: record.job.clone(),
                        entries: record.entries.clone(),
                    })
                    .collect(),
            }
        };
        if let Err(error) = write_queue(&self.paths.queue_path, &snapshot) {
            eprintln!(
                "E9001: failed to persist the task queue: {}: {}",
                error.code, error.message
            );
        }
    }

    fn record_progress(&self, task_id: &str, event: &ProgressEvent) {
        {
            let mut state = self.state.lock().unwrap();
            let Some(record) = state.tasks.get_mut(task_id) else {
                return;
            };
            if record.summary.state.is_terminal() {
                return;
            }
            record.summary.phase = event.phase;
            // TaskSummary keeps entry totals; journaling/finalizing report phase-local counts.
            if matches!(
                event.phase,
                ProgressPhase::Queued | ProgressPhase::Executing
            ) {
                record.summary.done = event.done;
                record.summary.total = event.total;
            }
            record.summary.message = event.message.clone();
        }
        self.emit_progress(event.clone());
    }

    fn finish(&self, task_id: &str, next: TaskState, report: ApplyReport, message: Option<String>) {
        let changed = {
            let mut state = self.state.lock().unwrap();
            let Some(record) = state.tasks.get_mut(task_id) else {
                return;
            };
            if record.summary.state.is_terminal() {
                return;
            }
            record.summary.state = next;
            record.summary.phase = ProgressPhase::Finalizing;
            record.summary.finished_at = Some(now_rfc3339());
            record.summary.message = message;
            record.entries = report.entries.clone();
            true
        };
        if changed {
            self.emit_state(task_id, next);
            self.emit(EVENT_DONE, json!({ "taskId": task_id, "summary": report }));
        }
        self.persist();
    }

    fn mark_started(&self, task_id: &str, paused: bool) {
        {
            let mut state = self.state.lock().unwrap();
            let Some(record) = state.tasks.get_mut(task_id) else {
                return;
            };
            if record.summary.started_at.is_none() {
                record.summary.started_at = Some(now_rfc3339());
            }
            record.summary.phase = ProgressPhase::Executing;
        }
        self.set_state(
            task_id,
            if paused {
                TaskState::Paused
            } else {
                TaskState::Running
            },
        );
    }
}

fn worker_loop(inner: Arc<Inner>, receiver: mpsc::Receiver<Job>) {
    while let Ok(job) = receiver.recv() {
        run_job(&inner, job);
    }
}

fn run_job(inner: &Arc<Inner>, job: Job) {
    let control = {
        let state = inner.state.lock().unwrap();
        let Some(record) = state.tasks.get(&job.task_id) else {
            return;
        };
        if record.summary.state.is_terminal() {
            return;
        }
        Arc::clone(&record.control)
    };

    loop {
        if control.cancel.load(Ordering::SeqCst) {
            let (plan_id, entries) = task_identity(inner, &job.task_id);
            inner.finish(
                &job.task_id,
                TaskState::Cancelled,
                ApplyReport {
                    plan_id,
                    task_id: job.task_id.clone(),
                    entries,
                    undone_by: None,
                },
                Some("cancelled before start".to_string()),
            );
            return;
        }
        if control.pause.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(20));
            continue;
        }
        break;
    }

    inner.mark_started(&job.task_id, control.pause.load(Ordering::SeqCst));
    let mut result = match job.kind {
        StoredJob::Apply { plan, options } => {
            run_apply(inner, &job.task_id, &control, plan, options)
        }
        StoredJob::Undo { plan_id } => run_undo(inner, &job.task_id, &control, &plan_id),
    };
    if job.cancel_after_rollback && result.state == TaskState::Completed {
        result.state = TaskState::Cancelled;
        result.message = Some("cancellation rollback completed after restart".to_string());
    }
    inner.finish(&job.task_id, result.state, result.report, result.message);
}

fn task_identity(inner: &Arc<Inner>, task_id: &str) -> (String, Vec<ApplyEntryReport>) {
    let state = inner.state.lock().unwrap();
    state
        .tasks
        .get(task_id)
        .map(|record| (record.plan_id.clone(), record.entries.clone()))
        .unwrap_or_default()
}

fn run_apply(
    inner: &Arc<Inner>,
    task_id: &str,
    control: &Arc<TaskControl>,
    plan: StoredPlan,
    options: ApplyOptions,
) -> JobResult {
    let plan_id = plan.plan.plan_id.clone();
    let mut journal = match FileJournal::open(inner.paths.journal_path.clone()) {
        Ok(journal) => journal,
        Err(error) => return failed_result(plan_id, task_id, error),
    };
    let adapter = CliFsAdapter::new(Some(inner.paths.quarantine_dir.clone()));
    let license = AlwaysAllowLicense;
    let sink = JobSink {
        inner: Arc::clone(inner),
        task_id: task_id.to_string(),
        control: Arc::clone(control),
    };
    let mut context = ApplyContext {
        fs: &adapter,
        license: &license,
        journal: &mut journal,
        task_id: task_id.to_string(),
        now_rfc3339: now_rfc3339(),
        protected_dirs: plan.protected_dirs,
        case_sense: plan.case_sense,
        progress: Some(&sink),
        cancel: Some(control.as_ref()),
    };
    match apply_plan(&plan.plan, &options, &mut context) {
        Ok(outcome) => JobResult {
            state: match outcome.state {
                ApplyState::Completed => TaskState::Completed,
                ApplyState::PartialFailed => TaskState::PartialFailed,
                ApplyState::Failed => TaskState::Failed,
                ApplyState::Cancelled => TaskState::Cancelled,
            },
            report: outcome.report,
            message: None,
        },
        Err(error) => failed_result(plan_id, task_id, error),
    }
}

fn run_undo(
    inner: &Arc<Inner>,
    task_id: &str,
    control: &Arc<TaskControl>,
    plan_id: &str,
) -> JobResult {
    let mut journal = match FileJournal::open(inner.paths.journal_path.clone()) {
        Ok(journal) => journal,
        Err(error) => return failed_result(plan_id.to_string(), task_id, error),
    };
    let cutoff = rfc3339_days_ago(DEFAULT_RETENTION_DAYS);
    let entries = match journal.entries_for_plan(plan_id) {
        Ok(entries) => entries,
        Err(error) => return failed_result(plan_id.to_string(), task_id, error),
    };
    if entries.iter().any(|entry| entry.ts <= cutoff) {
        return failed_result(
            plan_id.to_string(),
            task_id,
            KernelError::new(
                "E9001",
                "undo window expired; run retention cleanup first",
                None,
                false,
            ),
        );
    }
    let applied_count = entries
        .iter()
        .filter(|entry| entry.status == kernel::journal::JournalStatus::Applied)
        .count();
    let adapter = CliFsAdapter::new(None);
    let license = AlwaysAllowLicense;
    let sink = JobSink {
        inner: Arc::clone(inner),
        task_id: task_id.to_string(),
        control: Arc::clone(control),
    };
    let ports = UndoPorts {
        task_id: Some(task_id),
        progress: Some(&sink),
        cancel: Some(control.as_ref()),
    };
    match undo_plan_with_ports(plan_id, &adapter, &license, &mut journal, ports) {
        Ok(outcome) => {
            let processed = outcome.reverted + outcome.failed.len();
            let state = if control.cancel.load(Ordering::SeqCst) && processed < applied_count {
                TaskState::Cancelled
            } else if outcome.failed.is_empty() {
                TaskState::Completed
            } else if outcome.reverted > 0 {
                TaskState::PartialFailed
            } else {
                TaskState::Failed
            };
            JobResult {
                state,
                report: ApplyReport {
                    plan_id: plan_id.to_string(),
                    task_id: task_id.to_string(),
                    entries: outcome.entries,
                    undone_by: None,
                },
                message: if state == TaskState::Cancelled {
                    Some("cancelled during undo".to_string())
                } else {
                    None
                },
            }
        }
        Err(error) => failed_result(plan_id.to_string(), task_id, error),
    }
}

fn failed_result(plan_id: String, task_id: &str, error: KernelError) -> JobResult {
    JobResult {
        state: TaskState::Failed,
        report: ApplyReport {
            plan_id,
            task_id: task_id.to_string(),
            entries: Vec::new(),
            undone_by: None,
        },
        message: Some(format!("{}: {}", error.code, error.message)),
    }
}

fn load_queue(path: &Path) -> PersistedQueue {
    if !path.exists() {
        return PersistedQueue::default();
    }
    match std::fs::read_to_string(path)
        .map_err(|error| error.to_string())
        .and_then(|contents| {
            serde_json::from_str::<PersistedQueue>(&contents).map_err(|error| error.to_string())
        }) {
        Ok(queue) => queue,
        Err(error) => {
            let backup =
                path.with_extension(format!("corrupt.{}.json", now_rfc3339().replace(':', "-")));
            let _ = std::fs::rename(path, &backup);
            eprintln!(
                "E9001: task queue snapshot was corrupt ({error}); backed up to {}",
                backup.display()
            );
            PersistedQueue::default()
        }
    }
}

fn write_queue(path: &Path, queue: &PersistedQueue) -> Result<(), KernelError> {
    let json = serde_json::to_string(queue).map_err(|error| {
        KernelError::internal("task queue snapshot is not serializable", error.to_string())
    })?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| {
                KernelError::internal(
                    "task queue directory is not writable",
                    format!("{}: {error}", parent.display()),
                )
            })?;
        }
    }
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temp, json).map_err(|error| {
        KernelError::internal(
            "task queue temp file is not writable",
            format!("{}: {error}", temp.display()),
        )
    })?;
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(path).map_err(|error| {
            KernelError::internal(
                "task queue snapshot is not replaceable",
                format!("{}: {error}", path.display()),
            )
        })?;
    }
    std::fs::rename(&temp, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        KernelError::internal(
            "task queue snapshot is not writable",
            format!("{}: {error}", path.display()),
        )
    })
}

fn journal_plan_kind(paths: &TaskPaths, plan_id: &str) -> String {
    FileJournal::open(paths.journal_path.clone())
        .and_then(|journal| journal.entries_for_plan(plan_id))
        .ok()
        .and_then(|entries| entries.into_iter().find_map(|entry| entry.plan_kind))
        .unwrap_or_else(|| "c09".to_string())
}

fn applied_entry_ids(paths: &TaskPaths, plan_id: &str) -> Result<BTreeSet<String>, KernelError> {
    let journal = FileJournal::open(paths.journal_path.clone())?;
    Ok(journal
        .entries_for_plan(plan_id)?
        .into_iter()
        .filter(|entry| entry.status == kernel::journal::JournalStatus::Applied)
        .map(|entry| entry.entry_id)
        .collect())
}

fn applied_entry_count(paths: &TaskPaths, plan_id: &str) -> Option<u64> {
    applied_entry_ids(paths, plan_id)
        .ok()
        .map(|ids| ids.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::journal::{FsOp, JournalSink};
    use kernel::plan::{PlanEntry, PlanParamValue, PlanStats};
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("batchdesk-tasks-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn plan_for(plan_id: &str, files: &[PathBuf]) -> PlanDocument {
        let mut params = BTreeMap::new();
        params.insert(
            "mode".to_string(),
            PlanParamValue::String("full".to_string()),
        );
        let entries = files
            .iter()
            .enumerate()
            .map(|(index, path)| PlanEntry {
                entry_id: format!("{plan_id}-e{}", index + 1),
                source: path.to_string_lossy().into_owned(),
                target: None,
                action: kernel::plan::PlanAction::Delete,
                relation: None,
                reason: "test duplicate".to_string(),
                conflict: None,
                resolution: None,
                blocked: None,
                expected: None,
            })
            .collect();
        PlanDocument {
            plan_id: plan_id.to_string(),
            kind: "c09".to_string(),
            params,
            entries,
            stats: PlanStats::default(),
            warnings: Vec::new(),
        }
    }

    fn wait_terminal(queue: &TaskQueue, task_id: &str) -> TaskDetail {
        let start = Instant::now();
        loop {
            if let Some(detail) = queue.get(task_id) {
                if detail.summary.state.is_terminal() {
                    return detail;
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "task did not reach a terminal state"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn paths_for(temp: &TempDir) -> TaskPaths {
        TaskPaths {
            quarantine_dir: temp.0.join("quarantine"),
            journal_path: temp.0.join("journal.jsonl"),
            queue_path: temp.0.join("tasks.json"),
        }
    }

    fn write_snapshot(paths: &TaskPaths, task_id: &str, plan: &PlanDocument, state: TaskState) {
        let summary = TaskSummary {
            task_id: task_id.to_string(),
            kind: plan.kind.clone(),
            state,
            phase: ProgressPhase::Queued,
            done: 0,
            total: plan.entries.len() as u64,
            started_at: Some(now_rfc3339()),
            finished_at: None,
            message: None,
        };
        let queue = PersistedQueue {
            tasks: vec![PersistedTask {
                summary,
                job: StoredJob::Apply {
                    plan: StoredPlan {
                        plan: plan.clone(),
                        protected_dirs: Vec::new(),
                        case_sense: CaseSense::Sensitive,
                        protected_source: ProtectedSource::Params,
                    },
                    options: ApplyOptions::default(),
                },
                entries: Vec::new(),
            }],
        };
        write_queue(&paths.queue_path, &queue).unwrap();
    }

    #[test]
    fn apply_and_undo_run_through_the_queue_with_events() {
        let temp = TempDir::new();
        let root = temp.0.join("data");
        let quarantine = temp.0.join("quarantine");
        let journal = temp.0.join("journal.jsonl");
        let queue_path = temp.0.join("tasks.json");
        fs::create_dir_all(&root).unwrap();
        let first = root.join("a.txt");
        let second = root.join("b.txt");
        fs::write(&first, b"same").unwrap();
        fs::write(&second, b"same").unwrap();
        let plan = plan_for(&new_ulid("test-plan"), &[first.clone(), second.clone()]);
        let plan_id = plan.plan_id.clone();

        let events = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let sink = Arc::clone(&events);
        let queue = TaskQueue::new(
            TaskPaths {
                quarantine_dir: quarantine.clone(),
                journal_path: journal.clone(),
                queue_path: queue_path.clone(),
            },
            Arc::new(move |event, payload| {
                sink.lock().unwrap().push((event.to_string(), payload));
            }),
        );

        let apply_id = queue.enqueue_apply(
            StoredPlan {
                plan,
                protected_dirs: Vec::new(),
                case_sense: CaseSense::Sensitive,
                protected_source: ProtectedSource::Params,
            },
            ApplyOptions::default(),
        );
        let applied = wait_terminal(&queue, &apply_id);
        assert_eq!(applied.summary.state, TaskState::Completed);
        assert_eq!(applied.entries.len(), 2);
        assert!(!first.exists());
        assert!(!second.exists());
        assert_eq!(fs::read_dir(&quarantine).unwrap().count(), 2);

        let undo_id = queue.enqueue_undo(plan_id);
        let undone = wait_terminal(&queue, &undo_id);
        assert_eq!(undone.summary.state, TaskState::Completed);
        assert_eq!(undone.entries.len(), 2);
        assert!(first.exists());
        assert!(second.exists());

        let persisted: PersistedQueue =
            serde_json::from_str(&fs::read_to_string(&queue_path).unwrap()).unwrap();
        assert!(
            persisted.tasks.is_empty(),
            "terminal tasks must not be persisted for crash recovery"
        );

        let events = events.lock().unwrap();
        assert!(events.iter().any(|(name, _)| name == EVENT_PROGRESS));
        assert!(events.iter().any(|(name, _)| name == EVENT_STATE));
        assert!(events
            .iter()
            .any(|(name, payload)| name == EVENT_DONE && payload.get("summary").is_some()));
        assert!(events
            .iter()
            .any(|(_, payload)| payload.get("state").and_then(Value::as_str) == Some("completed")));
    }

    #[test]
    fn task_state_events_cover_pause_resume_and_queued_cancel() {
        let temp = TempDir::new();
        let events = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let sink = Arc::clone(&events);
        let queue = TaskQueue::new(
            TaskPaths {
                quarantine_dir: temp.0.join("quarantine"),
                journal_path: temp.0.join("journal.jsonl"),
                queue_path: temp.0.join("tasks.json"),
            },
            Arc::new(move |event, payload| {
                sink.lock().unwrap().push((event.to_string(), payload));
            }),
        );

        let paused_id = new_ulid("control-pause");
        insert_control_task(&queue, &paused_id);
        queue.pause(&paused_id);
        assert_eq!(
            queue.get(&paused_id).unwrap().summary.state,
            TaskState::Paused
        );
        queue.resume(&paused_id);
        assert_eq!(
            queue.get(&paused_id).unwrap().summary.state,
            TaskState::Running
        );
        queue.cancel(&paused_id);
        assert_eq!(
            queue.get(&paused_id).unwrap().summary.state,
            TaskState::Cancelling
        );

        let queued_id = new_ulid("control-cancel");
        insert_control_task(&queue, &queued_id);
        queue.cancel(&queued_id);
        let cancelled = queue.get(&queued_id).unwrap();
        assert_eq!(cancelled.summary.state, TaskState::Cancelled);
        assert!(cancelled.summary.finished_at.is_some());

        let events = events.lock().unwrap();
        assert!(events.iter().any(|(name, payload)| name == EVENT_STATE
            && payload.get("state").and_then(Value::as_str) == Some("cancelling")));
        assert!(events.iter().any(|(name, payload)| name == EVENT_STATE
            && payload.get("state").and_then(Value::as_str) == Some("cancelled")));
        assert!(events.iter().any(|(name, _)| name == EVENT_DONE));
    }

    #[test]
    fn restart_marks_unfinished_tasks_interrupted_and_resume_skips_applied_entries() {
        let temp = TempDir::new();
        let paths = paths_for(&temp);
        fs::create_dir_all(&paths.quarantine_dir).unwrap();
        let root = temp.0.join("data");
        fs::create_dir_all(&root).unwrap();
        let first = root.join("a.txt");
        let second = root.join("b.txt");
        fs::write(&first, b"same").unwrap();
        fs::write(&second, b"same").unwrap();
        let plan = plan_for(&new_ulid("recover-plan"), &[first.clone(), second.clone()]);
        let plan_id = plan.plan_id.clone();
        let task_id = new_ulid("recover-task");

        let quarantined = paths.quarantine_dir.join("a.txt");
        fs::rename(&first, &quarantined).unwrap();
        let mut journal = FileJournal::open(paths.journal_path.clone()).unwrap();
        journal
            .append_with_kind(
                &plan_id,
                &plan.entries[0].entry_id,
                &now_rfc3339(),
                "c09",
                FsOp::Quarantine {
                    from: first.to_string_lossy().into_owned(),
                    to: quarantined.to_string_lossy().into_owned(),
                },
                FsOp::Rename {
                    from: quarantined.to_string_lossy().into_owned(),
                    to: first.to_string_lossy().into_owned(),
                },
            )
            .unwrap();
        write_snapshot(&paths, &task_id, &plan, TaskState::Running);

        let queue = TaskQueue::new(paths, Arc::new(|_, _| {}));
        let detail = queue.get(&task_id).unwrap();
        assert_eq!(detail.summary.state, TaskState::Interrupted);
        assert!(detail.summary.finished_at.is_none());

        let resumed = queue.recover(&task_id, RecoveryAction::Resume).unwrap();
        assert_eq!(resumed, task_id);
        let done = wait_terminal(&queue, &task_id);
        assert_eq!(done.summary.state, TaskState::Completed);
        assert_eq!(done.entries.len(), 1, "only the unfinished entry runs");
        assert!(!second.exists());
        assert!(!first.exists());
        let journal = FileJournal::open(queue.inner.paths.journal_path.clone()).unwrap();
        assert_eq!(journal.entries_for_plan(&plan_id).unwrap().len(), 2);
    }

    #[test]
    fn recover_rollback_restores_applied_entries_and_cancels_the_original_task() {
        let temp = TempDir::new();
        let paths = paths_for(&temp);
        fs::create_dir_all(&paths.quarantine_dir).unwrap();
        let root = temp.0.join("data");
        fs::create_dir_all(&root).unwrap();
        let first = root.join("a.txt");
        fs::write(&first, b"same").unwrap();
        let plan = plan_for(&new_ulid("rollback-plan"), std::slice::from_ref(&first));
        let plan_id = plan.plan_id.clone();
        let task_id = new_ulid("rollback-task");

        let quarantined = paths.quarantine_dir.join("a.txt");
        fs::rename(&first, &quarantined).unwrap();
        let mut journal = FileJournal::open(paths.journal_path.clone()).unwrap();
        journal
            .append_with_kind(
                &plan_id,
                &plan.entries[0].entry_id,
                &now_rfc3339(),
                "c09",
                FsOp::Quarantine {
                    from: first.to_string_lossy().into_owned(),
                    to: quarantined.to_string_lossy().into_owned(),
                },
                FsOp::Rename {
                    from: quarantined.to_string_lossy().into_owned(),
                    to: first.to_string_lossy().into_owned(),
                },
            )
            .unwrap();
        write_snapshot(&paths, &task_id, &plan, TaskState::Running);

        let queue = TaskQueue::new(paths, Arc::new(|_, _| {}));
        assert_eq!(
            queue.get(&task_id).unwrap().summary.state,
            TaskState::Interrupted
        );
        let undo_id = queue.recover(&task_id, RecoveryAction::Rollback).unwrap();
        assert_ne!(undo_id, task_id);
        assert_eq!(
            queue.get(&task_id).unwrap().summary.state,
            TaskState::Cancelled
        );
        let undone = wait_terminal(&queue, &undo_id);
        assert_eq!(undone.summary.state, TaskState::Completed);
        assert!(first.exists());
        let journal = FileJournal::open(queue.inner.paths.journal_path.clone()).unwrap();
        assert_eq!(
            journal.entries_for_plan(&plan_id).unwrap()[0].status,
            kernel::journal::JournalStatus::Reverted
        );
    }

    #[test]
    fn recover_discard_keeps_applied_state_and_cancels_the_task() {
        let temp = TempDir::new();
        let paths = paths_for(&temp);
        fs::create_dir_all(&paths.quarantine_dir).unwrap();
        let root = temp.0.join("data");
        fs::create_dir_all(&root).unwrap();
        let first = root.join("a.txt");
        fs::write(&first, b"same").unwrap();
        let plan = plan_for(&new_ulid("discard-plan"), std::slice::from_ref(&first));
        let plan_id = plan.plan_id.clone();
        let task_id = new_ulid("discard-task");

        let quarantined = paths.quarantine_dir.join("a.txt");
        fs::rename(&first, &quarantined).unwrap();
        let mut journal = FileJournal::open(paths.journal_path.clone()).unwrap();
        journal
            .append_with_kind(
                &plan_id,
                &plan.entries[0].entry_id,
                &now_rfc3339(),
                "c09",
                FsOp::Quarantine {
                    from: first.to_string_lossy().into_owned(),
                    to: quarantined.to_string_lossy().into_owned(),
                },
                FsOp::Rename {
                    from: quarantined.to_string_lossy().into_owned(),
                    to: first.to_string_lossy().into_owned(),
                },
            )
            .unwrap();
        write_snapshot(&paths, &task_id, &plan, TaskState::Running);

        let queue = TaskQueue::new(paths, Arc::new(|_, _| {}));
        let discarded = queue.recover(&task_id, RecoveryAction::Discard).unwrap();
        assert_eq!(discarded, task_id);
        assert_eq!(
            queue.get(&task_id).unwrap().summary.state,
            TaskState::Cancelled
        );
        assert!(
            quarantined.exists(),
            "discard leaves applied state untouched"
        );
        assert!(!first.exists());
        let persisted: PersistedQueue =
            serde_json::from_str(&fs::read_to_string(&queue.inner.paths.queue_path).unwrap())
                .unwrap();
        assert!(persisted.tasks.is_empty());
    }

    #[test]
    fn corrupt_snapshot_is_backed_up_and_the_queue_starts_empty() {
        let temp = TempDir::new();
        let paths = paths_for(&temp);
        fs::write(&paths.queue_path, b"not json").unwrap();
        let queue = TaskQueue::new(paths.clone(), Arc::new(|_, _| {}));
        assert_eq!(queue.list(0, 10).total, 0);
        let backups: Vec<_> = fs::read_dir(&temp.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("tasks.corrupt.")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert!(!paths.queue_path.exists());
    }

    fn insert_control_task(queue: &TaskQueue, task_id: &str) {
        let summary = TaskSummary {
            task_id: task_id.to_string(),
            kind: "c09".to_string(),
            state: TaskState::Queued,
            phase: ProgressPhase::Queued,
            done: 0,
            total: 0,
            started_at: None,
            finished_at: None,
            message: None,
        };
        let mut state = queue.inner.state.lock().unwrap();
        state.tasks.insert(
            task_id.to_string(),
            TaskRecord {
                summary,
                plan_id: new_ulid("control-plan"),
                control: Arc::new(TaskControl::default()),
                entries: Vec::new(),
                job: StoredJob::Undo {
                    plan_id: new_ulid("control-undo"),
                },
                recoverable_from: None,
            },
        );
        state.order.push(task_id.to_string());
    }
}
