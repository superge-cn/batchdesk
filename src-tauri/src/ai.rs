//! F15 AI 助手（Phase 1 / F15-lite）编排骨架（设计 14）。
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

use std::sync::Arc;

use kernel::error::KernelError;
use kernel::plan::PlanDocument;
use platform::ids::new_ulid;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 当前启用的 AI 阶段（设计 14 §1：Phase 1＝单轮＋澄清、单计划、4 个功能）。
pub const AI_PHASE: u8 = 1;

/// FunctionCatalog 的版本；目录变化必须递增（设计 14 §5 规则 5）。
pub const CATALOG_VERSION: &str = "2026-11-09.p1";

/// Phase 1 允许 AI 生成计划的功能（设计 14 §1：F12、F05、F07、C07）。
///
/// 这是**唯一来源**：不在表里的 kind 一律 E6003/不可见，AI 侧不得另存一份清单。
pub const CATALOG: &[FunctionDescriptor] = &[
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
];

/// 工具 allowlist（设计 14 §6）。`loadRecipe` 因 OQ-40（输入通道未定）在 Phase 1 不启用。
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

/// 生产构建的 Provider 装配点。
///
/// 设计 14 §12 的「供应商/base URL allowlist」与密钥存放仍未裁决；本切片不新增
/// 联网/HTTP 依赖，因此返回 `None`：`getAiStatus.configured=false`，`aiAsk` 以
/// E6001 明确降级为手动操作。确定性测试通过 `AiService::new(Some(mock))` 注入。
pub fn production_provider() -> Option<Arc<dyn ModelProvider>> {
    None
}

pub const EVENT_DELTA: &str = "ai://delta";
pub const EVENT_TOOL: &str = "ai://tool";
pub const EVENT_BUNDLE: &str = "ai://bundle";
pub const EVENT_DONE: &str = "ai://done";
pub const EVENT_ERROR: &str = "ai://error";

/// 模型输出校验（设计 14 §5／§7）。
pub fn validate_intent(graph: &AiIntentGraph) -> Result<(), KernelError> {
    if graph.steps.is_empty() {
        return Err(ai_error("E6003", "model returned an empty intent"));
    }
    if graph.steps.len() > 1 {
        return Err(ai_error(
            "E6011",
            "multi-step intents are not supported in the current phase",
        ));
    }
    let step = &graph.steps[0];
    if step.needs_clarification == Some(true) {
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
    if !step.depends_on.is_empty() {
        return Err(ai_error(
            "E6010",
            "a single-step intent must not declare dependencies",
        ));
    }
    if !step.params.is_object() {
        return Err(ai_error("E6003", "intent params must be a JSON object"));
    }
    Ok(())
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

        // 只执行 allowlist 内的计划工具：AI 提供的参数交给 kernel 复核。
        let plan = plans.plan(&step.kind, &step.params)?;
        if cancelled() {
            return Err(ai_error("E6007", "cancelled by the user"));
        }
        let bundle = PlanBundle {
            bundle_id: new_ulid("ai-bundle"),
            steps: vec![PlanBundleStep {
                step_id: step.step_id.clone(),
                plan,
                depends_on: step.depends_on.clone(),
                status: "ready",
            }],
            conflicts: Vec::new(),
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
            "mock-phase1"
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
        fn plan(&self, kind: &str, _params: &Value) -> Result<PlanDocument, KernelError> {
            assert_eq!(kind, "f12");
            Ok(self.plan.clone())
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
                    consent_token: consent.map(|item| item.to_string()),
                },
                &MockPlans {
                    plan: plan_document(),
                },
                &|| false,
            )
            .expect("ask returns message id")
    }

    #[test]
    fn catalog_only_exposes_phase_one_plan_capable_kinds() {
        assert_eq!(supported_kinds(1), vec!["f12", "f05", "f07", "c07"]);
        assert!(descriptor("c09").is_none());
        assert!(descriptor("f15").is_none());
    }

    #[test]
    fn unknown_function_kind_is_rejected_with_e6003() {
        let (service, _, events) = make_service(ModelResponse {
            text: "ok".to_string(),
            intent: Some(intent("c09", json!({}))),
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
    fn multi_step_intent_is_rejected_with_e6011() {
        let mut graph = intent("f12", json!({}));
        graph.steps.push(AiIntentStep {
            step_id: "s2".to_string(),
            kind: "f05".to_string(),
            params: json!({}),
            depends_on: vec!["s1".to_string()],
            needs_clarification: None,
            question: None,
        });
        let (service, _, events) = make_service(ModelResponse {
            text: "two steps".to_string(),
            intent: Some(graph),
            tool_calls: Vec::new(),
        });
        ask(&service, AiContext::default(), None);
        let events = events.lock().unwrap();
        let error = events
            .iter()
            .find(|(event, _)| event == EVENT_ERROR)
            .expect("error event");
        assert_eq!(error.1["error"]["code"], "E6011");
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

    /// `fixtures/ai/` 的四个 Phase 1 fixture 走同一套确定性契约（AI 分册 §4）：
    /// 拒绝/澄清不产 bundle，正常意图才产；每条都必须声明 `must_not_call`。
    #[test]
    fn fixtures_ai_drive_the_phase_one_contract() {
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
