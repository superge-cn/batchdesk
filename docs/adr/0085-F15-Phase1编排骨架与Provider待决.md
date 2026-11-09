# 0085. F15 Phase 1 编排骨架与 Provider 待决

- **状态**：已采纳
- **日期**：2026-11-09
- **决策者**：开发代理（工程实现细节；产品行为以设计 14 v1 与方案第 14 章为准）
- **关联**：设计 14 §0/§1/§5/§6/§7/§9/§12；方案 §14.1/§14.3/§14.4；OQ-21；OQ-40；AI 测试分册 §4

---

## 1. 问题背景

F15 Phase 1（F15-lite）需要命令/事件/DTO、FunctionCatalog、意图校验、工具 allowlist、单计划
`PlanBundle`、数据最小化与 E6xxx 错误面。设计 14 §12 仍有两项未决：**生产 Provider 的
base URL allowlist** 与 **密钥存放**（OQ-21 只定了「OpenAI 兼容＋企业自带 key」）。桌面壳当前
没有任何 HTTP 客户端依赖；若为了一个未定的端点策略引入 TLS/HTTP 栈，会把安全边界和 SBOM
一起绑死在未裁决事项上。

## 2. 候选与结论

| 候选 | 一句话定义 | 结论 |
|---|---|---|
| A | 先落 Provider 无关的编排骨架：注入 `ModelProvider`，生产装配点返回 `None`（E6001），测试用确定性 mock | **选用** |
| B | 立即引入 HTTP+TLS 依赖并允许任意 HTTPS base URL | 否决（base URL allowlist 未决，等于用实现替产品定策；新增依赖无评审） |
| C | 只做 UI 面板、不落编排/校验 | 否决（安全底线在编排层，先做 UI 会绕开 allowlist/schema/数据最小化） |

## 3. 选择理由

- 候选 B 把「允许连哪里」写进代码，而设计 14 §12 明确该问题待决；同时新增 HTTP/TLS 依赖
  需要 cargo-deny 与 SBOM 评审，收益只是让一个未定端点可连。
- 候选 C 会把「AI 永不直接 apply、工具 allowlist、默认不发文件名」这些约束推迟到没有载体的
  地方，测试也无从落锚。
- 候选 A 的编排层是安全底线的实际载体：`validate_intent`（E6003/E6010/E6011）、`validate_tool`
  （E6004）、`ModelRequest` 只含数量/类型/语言（`file_names` 必须逐次同意，缺同意 E6005）、
  计划只经注入的 `PlanFactory` 调 kernel `planOperation`。Provider 只负责「文字→意图」，
  换真实实现时接口不变（方案 13.8 的同一条注入原则）。

## 4. 后果与退路

- **后果**：生产构建 `getAiStatus.configured=false`，`aiAsk` 以 E6001 降级并引导手动操作；
  Phase 1 尚不是用户可用功能。`fixtures/ai/` 与 12 条确定性单测是当前唯一回归锚点。
- **退路**：OQ-44（base URL allowlist＋密钥存放）裁决后，只替换 `ai::production_provider` 的
  装配实现并补 HTTP 依赖；编排、校验、allowlist、事件与 UI 契约不动。推翻本决策不需要改分层。

## 5. 关联

- 设计 14 v1 §12 的未决项；OQ-21（Provider 形态）、OQ-40（`loadRecipe` 输入通道，Phase 1 不启用）；
- 代码：`src-tauri/src/ai.rs`、`src-tauri/src/lib.rs` 的 `getAiStatus`/`aiAsk`/`aiCancel` 与
  `ShellPlans`；测试 `src-tauri/src/ai.rs::tests`；夹具 `fixtures/ai/`；
- 新增待决：OQ-44（生产 Provider 的 base URL allowlist 与密钥存放）。
