# fixtures/ai：AI 助手测试数据

本目录承载 **F15 AI 助手（Phase 1/2）** 的提示/工具调用测试数据，**不属于** `fixtures/corpus/` 的 28 条文件异常语料。

- 判定权威：方案第 14 章收编版 + [设计 14](../../docs/design/14-AI助手与工具调用契约.md)；本目录只提供可执行的输入/模拟输出。
- 用例编号：`AI-01…AI-18`；详细步骤见 [AI 测试分册](../../docs/testing/AI助手测试分册-AI-v0.1.md)。
- 测试方式：mock provider 返回 `mock_model` 中的确定性输出；不接真实供应商、不在 CI 放 API key。
- 每个 fixture 必须包含 `expected.must_not_call`；默认包含 `applyPlan`、`setConfig`、`activateLicense`。
- 安全用例（提示注入、工具越权、schema 失败）的 `expected.error_code` 使用设计 06 的 E6xxx。

## 文件格式

```json
{
  "id": "AI-01",
  "purpose": "一句话说明",
  "input": {
    "text": "用户输入",
    "context": {
      "fileCount": 12,
      "kinds": ["pdf"],
      "locale": "zh-CN",
      "includeFileNames": false
    }
  },
  "mock_model": {
    "intent": {"kind": "f12", "params": {"prefix": "2026_"}},
    "tool_calls": [{"name": "planOperation", "arguments": {"kind": "f12", "params": {"prefix": "2026_"}}}]
  },
  "expected": {
    "intent_kind": "f12",
    "tool_calls": ["planOperation"],
    "must_not_call": ["applyPlan", "setConfig", "activateLicense"],
    "needs_user_confirmation": true,
    "error_code": null
  }
}
```

> 当前放了 5 条代表性 fixture：AI-01（正常 f12）、AI-05（澄清）、AI-12（工具越权）、AI-13（不可计划 kind）、AI-14（Phase 2 的 c11）。目录覆盖矩阵由 `src-tauri/src/ai.rs` 的 `catalog_covers_every_shell_routable_plan_kind` 与 `every_catalog_kind_reaches_the_plan_factory_with_one_bundle_step` 承担（24 个壳层可计划功能逐个过编排层）；其余用例在 AI 测试分册中定义，随 harness 落地补齐。多计划 fixture 使用 `intent_graph` 字段，Phase 3 才启用。
