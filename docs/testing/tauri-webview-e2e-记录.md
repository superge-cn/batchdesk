# Tauri WebView 端到端测试记录（Windows 本机）

> 状态：实机执行记录 ｜ 日期：2026-11-24 ｜ 上游：T v1.13、产品方案 v1.24 ｜ 性质：C09 任务队列真 WebView e2e 的执行记录，不承载基线判定文本

本文是 `scripts/e2e-tauri.mjs` 在 Windows 实机上的首跑记录，用于结清「Tauri WebView e2e 本机不可跑」这条验证欠账。
记录口径：门禁结论一律以进程退出码为准；凡本机未实测到的，明确写「未验证」。

## 结论摘要

- 真跑，不是模拟：`tauri-driver` + `msedgedriver` 驱动真实 Tauri WebView（WebView2 运行时 157.0.4341.60），真实文件系统读写。
- 首跑整体结论：**未通过**（`EXIT=1`）。根因是一个**产品缺陷**，不是环境问题：取消任务后「是否一并撤销」对话框在真实运行中永不出现。
- 用例 1（执行 → 撤销）**通过**；用例 2（执行中取消 → 一并撤销）**失败**；用例 3（计划到执行的内容复检）**单独验证通过**，但在正式脚本顺序中被用例 2 阻断，未跑到。
- **构建已封装**：`node scripts/build-webview-e2e.mjs` 一条命令完成「清桌面壳缓存 → 注入 `TAURI_CONFIG`（调试端口）→ `cargo build`」。它同时把应用数据 identifier 改为 `com.batchdesk.e2e`，e2e 不再污染真实应用数据；但 **WebView2 浏览器 profile 仍未隔离**（见 §七）。
- **修复后复跑（2026-11-24 21:28，封装后又复跑两次）：整体通过（`EXIT=0`）**，三条用例全部 `PASS`。缺陷根因是 `src-tauri/src/tasks.rs::finish()` 把功能码（`c09`）塞进 `task://done` 的 `kind`，而契约（设计 01 §4.3，已打开原文核对）要求该字段是 `apply`／`undo`；修复见 main `a5409be`，Rust 侧先红实测 `left: ["c09","c09"] / right: ["apply","undo"]`。复跑前用 `TAURI_CONFIG` 注入重新构建了带调试端口的桌面二进制，`tauri-driver` 与 Vite 均返回 200。
- 环境侧欠账已补齐：msedgedriver 已安装到仓库外 `E:/tools/msedgedriver/157/`，版本与 WebView2 运行时一致。

## 一、环境

| 项目 | 值 |
| --- | --- |
| 操作系统 | Windows 11 Pro（Windows NT 10.0.26200） |
| Shell | Git Bash（POSIX sh），非 cmd/PowerShell |
| Node | v22.22.2 |
| 仓库工作区 | `E:/tmp/wt-e2e`（分支 `feat/tauri-webview-e2e`） |
| 基线 commit | `65f7b01`（2026-11-24 19:44 +0800），另含本文同批的 `scripts/e2e-tauri.mjs` 改动 |
| 桌面二进制 | `src-tauri/target/debug/batchdesk-desktop.exe`，构建于 2026-11-24 20:54:43 |
| Edge 浏览器 | 158.0.4370.33 |
| WebView2 运行时（应用实际使用） | 157.0.4341.60（`C:/Program Files (x86)/Microsoft/EdgeWebView/Application/157.0.4341.60`） |
| msedgedriver | 157.0.4341.60（`fbcb2989a6c61b72a4919fe1028e58e70131cd46`） |
| tauri-driver | 2.1.0 |

浏览器是 158、WebView2 运行时是 157：驱动必须匹配**运行时**而不是浏览器，所以选 157.0.4341.60。

## 二、前置准备

### 2.1 msedgedriver

下载源实测：

- `https://msedgedriver.azureedge.net/157.0.4341.60/edgedriver_win64.zip` → HTTP 404（返回的是 HTML 404 页）。
- `https://msedgedriver.azureedge.net/LATEST_RELEASE_157_WINDOWS` → HTTP 404，`LATEST_STABLE` 连接被重置。
- `https://msedgedriver.microsoft.com/157.0.4341.60/edgedriver_win64.zip` → HTTP 200，压缩包校验通过。

因此采用 `msedgedriver.microsoft.com` 直连版本号路径。解压到仓库外（未提交）：

```text
E:/tools/msedgedriver/157/msedgedriver.exe   # 实际使用
E:/tools/msedgedriver/158/msedgedriver.exe   # 预留（Edge 158 浏览器场景），未使用
```

### 2.2 桌面二进制必须带调试端口构建（关键环境适配）

直接 `cargo build` 出来的二进制**无法被 WebDriver 附加**：`msedgedriver` 通过环境变量
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 注入 `--remote-debugging-port=0`，而 wry 0.57.1
在 `create_environment` 里无条件 `set_additional_browser_arguments(...)`（默认
`--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`），WebView2 以该属性为准，
环境变量被覆盖，于是 `DevToolsActivePort` 文件永远不出现，报
`session not created: DevToolsActivePort file doesn't exist`。

