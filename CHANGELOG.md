# 变更记录

> 格式：Keep a Changelog。语义化版本。
> **版本与通道口径**（承接基线《办公辅助工具产品方案》§12.5）：单机版走 **1.x** 序列，内部构建号区分 **B（闭门测试）/ C（公发）** 通道；商业版从 **2.0** 起，不延续单机版序列。

## [Unreleased]

### Added

- 仓库骨架初始化：README、CONTRIBUTING、AGENTS.md（供 AI 编码代理读取）、ADR 体系（`docs/adr/`）、语料与基准数据集骨架（`fixtures/`）、CI 四 job 骨架（`.github/workflows/ci.yml`）。
- **语料回归范围门禁**：把 T v1.1 §3.2 的 28 条显式分为「自动化」与「人工」两栏——人工项 4 条（R05/E01/E02/E04）由 `fixtures/corpus/manual-only.txt` 登记、CI 显式排除，并单列跟踪台账 `fixtures/corpus/manual-results.md`；`check_scope.py` 强制两栏互斥且无遗漏（新增语料漏建脚本会直接失败）。

### Changed

- 无。

### Fixed

- 无。

### Removed

- 无。

---

<!--
填写约定：
- 面向"使用者能感知的变化"，不写内部重构细节（重构记 ADR）；
- 每条尽量带上方案章节号或 ADR 编号，便于回溯；
- 破坏性变更（配置格式、数据目录、输出文件命名）必须单独起一段并在段首标注「**Breaking**」；
- 发布时的版本号与通道标法：`1.0.0-B3`（闭门第 3 次构建）/ `1.0.0-C1`（公发第 1 次构建）。
-->