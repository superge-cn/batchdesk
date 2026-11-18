#!/usr/bin/env node
// 构建 Web 形态的 WASM 产物（ADR-0098／设计 05 §2）。
//
// 步骤：
//   1. `cargo build -p batchdesk-wasm --target wasm32-unknown-unknown --features web-zero-force`
//   2. `wasm-bindgen --target web` 生成 JS 胶水与 `.wasm`，落在 `web/wasm/`
//   3. 置 `web/wasm/ENABLED` 标记，前端 `VITE_BATCHDESK_WASM=1` 据此启用
//
// 产物**不入库**（`.gitignore` 排除 `web/wasm/`）；CI/本机需要时跑本脚本。

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const outDir = path.join(root, "web", "wasm");

function run(command, args) {
  execFileSync(command, args, { cwd: root, stdio: "inherit" });
}

const cargo = process.env.CARGO ?? "cargo";
const wasmBindgen = process.env.WASM_BINDGEN ?? "wasm-bindgen";

console.log("[wasm] 编译 wasm 目标…");
run(cargo, [
  "build",
  "-p",
  "batchdesk-wasm",
  "--target",
  "wasm32-unknown-unknown",
  "--features",
  "web-zero-force",
]);

const artifact = path.join(
  root,
  "target",
  "wasm32-unknown-unknown",
  "debug",
  "batchdesk_wasm.wasm"
);
if (!fs.existsSync(artifact)) {
  throw new Error(`未找到产物：${artifact}`);
}

fs.mkdirSync(outDir, { recursive: true });
console.log("[wasm] 生成 JS 胶水…");
run(wasmBindgen, [
  artifact,
  "--target",
  "web",
  "--out-dir",
  outDir,
  "--out-name",
  "batchdesk_wasm",
]);

// 启用标记：前端据此判断产物是否就位（缺失时按不可用降级，不判定）。
fs.writeFileSync(path.join(outDir, "ENABLED"), "1");
console.log(`[wasm] 完成：${outDir}`);
