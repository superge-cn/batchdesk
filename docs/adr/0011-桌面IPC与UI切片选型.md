# 0011. 桌面 IPC/UI 切片选用 Tauri v2 + React/Vite

- **状态**：已采纳（2026-10-29，用户“开始接 IPC/UI”）
- **日期**：2026-10-29
- **决策者**：用户（产品所有者）／工程实现
- **关联**：设计 12 v0.3、U v1.3、设计 01 §4、ADR-0004

---

## 1. 问题背景

C09 的 kernel/platform/CLI 已跑通，需要一个可操作的桌面入口：命令注册、计划暂存、隔离/Journal 路径注入，以及能展示计划预览、确认执行、撤销和保留期清理的界面。设计 12 指定 Tauri + React + shadcn 目标结构，但仓库此前没有 `src-tauri`/`web`，也没有前端构建链。

触发条件：用户确认“开始接 IPC/UI”；必须先定下壳与前端的最小可运行组合，否则无法验证命令契约与交互主链。

## 2. 候选与结论

| 候选 | 一句话定义 | 结论 |
|---|---|---|
| A | Tauri v2 + React 19 + Vite 7 + TypeScript；C09 单页，U token 自定义组件；IPC 薄封装 | **选用** |
| B | 复用评审用静态 HTML，加脚本模拟 IPC | 否决：无真实命令边界，无法验证计划暂存/隔离/Journal |
| C | 一次性接全 shadcn/Tailwind、路由与状态库 | 暂缓：范围超出 C09 切片；不排除后续引入 |

## 3. 选择理由

- Tauri v2 与设计 12 的壳边界一致；`src-tauri` 使用独立 Cargo workspace，避免根 `cargo test` 被 GUI 系统依赖拖慢；
- React/Vite 是设计 12 目标栈的最小可运行组合；C09 页面先用 U v1.3 的语义 token 与液态玻璃外壳，避免在第一个页面就引入整套 shadcn 组件；
- IPC 只做命令封装，浏览器无 Tauri 时使用**明确标注的 mock**，不在 Web 写业务规则或授权判定；
- 同步命令先跑通 plan→apply→undo/cleanup 主链，任务队列/事件作为下一层增量，避免一次性引入队列与事件总线阻塞首条 UI 路径。

## 4. 后果与退路

- **后果**：`web` 新增 Node/pnpm 构建链与 CI `web` job；`src-tauri` 依赖 Tauri/系统 WebView，CI 暂只做源码检查，真实 E2E 待补；当前 UI 无路由/状态库、i18n、虚拟滚动和 shadcn 完整组件，均需后续增量；
- **后果**：IPC 当前为同步返回，尚未按 01 §4 返回 `taskId` 并推送 `task://*` 事件；这是已知切片偏差，接入任务队列时一次性对齐；
- **退路**：若 shadcn/路由/状态选型另有结论，保留 `web/src/features/c09` 与 `lib/ipc` 边界，替换 UI 层与路由即可；若 Tauri 系统依赖不可用，回退为浏览器 mock 预览 + CLI 验证，不改 kernel/platform。

## 5. 关联

- 设计 12 v0.3、U v1.3、设计 01 §4、ADR-0004；
- `web/package.json`、`src-tauri/Cargo.toml`、`src-tauri/src/lib.rs`、`web/src/lib/ipc/`。
