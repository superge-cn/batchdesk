# 0060. 桌面壳 CSP 与 capabilities 的最小化取值

- **状态**：已采纳（2026-11-03，设计 12 §1「壳层加固」的 CSP/capabilities 落地，ADR-0002 待办②）
- **日期**：2026-11-03
- **决策者**：开发代理（安全配置取值与断言形态的工程决策；产品行为、错误码、窗口尺寸均不变）
- **关联**：设计 12 §1（壳层加固）／§7 待决项（本文关闭其中一条）、ADR-0002（Tauri 选型的待办②）、ADR-0004（命令只经生成绑定，故禁 `withGlobalTauri`）；`src-tauri/tauri.conf.json`、`src-tauri/capabilities/default.json`、`src-tauri/tests/security_config.rs`

---

## 1. 问题

设计 12 §1 写着「capabilities 最小权限、CSP、生产构建关 devtools、不开 `withGlobalTauri`、不装用不到的插件」，但实际配置是：

- `app.security.csp = null`——**等于没有 CSP**，页面可以加载任意来源的脚本与样式；
- `capabilities/default.json` 只有 `core:default`（这一点是对的），但没有任何东西阻止它被顺手扩张；
- `withGlobalTauri` 未显式声明，`freezePrototype` 未开。

这三个都是"文档写了、机器不检查"的项。CSP 尤其两面性：写松了等于没防护；写紧了直接白屏，而仓库现有的真机验证（`scripts/e2e-tauri.mjs`）是**手动**的，CI 的 desktop job 只跑 `cargo check`＋单测，抓不到白屏。所以取值要保守、且必须有静态断言兜住"配置被改松"。

## 2. 候选与结论

| 议题 | 候选 | 结论 |
|---|---|---|
| 何时上 CSP | a) 留到发布前统一做；b) 现在就上 | **b**。现在前端没有任何外部资源、没有内联脚本（`web/index.html` 只有一条 `<script type="module" src>`，Vite 生产构建把脚本与样式都出成外部文件），是加 CSP 成本最低的时刻；越晚加，越容易某次"顺手引一个远端字体/内联脚本"后回不去 |
| 指令集 | a) 抄文档示例 `default-src 'self'`；b) 按本仓库实际资源逐项列 | **b**。显式写 `script-src`/`style-src`/`img-src`/`font-src`/`connect-src`，再补零成本的降面项 `object-src 'none'`、`base-uri 'self'`、`form-action 'none'`——逐项写出来才能在评审时被质疑，`default-src` 一把抓看不出取舍 |
| `script-src` | a) `'self' 'unsafe-inline'`（省事）；b) `'self'` | **b**。生产构建不产内联脚本；Tauri 自己注入的引导脚本由 Tauri 在响应 CSP 时加 nonce（`dangerousDisableAssetCspModification` 保持默认 false） |
| `style-src` | a) 严格 `'self'`；b) `'self' 'unsafe-inline'` | **b**，且**只此一条**允许 `'unsafe-inline'`。`VirtualList` 用 React 的 `style` 属性做窗口位移（`translateY`），CSP3 下 style 属性回落 `style-src`。把行内样式改成 class 需要重写窗口化实现，本批不做；宁可有这一条明确记录的例外，也不要为了"看起来严格"而让界面错位 |
| dev 与 prod | a) 一份 CSP 兼顾两边；b) 用 `devCsp` 分开 | **b**。Vite dev 注入内联的 react-refresh 预置脚本、HMR 走 `ws://localhost:1420`；"兼顾"等于生产也放开内联脚本 |
| IPC 白名单 | a) 只写 `ipc:`；b) `ipc:` ＋ `http://ipc.localhost` | **b**。Tauri v2 在 Linux/macOS 走自定义协议 `ipc:`，Windows 走 `http://ipc.localhost`；只写一个会在另一平台静默断连（表现为"命令全部超时"，很难查） |
| capabilities | a) 维持现状，靠评审纪律；b) 断言 `permissions == ["core:default"]` | **b**。能力集是"页面能调什么命令"的边界，扩张应当是一次显式决策（改本 ADR），不该是某次需求的顺手一行 |
| `withGlobalTauri` | a) 不写（默认 false）；b) 显式写 `false` 并断言 | **b**。"默认关闭"是实现细节；显式写出＋断言才挡得住后人为了调试打开它。且本仓库命令只经 `lib/ipc` 的生成封装（ADR-0004），本就不需要全局对象 |
| `freezePrototype` | a) 不动；b) 打开 | **b**。零成本，挡原型链投毒类脚本 |
| 断言形态 | a) 只写在设计文档里；b) 落成 `src-tauri/tests/security_config.rs` | **b**。这三个配置错误在 CI 里**没有任何症状**，只有真机白屏或安全评审才暴露；静态断言读 `include_str!` 的两个文件，不需要 WebView，跑在已有的 desktop job 里 |

