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
| 《办公辅助工具产品方案》 | v1.6 | 产品怎么做：功能定义与优先级、授权分组、阈值唯一来源、三条交付线 | **任何改动之前** | 产品行为改动走方案 §11.6 书面确认流程；阈值只认 §9.1 与 §13.10.2 |
| 《功能测试方案与验收标准》 | T v1.1 | 怎么验：用例、异常语料、四级验收、缺陷分级 | 提测前、验收前 | 随方案联动修订；用例预期必须可追溯章节号 |
| 《UI 视觉规范》 | U v1.1 | 界面长什么样：token 三层架构、组件状态、对比度硬约束 | 写任何界面前 | 视觉改动回填 U 并升版；交互问题回方案 |
| 《原型走查包》 | 独立附件 | 走查怎么跑：线框规格、走查脚本、视觉基调盲测三问 | 走查前 | 随 U 联动修订 |

> 基线正文落位于 `docs/baseline/`（见该目录 README 的落位说明）。

---

## 2. 仓库结构

按 **多 crate workspace** 组织（ADR-0001，动机之一是支撑方案 §13.5 的 WASM 复用）：

```
batchdesk/
├── crates/
│   ├── kernel/        ① 核心引擎（可编译为 WASM 供 Web 版复用，见 §13.5）
│   └── cli/           ② 命令行入口（方案 §9.5 的 C40，P1 预留）
├── src-tauri/         ③ 桌面壳（Tauri；文件系统适配、任务队列接线）
├── web/               ④ Web 前端（shadcn 体系，见 U §2.0）
├── fixtures/          ⑤ 语料与性能数据集（T §3.2、附录 C）
├── docs/
│   ├── baseline/      ⑥ 四份基线文档
│   └── adr/           ⑦ 工程决策记录（见其 README 的判据）
├── .github/workflows/ ⑧ CI（四 job：lint / deny / test / bench）
├── README.md  CONTRIBUTING.md  CHANGELOG.md  AGENTS.md  .gitignore
```

**分层纪律**：`kernel` 不依赖 `src-tauri`／`web`（单向依赖）；引擎层只面向 `kernel` 内部的适配层接口编程（对应方案 §4.2 的四层单向依赖）。

---

## 3. 快速开始

**环境要求**：Rust toolchain（stable）、Node.js（LTS）＋ **pnpm**（前端包管理，Cargo 管 Rust 侧）、Tauri CLI、Python 3（仅用于生成 fixtures，见 D-R3）。

```bash
# 构建
cargo build --workspace

# 语料准备（必须先跑：门禁核对 → 幂等生成样本；cargo test 本身不生成语料）
python fixtures/corpus/check_scope.py
for s in fixtures/corpus/*/gen_*.py; do [ -e "$s" ] && python "$s"; done

# 测试（单元 + 语料回归）
cargo test --workspace

# 冒烟（对应 T v1.1 §6.2 的 S-A/S-B/S-C，30 分钟内完成）
# 说明：冒烟是本地或打包后的人工动作，不在 CI 内（见 .github/workflows/ci.yml 文件头）
```

---

## 4. 开发约定摘要

1. **门禁零容忍**：`clippy -D warnings` 从第一行代码起生效，`rustfmt --check` 必须通过。
2. **许可证与漏洞**：新增依赖须过 `cargo-deny`（许可证 + 漏洞），移植代码须在文件头登记来源并进 SBOM（方案 §4.11.4）。
3. **语料回归不豁免**：移植改造的代码与其他代码同等，跑 `fixtures/` 全量（方案 §4.11.2a④）。
4. **错误码统一**：一律走方案 §4.6 的 `E1xxx–E9xxx` 分段，自带平行错误处理视为评审打回。
5. **变更路由**：产品行为 → 方案 §11.6；视觉 → U；工程实现细节 → 写 ADR。

> 全文见 [CONTRIBUTING.md](./CONTRIBUTING.md)。

---

## 5. 版本与通道

- **单机版走 1.x 序列**，内部构建号区分通道：**B（闭门测试）/ C（公发）**；
- 商业版从 **2.0** 起，不延续单机版序列（承接方案 §12.5 的版本号策略）；
- 配置、日志、备份与撤销数据目录按通道隔离，允许同机并存，便于把单机版真实数据带到商业版做迁移验证。

版本变更记录见 [CHANGELOG.md](./CHANGELOG.md)。