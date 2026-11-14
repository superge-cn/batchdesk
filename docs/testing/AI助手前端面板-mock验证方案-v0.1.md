# AI 助手前端面板 · mock IPC 验证方案（v0.1）

> 状态：方案草案（2026-11-09）｜上游：产品方案 v1.24 第 14 章、设计 14 v1 §3/§4/§6/§7/§9、设计 12 §2/§3、AI 测试分册 v0.1 §2–§4
> 适用：`web/src` 的 F15 前端面板（尚未实现）｜性质：**验证方案**，不是实现，也不替代真实 Provider/真实 WebView 验收
> 前置：后端编排骨架已落地（`src-tauri/src/ai.rs`、`ai::tests` 12 条、`fixtures/ai/` 4 条，ADR-0085）；真实 Provider 阻断于 **OQ-44**

## 1. 目标与边界

| 项 | 内容 |
|---|---|
| 要验证什么 | 前端 AI 面板在**契约层**的行为：命令入参、事件消费、同意/数据最小化、工具卡、计划束交接、E6xxx 错误、取消、双语文案、可访问性 |
| 不验证什么 | 真实模型质量/提示词效果、真实网络与密钥、真实 WebView（`scripts/e2e-tauri.mjs` 另行覆盖）、Phase 2/3 多计划 |
| 权威 | 设计 14 v1 的命令/事件/DTO 与安全底线；T v1.13 的 B7 判定点；本方案只定义执行载体 |
| 关键证据 | Vitest 组件断言＋Playwright 真浏览器断言＋浏览器 mock 记录的「实际发出的请求」；**不以 mock 绿灯表述为 F15 验收通过** |

## 2. 被测契约（面板必须实现的形状）

### 2.1 命令与事件

| 名称 | 入参/载荷 | 面板职责 |
|---|---|---|
| `getAiStatus` | — → `AiStatus` | 打开面板时读取；`configured=false` 时禁用输入并显示 E6001 引导 |
| `aiAsk` | `{sessionId?, text, context, consentToken?}` → `{messageId}` | 发送前过同意弹窗；记录 `messageId` 供取消 |
| `aiCancel` | `{messageId}` → `boolean` | 点「取消」调用；重复点幂等 |
| `ai://delta` | `{messageId, text}` | 追加流式文本；按 messageId 过滤 |
| `ai://tool` | `{messageId, call, allowed}` | 渲染工具卡；`allowed=false` 标红并等待 error |
| `ai://bundle` | `{messageId, bundle}` | 渲染 1 步计划摘要；提供「去预览」；**不提供执行按钮** |
| `ai://done` | `{messageId, message, bundle?, bundleId?}` | 结束态；只转述，不补造成功 |
| `ai://error` | `{messageId, error}` | 按码位渲染 `error.<code>.summary/action`；不显示后端原文 |

### 2.2 UI 契约（建议命名，实现时可微调但测试锚点必须稳定）

| 锚点 | 用途 |
|---|---|
| `data-testid="ai-panel"` | 面板根；`role="complementary"`＋`aria-label` |
| `data-testid="ai-status"` | 未配置/可用状态行 |
| `data-testid="ai-log"` | 消息列表；`role="log"`＋`aria-live="polite"` |
| `data-testid="ai-tool-card"` | 工具卡（可多条） |
| `data-testid="ai-bundle"` | 计划束摘要 |
| `data-testid="ai-open-preview"` | 「去预览」按钮（唯一向执行链路的入口） |
| `data-testid="ai-cancel"` | 取消按钮 |
| `data-testid="ai-consent"` | 同意弹窗；`role="alertdialog"` |
| `data-testid="ai-error"` | 错误条（码位＋本地化文案） |

约束：面板内**不得**出现 `applyPlan` 调用或「执行」按钮；唯一出口是「去预览」，执行仍由既有预览面板与用户确认完成（设计 14 §7.1）。

## 3. Mock IPC 架构

### 3.1 文件与职责