## 3. 实现

**生产 CSP**（`tauri.conf.json` → `app.security.csp`）

```text
default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self' data:; font-src 'self' data:;
connect-src 'self' ipc: http://ipc.localhost;
object-src 'none'; base-uri 'self'; form-action 'none'
```

**开发 CSP**（`app.security.devCsp`）：同上，但 `script-src 'self' 'unsafe-inline'`、`connect-src` 追加 `ws://localhost:1420`。不加 `unsafe-eval`——Vite 7 dev 不需要。

**capabilities**（`capabilities/default.json`）：保持 `identifier: "default"`、`windows: ["main"]`、`permissions: ["core:default"]`。

**断言**（`src-tauri/tests/security_config.rs`，6 条）

1. 生产 CSP 非 null、`default-src`/`script-src` 恰为 `'self'`、含 `object-src`/`base-uri`/`form-action 'none'`、无 `unsafe-eval`、无通配符；
2. `style-src` 含 `'self'` 与 `'unsafe-inline'`，且**逐指令**校验其他指令都不得带 `unsafe-inline`；
3. `connect-src` 含 `ipc:` 与 `http://ipc.localhost`，且除这两者与 `'self'` 外无 http(s) 来源；
4. `devCsp` 与 `csp` 不相同、dev 的 `script-src` 才带 `unsafe-inline`、`connect-src` 含 `ws://localhost:1420`、dev 也不得有远端来源；
5. capabilities 的 `windows == ["main"]`、`permissions == ["core:default"]`，且逐项前缀必须是 `core:`；
6. `withGlobalTauri == false`、`freezePrototype == true`。

**先红实证**：第 5 条首次运行时真的红了——但它抓到的是**断言本身的缺陷**（扫描整份 JSON 文本，`description` 里的 "C09 desktop shell:" 被当成 `shell:` 插件权限），而不是配置问题。改为逐权限项判前缀后转绿。这条记录保留在本 ADR 里，因为它说明"扫文本"式断言会误报、也会漏报（换行/转义即可绕过）。

## 4. 已接受的坑与边界

- **未在真 WebView 里跑过**。本机缺 WebKitGTK 与 tauri-driver，`scripts/e2e-tauri.mjs` 是手动项。CSP 是否与 Tauri 的 nonce 注入、Vite 产物完全兼容，需要在桌面日（Windows 实机）各跑一次；这是本项的**残余风险**，已登记到开发计划 §6.1 的剩余缺口，不得写成"已验证"。
- **`style-src 'unsafe-inline'` 是真实弱点**：样式注入可被用来做点击劫持式的视觉欺骗。但 React 行内 `style` 是本仓库现有 UI 的用法（窗口化的 `translateY`、若干宽度百分比），改成 class 需要重写 `VirtualList`，超出本批范围。记在这里而不是假装没有。
- **断言是配置层的**：它保证"配置写对"，不保证"运行时无违规"。真机仍需看 WebView 控制台的 CSP violation（放桌面日）。
- **devtools 未单独配置**：Tauri v2 的生产构建是否含 devtools 由 Cargo feature 决定，本仓库 `tauri` 依赖未启 `devtools` feature，故不写配置项——这一点记在本节，避免下次有人以为"漏了"。
- **`connect-src` 里的 `'self'`** 是为将来 Web 形态（同源 HTTP）留的；桌面形态下它等价于自定义协议的同源，不构成额外放开。
- **未做**：发布流水线里的 CSP 断言（构建产物层面，例如检查 dist 里是否有内联脚本）——那属于发布契约（设计 11），本批只在配置层收口。

## 5. 验证

- `cargo test --manifest-path src-tauri/Cargo.toml`：壳层 **27 → 33 项**（新增 `security_config` 6 项），全绿；
- `cargo check --manifest-path src-tauri/Cargo.toml`：`tauri-build` 在 build.rs 里解析 `tauri.conf.json`，新键（`devCsp`/`freezePrototype`/`withGlobalTauri`）经 `tauri-utils 2.10.2` 的 `SecurityConfig`/`Config` 字段核对存在（源码 grep 实证），编译通过；
- CI 的 desktop job 已跑该套测试，配置被改松时该 job 先红。
