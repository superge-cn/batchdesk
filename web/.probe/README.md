# 无 WASM 环境下的界面行为探针（临时通道）

**这不是测试套件，也不能替代 `pnpm test`。** 权威仍是 Vitest（CI 的 web job）与 Playwright；
这里只在**宿主无法分配 WASM**、导致 Vite/Vitest 根本起不来时，用 esbuild（原生二进制）＋ jsdom
把同一批**用户可见行为**真跑一遍，拿到可复核的本地证据。

## 什么时候用

先做一次 10 秒判定：

```bash
node -e "try{new WebAssembly.Memory({initial:1});console.log('WASM_OK')}catch(e){console.log('WASM_FAIL')}"
```

- `WASM_OK` → 直接 `pnpm test`，不要用本目录。
- `WASM_FAIL` → `vitest` / `vite build` / `playwright` 都会死于
  `RangeError: WebAssembly.instantiate(): Out of memory`（V8 为 WASM 预留保护段被内核拒绝，
  常见于宿主内存严重超卖：`Committed_AS` ≫ `CommitLimit`）。此时本目录可用。

## 怎么跑

```bash
cd web/.probe
node build.mjs                                                   # esbuild 打包两个入口（原生二进制，不碰 WASM）
node --import ./globals.mjs entry-settings.cjs                   # 设置页「重置界面状态」7 项
node --import ./globals.mjs entry-app.cjs                        # App 的上次目录/上次页面 11 项
node --import ./globals.mjs entry-cancel.cjs                     # 取消后的撤销询问 12 项
node --import ./globals.mjs entry-errors.cjs                     # 错误文案分层 24 项
node --import ./globals.mjs entry-nav.cjs                        # 23 视图导航可达 16 项（约 30–60 秒，逐视图打印进度）
node --import ./globals.mjs entry-warnings.cjs                   # 功能页集中报告 7 项
```

`--import ./globals.mjs` 必须在入口之前跑：它先建 jsdom，再把 `window`/`document`/`navigator`
挂到 `globalThis`，否则 React 19 会认为自己在服务端。

## 覆盖了什么（对应哪条 committed 断言）

| 探针 | 对应 `web/src` 里的测试 | 断言 |
|---|---|---|
| `entry-settings.cjs` 7 项 | `SettingsPage.test.tsx`「本机界面状态」2 项 | 区块渲染；点击调用回调一次并给出提示；**`setConfig`/`resetScope` 零调用**；不传回调时按钮 disabled |
| `entry-app.cjs` 11 项 | `App.uiState.test.tsx` 5 项 | 启动预填上次目录＋候选；生成预览后回写（去重置顶、`lastSection` 不被擦）；无记录不预填不渲染候选；启动回到上次功能页；切页写回 `lastSection` |
| `entry-cancel.cjs` 12 项 | `App.cancel-review.test.tsx` 5 项 | 取消且有已完成项→询问（数量/已落盘文案/初始焦点在「保留」）；确认→`undoPlan(planId)` 且关框；保留→不撤销；零完成/undo 任务/正常完成三个对照都不打扰 |
| `entry-warnings.cjs` 7 项 | `FeaturePages.warnings.test.tsx` 3 项 | F11 集中报告逐项点名：类别走界面文案（不露内核 slug）＋`caution` 风险级＋建议行保留内核原文；未登记类别显示成缺陷（`未登记的预警类别（…）`）；对照——无 warnings 时不得凭空出现 `.warnings` 块 |
| `entry-nav.cjs` 16 项 | `App.navigation.test.tsx` 6 项 | **23 个功能视图逐个可达**（标题＋生成计划预览＋撤销，防"import 了 FeaturePages 却不渲染"的历史缺陷回归）；默认 C09 且可经「返回 C09」返回；任务与撤销真视图（标题＋空队列指引）；设置五分组＋界面语言 zh-CN；处理模板真渲染＋列表非空＋OQ-36 写明＋不给假「应用」按钮 |
| `entry-errors.cjs` 24 项 | `errors.test.tsx` 7 项 | 已知码位走本地化摘要＋动作（后端原文不当主行）；英文切换；未知码位回退并标 `localized=false`；**码表全量 55**、七分段非空、两语言两段齐全无占位符；横幅语义（alert/码位/摘要/动作/关闭）与未知码位不出现 raw key |

## 它是怎么搭起来的（三个坑）

1. **`./lib/ipc` 必须被替换**：`build.mjs` 用一个 esbuild `onResolve` 插件把该导入重定向到
   `fake-ipc.ts`；替身按 App/FeaturePages/SettingsPage 的真实导入面实现（缺一个导出就打包失败，
   这本身是一道检查）。
2. **产物必须是 CJS**：`lucide-react` 是 CJS 包，塞进 ESM bundle 会触发
   `Dynamic require of "react" is not supported`；改成 `format: "cjs"` + `packages: "external"`
   让 Node 自己解析即可。入口因此不能用顶层 await（`main()` + `void main()`）。
3. **`globalThis.navigator` 只有 getter**（Node 22），必须 `Object.defineProperty`。

## 共享模块\n\n- `settle.ts`：轮询等待，**返回谓词值本身**（entry-nav 初版在这里 `return true` 过，调用点拿到 boolean，错得莫名其妙）。新增探针一律 `import { settle } from "./settle"`。\n\n## 边界（不要当成正式测试）

- 断言是**手写**的，与 committed 测试**同义但不同源**——改了 `src` 的行为要两边一起改，
  否则会出现"探针绿、CI 红"或反之。
- 不覆盖：`vi.mock` 的模块语义、snapshot、fake timers、coverage；DOM 断言用 `querySelector`
  而非 RTL 的可访问性查询（少一层 a11y 校验）。
- 探针进程退出码即结论（0＝全过），可直接接进临时脚本；但**不要**把它加进 CI——
  CI 里 Vitest 能跑，别用弱证据替换强证据。