实测核对（因果链完整）：

1. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9999"` 直接启动应用 → `9999` 无监听，证明环境变量被覆盖。
2. 用 `TAURI_CONFIG` 在构建期把端口写进 `app.windows[0].additionalBrowserArgs` → 应用启动后
   `%LOCALAPPDATA%/com.batchdesk.desktop/EBWebView/DevToolsActivePort` 出现，`/json/version` 返回
   `Edg/157.0.4341.60`。

采用构建期注入（不改 `src-tauri/tauri.conf.json`，避免把调试端口带进发布配置）：

```bash
export TAURI_CONFIG='{"app":{"windows":[{"label":"main","title":"BatchDesk","width":1280,"height":820,"minWidth":1024,"minHeight":680,"resizable":true,"additionalBrowserArgs":"--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port=0"}]}}'
cargo build --manifest-path src-tauri/Cargo.toml
```

副作用：这份二进制是「测试构建」，每次跑 e2e 前都要用上面的命令重建；普通 `cargo build` 的产物不能被 WebDriver 附加。

注意：本记录走 **Vite dev + Tauri IPC** 通道，不加载 `web/dist`，因此**不需要** `web/wasm` 产物（`web/src/lib/wasm/load.ts:51` 是动态导入，`worker.ts` 只在 WASM 通道启用时才被加载；本机 `web/wasm/ENABLED` 不存在，前端自动走 IPC）。
反之，凡走 `pnpm build` / `pnpm tauri build` 的发布构建，前置必须先跑 `node scripts/build-wasm.mjs`（据 release-track 实机验证：缺产物直接 `TS2307`；本记录未复现该失败，仅作前置条件提示）。

### 2.3 起服务

```bash
# 1) Vite（dev 构建的桌面壳加载 devUrl http://localhost:1420）
cd web && ./node_modules/.bin/vite --host 127.0.0.1 --port 1420 --strictPort
# 2) WebDriver 代理
tauri-driver --port 4444 --native-driver "E:/tools/msedgedriver/157/msedgedriver.exe"
```

## 三、执行命令与退出码

| 步骤 | 命令 | 退出码 | 说明 |
| --- | --- | --- | --- |
| 构建（首版） | `cargo build --manifest-path src-tauri/Cargo.toml` | 0 | 增量 54.8s，产物 20:45 |
| 构建（带调试端口） | `TAURI_CONFIG=... cargo build --manifest-path src-tauri/Cargo.toml` | 0 | 增量 17.7s，产物 20:54 |
| e2e 首跑 | `node scripts/e2e-tauri.mjs --app src-tauri/target/debug/batchdesk-desktop.exe` | 1 | `DevToolsActivePort file doesn't exist`（环境侧，已解决） |
| e2e 正式跑（脚本适配后） | `node scripts/e2e-tauri.mjs` | 1 | 用例 1 通过，用例 2 卡在取消复核对话框 |
| 用例 3 单独验证 | 临时副本跳过 `cancelScenario()` | 0 | 用例 1、3 通过（该副本未提交） |

## 四、用例结果

| 用例 | 断言要点 | 结果 | 证据 |
| --- | --- | --- | --- |
| 1. 执行 → 撤销 | 2 个重复文件中隔离 1 个；隔离目录 +1；撤销后源目录恢复 2 个、隔离目录回到基线；清理演练完成 | 通过 | 脚本打印 `PASS apply -> undo (WebView, real filesystem)` |
| 2. 执行中取消 → 一并撤销 | 取消后任务终态为「已取消」；出现「是否一并撤销」询问；确认后源目录恢复 150 个、隔离目录回基线 | **失败** | 取消落在 84/149，任务面板显示「执行任务 已取消 … 收尾 · 84/149」，结果表 84 条「成功」，但页面 30s 内始终没有「是否一并撤销」；见第五节根因 |
| 3. 计划到执行的内容复检 | 预览后改写源文件，执行时报 `E2010`，两个文件都留在原地、源文件内容未被改动、隔离目录不增 | 通过（单独验证） | 临时跳过用例 2 的副本 `EXIT=0` 并打印该条 PASS；正式顺序中未跑到 |

截图（用例 3 那轮产出）：`target/e2e/tauri-task-queue.png`。

## 五、失败归类

### 5.1 产品缺陷（未修复，交回主线程裁决）

**现象**：C09 执行任务中途取消、且已有条目落盘后，界面不弹「已完成 N 项，是否一并撤销？」。

**根因**：`task://done` 事件里作为判别字段的 `kind` 下发的是**功能码**，前端却按**任务类别**判定。

- 后端 `src-tauri/src/tasks.rs:1181`：`json!({ "taskId": task_id, "kind": kind, "summary": report })`，
  此处 `kind` 来自 `record.summary.kind`（`tasks.rs:1165`），即功能码 `c09`／`f12` 一类。