| 文件 | 变更 | 说明 |
|---|---|---|
| `web/src/lib/ipc/types.ts` | 新增 AI DTO | `AiStatus`/`AiContext`/`AiAskInput`/`AiAskOutput`/`AiIntentGraph`/`AiToolCall`/`PlanBundle`/`PlanBundleStep`；与 Rust `ai.rs` 的 camelCase 形状一致 |
| `web/src/lib/ipc/mock.ts` | 扩展 AI 段 | `mockGetAiStatus`/`mockAiAsk`/`mockAiCancel`＋`mockSetAiScenario`/`resetAiMock`；复用已有的 `listeners`/`emit`/`wait`，**不建第二套事件总线** |
| `web/src/lib/ipc/index.ts` | 新增包装 | `getAiStatus`/`aiAsk`/`aiCancel`；`listenAiDelta/Tool/Bundle/Done/Error`（Tauri 走 `listen`，浏览器走 `mockListen`） |
| `web/src/AiPanel.tsx` | 新增组件 | 纯展示＋调用上述包装；事件先订阅后调用 `aiAsk` |
| `web/src/App.tsx` | 挂载与交接 | 右侧可折叠面板；`onOpenPlan(plan)` → 切到对应功能视图并复用现有预览面板 |
| `web/e2e/ai-panel.spec.ts` | 新增 | Playwright＋Chromium，走浏览器 mock |

### 3.2 场景开关与确定性

| 机制 | 用法 | 约束 |
|---|---|---|
| `?aiScenario=<name>` | Playwright 打开页面时选择 mock 场景 | **只在 `isTauri()===false` 的浏览器 mock 生效**；Tauri 构建忽略 |
| `mockSetAiScenario(name)` | Vitest `beforeEach` 精确指定 | 显式 setter 优先于 query 参数 |
| `resetAiMock()` | 每个用例前重置场景、取消标记、请求记录与定时器 | 清理 `listeners` 中 AI 事件，避免跨用例串扰 |
| 事件时序 | `aiAsk` 先返回 `{messageId}`，事件用 `wait(0/10/20)` 异步发出 | 面板必须容忍「事件早于 invoke resolve」与迟到/重复事件 |

场景清单：`unconfigured`、`ready`、`forbidden-tool`、`bad-schema`、`multi-step`、`consent-missing`、`timeout`、`cancelled`、`partial-failed`（仅用于 done 文案负向断言）。

### 3.3 mock 记录面（断言依据）

`resetAiMock()` 同时清空并由测试读取：`lastAskInput`（完整 `AiAskInput`）、`askCount`、`cancelCount`、`applyCount`（应恒为 0）、`emitted`（按序事件名）。这些记录只存在于浏览器 mock，不进入生产代码路径。

## 4. 测试分层

| 层 | 载体 | 数量目标 | 说明 |
|---|---|---|---|
| Rust 编排 | `src-tauri/src/ai.rs::tests` | 已有 12 条 | 已覆盖 Catalog/意图/allowlist/数据最小化/取消（本轮不改） |
| 组件级 | `web/src/AiPanel.test.tsx` | 12–14 条 | jsdom＋真实浏览器 mock；逐状态断言 |
| 集成级 | `web/src/App.aiPanel.test.tsx` | 3–4 条 | 面板开关、`ai://bundle` → 功能视图预览交接、无自动 apply |
| 真浏览器 | `web/e2e/ai-panel.spec.ts` | 5–6 条 | Chromium＋浏览器 mock；覆盖未配置/happy/禁止工具/多步/取消/英文 |
| 门禁 | `pnpm build`、`pnpm test`、`pnpm test:e2e`、`check_locales.py`、`check_docs.py` | 全绿 | 见 §7 |

## 5. 用例矩阵

### 5.1 组件级（Vitest）

