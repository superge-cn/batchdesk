#!/usr/bin/env node
// 构建可被 WebDriver 附加的桌面二进制（Tauri WebView e2e 专用）。
//
// 背景（见 docs/testing/tauri-webview-e2e-记录.md §2.2）：
//   wry 0.57.1 无条件 `set_additional_browser_arguments(...)`，会覆盖
//   `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 环境变量；普通 `cargo build` 产物
//   的 DevToolsActivePort 永远不出现，WebDriver 无法附加。已实测可行的通道是
//   在**构建期**用 `TAURI_CONFIG` 把参数写进 `app.windows[0].additionalBrowserArgs`，
//   而不是改 tauri.conf.json（避免调试端口进发布配置）。
//
// 本脚本同时把 WebView2 用户数据目录隔离到 `target/e2e/webview-profile`——
// 否则 e2e 会写真实 profile（%LOCALAPPDATA%/com.batchdesk.desktop/EBWebView）
// 里的 localStorage/ui-state。
//
// 用法：node scripts/build-webview-e2e.mjs [--profile-dir <absolute-path>]
// 构建完成后按记录里的命令起 tauri-driver，再跑 `node scripts/e2e-tauri.mjs`。

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function option(name, fallback) {
  const index = process.argv.indexOf(name);
  return index >= 0 && process.argv[index + 1] ? process.argv[index + 1] : fallback;
}

// 构建期改 **identifier**，让**应用自身的数据目录**（Config/UiState/Journal/
// 隔离目录/localStorage）落到 `com.batchdesk.e2e`，与真实 `com.batchdesk.desktop`
// 隔离——2026-11-24 实测该目录确实被创建，e2e 不再污染真实应用数据。
//
// **边界（如实登记）**：WebView2 浏览器 profile 仍未隔离。同一天实测三条通道
// 均无效：① `additionalBrowserArgs` 加 `--user-data-dir=<temp>`；② 改 identifier；
// ③ 给 driver 进程注入 `WEBVIEW2_USER_DATA_FOLDER`。e2e 仍写真实
// `%LOCALAPPDATA%/com.batchdesk.desktop/EBWebView`。根因是 wry 建 WebView2 环境时
// 显式传了 userDataFolder，覆盖了上述通道；要彻底隔离需改产品代码或用独立
// Windows 用户跑 e2e。
const identifier = "com.batchdesk.e2e";
const config = {
  identifier,
  app: {
    windows: [
      {
        label: "main",
        title: "BatchDesk",
        width: 1280,
        height: 820,
        minWidth: 1024,
        minHeight: 680,
        resizable: true,
        additionalBrowserArgs:
          "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection " +
          "--remote-debugging-port=0"
      }
    ]
  }
};

function run(command, args, env = process.env) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", env });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

// tauri-build 不会把 TAURI_CONFIG 当依赖跟踪：参数变了但 cargo 复用缓存时，
// 产物里还是旧配置。先只清桌面壳这一包（10 秒内可重编），避免全量 clean。
console.log("[e2e] 清桌面壳构建缓存，确保 TAURI_CONFIG 生效…");
run("cargo", ["clean", "--manifest-path", "src-tauri/Cargo.toml", "-p", "batchdesk-desktop"]);

console.log(`[e2e] 构建调试端口二进制；identifier＝${identifier}（应用数据隔离，WebView2 浏览器 profile 仍共用真实目录）`);
run("cargo", ["build", "--manifest-path", "src-tauri/Cargo.toml"], {
  ...process.env,
  TAURI_CONFIG: JSON.stringify(config)
});

console.log("[e2e] 完成。下一步：");
console.log("  tauri-driver --port 4444 --native-driver <msedgedriver 路径>");
console.log("  node scripts/e2e-tauri.mjs");
console.log("  验证隔离：%LOCALAPPDATA%/com.batchdesk.e2e/ 应出现（应用数据）；EBWebView 仍属真实目录");
