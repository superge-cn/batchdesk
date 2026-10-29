#!/usr/bin/env node
// WebView end-to-end check for the C09 task queue.
//
// Prerequisites (Linux):
//   1. Vite dev server: `pnpm dev` in web/
//   2. tauri-driver listening on 127.0.0.1:4444, for example:
//        tauri-driver --port 4444 --native-driver /path/to/WebKitWebDriver
//   3. A debug desktop binary: `cargo build --manifest-path src-tauri/Cargo.toml`
//
// Usage:
//   node scripts/e2e-tauri.mjs [--app <binary>] [--driver http://127.0.0.1:4444]
//                              [--dev-url http://127.0.0.1:1420] [--keep]
//
// The script drives the real Tauri WebView through apply -> undo and a
// cancel-during-apply scenario, verifying the actual filesystem results.
// The app uses its normal app-data quarantine/journal; launch the driver with
// an isolated XDG_DATA_HOME if you do not want test entries in your profile.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const args = process.argv.slice(2);
function option(name, fallback) {
  const index = args.indexOf(name);
  return index >= 0 && args[index + 1] ? args[index + 1] : fallback;
}
const keep = args.includes("--keep");
const driverUrl = option("--driver", "http://127.0.0.1:4444").replace(/\/+$/, "");
const devUrl = option("--dev-url", "http://127.0.0.1:1420").replace(/\/+$/, "");
const appBinary = path.resolve(
  option("--app", "src-tauri/target/debug/batchdesk-desktop")
);
const screenshotPath = path.resolve("target/e2e/tauri-task-queue.png");

let sessionId = null;
const tempRoots = [];

function fail(message) {
  throw new Error(message);
}

async function request(method, pathname, body) {
  const response = await fetch(`${driverUrl}${pathname}`, {
    method,
    headers: body === undefined ? undefined : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body)
  });
  const payload = await response.json();
  if (payload?.value?.error) {
    fail(`${payload.value.error}: ${payload.value.message ?? ""}`);
  }
  if (!response.ok) {
    fail(`${method} ${pathname} failed with HTTP ${response.status}`);
  }
  return payload.value;
}

const sessionPath = () => `/session/${sessionId}`;
const execute = (script) =>
  request("POST", `${sessionPath()}/execute/sync`, { script, args: [] });

async function waitFor(label, script, timeoutMs = 30000, intervalMs = 150) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    last = await execute(`return Boolean(${script});`);
    if (last === true) return;
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
  fail(`timed out waiting for ${label}; last=${JSON.stringify(last)}`);
}

function makeDuplicates(count) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "batchdesk-e2e-"));
  tempRoots.push(root);
  const data = path.join(root, "data");
  fs.mkdirSync(data);
  for (let index = 0; index < count; index += 1) {
    fs.writeFileSync(path.join(data, `copy-${String(index).padStart(3, "0")}.txt`), "same-content\n");
  }
  return data;
}

async function typeInto(selector, value) {
  const element = await request("POST", `${sessionPath()}/element`, {
    using: "css selector",
    value: selector
  });
  const elementId = element["element-6066-11e4-a52e-4f735466cecf"];
  await request("POST", `${sessionPath()}/element/${elementId}/clear`, {});
  await request("POST", `${sessionPath()}/element/${elementId}/value`, {
    text: value,
    value: [...value]
  });
  return execute(
    `return document.querySelector(${JSON.stringify(selector)}).value === ${JSON.stringify(value)}`
  );
}

async function clickByText(text) {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    const clicked = await execute(`return (() => {
      const button = [...document.querySelectorAll("button")].find(
        (item) => item.textContent.includes(${JSON.stringify(text)})
      );
      if (!button) return false;
      button.click();
      return true;
    })();`);
    if (clicked) return true;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return false;
}

async function planFor(root, expectedRows) {
  await waitFor(
    "scan input",
    'document.querySelector("input#root") !== null && document.body.innerText.includes("扫描与清理参数")'
  );
  let entered = false;
  for (let attempt = 0; attempt < 10 && !entered; attempt += 1) {
    entered = await typeInto("input#root", root).catch(() => false);
    if (!entered) await new Promise((resolve) => setTimeout(resolve, 100));
  }
  if (!entered) fail("scan directory input did not accept the path");
  await waitFor(
    "plan button",
    '[...document.querySelectorAll("button")].some((item) => item.textContent.includes("生成计划预览"))'
  );
  if (!(await clickByText("生成计划预览"))) fail("plan button is missing");
  await waitFor(
    `plan with ${expectedRows} rows`,
    `document.querySelectorAll(".row:not(.row-head)").length === ${expectedRows}`
  );
}

async function confirmAndApply() {
  await execute('document.querySelector("#confirm").click(); true');
  if (!(await clickByText("移入隔离目录"))) fail("apply button is missing");
}

async function waitApplyCompleted() {
  await waitFor(
    "apply task completion",
    `(() => {
      const panel = [...document.querySelectorAll(".task-panel .task-role")]
        .some((item) => item.textContent.includes("执行任务"));
      const done = [...document.querySelectorAll(".task-panel")]
        .some((item) => item.textContent.includes("已完成"));
      const rows = document.querySelectorAll(".result-row").length > 0;
      return panel && done && rows;
    })()`
  );
}