| ID | 场景 | 前置/操作 | 断言 |
|---|---|---|---|
| AI-UI-01 | 未配置模型 | `unconfigured`，打开面板 | `ai-status` 显示不可用；输入与发送禁用；`askCount=0`；引导手动操作 |
| AI-UI-02 | 首次发送弹同意 | `ready`，输入文字→发送 | `ai-consent` 出现；列出「将发送：文字/文件数量/类型/语言」与「不发送：文件名/路径/内容/许可证」 |
| AI-UI-03 | 同意弹窗取消 | 同 AI-UI-02，点取消 | 弹窗关闭；`askCount=0`；输入保留 |
| AI-UI-04 | 确认后发送（默认最小化） | 同意弹窗点确认 | `lastAskInput.context.includeFileNames=false`；`fileNames` 不存在；`consentToken` 非空；`askCount=1` |
| AI-UI-05 | 文件名 opt-in | 勾选「发送文件名」并确认 | 弹窗列出文件名清单；`lastAskInput.context.fileNames` 与清单一致 |
| AI-UI-06 | 流式文本 | `ready` 连续两条 delta | `ai-log` 文本按序拼接；无重复/丢字 |
| AI-UI-07 | 工具卡（允许） | `ready` 的 `planOperation` 调用 | `ai-tool-card` 显示工具名；`allowed=true`；无错误条 |
| AI-UI-08 | 工具卡（禁止） | `forbidden-tool` | 工具卡标 `allowed=false`；`ai-error` 显示 E6004 本地化文案；无 bundle |
| AI-UI-09 | 计划束与唯一出口 | `ready` 完成 | `ai-bundle` 显示 1 步与条目数；存在「去预览」；**不存在**「执行」按钮 |
| AI-UI-10 | 多步拒绝 | `multi-step` | E6011 文案；无 bundle；不出现「去预览」 |
| AI-UI-11 | 非法 schema | `bad-schema` | E6003 文案；无 bundle |
| AI-UI-12 | 超时 | `timeout` | E6006 文案＋可重试提示；无 bundle |
| AI-UI-13 | 取消 | `ready` 发送后点「取消」 | `cancelCount=1`；显示已取消；无 bundle；再次点击不重复调用（幂等） |
| AI-UI-14 | 事件去重与迟到 | 同一 messageId 重复 `done`；卸载后再发事件 | 只处理一次；unmount 后无 setState 警告/崩溃 |
| AI-UI-15 | 双语 | zh-CN 与 en-US 各跑 AI-UI-01/08/10 | 文案来自 `t()`，语言切换后一致；无硬编码中文 |
| AI-UI-16 | 可访问性 | 打开面板与同意弹窗 | 面板 `role/aria-label`；`ai-log` 是 `role=log`＋`aria-live`；弹窗 `role=alertdialog` 且初始焦点在「取消」 |

### 5.2 集成级（Vitest）

| ID | 场景 | 操作 | 断言 |
|---|---|---|---|
| AI-INT-01 | 面板开关 | 点顶部「AI 助手」→ 关闭 | 面板出现/消失；重新打开保留本次会话消息（会话内历史） |
| AI-INT-02 | 计划束交接 | `ready` → 点「去预览」 | 导航到 `f12` 功能视图；既有预览表出现 AI 计划的条目；`applyCount=0` |
| AI-INT-03 | 交接后不自动执行 | AI-INT-02 后不点确认 | 执行按钮仍关闭（既有 §7.3 的确认/失效机制不变） |
| AI-INT-04 | 无直接 apply | 全流程 | `mockApply` 调用数为 0；界面无「执行」入口 |

### 5.3 真浏览器（Playwright）

| ID | 场景 | 断言 |
|---|---|---|
| AI-E2E-01 | 未配置 | 状态行与手动操作引导可见；输入禁用 |
| AI-E2E-02 | happy path | 同意→流式→工具卡→计划束→「去预览」→ f12 预览出现；无执行按钮 |
| AI-E2E-03 | 禁止工具 | E6004 错误条；无 bundle |
| AI-E2E-04 | 多步 | E6011；无 bundle |
| AI-E2E-05 | 取消 | 取消后无 bundle；按钮恢复可用 |
| AI-E2E-06 | 英文 | 同 AI-E2E-02 的关键断言在 en-US 下成立 |

## 6. 关键断言细则

