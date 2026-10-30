# batchdesk

面向非技术办公人员的本地批量文件处理工作台——**先预览、再执行、可回滚、有日志**。
（产品定位见基线《办公辅助工具产品方案》§1.1；本仓库文档不复述其内容。）

## 0. 命名与远程仓库

仓库名 **batchdesk** = batch（批量）+ desk（工作台），对应产品定位中的"工作台"。

- 全小写、无连字符，可直接用作 crate 名 / npm 包名 / 可执行文件名；
- **远程仓库**：`https://github.com/superge-cn/batchdesk`；
- **仍待核实**：crates.io / npm 的同名可用性，以及商标检索——发布到这两处之前需各自确认；
- 备选（未采用）：`drybatch`——直指"干跑优先"的核心差异（方案 §7.1），但 dry 在英文语境有"枯燥"歧义；`batchguard`——强调安全护栏（预览/撤销/隔离目录），但 guard 偏安全软件气质。

---

## 1. 基线文档地图

四份基线是设计权威。仓库文档只引用其**章节号**，不复制原文（数值单源纪律延伸到仓库文档）。

| 文档 | 版本 | 回答什么问题 | 何时读 | 变更纪律 |
|---|---|---|---|---|
| 《办公辅助工具产品方案》 | v1.19 | 产品怎么做：功能定义与优先级、授权分组、阈值唯一来源、三条交付线 | **任何改动之前** | 产品行为改动走方案 §11.6 书面确认流程；阈值只认 §9.1 与 §13.10.2 |
| 《功能测试方案与验收标准》 | T v1.12 | 怎么验：用例、异常语料、四级验收、缺陷分级 | 提测前、验收前 | 随方案联动修订；用例预期必须可追溯章节号 |
| 《UI 视觉规范》 | U v1.3 | 界面长什么样：token 三层架构、组件状态、对比度硬约束 | 写任何界面前 | 视觉改动回填 U 并升版；交互问题回方案 |
| 《原型走查包》 | 独立附件 | 走查怎么跑：线框规格、走查脚本、视觉基调盲测三问 | 走查前 | 随 U 联动修订 |

> 基线正文落位于 `docs/baseline/`（见该目录 README 的落位说明）。
> 全量文档索引见 [`docs/README.md`](./docs/README.md)；设计契约与测试执行文档分别见 [`docs/design/README.md`](./docs/design/README.md)、[`docs/testing/README.md`](./docs/testing/README.md)。
> 实施顺序与完成度台账见 [`docs/DEVELOPMENT-PLAN.md`](./docs/DEVELOPMENT-PLAN.md)。

---

## 2. 仓库结构

按 **多 crate workspace** 组织（ADR-0001，动机之一是支撑方案 §13.5 的 WASM 复用）：

```
batchdesk/
├── crates/
│   ├── kernel/        ① 核心引擎（可编译为 WASM 供 Web 版复用，见 §13.5）
│   ├── platform/      ② 共享壳层适配（扫描/原生 FsAdapter/Journal/保留期）
│   └── cli/           ③ 命令行入口（方案 §9.5 的 C40，P1 预留）
├── src-tauri/         ④ 桌面壳（Tauri；C09 IPC 切片，独立 workspace）
├── web/               ⑤ Web 前端（React/Vite；C09 UI 切片，U v1.3 token）
├── fixtures/          ⑤ 语料、基准数据集与配置迁移样本（T §3.2、附录 C）
├── docs/
│   ├── baseline/      ⑥ 四份基线文档 + MANIFEST（版本/哈希）
│   ├── adr/           ⑦ 工程决策记录（见其 README 的判据）
│   ├── design/        ⑧ 设计契约 01–13（见其 README）
│   ├── testing/       ⑨ 用例库/Web 分册/验收表单
│   ├── porting/       移植 triage/PDR/来源台账
│   └── OPEN-QUESTIONS.md ⑩ 待决事项登记
├── scripts/           ⑪ 文档/契约门禁脚本（check_docs.py）
├── .github/workflows/ ⑫ CI（六 job：lint / deny / test / web / desktop / bench）
├── rust-toolchain.toml ⑬ 工具链统一（stable + rustfmt/clippy）
├── deny.toml          ⑭ 依赖许可证/来源策略（OQ-13）
├── README.md  CONTRIBUTING.md  CHANGELOG.md  AGENTS.md  LICENSE
├── THIRD_PARTY_NOTICES.md  .gitignore
```

> `src-tauri/` 与 `web/` 已建立 C09 垂直切片；`src-tauri` 是独立 Cargo workspace（不随根 `cargo test --workspace` 编译），需单独 `cargo check --manifest-path src-tauri/Cargo.toml`。目标架构见 [`docs/design/12-前端与桌面壳架构.md`](./docs/design/12-前端与桌面壳架构.md)。

**分层纪律**：`kernel` 不依赖 `src-tauri`／`web`（单向依赖）；引擎层只面向 `kernel` 内部的适配层接口编程（对应方案 §4.2 的四层单向依赖）。

---

## 3. 快速开始

**环境要求**：Rust toolchain（stable；本地使用 `rust-toolchain.toml` 统一 rustfmt/clippy，CI 使用 stable，升级时同步）、Node.js（LTS）＋ **pnpm**（前端包管理，Cargo 管 Rust 侧）、Tauri CLI、Python 3（仅用于生成 fixtures，见 D-R3）。MVP 平台范围为 Windows 10/11（方案 11.6-4 已确认，macOS/Linux 顺延）。