async function waitUndoCompleted() {
  await waitFor(
    "undo task completion",
    `(() => {
      const text = document.body.innerText;
      return text.includes("Journal 已恢复") &&
        [...document.querySelectorAll(".task-panel")]
          .some((item) => item.textContent.includes("撤销任务") && item.textContent.includes("已完成"));
    })()`
  );
}

function quarantineCount(storageText) {
  const entries = fs.readdirSync(storageText);
  return entries.length;
}

async function readStoragePaths() {
  const paths = await execute(
    `return [...document.querySelectorAll(".storage-path")].map((item) => item.textContent.trim()).filter(Boolean)`
  );
  if (!path.isAbsolute(paths[0] ?? "")) fail("could not read quarantine path from the UI");
  return paths;
}

async function applyUndoScenario() {
  const root = makeDuplicates(2);
  const storage = await readStoragePaths();
  const before = quarantineCount(storage[0]);
  await planFor(root, 1);
  await confirmAndApply();
  await waitApplyCompleted();
  if (fs.readdirSync(root).length !== 1) fail("apply did not quarantine exactly one file");
  if (quarantineCount(storage[0]) !== before + 1) fail("quarantine count did not increase by one");
  if (!(await clickByText("撤销本次计划"))) fail("undo button is missing");
  await waitUndoCompleted();
  if (fs.readdirSync(root).length !== 2) fail("undo did not restore both files");
  if (quarantineCount(storage[0]) !== before) fail("quarantine count did not return to baseline");
  if (!(await clickByText("演练清理"))) fail("cleanup dry-run button is missing");
  await waitFor("cleanup dry-run", 'document.body.innerText.includes("清理演练完成")');
  return storage[0];
}

async function cancelScenario() {
  const count = 150;
  const root = makeDuplicates(count);
  await planFor(root, count - 1);
  const storage = await readStoragePaths();
  const before = quarantineCount(storage[0]);
  await confirmAndApply();
  await waitFor(
    "running apply task",
    `[...document.querySelectorAll(".task-panel")]
      .some((item) => item.textContent.includes("执行任务") &&
        (item.textContent.includes("执行中") || item.textContent.includes("排队中")))`
  );
  if (!(await clickByText("取消任务"))) fail("cancel button is missing");
  await waitFor(
    "cancelled task",
    `[...document.querySelectorAll(".task-panel")]
      .some((item) => item.textContent.includes("执行任务") && item.textContent.includes("已取消"))`
  );
  if (fs.readdirSync(root).length !== count) {
    fail("cancel did not restore every source file");
  }
  if (quarantineCount(storage[0]) !== before) {
    fail("cancel left quarantine files behind");
  }
}

async function revalidationScenario() {
  const root = makeDuplicates(2);
  await planFor(root, 1);
  const planned = await execute(
    'return document.querySelector(".row:not(.row-head) .path").textContent.trim()'
  );
  fs.writeFileSync(planned, "changed-after-preview\n");
  const storage = await readStoragePaths();
  const before = quarantineCount(storage[0]);
  await confirmAndApply();
  await waitFor(
    "apply skips changed content",
    `document.body.innerText.includes("E2010")`
  );
  if (fs.readdirSync(root).length !== 2) {
    fail("revalidation did not leave both files in place");
  }
  if (fs.readFileSync(planned, "utf8") !== "changed-after-preview\n") {
    fail("revalidation unexpectedly modified the changed source");
  }
  if (quarantineCount(storage[0]) !== before) {
    fail("revalidation unexpectedly wrote a quarantine file");
  }
}

async function main() {
  if (!fs.existsSync(appBinary)) fail(`desktop binary not found: ${appBinary}`);
  const health = await fetch(`${driverUrl}/status`).then((response) => response.json());
  if (!health?.value?.ready) fail("tauri-driver is not ready");
  const page = await fetch(devUrl).catch(() => null);
  if (!page?.ok) fail(`Vite dev server is not reachable at ${devUrl}`);

  const session = await request("POST", "/session", {
    capabilities: {
      alwaysMatch: {
        "tauri:options": { application: appBinary }
      }
    }
  });
  sessionId = session.sessionId;
  await waitFor("BatchDesk shell", 'document.title.includes("BatchDesk")');
  await waitFor(
    "storage info",
    'document.querySelectorAll(".storage-path").length > 0 && !document.querySelector(".storage-path").textContent.includes("读取中")'
  );

  await applyUndoScenario();
  console.log("PASS apply -> undo (WebView, real filesystem)");
  await cancelScenario();
  console.log("PASS cancel mid-apply and rollback (WebView, real filesystem)");
  await revalidationScenario();
  console.log("PASS plan-to-apply content revalidation (WebView, real filesystem)");

  fs.mkdirSync(path.dirname(screenshotPath), { recursive: true });
  const screenshot = await request("GET", `${sessionPath()}/screenshot`);
  fs.writeFileSync(screenshotPath, Buffer.from(screenshot, "base64"));
  console.log(`screenshot: ${screenshotPath}`);
}

main()
  .catch((error) => {
    console.error(`FAIL ${error.stack ?? error.message}`);
    process.exitCode = 1;
  })
  .finally(async () => {
    if (sessionId) {
      await request("DELETE", `${sessionPath()}`).catch(() => undefined);
    }
    if (!keep) {
      for (const root of tempRoots) fs.rmSync(root, { recursive: true, force: true });
    }
  });