| 主题 | 断言方式 |
|---|---|
| 数据最小化 | 只读 mock 的 `lastAskInput`；默认断言 `includeFileNames=false`、无 `fileNames`、文本中无 `/data/` 等路径；勾选后才出现文件名 |
| 永不直接 apply | mock 记录 `applyCount` 恒为 0；DOM 无「执行」按钮；`ai-open-preview` 是唯一向执行链路跳转的入口 |
| 错误码 | 逐码断言 E6001/E6002（可留场景）/E6003/E6004/E6005/E6006/E6007/E6010/E6011 的本地化 summary；不渲染后端 message/detail（沿用 ADR-0061） |
| 事件时序 | 先订阅后 `aiAsk`；测试用 `waitFor`/`expect.poll`，不写固定 sleep；重复/迟到事件按 messageId 幂等 |
| 交接 | 预览显示的条目与 bundle.steps[0].plan 一致；导航后既有表单快照机制不得误判为 stale，也不得因此绕过确认 |
| i18n | 新增 key 必须 zh-CN/en-US 成对；`check_locales.py` 通过；源码无硬编码中文 |
| 可访问性 | `role`/`aria-live`/`aria-label`/`alertdialog` 焦点；键盘 Enter/Escape 行为（Escape 关闭同意弹窗视为取消） |

## 7. 执行与门禁

| 命令 | 期望 |
|---|---|
| `cd web && pnpm test -- AiPanel App.aiPanel` | 组件＋集成用例全绿 |
| `cd web && pnpm build` | tsc 与 vite 构建通过（含新 i18n key） |
| `cd web && pnpm build && pnpm exec playwright test e2e/ai-panel.spec.ts` | 真浏览器用例全绿；**必须先 build**，Playwright 读的是 `dist` |
| `python scripts/check_locales.py` | key 成对、无硬编码中文、错误码文案齐全 |
| `python scripts/check_docs.py` | 文档索引/链接/表格列数一致 |
| 新增用例连跑 3 次 | 零 flake；禁止固定 `sleep` |
| 全量 `pnpm test`、`pnpm test:e2e` | 不回归既有 124＋18 项 |

CI 证据：web job 的 Vitest/Playwright 步骤；完成后在 `TRACEABILITY`/`DEVELOPMENT-PLAN` 回填 run 号与用例数，不以本地绿灯代替 CI。

## 8. 退出标准（DoD）

- [ ] §5 矩阵全部落地且通过（组件 16、集成 4、E2E 6，可按实现微调但不得删减关键断言）；
- [ ] `applyCount=0`、默认请求无文件名/路径/内容、同意弹窗字段清单正确；
- [ ] E6001/E6003/E6004/E6005/E6006/E6007/E6010/E6011 均有本地化断言；
- [ ] `ai://bundle` 只能经「去预览」进入既有预览，执行仍由用户确认；
- [ ] 中英文一致；`check_locales.py`/`check_docs.py`/`pnpm build` 全绿；
- [ ] 新增/更新文档：TRACEABILITY 的 F15 行、DEVELOPMENT-PLAN Wave 4、AI 测试分册 §3 执行方式、CHANGELOG；
- [ ] CI run 全绿并回填。

## 9. 风险与未决

| 风险/未决 | 处理 |
|---|---|
| 真实 Provider 未接（OQ-44） | 本方案只验证 UI 契约；`unconfigured` 场景覆盖生产真实状态；Provider 接入后另补 staging 手工验证 |
| 计划束交接与既有预览的状态模型 | 交接时把 `plannedForm`/执行期版本号快照设成当前值，或明确置 stale 并要求重新预览；**不得**为了让交接好看而绕过 §7.3 |
| mock 场景开关进入生产 | 只在 `isTauri()===false` 分支读取 `aiScenario`；Tauri 构建无此路径，测试断言 `isTauri()===true` 时不读取 query |
| 事件早到/重复 | 面板按 `messageId` 归档，重复事件幂等；测试覆盖迟到与重复 |
| mock 与 Rust 形状漂移 | DTO 以 Rust `ai.rs` 的 camelCase 为准；建议在 `ai::tests` 增加一条 `serde_json::to_value(AiStatus)` 快照断言，前端 mock 复用同名字段 |

## 10. 交付物清单

1. `web/src/lib/ipc/types.ts` 的 AI DTO；`mock.ts` 的 AI 段与场景/重置函数；`index.ts` 的包装与监听；
2. `web/src/AiPanel.tsx`＋样式（只用 U v1.3 token）；
3. `web/src/AiPanel.test.tsx`、`web/src/App.aiPanel.test.tsx`、`web/e2e/ai-panel.spec.ts`；
4. i18n `ai.*` key（zh-CN/en-US 成对）；
5. 文档回填与 CI 证据。
