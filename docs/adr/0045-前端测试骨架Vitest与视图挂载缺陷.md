# 0045. 前端测试骨架：Vitest＋jsdom 组件级断言进 CI，并记录一次"视图从未挂载"的缺陷

- **状态**：已采纳（2026-11-01，开发代理按台账既定方向实施）
- **日期**：2026-11-01
- **决策者**：开发代理
- **关联**：ADR-0040…0044（UI 接入五批）；设计 12 §3.1；T v1.12 §6.1（环境矩阵）；AGENTS 修改完成的定义（涉及 web/src-tauri 的项）

---

## 1. 问题背景

映射表 v0.2 里 E4（任务控制）与部分 RBK 行的状态是 ◐，理由都是"取消/部分失败只在队列层测过，功能级未断言"。搭骨架时先做了一个前提检查——**功能视图到底渲染不渲染**，结果发现真缺陷：

- `web/src/App.tsx` 第 43 行 `import { FeaturePages } from "./FeaturePages";`，但全文件**没有任何 `<FeaturePages …>` 使用点**；主区只有 `{activeView === "c09" ? (…) : 什么都没有}`。
- 后果：ADR-0040…0044 交付的 23 个"功能视图"，点击导航后主区实际是空白。台账把它记成"已接入"，是因为验证手段只有 `tsc --noEmit` ＋ `vite build` ＋ cargo 测试——三者都看不见"组件没挂载"这件事（未使用的 import 不报错，构建产物里没有该组件字符串也不会失败）。
- 实测证据：修好挂载后，bundle 从 263.55 kB 涨到 289.06 kB，且 `FeaturePages` 独有文案才第一次出现在产物里。

## 2. 候选与结论

| 议题 | 候选 | 结论 |
|---|---|---|
| 前端测试框架 | a) Playwright（真浏览器）；b) Vitest＋jsdom（组件级）＋保留 Playwright 作后续 e2e | **b 起步**：本批要解的是"调用形状与可达性"，jsdom 足够且能进 CI（无需 WebView/驱动）；a 仍缺，登记为后续（真机 e2e 由 `scripts/e2e-tauri.mjs` 手动覆盖 C09） |
| 交互层 | a) 自建 fireEvent 封装；b) @testing-library/react ＋ user-event ＋ jest-dom | **b**：按可访问性角色查询（role/label），与 U v1.3 的语义化标记一致，不依赖 class 名 |
| IPC 打桩 | a) 走既有浏览器 mock（`lib/ipc/mock.ts`）；b) `vi.mock("./lib/ipc")` 逐函数打桩 | **两者并用**：导航可达性测试用真实 mock 路径（顺带验证 `isTauri()` 判定），任务控制测试用 vi.mock 断言 `planOperation/applyPlan/cancelTask/retryTask/undoPlan` 的**调用与参数形状** |
| 依赖许可 | 新增 devDependencies：vitest、jsdom、@testing-library/{react,dom,user-event,jest-dom} | 已核 `pnpm licenses list`：全部落在 MIT／Apache-2.0／BSD-2/3／ISC／MIT-0 内（CC-BY-4.0 的 caniuse-lite 属既有 browserslist 传递依赖，非本次引入）；仅 devDependency，不进产物、不受 cargo-deny 覆盖但按 OQ-13 同口径自查 |
| 部分失败的注入方式 | a) 生产代码留测试钩子；b) 在 mock 适配层注入（`TestFs.fail_writes`） | **b**：内核不变量不允许产品代码为测试让路；条目级失败由适配层返回 E5001 模拟，走的就是真实错误传播路径 |

## 3. 选择理由

- **可达性必须是断言而不是观察**：导航项与视图标题在两侧各写一遍，靠人点击发现不了"import 了没用"。`App.navigation.test.tsx` 把 23 个视图的 `导航标签 → 页面标题 → 参数区/执行区按钮` 串成一条断言，任何一侧漂移即红。
- **任务控制的可测部分是调用形状**：取消/重试在真实文件系统上的效果由 kernel/壳层测试负责（见下条），前端只需证明"点了正确的命令、传了正确的 taskId、undo 任务不给重试按钮"。
- **功能级取消与部分失败落到内核断言**：`generic_apply` 新增两个测试——`cancel_rolls_back_applied_entries_in_each_write_arm`（copy／move／edit-content／split(1:N)／csv-extract(1:1 产物)／create-dir 六臂，取消后已完成条目回滚、Journal 不残留 Applied）与 `a_failing_entry_marks_partial_and_undo_reverts_only_applied_entries`（失败条目不落半成品、只有成功条目进 Journal、undo 只回滚已应用）。这才是映射表 ◐ 的实质解法，前端测试只是把它呈现出来。

## 4. 后果与退路

- **后果**：CI 的 web job 多一步 `pnpm test`（本机 30 s，其中导航用例 15 s）；`testTimeout` 提到 90 s 容纳逐视图点击。台账口径修正：**"功能视图已接入"自本批起才成立**，此前五批的记录应理解为"壳层计划路由已接入、前端组件已就位但未挂载"。
- **退路**：若 jsdom 与真实 WebView 行为出现分歧（如 `display: contents` 布局），把导航用例迁到 Playwright/WebDriver 运行器即可——用例按角色查询，不依赖 jsdom 私有 API。`scripts/e2e-tauri.mjs` 保持手动执行不变。

## 5. 关联

- 代码：`web/src/App.tsx`（挂载 FeaturePages ＋ else 分支）、`web/src/FeaturePages.tsx`（取消/重试失败项入口）、`web/src/App.navigation.test.tsx`、`web/src/FeaturePages.controls.test.tsx`、`web/vitest.config.ts`、`web/src/test/setup.ts`、`.github/workflows/ci.yml`（web job 增 `pnpm test`）、`crates/kernel/src/generic_apply.rs`（六臂取消 ＋ 部分失败注入）；
- 台账：`docs/testing/用例覆盖映射表-v0.2.csv`（EXE-04/RBK-02 行的锚点与状态）、`docs/DEVELOPMENT-PLAN.md` §6.1、`docs/design/TRACEABILITY.md` 12 行与 §2。
