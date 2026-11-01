# 0046. 真浏览器 UI 语义层：Playwright＋Chromium 与浏览器 mock 夹具的边界

- **状态**：已采纳（2026-11-01，开发代理按 ADR-0045 预留的第二层实施）
- **日期**：2026-11-01
- **决策者**：开发代理
- **关联**：ADR-0045（Vitest 骨架，本文是其"Playwright 后续"项的落地）；设计 12 §3.1；TC v0.3 §2.2 预览配方 P3/P4、§3 S01/S02 行

---

## 1. 问题背景

映射表 v0.2 的 D 类（纯 UI 语义）5 条断言不了：C05 的"预览失效/预览刷新/未选策略阻断"、S01 的"报告可读""无跳过预检入口"、S02 的"二次确认文案"。jsdom 只证明调用形状，证明不了"面板真的换掉了旧内容""报告把三类结论摆出来了"。

同时有一个环境事实：本机 **没有 `tauri-driver`／`WebKitWebDriver`**（已核实：`command -v` 全部为空；webkit2gtk 库存在、桌面壳调试二进制存在）。所以"真 WebView 验证"这一层今天写不出可执行断言——Playwright 自带的 WebKit 是它自己构建的 WebKit，**不是** Tauri 在 Linux 上用的 WebKitGTK，用它冒充真 WebView 会得到虚假的绿灯。

## 2. 候选与结论

| 议题 | 候选 | 结论 |
|---|---|---|
| 第二层用什么跑 | a) 扩写 `scripts/e2e-tauri.mjs`（WebDriver＋tauri-driver）；b) Playwright＋Chromium 驱动 `vite preview` 的构建产物 | **b 落地、a 保留**：a 需要 tauri-driver/WebKitWebDriver，本机没有且安装要动系统包（需人类授权）；b 能立刻把 D 类语义变成断言。两者覆盖面不同，不能互相冒充 |
| Playwright 用哪个引擎 | a) chromium＋webkit 双项目；b) 仅 chromium | **b**：webkit 项目会让人误以为在验证 Tauri 引擎，制造虚假信心；先只留 chromium，将来接真 WebView 用 e2e-tauri 那条链路 |
| 功能页在浏览器预览里怎么拿到计划 | a) 功能页在 mock 模式下禁用"生成计划预览"；b) `mockPlan` 为非 c09 kind 返回同形状最小夹具 | **b**：现状是功能页点"生成计划预览"会得到 C09 专属的 `E1001 请填写扫描目录`（错误码与场景无关，属误导）。夹具明确标注"真实执行与撤销需桌面壳"，且只用于 UI 语义断言 |
| 浏览器 mock 的边界声明 | 在测试与文档里写清"不代表真机文件系统" | **必须**：夹具只证明面板语义与可达性；凡断言"文件真的被改/被回滚"的用例仍归 kernel/CLI/壳层测试与 e2e-tauri |
| CI 成本 | 在 web job 内加 `npx playwright install --with-deps chromium` ＋ `pnpm test:e2e` | **接受**：本机 8 条用例 32 s；CI 多一次浏览器下载，换取 D 类语义每次提交都被守 |

## 3. 选择理由与落地

- 三个用例文件、8 条断言：`web/e2e/feature-views.spec.ts`（23 视图逐个可达＋不残留上一视图，A 层是真实浏览器布局下的挂载回归）、`web/e2e/rename-preview.spec.ts`（C05 PRE-03/04/06 与 C06 回滚预演面板）、`web/e2e/preflight-report.spec.ts`（S01 报告三类计数、以及"不存在跳过预检入口"的负向断言）。
- 落地过程中修掉两个自身错误：① `new RegExp(\`^${label}\s*${code}$\`)` 里 `\s` 被 JS 字符串吞成 `s`，导致导航项永远匹配不到——改为 `\\s`；② 预检用例用 `getByLabel(/扫描目录/)` 定位失败，因为 C09 的参数框只有 placeholder、没有 `<label for>` 关联，改按 placeholder 定位。
- 映射表相应变化：D 类 5 条清空——C05-PRE-06、S01-RBK-02 转 ✅（带 e2e 锚点），C05-PRE-03/04、S01-PRE-06 转 ◐（面板已断言，但缺口换成更准确的描述：UI 恒传 `onConflict=skip` 无策略选择入口；改参数未重新预览时旧 planId 仍可执行；文案观感属人工走查），**S02-PRE-06 由 🔶 改判 ⛔ 待实现**（界面上根本没有保护命中的二次确认弹窗，谈不上"待 UI 断言"）。

## 4. 后果与退路

- **后果**：CI web job 变长；`mockPlan` 多一条非 c09 分支（浏览器预览专用，不影响桌面路径）；映射表的载体列新增"Playwright＋Chromium ＋ 浏览器 mock 夹具"一类，与"根 workspace CI／壳层 CI／真人工"并列。
- **未解除**：真 WebView（WebKitGTK）验证仍只有 `scripts/e2e-tauri.mjs` 的 C09 链路，且需 `tauri-driver` 环境；S02 二次确认弹窗属功能未实现。
- **退路**：若 CI 装浏览器不稳定，可把 e2e 步骤改成 nightly/手动触发，但**不得静默跳过**（跳过要让 job 变红或显式标注，避免"CI 绿＝UI 语义已验"的误读）。

## 5. 关联

- 代码：`web/playwright.config.ts`、`web/e2e/*.spec.ts`、`web/src/lib/ipc/mock.ts`（非 c09 夹具）、`.github/workflows/ci.yml`（web job 两步）、[scripts/gen_case_map.py](../../scripts/gen_case_map.py)（映射表生成器）；
- 台账：`docs/testing/用例覆盖映射表-v0.2.md/.csv`（✅155／◐104／⛔37，D 类清零）、`docs/DEVELOPMENT-PLAN.md` §6.1、`docs/design/TRACEABILITY.md` 12 行与 §2。
