# batchdesk web

C09 重复文件查找的桌面 UI 垂直切片。`web/` 只做表现与交互，不包含 plan/apply、文件系统或授权规则；真实执行由 `src-tauri` 命令调用 `crates/kernel` + `crates/platform`。

```bash
pnpm install
pnpm dev        # http://127.0.0.1:1420
pnpm build      # tsc --noEmit + vite build
# 在仓库根：pnpm install && pnpm tauri dev
# 需要 WebKitGTK（Linux）或 WebView2（Windows）
```

- 浏览器直接打开时使用 `src/lib/ipc/mock.ts`，页面会明确显示“浏览器预览 · mock IPC”；mock 只提供固定演示数据，不读写文件。
- 在 Tauri 中运行时使用 `src/lib/ipc/index.ts` 的 `invoke`，命令名为设计 01 的 camelCase 名称。
- 当前 UI 使用 U v1.3 token 与液态玻璃外壳；shadcn 完整组件、i18n、虚拟滚动、Vitest/Playwright 仍待补。
- 页面主链：参数 → 计划预览 → 勾选确认 → apply → undo / 保留期 cleanup。
- Tauri 壳已编译并启动窗口；WebView 内的 IPC 点击级 E2E 仍待补（当前浏览器 mock 流已验证）。