- `TaskSummary` 的字段注释本身写明「被处理计划的功能码（`f12`／`c09`…），**不承担执行/回滚判别**」
  （`src-tauri/src/tasks.rs:74-76`），真正的任务类别在同结构体的 `job` 字段（`tasks.rs:76-79`）。
- 前端 `web/src/App.tsx:588`：`if (event.kind === "apply" && detail.summary.state === "cancelled" && completed > 0)`，
  而 `web/src/lib/ipc/types.ts:180-184` 把 `TaskDoneEvent.kind` 声明为 `"apply" | "undo"`。
- 运行时实测：用 `__TAURI_INTERNALS__` 订阅 `task://done` 抓到的负载是 `{"kind":"c09"}` —— 条件恒为假，对话框是死代码。

**为什么现有测试没拦住**：`web/src/App.cancel-review.test.tsx` 把 `kind` mock 成 `"apply"`，
断言与实现互相印证了错误契约；Rust 侧 `tasks.rs` 的 done 事件测试只断言事件存在（`tasks.rs:3074`），不断言 `kind` 取值。

**影响**：本用例断言失败；真实用户取消大任务后不会被询问是否回滚，只能自己到任务面板再点撤销。是否修、怎么修由主线程决定，本次未改动产品代码。

### 5.2 环境问题（已在本机解决）

- 缺 msedgedriver → 已装 157.0.4341.60 到仓库外。
- `DevToolsActivePort` 缺失 → 用 `TAURI_CONFIG` 构建期注入调试端口。
- 旧二进制（2026-11-14）→ 已重建。

### 5.3 测试脚本与界面漂移（已在脚本内适配，未放宽断言）

三处适配都是「界面对不上旧选择器 / 时序没等稳」，断言强度不降：

1. **预览条数**：预览表已改成窗口化列表（`web/src/VirtualList.tsx`，注释称方案 6.2，原文未核对），DOM 只挂可视行（149 条时只有 15 个 `.row`），旧断言数 `.row` 永远凑不够。改为读列表容器的 `aria-rowcount`，仍要求精确等于期望条数。
2. **结果条数**：旧断言找 `.result-row`，该 class 只存在于模板页；C09 结果表实际是虚拟列表里的 `.row`。改为读 `section[aria-labelledby="result-title"]` 内列表的 `aria-rowcount > 0`。
3. **计划收尾等待**：行数到位时扫描按钮仍可能是「正在扫描与分组…」，`busy` 未清零导致确认框与执行按钮 `disabled`，点击是空操作。新增等待「`#confirm` 可用」。
4. **取消时机**：149 条小文件约 0.8s 跑完，取消若早于任何条目落盘就不会弹询问框（询问只对已完成条目成立）。把「执行中」的等待条件收紧为 `done > 0` 后再取消。

另：Windows 默认产物带 `.exe`，脚本缺省 `--app` 值按平台补后缀。

## 六、本次改动文件

| 文件 | 性质 | 说明 |
| --- | --- | --- |
| `scripts/e2e-tauri.mjs` | 修改 | 上述 4 处界面漂移/时序适配 + Windows 默认路径；断言未放宽 |
| `docs/testing/tauri-webview-e2e-记录.md` | 新建 | 本文 |

未改动任何产品代码、`src-tauri/tauri.conf.json`、`web/src` 下文件，也未改动主线程统一维护的共享文件。

## 七、剩余缺口

1. ~~产品缺陷未修~~（**已关闭**）：`task://done.kind` 已改发任务类别（`a5409be`），复跑用例 2 通过。
2. ~~测试构建要求~~（**已封装**）：`node scripts/build-webview-e2e.mjs` 一条命令搞定，已验证构建后 e2e 仍 3/3 PASS。CI 的 ubuntu runner 没有 Edge/WebView2，不做 CI job。
3. ~~用例 3 未在正式顺序跑到~~（**已关闭**）：复跑时三条用例按脚本顺序全部执行并通过。
4. **WebView2 浏览器 profile 仍未隔离**：2026-11-24 实测三条通道都无效——① `additionalBrowserArgs` 加 `--user-data-dir=<temp>`；② 构建期改 identifier（只隔离了应用数据，`com.batchdesk.e2e` 目录被创建但无 `EBWebView`）；③ driver 进程注入 `WEBVIEW2_USER_DATA_FOLDER`。e2e 仍写真实 `%LOCALAPPDATA%/com.batchdesk.desktop/EBWebView`。根因是 wry 建 WebView2 环境时显式传了 userDataFolder，覆盖上述通道；要彻底隔离需改产品代码或用独立 Windows 用户跑 e2e。
5. **可移植性**：本记录只覆盖 Windows + WebView2；Linux（WebKitWebDriver）未验证。CI 环境无 Edge/WebView2，e2e 仍只能在实机跑。
6. **引用核对**：文中「设计 01 §4.3」已在本次修复时打开原文核对（`task://done` 契约明确 `kind ∈ apply|undo`）；「方案 §7.7」「方案 6.2」仍未打开原文核对，按 AGENTS 第 1 条标注为**未核对，待人工确认**。
