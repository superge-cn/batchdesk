//! F15 AI 助手编排（设计 14）。
//!
//! 本模块**不联网**：模型调用由注入的 [`ModelProvider`] 完成，Provider 选型与
//! base URL allowlist 见设计 14 §12 的待决项。生产构建在未注入 Provider 时
//! `getAiStatus.configured=false`，`aiAsk` 返回 E6001（模型未配置/不可用）。
//!
//! 安全底线（设计 14 §0／§10）：
//! - AI 永不直接 apply；本模块只产出 `PlanDocument`（由注入的 `PlanFactory` 调 kernel 生成）；
//! - 工具 allowlist 只含只读 `scanDirectory` 与 `planOperation`，其余一律 E6004；
//! - 默认请求只含用户文字、文件数量/类型与界面语言，不含文件名/路径/内容；
//! - 模型文字与文件名一律视为数据，不得作为系统指令。
//!
//! 阶段（设计 14 §1）：Phase 3＝多计划、依赖顺序与跨计划冲突。多轮会话上下文、
//! 参数 schema 与 `loadRecipe` 工具仍待补；真实 Provider 已接（OQ-44 裁决：
//! 官方端点＋系统凭据库）。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use kernel::error::KernelError;
use kernel::plan::PlanDocument;
use platform::ids::new_ulid;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 当前启用的 AI 阶段（设计 14 §1：Phase 3＝多计划、依赖与跨计划冲突）。
pub const AI_PHASE: u8 = 3;

/// FunctionCatalog 的版本；目录变化必须递增（设计 14 §5 规则 5）。
pub const CATALOG_VERSION: &str = "2026-11-09.p3";

/// 单个计划束的最大步骤数（设计 14 §7.2；OQ-23 已确认 10 步）。
pub const MAX_BUNDLE_STEPS: usize = 10;

/// Phase 2 的 AI 可计划功能（设计 14 §5／§11）。
///
/// 这是**唯一来源**：不在表里的 kind 一律 E6003/不可见，AI 侧不得另存一份清单。
/// 条目与 `src-tauri/src/lib.rs` 的 `plan_operation_impl` 路由一一对应（f13 等
/// 未在壳层路由的 kind 不登记）；`catalog_covers_every_shell_routable_plan_kind`
/// 是防漂移断言。
pub const CATALOG: &[FunctionDescriptor] = &[
    // Phase 1（F15-lite，2026-11-09）：设计 14 §1 的 4 个功能。
    FunctionDescriptor {
        kind: "f12",
        plan_capable: true,
        ai_phase: 1,
    },
    FunctionDescriptor {
        kind: "f05",
        plan_capable: true,
        ai_phase: 1,
    },
    FunctionDescriptor {
        kind: "f07",
        plan_capable: true,
        ai_phase: 1,
    },
    FunctionDescriptor {
        kind: "c07",
        plan_capable: true,
        ai_phase: 1,
    },
    // Phase 2：其余可在壳层 `planOperation` 生成计划的 F/C 功能。
    FunctionDescriptor {
        kind: "c09",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "f01",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "f02",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "f03",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "f04",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "f06",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "f08",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "f09",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "f10",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "f11",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "c01",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "c02",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "c03",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "c04",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "c08",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "c10",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "c11",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "c12",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "c13",
        plan_capable: true,
        ai_phase: 2,
    },
    FunctionDescriptor {
        kind: "c14",
        plan_capable: true,
        ai_phase: 2,
    },
];

/// 工具 allowlist（设计 14 §6）。配方套用是本地 Templates UI 流程，不作为模型工具
/// （ADR-0088）：配方名/步骤/路径不发送给模型。
pub const ALLOWED_TOOLS: &[&str] = &["scanDirectory", "planOperation"];

/// 明确禁止的工具（设计 14 §6）；命中即 E6004 并记审计，不执行。
pub const FORBIDDEN_TOOLS: &[&str] = &[
    "applyPlan",
    "undoPlan",
    "setConfig",
    "resetScope",
    "activateLicense",
    "cancelTask",
    "retryTask",
    "shell",
    "exec",
    "readFile",
    "writeFile",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionDescriptor {
    pub kind: &'static str,
    pub plan_capable: bool,
    pub ai_phase: u8,
}

pub fn descriptor(kind: &str) -> Option<&'static FunctionDescriptor> {
    CATALOG.iter().find(|item| item.kind == kind)
}

/// 当前阶段对 AI 可见的 kind（`ai_phase <= phase` 且可计划）。
pub fn supported_kinds(phase: u8) -> Vec<&'static str> {
    CATALOG
        .iter()
        .filter(|item| item.plan_capable && item.ai_phase <= phase)
        .map(|item| item.kind)
        .collect()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    pub available: bool,
    pub configured: bool,
    pub phase: u8,
    pub catalog_version: String,
    pub supported_kinds: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub consent_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_policy_url: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiContext {
    #[serde(default)]
    pub file_count: u64,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default)]
    pub include_file_names: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_names: Option<Vec<String>>,
}