```bash
# 构建
cargo build --workspace

# 语料准备（生成样本 + 范围核对；当前 cargo test 不跑语料断言）
python fixtures/corpus/check_scope.py
for s in fixtures/corpus/*/gen_*.py; do [ -e "$s" ] && python "$s"; done

# 测试（当前为单元测试；语料回归断言尚未实现，见 CONTRIBUTING §2）
cargo test --workspace

# C09 只读检测切片（大小 → 快速哈希 → 全量 BLAKE3；不删除）
# 快速窗口必须显式给出，无产品默认值；--mode fast 只输出候选组
cargo run -p batchdesk-cli -- c09 <目录> --quick-window-bytes <N> --mode full
cargo run -p batchdesk-cli -- c09 <目录> --quick-window-bytes <N> --mode fast --json

# C09 计划预览（只生成 PlanDocument，不落盘；--keep 缺省 earliest）
# keep 可选 earliest | shortest-path | newest；--plan-id 缺省由 CLI 生成 ULID
cargo run -p batchdesk-cli -- c09 <目录> --quick-window-bytes <N> --plan --json > plan.json
cargo run -p batchdesk-cli -- c09 <目录> --quick-window-bytes <N> --plan --keep shortest-path --json

# S02 保护目录：计划期标 blocked E2009，apply 期再复核（可重复传）
cargo run -p batchdesk-cli -- c09 <目录> --quick-window-bytes <N> --plan --protected-dir <保护目录> --json

# C09 执行（只移入隔离目录，不永久删除；Journal 用于撤销）
# 隔离目录与 Journal 路径必须显式给出，当前 CLI 不设产品默认路径
cargo run -p batchdesk-cli -- apply --plan plan.json --quarantine-dir <隔离目录> --journal <journal.jsonl> --json
# 撤销：按 planId 逆序执行 Journal reverse；超过保留期会拒绝
cargo run -p batchdesk-cli -- undo --plan <planId> --journal <journal.jsonl> --retention-days 7 --json
# 保留期清理：默认 dry-run；--apply 才永久删除过期隔离文件与 Journal 条目
cargo run -p batchdesk-cli -- cleanup --journal <journal.jsonl> --quarantine-dir <隔离目录> --retention-days 7 --apply --json

# C-6 只读与计划预览断言（Unix 主机；本地手动，CI 不生成 C-6）
python fixtures/bench/C-6-重复文件集/assert_c09.py --generate

# C09 桌面 UI（Vite 浏览器预览；Tauri 壳需系统 WebView 依赖）
(cd web && pnpm install)
(cd web && pnpm dev)     # http://127.0.0.1:1420
(cd web && pnpm build)   # 类型检查 + 生产构建
pnpm install             # 根目录安装 Tauri CLI
pnpm tauri dev           # 启动 Tauri 桌面壳（需 WebKitGTK/WebView2）

# 桌面任务队列测试（独立 workspace）
cargo test --manifest-path src-tauri/Cargo.toml --offline
# 真实 WebView E2E（需先启动 Vite 与 tauri-driver；前置条件见脚本文件头）
node scripts/e2e-tauri.mjs

# 文档与契约检查（CI 必跑）
python scripts/check_docs.py
python fixtures/migrations/check_schema.py
python fixtures/corpus/verify_corpus.py --hashes

# 冒烟（对应 T v1.12 §6.2 的 S-A/S-B/S-C，30 分钟内完成）
# 说明：冒烟是本地或打包后的人工动作，不在 CI 内（见 .github/workflows/ci.yml 文件头）
```

> `web/` 与 `src-tauri/` 已建立 C09 垂直切片；`src-tauri` 是独立 Cargo workspace，不随根 `cargo test --workspace` 编译，需单独 `cargo check --manifest-path src-tauri/Cargo.toml`（依赖系统 WebView 库）。
> C09 的 CLI `apply/undo/cleanup` 为同步命令：隔离目录与 Journal 路径必须显式给出；桌面壳已接入任务队列与 `task://*` 事件（`applyPlan`/`undoPlan` 返回 taskId，支持进度/暂停/取消），真实 WebView E2E 覆盖 apply→undo、cleanup 演练与取消回滚；S02 用 `--protected-dir`，保留期默认 7 天/可配 1–30（基线 §7.8）。`retryTask` 与 S03 任务快照持久化/恢复（`recoverTask`）已接入。

---

## 4. 开发约定摘要

1. **门禁零容忍**：`clippy -D warnings` 从第一行代码起生效，`rustfmt --check` 必须通过。
2. **许可证与漏洞**：新增依赖须过 `cargo-deny`（许可证 + 漏洞）；移植代码须在文件头登记来源，并在 `THIRD_PARTY_NOTICES.md`/台账留档（依赖仍进 SBOM；方案 §4.11.4）。项目源代码为**专有 / All rights reserved**（见 `LICENSE`；OQ-11 已确认 2026-10-28）；依赖白名单见 `deny.toml`（OQ-13）。
3. **语料准备不豁免**：移植改造的代码与其他代码同等，必须跑 `fixtures/` 全量生成与范围核对（方案 §4.11.2a④）；逐条断言 harness 落地后再按全量回归执行。
4. **错误码统一**：一律走方案 §4.6 的 `E1xxx–E9xxx` 分段，自带平行错误处理视为评审打回。
5. **变更路由**：产品行为 → 方案 §11.6；视觉 → U；工程实现细节 → 写 ADR。

> 全文见 [CONTRIBUTING.md](./CONTRIBUTING.md)。

---

## 5. 版本与通道

- **单机版走 1.x 序列**，内部构建号区分通道：**B（闭门测试）/ C（公发）**；
- 商业版从 **2.0** 起，不延续单机版序列（承接方案 §12.5 的版本号策略）；
- 配置、日志、备份与撤销数据目录按通道隔离，允许同机并存，便于把单机版真实数据带到商业版做迁移验证。

版本变更记录见 [CHANGELOG.md](./CHANGELOG.md)。