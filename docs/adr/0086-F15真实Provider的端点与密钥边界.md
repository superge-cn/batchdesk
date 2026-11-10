# 0086. F15 真实 Provider 的端点与密钥边界

- **状态**：已采纳
- **日期**：2026-11-09
- **决策者**：产品所有者（OQ-44 三项裁决）＋开发代理（HTTP/凭据库的工程实现）
- **关联**：设计 14 §1/§8/§9/§12；方案 §14.5；OQ-21；OQ-44；ADR-0085

---

## 1. 问题背景

ADR-0085 的 Phase 1 编排骨架把生产 `production_provider()` 留空（`configured=false`，`aiAsk`
以 E6001 降级），因为设计 14 §12 的 **base URL allowlist** 与 **API key 存放**未决（OQ-44）。
F15 Phase 2/3 的目录与多计划已落地，真实模型调用成为唯一阻断项。

## 2. 裁决与实现

产品所有者对 OQ-44 的裁决（2026-11-09）：

1. base URL 只允许官方 OpenAI 域名；企业代理域名后续另行加入白名单；
2. API key 存系统凭据库，不进 ConfigDocument、UiState、日志；
3. 不允许任意 HTTPS 端点。

工程实现：

| 项 | 选择 | 说明 |
|---|---|---|
| 端点 | `https://api.openai.com/v1/chat/completions`（常量） | 不读取任何可覆盖端点；换域名必须改代码与白名单评审 |
| HTTP/TLS | `ureq 2`（rustls，不引入 OpenSSL） | 阻塞调用；`ModelProvider::complete` 同步签名不变 |
| 凭据库 | Windows：`keyring 3`（Credential Manager，service `batchdesk`／user `openai`）；非 Windows：`BATCHDESK_AI_API_KEY` 环境变量 | 非 Windows 仅开发/CI 回退；生产 Windows 走系统凭据库 |
| 模型/超时 | 环境变量 `BATCHDESK_AI_MODEL` 与 `BATCHDESK_AI_TIMEOUT_SECS`，均为必填 | 不在代码里自定模型名或数值阈值；任一缺失即 `configured=false`／E6001 |
| 设置入口 | `setAiApiKey` 命令＋AI 面板「配置 API 密钥」 | 只写凭据库；前端不持久化、不回显 |
| 响应解析 | 要求模型输出单个 JSON（`text`/`intent`/`tool_calls`），容忍代码围栏 | 解析失败 E6003；不把自由文本当意图 |
| 状态码 | 401/403→E6002、408→E6006、429→E6009、5xx→E6006、其它 4xx→E6001 | 供应商原文只进日志，不展示 |

## 3. 候选与结论

| 候选 | 一句话定义 | 结论 |
|---|---|---|
| A | 固定官方端点＋系统凭据库＋模型/超时由部署方给 | **选用**（落实 OQ-44 三项裁决，且不把数值阈值写进代码） |
| B | 允许任意 HTTPS base URL | 否决（违反裁决③，等于把 SSRF/数据出境面交给配置） |
| C | 继续无 Provider，只做 Phase 2/3 编排 | 否决（无法验证真实模型下的意图/工具调用；用户已批准接入） |
| D | key 存 ConfigDocument 或环境变量作为生产路径 | 否决（违反裁决②；配置与日志会扩散密钥） |

## 4. 后果与已知限制

- **取消不等于中断**：`aiCancel` 在请求前后检查；已发出的阻塞 HTTP 请求要等超时/返回。
  设计 14 §8 的 E6007 语义与「无文件副作用」不受影响，但响应延迟可能到超时上限。
- **模型名/超时外部化**：部署方必须设置两个环境变量；未设置时 AI 明确不可用（E6001），
  不影响其它功能。
- **非 Windows 密钥回退**：仅开发/CI；生产形态（Windows 桌面）走 Credential Manager。
- **未完成**：真实 Provider 的 staging 验收、多轮会话、AI 侧参数 schema、`loadRecipe` 工具；
  束级自动回滚按 OQ-23 不做。
- **依赖审计**：新增 `ureq`（MIT/Apache-2.0）与 Windows-only `keyring`（MIT/Apache-2.0）；
  本地未安装 cargo-deny，许可证/来源由 CI deny job 复核。

## 5. 关联

- 代码：`src-tauri/src/ai.rs` 的 `production_provider`／`OpenAiProvider`／`store_api_key`、
  `src-tauri/src/lib.rs` 的 `setAiApiKey`；前端 `web/src/AiPanel.tsx` 的配置区、
  `web/src/lib/ipc/index.ts` 的 `setAiApiKey`；
- 测试：`ai::tests` 的 `provider_parses_plain_and_fenced_json`、`http_status_maps_to_e6xxx`、
  `provider_request_body_is_minimal_and_lists_supported_kinds`、`storing_a_blank_api_key_is_rejected`；
- OQ-44 关闭；设计 14 §12 的端点/密钥两项由本 ADR 落实。