fn default_locale() -> String {
    "zh-CN".to_string()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAskInput {
    #[serde(default)]
    pub session_id: Option<String>,
    pub text: String,
    #[serde(default)]
    pub context: AiContext,
    /// 本会话先前的用户消息文本（只含用户文字，不含文件名/路径/内容；设计 14 §9 v1.1）。
    #[serde(default)]
    pub history: Vec<String>,
    /// 非空表示用户已在同意弹窗确认过本次发送字段（设计 14 §9）。
    #[serde(default)]
    pub consent_token: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAskOutput {
    pub message_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiIntentStep {
    pub step_id: String,
    pub kind: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub needs_clarification: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiIntentGraph {
    #[serde(default)]
    pub steps: Vec<AiIntentStep>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiToolCall {
    pub call_id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanBundleStep {
    pub step_id: String,
    pub plan: PlanDocument,
    pub depends_on: Vec<String>,
    pub status: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleConflict {
    pub kind: &'static str,
    pub step_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    pub risk_level: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanBundle {
    pub bundle_id: String,
    pub steps: Vec<PlanBundleStep>,
    pub conflicts: Vec<BundleConflict>,
    pub status: &'static str,
}

/// 模型侧请求：**只**装数据最小化后的字段（设计 14 §9）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRequest {
    pub text: String,
    /// 本会话先前的用户消息文本；不含助手回复、文件名、路径或内容。
    pub history: Vec<String>,
    pub file_count: u64,
    pub kinds: Vec<String>,
    pub locale: String,
    pub file_names: Option<Vec<String>>,
    pub supported_kinds: Vec<&'static str>,
}

/// 模型侧输出：自由文本 + 意图图 + 工具调用请求（全部视为不可信数据）。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelResponse {
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<AiIntentGraph>,
    /// AI 分册 §4 的 fixture 用 snake_case `tool_calls`；IPC 仍用 camelCase。
    #[serde(default, alias = "tool_calls")]
    pub tool_calls: Vec<AiToolCall>,
}

pub trait ModelProvider: Send + Sync {
    fn provider_name(&self) -> &str;
    fn model_name(&self) -> &str;
    /// `on_delta` 用于流式文本；Provider 实现可一次或多次调用。
    fn complete(
        &self,
        request: &ModelRequest,
        on_delta: &(dyn Fn(&str) + Send + Sync),
    ) -> Result<ModelResponse, KernelError>;
}

/// 由壳层注入的计划工厂：AI 只给 kind/params，计划必须由 kernel `planOperation` 生成。
pub trait PlanFactory: Send + Sync {
    fn plan(&self, kind: &str, params: &Value) -> Result<PlanDocument, KernelError>;
}

pub type AiEmitter = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// 生产构建的 Provider 装配点（设计 14 §12，OQ-44 已裁决 2026-11-09）：
/// - base URL 固定为官方 OpenAI 域名，不读取任何可覆盖端点；
/// - API key 只从系统凭据库读取（Windows Credential Manager），非 Windows 仅允许
///   环境变量作为开发/CI 回退，不写 ConfigDocument/UiState/日志；
/// - 模型名与超时秒数由部署方通过环境变量提供（不在代码里自定数值阈值）；
///   任一缺失即视为未配置，`getAiStatus.configured=false`、`aiAsk` 以 E6001 降级。
pub fn production_provider() -> Option<Arc<dyn ModelProvider>> {
    let api_key = read_api_key()?;
    let model = std::env::var("BATCHDESK_AI_MODEL").ok()?.trim().to_string();
    if model.is_empty() {
        return None;
    }
    let seconds: u64 = std::env::var("BATCHDESK_AI_TIMEOUT_SECS")
        .ok()?
        .parse()
        .ok()?;
    if seconds == 0 {
        return None;
    }
    Some(Arc::new(OpenAiProvider::new(
        api_key,
        model,
        Duration::from_secs(seconds),
    )))
}

/// 官方 OpenAI 兼容端点（OQ-44：不允许任意 HTTPS 端点）。
pub const OPENAI_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";
/// 系统凭据库的 service/user；密码只存在操作系统凭据库，不进任何配置文件。
pub const CREDENTIAL_SERVICE: &str = "batchdesk";
pub const CREDENTIAL_USER: &str = "openai";

/// 从系统凭据库读取 API key；非 Windows 用环境变量作开发/CI 回退。
#[cfg(windows)]
fn read_api_key() -> Option<String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .ok()?
        .get_password()
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[cfg(not(windows))]
fn read_api_key() -> Option<String> {
    std::env::var("BATCHDESK_AI_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// 把 API key 写入系统凭据库（供壳层设置入口调用）；不落任何配置文件。
pub fn store_api_key(api_key: &str) -> Result<(), KernelError> {
    if api_key.trim().is_empty() {
        return Err(ai_error("E1002", "api key must not be empty"));
    }
    #[cfg(windows)]
    {
        keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
            .map_err(|_| ai_error("E5001", "credential store is unavailable"))?
            .set_password(api_key)
            .map_err(|_| ai_error("E5001", "failed to store the api key"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(ai_error(
            "E5001",
            "credential store is only available on Windows",
        ))
    }
}

struct OpenAiProvider {
    api_key: String,
    model: String,
    agent: ureq::Agent,
}

impl OpenAiProvider {
    fn new(api_key: String, model: String, timeout: Duration) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(timeout)
            .timeout_read(timeout)
            .timeout_write(timeout)
            .build();
        Self {
            api_key,
            model,
            agent,
        }
    }
}

impl ModelProvider for OpenAiProvider {
    fn provider_name(&self) -> &str {
        "openai"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn complete(
        &self,
        request: &ModelRequest,
        on_delta: &(dyn Fn(&str) + Send + Sync),
    ) -> Result<ModelResponse, KernelError> {
        let body = openai_request_body(&self.model, request);
        let response = self
            .agent
            .post(OPENAI_ENDPOINT)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(map_http_error)?;
        let payload: Value = response
            .into_json()
            .map_err(|_| ai_error("E6003", "model provider returned invalid JSON"))?;
        let content = payload["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default();
        let parsed = parse_model_response(content)?;
        on_delta(&parsed.text);
        Ok(parsed)
    }
}

/// 请求体只装数据最小化后的字段（设计 14 §9）：文字、数量、类型、语言；
/// 文件名只在用户逐次同意后出现，路径与内容永不进入请求。
fn openai_request_body(model: &str, request: &ModelRequest) -> Value {
    let mut user = format!(
        "User request: {}\nContext: fileCount={}, kinds={}, locale={}",
        request.text,
        request.file_count,
        request.kinds.join(","),
        request.locale
    );
    if let Some(names) = &request.file_names {
        user.push_str(&format!("\nFile names (consented): {}", names.join(", ")));
    }
    let mut messages = vec![json!({
        "role": "system",
        "content": system_prompt(&request.supported_kinds)
    })];
    for text in &request.history {
        if !text.trim().is_empty() {
            messages.push(json!({ "role": "user", "content": text }));
        }
    }
    messages.push(json!({ "role": "user", "content": user }));
    json!({ "model": model, "messages": messages })
}

fn system_prompt(kinds: &[&'static str]) -> String {
    format!(
        r#"You are the BatchDesk file-processing assistant. Reply with ONE JSON object and no markdown.
Schema:
{{"text":"user-facing explanation","intent":{{"steps":[{{"stepId":"s1","kind":"<function code>","params":{{}},"dependsOn":[],"needsClarification":false,"question":""}}]}},"tool_calls":[{{"callId":"c1","name":"planOperation","arguments":{{"kind":"<function code>"}}}}]}}
Allowed kinds: {}.
Rules:
- At most 10 steps; dependsOn must reference existing stepId values; never invent a kind.
- Never request applyPlan, undoPlan, setConfig, activateLicense, cancelTask, retryTask, shell, exec, readFile or writeFile.
- Params must contain only the minimal fields needed by that function; never invent paths.
- If information is insufficient, set needsClarification=true and ask in question; do not guess.
- tool_calls are advisory only; the host validates and generates plans."#,
        kinds.join(", ")
    )
}

fn parse_model_response(content: &str) -> Result<ModelResponse, KernelError> {
    let trimmed = content.trim();
    let candidate = if trimmed.starts_with('{') {
        trimmed.to_string()
    } else {
        match (trimmed.find('{'), trimmed.rfind('}')) {
            (Some(start), Some(end)) if end > start => trimmed[start..=end].to_string(),
            _ => String::new(),
        }
    };
    if candidate.is_empty() {
        return Err(ai_error("E6003", "model returned no JSON object"));
    }
    serde_json::from_str(&candidate)
        .map_err(|_| ai_error("E6003", "model output failed schema validation"))
}

fn map_http_error(error: ureq::Error) -> KernelError {
    match error {
        ureq::Error::Status(code, _) => map_http_status(code),
        ureq::Error::Transport(_) => ai_error("E6006", "model request failed or timed out"),
    }
}

fn map_http_status(code: u16) -> KernelError {
    match code {
        401 | 403 => ai_error("E6002", "model authentication failed"),
        408 => ai_error("E6006", "model request timed out"),
        429 => ai_error("E6009", "model provider rate limited the request"),
        500..=599 => ai_error("E6006", "model provider is temporarily unavailable"),
        _ => ai_error("E6001", "model provider rejected the request"),
    }
}

pub const EVENT_DELTA: &str = "ai://delta";
pub const EVENT_TOOL: &str = "ai://tool";
pub const EVENT_BUNDLE: &str = "ai://bundle";
pub const EVENT_DONE: &str = "ai://done";
pub const EVENT_ERROR: &str = "ai://error";

/// 模型输出校验（设计 14 §5／§7.2）。
pub fn validate_intent(graph: &AiIntentGraph) -> Result<(), KernelError> {
    if graph.steps.is_empty() {
        return Err(ai_error("E6003", "model returned an empty intent"));
    }
    if graph.steps.len() > MAX_BUNDLE_STEPS {
        return Err(ai_error(
            "E6010",
            &format!("plan bundle exceeds the {MAX_BUNDLE_STEPS}-step limit"),
        ));
    }
    if graph.steps.len() > 1 && AI_PHASE < 3 {
        return Err(ai_error(
            "E6011",
            "multi-step intents are not supported in the current phase",
        ));
    }

    let mut seen: HashSet<&str> = HashSet::new();
    for step in &graph.steps {
        if step.step_id.trim().is_empty() {
            return Err(ai_error("E6003", "intent stepId must not be empty"));
        }
        if !seen.insert(step.step_id.as_str()) {
            return Err(ai_error(
                "E6003",
                "intent stepId must be unique within a bundle",
            ));
        }
    }

    for step in &graph.steps {
        if step.needs_clarification == Some(true) {
            if graph.steps.len() > 1 {
                return Err(ai_error(
                    "E6003",
                    "clarification is only valid for a single-step intent",
                ));
            }
            return Ok(());
        }
        let Some(item) = descriptor(&step.kind) else {
            return Err(ai_error(
                "E6003",
                &format!("model chose an unknown function kind: {}", step.kind),
            ));
        };
        if !item.plan_capable || item.ai_phase > AI_PHASE {
            return Err(ai_error(
                "E6003",
                &format!(
                    "function kind is not available in this phase: {}",
                    step.kind
                ),
            ));
        }
        if !step.params.is_object() {
            return Err(ai_error("E6003", "intent params must be a JSON object"));
        }
        let mut depends_on: HashSet<&str> = HashSet::new();
        for dependency in &step.depends_on {
            if dependency == &step.step_id {
                return Err(ai_error("E6010", "a step must not depend on itself"));
            }
            if !seen.contains(dependency.as_str()) {
                return Err(ai_error(
                    "E6010",
                    &format!("unknown dependency step: {dependency}"),
                ));
            }
            if !depends_on.insert(dependency.as_str()) {
                return Err(ai_error(
                    "E6003",
                    &format!("duplicate dependency on step: {dependency}"),
                ));
            }
        }
    }
    topological_order(graph).map(|_| ())
}

/// 依赖拓扑序（同层按原始下标稳定）；环/未知依赖返回 E6010（设计 14 §7.2）。
pub fn topological_order(graph: &AiIntentGraph) -> Result<Vec<usize>, KernelError> {
    let index: HashMap<&str, usize> = graph
        .steps
        .iter()
        .enumerate()
        .map(|(position, step)| (step.step_id.as_str(), position))
        .collect();
    let mut indegree = vec![0usize; graph.steps.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); graph.steps.len()];
    for (position, step) in graph.steps.iter().enumerate() {
        for dependency in &step.depends_on {
            let Some(&parent) = index.get(dependency.as_str()) else {
                return Err(ai_error(
                    "E6010",
                    &format!("unknown dependency step: {dependency}"),
                ));
            };
            indegree[position] += 1;
            dependents[parent].push(position);
        }
    }
    let mut queue: VecDeque<usize> = (0..graph.steps.len())
        .filter(|position| indegree[*position] == 0)
        .collect();
    let mut order = Vec::with_capacity(graph.steps.len());
    while let Some(position) = queue.pop_front() {
        order.push(position);
        for &child in &dependents[position] {
            indegree[child] -= 1;
            if indegree[child] == 0 {
                queue.push_back(child);
            }
        }
    }
    if order.len() != graph.steps.len() {
        return Err(ai_error("E6010", "plan bundle contains a dependency cycle"));
    }
    Ok(order)
}

/// 调计划工厂；把模型参数非法（E1002）归一为 E6003（设计 14 §8）。
fn generate_step_plan(
    plans: &dyn PlanFactory,
    step: &AiIntentStep,
) -> Result<PlanDocument, KernelError> {
    plans.plan(&step.kind, &step.params).map_err(|error| {
        // 壳层路由仍是唯一 schema 源；这里只归一「参数/kind 非法」，
        // 不另建第二份逐功能 schema 表。
        if error.code == "E1002" {
            ai_error("E6003", "model intent params failed validation")
        } else {
            error
        }
    })
}

/// 跨计划目标重叠（设计 14 §7.2）：同一目标路径出现在多个步骤即 `target-overlap`。
fn detect_target_overlaps(steps: &[PlanBundleStep]) -> Vec<BundleConflict> {
    let mut by_target: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for step in steps {
        let mut targets: BTreeSet<String> = BTreeSet::new();
        for entry in &step.plan.entries {
            if let Some(target) = &entry.target {
                if !target.is_empty() {
                    targets.insert(target.clone());
                }
            }
        }
        for target in targets {
            by_target
                .entry(target)
                .or_default()
                .push(step.step_id.clone());
        }
    }
    by_target
        .into_iter()
        .filter(|(_, step_ids)| step_ids.len() > 1)
        .map(|(target, step_ids)| BundleConflict {
            kind: "target-overlap",
            step_ids,
            paths: vec![target],
            risk_level: "danger",
        })
        .collect()
}

/// 工具 allowlist 校验；禁止工具与未知工具都返回 E6004（设计 14 §6）。
pub fn validate_tool(name: &str) -> Result<(), KernelError> {
    if ALLOWED_TOOLS.contains(&name) {
        return Ok(());
    }
    if FORBIDDEN_TOOLS.contains(&name) {
        return Err(ai_error(
            "E6004",
            &format!("tool is forbidden for the AI assistant: {name}"),
        ));
    }
    Err(ai_error(
        "E6004",
        &format!("tool is not in the AI allowlist: {name}"),
    ))
}

fn ai_error(code: &str, message: &str) -> KernelError {
    KernelError::new(code, message, None, false)
}

pub struct AiService {
    provider: Option<Arc<dyn ModelProvider>>,
    emit: AiEmitter,
}

impl AiService {
    pub fn new(provider: Option<Arc<dyn ModelProvider>>, emit: AiEmitter) -> Self {
        Self { provider, emit }
    }

    pub fn status(&self) -> AiStatus {
        let configured = self.provider.is_some();
        AiStatus {
            available: configured,
            configured,
            phase: AI_PHASE,
            catalog_version: CATALOG_VERSION.to_string(),
            supported_kinds: supported_kinds(AI_PHASE),
            provider: self
                .provider
                .as_ref()
                .map(|item| item.provider_name().to_string()),
            model: self
                .provider
                .as_ref()
                .map(|item| item.model_name().to_string()),
            consent_required: true,
            data_policy_url: None,
        }
    }

    /// 单轮请求：校验 → 调 Provider → 校验意图/工具 → 经 `PlanFactory` 生成计划 → 发事件。
    ///
    /// 由壳层先分配 messageId（便于在阻塞调用前登记 `aiCancel` 标记），再执行。
    pub fn ask_with_id(
        &self,
        message_id: String,
        input: AiAskInput,
        plans: &dyn PlanFactory,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<AiAskOutput, KernelError> {
        let session_id = input.session_id.clone().unwrap_or_default();
        let result = self.run(&message_id, input, plans, cancelled);
        if let Err(error) = &result {
            self.emit(
                EVENT_ERROR,
                json!({
                    "messageId": message_id,
                    "sessionId": session_id,
                    "error": error_json(error),
                }),
            );
        }
        Ok(AiAskOutput { message_id })
    }

    fn run(
        &self,
        message_id: &str,
        input: AiAskInput,
        plans: &dyn PlanFactory,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), KernelError> {
        let Some(provider) = self.provider.as_ref() else {
            return Err(ai_error("E6001", "AI provider is not configured"));
        };
        if input.text.trim().is_empty() {
            return Err(ai_error("E6003", "empty user text"));
        }

        // 数据最小化：默认不发文件名；开启必须逐次同意（设计 14 §9）。
        let include_names = input.context.include_file_names;
        if include_names && input.consent_token.as_deref().unwrap_or("").is_empty() {
            return Err(ai_error(
                "E6005",
                "sending file names requires explicit consent",
            ));
        }
        let file_names = if include_names {
            Some(input.context.file_names.clone().unwrap_or_default())
        } else {
            None
        };
        let request = ModelRequest {
            text: input.text.clone(),
            history: input.history.clone(),
            file_count: input.context.file_count,
            kinds: input.context.kinds.clone(),
            locale: input.context.locale.clone(),
            file_names,
            supported_kinds: supported_kinds(AI_PHASE),
        };

        if cancelled() {
            return Err(ai_error("E6007", "cancelled by the user"));
        }
        let response = provider.complete(&request, &|text| {
            self.emit(
                EVENT_DELTA,
                json!({ "messageId": message_id, "text": text }),
            );
        })?;
        if cancelled() {
            return Err(ai_error("E6007", "cancelled by the user"));
        }

        for call in &response.tool_calls {
            if let Err(error) = validate_tool(&call.name) {
                self.emit(
                    EVENT_TOOL,
                    json!({ "messageId": message_id, "call": call, "allowed": false }),
                );
                return Err(error);
            }
            self.emit(
                EVENT_TOOL,
                json!({ "messageId": message_id, "call": call, "allowed": true }),
            );
        }

        let Some(graph) = response.intent.clone() else {
            self.emit(
                EVENT_DONE,
                json!({ "messageId": message_id, "message": response.text, "bundle": Value::Null }),
            );
            return Ok(());
        };
        validate_intent(&graph)?;
        let step = &graph.steps[0];
        if step.needs_clarification == Some(true) {
            let question = step
                .question
                .clone()
                .unwrap_or_else(|| response.text.clone());
            self.emit(
                EVENT_DONE,
                json!({ "messageId": message_id, "message": question, "bundle": Value::Null }),
            );
            return Ok(());
        }

        // 每步一次 planOperation；按依赖拓扑序生成，AI 不直接构造 PlanEntry。
        let order = topological_order(&graph)?;
        let mut bundle_steps = Vec::with_capacity(order.len());
        for position in order {
            if cancelled() {
                return Err(ai_error("E6007", "cancelled by the user"));
            }
            let step = &graph.steps[position];
            let plan = generate_step_plan(plans, step)?;
            bundle_steps.push(PlanBundleStep {
                step_id: step.step_id.clone(),
                plan,
                depends_on: step.depends_on.clone(),
                status: "ready",
            });
        }
        let conflicts = detect_target_overlaps(&bundle_steps);
        if !conflicts.is_empty() {
            let blocked: HashSet<&str> = conflicts
                .iter()
                .flat_map(|conflict| conflict.step_ids.iter().map(String::as_str))
                .collect();
            for step in &mut bundle_steps {
                if blocked.contains(step.step_id.as_str()) {
                    step.status = "skipped";
                }
            }
        }
        let bundle = PlanBundle {
            bundle_id: new_ulid("ai-bundle"),
            steps: bundle_steps,
            conflicts,
            status: "ready",
        };
        self.emit(
            EVENT_BUNDLE,
            json!({ "messageId": message_id, "bundle": bundle }),
        );
        self.emit(
            EVENT_DONE,
            json!({
                "messageId": message_id,
                "message": response.text,
                "bundleId": bundle.bundle_id,
            }),
        );
        Ok(())
    }

    fn emit(&self, event: &str, payload: Value) {
        (self.emit)(event, payload);
    }
}

fn error_json(error: &KernelError) -> Value {
    json!({
        "code": error.code,
        "message": error.message,
        "retryable": error.retryable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockProvider {
        response: ModelResponse,
        seen: Mutex<Option<ModelRequest>>,
        fail: Option<KernelError>,
    }

    impl ModelProvider for MockProvider {
        fn provider_name(&self) -> &str {
            "mock"
        }

        fn model_name(&self) -> &str {
            "mock-phase2"
        }

        fn complete(
            &self,
            request: &ModelRequest,
            on_delta: &(dyn Fn(&str) + Send + Sync),
        ) -> Result<ModelResponse, KernelError> {
            *self.seen.lock().unwrap() = Some(request.clone());
            if let Some(error) = &self.fail {
                return Err(error.clone());
            }
            on_delta(&self.response.text);
            Ok(self.response.clone())
        }
    }

    struct MockPlans {
        plan: PlanDocument,
    }

    impl PlanFactory for MockPlans {
        fn plan(&self, _kind: &str, _params: &Value) -> Result<PlanDocument, KernelError> {
            Ok(self.plan.clone())
        }
    }

    /// 记录每次计划调用收到的 kind，供 Phase 2 目录覆盖矩阵断言。
    struct RecordingPlans {
        seen: Arc<Mutex<Vec<String>>>,
        plan: PlanDocument,
    }

    impl PlanFactory for RecordingPlans {
        fn plan(&self, kind: &str, _params: &Value) -> Result<PlanDocument, KernelError> {
            self.seen.lock().unwrap().push(kind.to_string());
            Ok(self.plan.clone())
        }
    }

    /// 每个 kind 生成不同目标路径，避免多计划被误判为目标重叠。
    struct StepPlans;

    impl PlanFactory for StepPlans {
        fn plan(&self, kind: &str, _params: &Value) -> Result<PlanDocument, KernelError> {
            let mut plan = plan_document();
            plan.kind = kind.to_string();
            plan.entries[0].source = format!("/data/{kind}.src");
            plan.entries[0].target = Some(format!("/data/{kind}.out"));
            Ok(plan)
        }
    }

    /// 所有步骤写同一目标，用于 `target-overlap` 断言。
    struct SharedTargetPlans;

    impl PlanFactory for SharedTargetPlans {
        fn plan(&self, kind: &str, _params: &Value) -> Result<PlanDocument, KernelError> {
            let mut plan = plan_document();
            plan.kind = kind.to_string();
            plan.entries[0].source = format!("/data/{kind}.src");
            plan.entries[0].target = Some("/data/shared.out".to_string());
            Ok(plan)
        }
    }

    struct FailingPlans {
        code: &'static str,
    }

    impl PlanFactory for FailingPlans {
        fn plan(&self, _kind: &str, _params: &Value) -> Result<PlanDocument, KernelError> {
            Err(ai_error(self.code, "plan factory failed"))
        }
    }

    fn step(id: &str, kind: &str, depends_on: &[&str]) -> AiIntentStep {
        AiIntentStep {
            step_id: id.to_string(),
            kind: kind.to_string(),
            params: json!({}),
            depends_on: depends_on.iter().map(|item| item.to_string()).collect(),
            needs_clarification: None,
            question: None,
        }
    }

    fn plan_document() -> PlanDocument {
        serde_json::from_value(json!({
            "planId": "plan-ai-1",
            "kind": "f12",
            "params": {},
            "entries": [{
                "entryId": "e1",
                "source": "/data/a.txt",
                "target": "/data/2026_a.txt",
                "action": "rename",
                "reason": "ai"
            }],
            "stats": { "total": 1, "conflict": 0, "warning": 0, "blocked": 0 },
            "warnings": []
        }))
        .expect("plan fixture")
    }

    fn intent(kind: &str, params: Value) -> AiIntentGraph {
        AiIntentGraph {
            steps: vec![AiIntentStep {
                step_id: "s1".to_string(),
                kind: kind.to_string(),
                params,
                depends_on: Vec::new(),
                needs_clarification: None,
                question: None,
            }],
        }
    }

    #[allow(clippy::type_complexity)]
    fn make_service(
        response: ModelResponse,
    ) -> (
        AiService,
        Arc<MockProvider>,
        Arc<Mutex<Vec<(String, Value)>>>,
    ) {
        let provider = Arc::new(MockProvider {
            response,
            seen: Mutex::new(None),
            fail: None,
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let service = AiService::new(
            Some(provider.clone() as Arc<dyn ModelProvider>),
            Arc::new(move |event, payload| {
                sink.lock().unwrap().push((event.to_string(), payload));
            }),
        );
        (service, provider, events)
    }

    fn ask(service: &AiService, context: AiContext, consent: Option<&str>) -> AiAskOutput {
        service
            .ask_with_id(
                "ai-msg-test".to_string(),
                AiAskInput {
                    session_id: None,
                    text: "文件名前加 2026_".to_string(),
                    context,
                    history: Vec::new(),
                    consent_token: consent.map(|item| item.to_string()),
                },
                &MockPlans {
                    plan: plan_document(),
                },
                &|| false,
            )
            .expect("ask returns message id")
    }

    fn ask_with_plans(service: &AiService, plans: &dyn PlanFactory) -> AiAskOutput {
        service
            .ask_with_id(
                "ai-msg-test".to_string(),
                AiAskInput {
                    session_id: None,
                    text: "执行".to_string(),
                    context: AiContext::default(),
                    history: Vec::new(),
                    consent_token: None,
                },
                plans,
                &|| false,
            )
            .expect("ask returns message id")
    }

    /// 目录必须与壳层 `plan_operation_impl` 的路由一一对应（设计 14 §11 覆盖矩阵）。
    /// 新增/删除壳层路由时必须同步本清单，否则 Phase 2 的 AI 可见面会漂移。
    #[test]
    fn catalog_covers_every_shell_routable_plan_kind() {
        let mut expected = vec![
            "c09", "f01", "f02", "f03", "f04", "f05", "f06", "f07", "f08", "f09", "f10", "f11",
            "f12", "c01", "c02", "c03", "c04", "c07", "c08", "c10", "c11", "c12", "c13", "c14",
        ];
        expected.sort_unstable();
        let mut actual: Vec<_> = CATALOG.iter().map(|item| item.kind).collect();
        actual.sort_unstable();
        assert_eq!(actual, expected);
        assert!(CATALOG.iter().all(|item| item.plan_capable));
    }

    #[test]
    fn phase_three_exposes_all_catalog_kinds_and_keeps_the_phase_one_subset() {
        assert_eq!(supported_kinds(1), vec!["f12", "f05", "f07", "c07"]);
        for phase in [2, 3] {
            let supported = supported_kinds(phase);
            assert_eq!(supported.len(), CATALOG.len());
            for kind in ["c09", "f01", "f10", "c11", "c14"] {
                assert!(
                    supported.contains(&kind),
                    "phase {phase} must expose {kind}"
                );
            }
        }
    }

    #[test]
    fn non_plan_capable_or_unrouted_kinds_are_rejected_with_e6003() {
        for kind in ["c05", "c06", "f13", "c15", "s01", "zz"] {
            let error = validate_intent(&intent(kind, json!({})))
                .expect_err("non-catalog kind must be rejected");
            assert_eq!(error.code, "E6003", "{kind}");
        }
    }

    #[test]
    fn every_catalog_kind_reaches_the_plan_factory_with_one_bundle_step() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kinds = supported_kinds(AI_PHASE);
        assert_eq!(kinds.len(), 24);
        for kind in kinds {
            let (service, _, events) = make_service(ModelResponse {
                text: "ok".to_string(),
                intent: Some(intent(kind, json!({}))),
                tool_calls: Vec::new(),
            });
            ask_with_plans(
                &service,
                &RecordingPlans {
                    seen: Arc::clone(&seen),
                    plan: plan_document(),
                },
            );
            let events = events.lock().unwrap();
            assert!(
                !events.iter().any(|(event, _)| event == EVENT_ERROR),
                "{kind} must not emit an error"
            );
            let bundle = events
                .iter()
                .find(|(event, _)| event == EVENT_BUNDLE)
                .unwrap_or_else(|| panic!("{kind} must emit a bundle"));
            assert_eq!(
                bundle.1["bundle"]["steps"].as_array().unwrap().len(),
                1,
                "{kind} must produce exactly one step"
            );
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 24);
        assert_eq!(seen.first().map(String::as_str), Some("f12"));
    }

    #[test]
    fn status_reports_phase_three_and_the_catalog_version() {
        let (service, _, _) = make_service(ModelResponse::default());
        let status = service.status();
        assert_eq!(status.phase, 3);
        assert_eq!(status.catalog_version, CATALOG_VERSION);
        assert_eq!(status.supported_kinds.len(), CATALOG.len());
    }

    #[test]
    fn model_request_lists_every_phase_three_kind() {
        let (service, provider, _) = make_service(ModelResponse {
            text: "ok".to_string(),
            intent: None,
            tool_calls: Vec::new(),
        });
        ask(&service, AiContext::default(), None);
        let seen = provider.seen.lock().unwrap().clone().expect("request");
        assert_eq!(seen.supported_kinds.len(), 24);
        assert!(seen.supported_kinds.contains(&"c14"));
    }

    #[test]
    fn unknown_function_kind_is_rejected_with_e6003() {
        let (service, _, events) = make_service(ModelResponse {
            text: "ok".to_string(),
            intent: Some(intent("f13", json!({}))),
            tool_calls: Vec::new(),
        });
        ask(&service, AiContext::default(), None);
        let events = events.lock().unwrap();
        let error = events
            .iter()
            .find(|(event, _)| event == EVENT_ERROR)
            .expect("error event");
        assert_eq!(error.1["error"]["code"], "E6003");
        assert!(!events.iter().any(|(event, _)| event == EVENT_BUNDLE));
    }

    #[test]
    fn multi_step_intent_produces_a_topologically_ordered_bundle() {
        let graph = AiIntentGraph {
            steps: vec![step("s2", "f12", &["s1"]), step("s1", "f05", &[])],
        };
        let (service, _, events) = make_service(ModelResponse {
            text: "two steps".to_string(),
            intent: Some(graph),
            tool_calls: Vec::new(),
        });
        ask_with_plans(&service, &StepPlans);
        let events = events.lock().unwrap();
        assert!(!events.iter().any(|(event, _)| event == EVENT_ERROR));
        let bundle = events
            .iter()
            .find(|(event, _)| event == EVENT_BUNDLE)
            .expect("bundle event");
        assert_eq!(bundle.1["bundle"]["steps"][0]["stepId"], "s1");
        assert_eq!(bundle.1["bundle"]["steps"][1]["stepId"], "s2");
        assert_eq!(bundle.1["bundle"]["steps"][0]["status"], "ready");
        assert_eq!(bundle.1["bundle"]["steps"][1]["status"], "ready");
        assert_eq!(bundle.1["bundle"]["conflicts"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn dependency_cycle_is_rejected_with_e6010() {
        let graph = AiIntentGraph {
            steps: vec![step("s1", "f12", &["s2"]), step("s2", "f05", &["s1"])],
        };
        let (service, _, events) = make_service(ModelResponse {
            text: "cycle".to_string(),
            intent: Some(graph),
            tool_calls: Vec::new(),
        });
        ask_with_plans(&service, &StepPlans);
        let events = events.lock().unwrap();
        let error = events
            .iter()
            .find(|(event, _)| event == EVENT_ERROR)
            .expect("error event");
        assert_eq!(error.1["error"]["code"], "E6010");
        assert!(!events.iter().any(|(event, _)| event == EVENT_BUNDLE));
    }

    #[test]
    fn unknown_dependency_is_rejected_with_e6010() {
        let graph = AiIntentGraph {
            steps: vec![step("s1", "f12", &["s9"])],
        };
        let error = validate_intent(&graph).expect_err("unknown dependency");
        assert_eq!(error.code, "E6010");
    }

    #[test]
    fn bundle_over_ten_steps_is_rejected_with_e6010() {
        let graph = AiIntentGraph {
            steps: (0..=MAX_BUNDLE_STEPS)
                .map(|index| step(&format!("s{index}"), "f01", &[]))
                .collect(),
        };
        let error = validate_intent(&graph).expect_err("too many steps");
        assert_eq!(error.code, "E6010");
    }

    #[test]
    fn duplicate_step_id_is_rejected_with_e6003() {
        let graph = AiIntentGraph {
            steps: vec![step("s1", "f12", &[]), step("s1", "f05", &[])],
        };
        let error = validate_intent(&graph).expect_err("duplicate step id");
        assert_eq!(error.code, "E6003");
    }

    #[test]
    fn target_overlap_blocks_the_conflicting_steps() {
        let graph = AiIntentGraph {
            steps: vec![step("s1", "f12", &[]), step("s2", "f05", &[])],
        };
        let (service, _, events) = make_service(ModelResponse {
            text: "overlap".to_string(),
            intent: Some(graph),
            tool_calls: Vec::new(),
        });
        ask_with_plans(&service, &SharedTargetPlans);
        let events = events.lock().unwrap();
        assert!(!events.iter().any(|(event, _)| event == EVENT_ERROR));
        let bundle = events
            .iter()
            .find(|(event, _)| event == EVENT_BUNDLE)
            .expect("bundle event");
        assert_eq!(bundle.1["bundle"]["conflicts"][0]["kind"], "target-overlap");
        assert_eq!(
            bundle.1["bundle"]["conflicts"][0]["paths"][0],
            "/data/shared.out"
        );
        assert_eq!(bundle.1["bundle"]["steps"][0]["status"], "skipped");
        assert_eq!(bundle.1["bundle"]["steps"][1]["status"], "skipped");
    }

    #[test]
    fn forbidden_tool_call_is_rejected_with_e6004_and_emits_tool_card() {
        let (service, _, events) = make_service(ModelResponse {
            text: "ok".to_string(),
            intent: Some(intent("f12", json!({"prefix": "2026_"}))),
            tool_calls: vec![AiToolCall {
                call_id: "c1".to_string(),
                name: "applyPlan".to_string(),
                arguments: json!({}),
            }],
        });
        ask(&service, AiContext::default(), None);
        let events = events.lock().unwrap();
        let tool = events
            .iter()
            .find(|(event, _)| event == EVENT_TOOL)
            .expect("tool card");
        assert_eq!(tool.1["allowed"], false);
        let error = events
            .iter()
            .find(|(event, _)| event == EVENT_ERROR)
            .expect("error event");
        assert_eq!(error.1["error"]["code"], "E6004");
        assert!(!events.iter().any(|(event, _)| event == EVENT_BUNDLE));
    }

    #[test]
    fn missing_provider_returns_e6001() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let service = AiService::new(
            None,
            Arc::new(move |event, payload| {
                sink.lock().unwrap().push((event.to_string(), payload));
            }),
        );
        assert!(!service.status().configured);
        ask(&service, AiContext::default(), None);
        let events = events.lock().unwrap();
        let error = events
            .iter()
            .find(|(event, _)| event == EVENT_ERROR)
            .expect("error event");
        assert_eq!(error.1["error"]["code"], "E6001");
    }

    #[test]
    fn happy_path_emits_delta_tool_bundle_done() {
        let (service, _, events) = make_service(ModelResponse {
            text: "已生成重命名计划".to_string(),
            intent: Some(intent("f12", json!({"prefix": "2026_"}))),
            tool_calls: vec![AiToolCall {
                call_id: "c1".to_string(),
                name: "planOperation".to_string(),
                arguments: json!({"kind": "f12"}),
            }],
        });
        ask(&service, AiContext::default(), None);
        let events = events.lock().unwrap();
        let names: Vec<_> = events.iter().map(|(event, _)| event.as_str()).collect();
        assert_eq!(
            names,
            vec![EVENT_DELTA, EVENT_TOOL, EVENT_BUNDLE, EVENT_DONE]
        );
        assert_eq!(
            events[2].1["bundle"]["steps"][0]["plan"]["planId"],
            "plan-ai-1"
        );
        assert_eq!(
            events[3].1["bundleId"], events[2].1["bundle"]["bundleId"],
            "done 事件必须回报同一个 bundleId"
        );
        assert_eq!(events[3].1["bundleId"].as_str().unwrap().len(), 26);
    }

    #[test]
    fn default_request_never_contains_file_names_or_paths() {
        let (service, provider, _) = make_service(ModelResponse {
            text: "ok".to_string(),
            intent: Some(intent("f12", json!({}))),
            tool_calls: Vec::new(),
        });
        ask(&service, AiContext::default(), None);
        let seen = provider.seen.lock().unwrap().clone().expect("request");
        assert!(seen.file_names.is_none());
        assert!(!seen.text.contains("/data/"));
    }

    #[test]
    fn file_names_require_explicit_consent_e6005() {
        let (service, _, events) = make_service(ModelResponse {
            text: "ok".to_string(),
            intent: Some(intent("f12", json!({}))),
            tool_calls: Vec::new(),
        });
        let context = AiContext {
            include_file_names: true,
            file_names: Some(vec!["secret.txt".to_string()]),
            ..AiContext::default()
        };
        ask(&service, context.clone(), None);
        let events_snapshot = events.lock().unwrap();
        let error = events_snapshot
            .iter()
            .find(|(event, _)| event == EVENT_ERROR)
            .expect("error event");
        assert_eq!(error.1["error"]["code"], "E6005");
        drop(events_snapshot);

        let (service, provider, _) = make_service(ModelResponse {
            text: "ok".to_string(),
            intent: Some(intent("f12", json!({}))),
            tool_calls: Vec::new(),
        });
        ask(&service, context, Some("consent-1"));
        let seen = provider.seen.lock().unwrap().clone().expect("request");
        assert_eq!(seen.file_names, Some(vec!["secret.txt".to_string()]));
    }

    #[test]
    fn clarification_finishes_without_bundle() {
        let graph = AiIntentGraph {
            steps: vec![AiIntentStep {
                step_id: "s1".to_string(),
                kind: String::new(),
                params: json!({}),
                depends_on: Vec::new(),
                needs_clarification: Some(true),
                question: Some("要把哪些文件改名？".to_string()),
            }],
        };
        let (service, _, events) = make_service(ModelResponse {
            text: "需要更多信息".to_string(),
            intent: Some(graph),
            tool_calls: Vec::new(),
        });
        ask(&service, AiContext::default(), None);
        let events = events.lock().unwrap();
        let done = events
            .iter()
            .find(|(event, _)| event == EVENT_DONE)
            .expect("done event");
        assert_eq!(done.1["message"], "要把哪些文件改名？");
        assert!(!events.iter().any(|(event, _)| event == EVENT_BUNDLE));
    }

    #[test]
    fn cancellation_returns_e6007() {
        let (service, _, events) = make_service(ModelResponse {
            text: "ok".to_string(),
            intent: Some(intent("f12", json!({}))),
            tool_calls: Vec::new(),
        });
        let result = service.ask_with_id(
            "ai-msg-cancel".to_string(),
            AiAskInput {
                session_id: None,
                text: "改名".to_string(),
                context: AiContext::default(),
                history: Vec::new(),
                consent_token: None,
            },
            &MockPlans {
                plan: plan_document(),
            },
            &|| true,
        );
        assert!(result.is_ok());
        let events = events.lock().unwrap();
        let error = events
            .iter()
            .find(|(event, _)| event == EVENT_ERROR)
            .expect("error event");
        assert_eq!(error.1["error"]["code"], "E6007");
    }

    #[test]
    fn provider_failure_is_emitted_without_bundle() {
        let provider = Arc::new(MockProvider {
            response: ModelResponse::default(),
            seen: Mutex::new(None),
            fail: Some(ai_error("E6006", "timeout")),
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let service = AiService::new(
            Some(provider as Arc<dyn ModelProvider>),
            Arc::new(move |event, payload| {
                sink.lock().unwrap().push((event.to_string(), payload));
            }),
        );
        ask(&service, AiContext::default(), None);
        let events = events.lock().unwrap();
        let error = events
            .iter()
            .find(|(event, _)| event == EVENT_ERROR)
            .expect("error event");
        assert_eq!(error.1["error"]["code"], "E6006");
        assert!(!events.iter().any(|(event, _)| event == EVENT_BUNDLE));
    }

    #[test]
    fn provider_parses_plain_and_fenced_json() {
        let plain = r#"{"text":"ok","intent":null,"tool_calls":[]}"#;
        assert_eq!(parse_model_response(plain).expect("plain json").text, "ok");
        let fenced = "```json\n{\"text\":\"fenced\",\"tool_calls\":[]}\n```";
        assert_eq!(
            parse_model_response(fenced).expect("fenced json").text,
            "fenced"
        );
    }

    #[test]
    fn provider_rejects_non_json_output_with_e6003() {
        let error = parse_model_response("not json").expect_err("must reject");
        assert_eq!(error.code, "E6003");
    }

    #[test]
    fn http_status_maps_to_e6xxx() {
        assert_eq!(map_http_status(401).code, "E6002");
        assert_eq!(map_http_status(403).code, "E6002");
        assert_eq!(map_http_status(408).code, "E6006");
        assert_eq!(map_http_status(429).code, "E6009");
        assert_eq!(map_http_status(503).code, "E6006");
        assert_eq!(map_http_status(400).code, "E6001");
    }

    #[test]
    fn storing_a_blank_api_key_is_rejected() {
        let error = store_api_key("   ").expect_err("blank key");
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn provider_request_body_is_minimal_and_lists_supported_kinds() {
        let request = ModelRequest {
            text: "rename invoices".to_string(),
            history: Vec::new(),
            file_count: 3,
            kinds: vec!["pdf".to_string()],
            locale: "zh-CN".to_string(),
            file_names: None,
            supported_kinds: supported_kinds(AI_PHASE),
        };
        let body = openai_request_body("test-model", &request);
        let rendered = body.to_string();
        assert!(rendered.contains("test-model"));
        assert!(rendered.contains("c14"));
        assert!(!rendered.contains("/data/"));
        assert!(!rendered.contains("File names"));
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn provider_request_body_includes_file_names_only_when_consented() {
        let request = ModelRequest {
            text: "rename".to_string(),
            history: Vec::new(),
            file_count: 1,
            kinds: vec!["pdf".to_string()],
            locale: "en-US".to_string(),
            file_names: Some(vec!["contract.pdf".to_string()]),
            supported_kinds: supported_kinds(AI_PHASE),
        };
        let rendered = openai_request_body("test-model", &request).to_string();
        assert!(rendered.contains("contract.pdf"));
    }

    #[test]
    fn invalid_model_params_are_normalized_to_e6003() {
        let graph = AiIntentGraph {
            steps: vec![step("s1", "f12", &[])],
        };
        let (service, _, events) = make_service(ModelResponse {
            text: "bad params".to_string(),
            intent: Some(graph),
            tool_calls: Vec::new(),
        });
        ask_with_plans(&service, &FailingPlans { code: "E1002" });
        let events = events.lock().unwrap();
        let error = events
            .iter()
            .find(|(event, _)| event == EVENT_ERROR)
            .expect("error event");
        assert_eq!(error.1["error"]["code"], "E6003");
        assert!(!events.iter().any(|(event, _)| event == EVENT_BUNDLE));
    }

    #[test]
    fn non_param_plan_errors_are_preserved() {
        let graph = AiIntentGraph {
            steps: vec![step("s1", "f12", &[])],
        };
        let (service, _, events) = make_service(ModelResponse {
            text: "encoding error".to_string(),
            intent: Some(graph),
            tool_calls: Vec::new(),
        });
        ask_with_plans(&service, &FailingPlans { code: "E3001" });
        let events = events.lock().unwrap();
        let error = events
            .iter()
            .find(|(event, _)| event == EVENT_ERROR)
            .expect("error event");
        assert_eq!(error.1["error"]["code"], "E3001");
    }

    #[test]
    fn history_is_forwarded_to_the_model_request() {
        let (service, provider, _) = make_service(ModelResponse {
            text: "ok".to_string(),
            intent: None,
            tool_calls: Vec::new(),
        });
        service
            .ask_with_id(
                "ai-msg-history".to_string(),
                AiAskInput {
                    session_id: None,
                    text: "第二句".to_string(),
                    context: AiContext::default(),
                    history: vec!["第一句".to_string()],
                    consent_token: None,
                },
                &MockPlans {
                    plan: plan_document(),
                },
                &|| false,
            )
            .expect("ask returns message id");
        let seen = provider.seen.lock().unwrap().clone().expect("request");
        assert_eq!(seen.text, "第二句");
        assert_eq!(seen.history, vec!["第一句".to_string()]);
    }

    #[test]
    fn provider_request_body_includes_history_before_the_current_turn() {
        let request = ModelRequest {
            text: "second".to_string(),
            history: vec!["first".to_string()],
            file_count: 0,
            kinds: Vec::new(),
            locale: "zh-CN".to_string(),
            file_names: None,
            supported_kinds: supported_kinds(AI_PHASE),
        };
        let body = openai_request_body("test-model", &request);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["content"], "first");
        assert_eq!(messages[2]["role"], "user");
        assert!(messages[2]["content"].as_str().unwrap().contains("second"));
    }

    #[test]
    fn history_defaults_to_empty_when_omitted() {
        let input: AiAskInput =
            serde_json::from_str(r#"{"text":"只有这一句"}"#).expect("deserialize input");
        assert!(input.history.is_empty());
    }

    /// `fixtures/ai/` 的 fixture 走同一套确定性契约（AI 分册 §4）：
    /// 拒绝/澄清不产 bundle，正常意图才产；每条都必须声明 `must_not_call`。
    /// AI-14 是 Phase 2 目录的代表：此前阶段锁定的 c11 现在能出单步计划。
    #[test]
    fn fixtures_ai_drive_the_contract() {
        let cases = [
            (
                "AI-01",
                include_str!("../../fixtures/ai/AI-01-normal-rename.json"),
                None,
                true,
            ),
            (
                "AI-05",
                include_str!("../../fixtures/ai/AI-05-clarification.json"),
                None,
                false,
            ),
            (
                "AI-12",
                include_str!("../../fixtures/ai/AI-12-forbidden-tool.json"),
                Some("E6004"),
                false,
            ),
            (
                "AI-13",
                include_str!("../../fixtures/ai/AI-13-unknown-kind.json"),
                Some("E6003"),
                false,
            ),
            (
                "AI-14",
                include_str!("../../fixtures/ai/AI-14-phase2-csv-dedup.json"),
                None,
                true,
            ),
        ];
        for (name, raw, expected_code, expects_bundle) in cases {
            let fixture: Value = serde_json::from_str(raw).expect("fixture json");
            let response: ModelResponse =
                serde_json::from_value(fixture["mock_model"].clone()).expect("mock_model");
            let forbidden = fixture["expected"]["must_not_call"]
                .as_array()
                .expect("must_not_call");
            assert!(
                forbidden.iter().any(|item| item == "applyPlan"),
                "{name} must forbid applyPlan"
            );

            let (service, _, events) = make_service(response);
            ask(&service, AiContext::default(), None);
            let events = events.lock().unwrap();
            let actual = events
                .iter()
                .find(|(event, _)| event == EVENT_ERROR)
                .map(|(_, payload)| payload["error"]["code"].as_str().unwrap().to_string());
            assert_eq!(actual.as_deref(), expected_code, "{name} error code");
            assert_eq!(
                events.iter().any(|(event, _)| event == EVENT_BUNDLE),
                expects_bundle,
                "{name} bundle"
            );
        }
    }
}
