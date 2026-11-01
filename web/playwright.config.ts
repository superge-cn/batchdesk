import { defineConfig, devices } from "@playwright/test";

// 真实浏览器（Chromium）下的界面语义测试。
// 与 jsdom（pnpm test）的分工：jsdom 断言"调用形状与可达性"，这里断言"渲染出来的面板语义"。
// 与 scripts/e2e-tauri.mjs 的分工：那条链路驱动真实 Tauri WebView（WebKitGTK，需 tauri-driver），
// 这里驱动 Chromium ＋ 浏览器 mock IPC（src/lib/ipc/mock.ts 夹具），不覆盖真机文件系统。
const PORT = 4173;
const BASE_URL = `http://127.0.0.1:${PORT}`;

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  // 23 视图逐个点击 ＋ 每步 4 项断言，默认 30 s 预算不够
  timeout: 150_000,
  workers: 1,
  reporter: process.env.CI ? [["list"], ["github"]] : [["list"]],
  use: {
    baseURL: BASE_URL,
    trace: "retain-on-failure",
    viewport: { width: 1280, height: 900 }
  },
  webServer: {
    // dist 由 `pnpm build` 产出；CI 在类型检查与构建步骤之后运行本测试。
    command: `pnpm preview --host 127.0.0.1 --port ${PORT} --strictPort`,
    url: BASE_URL,
    timeout: 120_000,
    reuseExistingServer: !process.env.CI
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }]
});
