# 文档总览

> **权威顺序**：四份基线（`docs/baseline/`）＞ 工程决策（`docs/adr/`）与设计契约（`docs/design/`）＞ 测试执行文档（`docs/testing/`）＞ 根 README／AGENTS。
> 本仓库文档只引用基线**章节号**，不复制原文；数值单源纪律见 `docs/baseline/README.md` 与 `docs/baseline/MANIFEST.md`。

## 1. 文档地图

| 目录／文件 | 回答什么问题 | 入口 |
|---|---|---|
| `docs/baseline/` | 产品、测试、视觉、走查的权威定义 | [baseline/README.md](./baseline/README.md) |
| `docs/baseline/MANIFEST.md` | 基线版本与快照哈希 | [MANIFEST.md](./baseline/MANIFEST.md) |
| `docs/baseline/PENDING-DECISIONS.md` | 用户已确认、待基线收编的决策 | [PENDING-DECISIONS.md](./baseline/PENDING-DECISIONS.md) |
| `docs/adr/` | 为什么这样实现、否决了什么、退路是什么 | [adr/README.md](./adr/README.md) |
| `docs/porting/` | 哪个功能从哪个项目移植、移植哪些部件、许可证与 PDR 台账 | [porting/README.md](./porting/README.md) |
| `docs/design/` | 工程契约：IPC、Web 传输、配置、文件系统、强制力、错误码、云端、授权、i18n、日志、发布、前端/壳 | [design/README.md](./design/README.md)（[实现追踪](./design/TRACEABILITY.md)） |
| `docs/testing/` | 用例库、Web 分册、验收表单、自动化映射 | [testing/README.md](./testing/README.md) |
| `fixtures/` | 28 条异常语料与 C-1…C-8 性能数据集 | [fixtures/README.md](../fixtures/README.md) |
| `docs/OPEN-QUESTIONS.md` | 尚未闭环、需要人或外部输入的事项 | [OPEN-QUESTIONS.md](./OPEN-QUESTIONS.md) |
| `docs/DEVELOPMENT-PLAN.md` | 先做什么、后做什么、各 Wave/功能完成到哪 | [DEVELOPMENT-PLAN.md](./DEVELOPMENT-PLAN.md) |
| `docs/engineering-decisions.md` | D-R1…D-R5 仓库级工程约定登记 | [engineering-decisions.md](./engineering-decisions.md) |

## 2. 推荐阅读顺序

1. **新成员**：根 `README.md` → `docs/baseline/README.md` → `docs/design/README.md` → `docs/DEVELOPMENT-PLAN.md` → `docs/testing/README.md` → `fixtures/README.md`。
2. **开工前**：对应功能的基线章节 → 设计契约 → ADR → 测试用例/语料。产品行为改动先走方案 §11.6；视觉改动回填 U。
3. **提测/验收前**：`docs/testing/测试用例库-TC-v0.3.md`、`Web测试分册-TW-v0.2.md`、`验收签署模板-v0.2.md`；语料范围与哈希由 `fixtures/corpus/check_scope.py`、`verify_corpus.py` 校验。

## 3. 维护规则

- **新增设计文档**：使用“状态＋日期＋适用＋权威来源索引＋待决”结构；在 `docs/design/README.md` 登记；涉及数值只引用基线章节，不抄录具体值。
- **新增 ADR**：按 `docs/adr/README.md` 的判据与编号规则；已采纳 ADR 不改正文，状态变化新写一篇。
- **新增测试文档/用例**：必须以 T v1.12 的章节号作为判定权威；无出处时标“阻塞”，不臆造预期。
- **新增语料**：按 `fixtures/README.md` §5 沉淀；同步 `manifest.md`、`manual-only.txt` 与 `expected.md`。
- **完成即标注**（[AGENTS.md](../AGENTS.md) 协作纪律第 4 条）：完成一个任务、功能或 Wave 时，当日同步 `docs/DEVELOPMENT-PLAN.md`（Wave 状态 + §6.1 记录）与 `docs/design/TRACEABILITY.md`；附可复核证据（测试/门禁/日期/回链），未过测试的不写“完成”。
- **提交前**：`python scripts/check_docs.py`、`python fixtures/migrations/check_schema.py`、`python fixtures/corpus/check_scope.py`；生成语料后跑 `python fixtures/corpus/verify_corpus.py --hashes`。

## 4. 当前状态（2026-10-29）

- 代码已有 `crates/kernel`、`crates/platform`、`crates/cli`、`src-tauri`（独立 workspace）与 `web`；C09 的纯分组、计划、隔离 apply/undo、S02、保留期 cleanup、任务队列/`task://*` 事件与事件驱动 React UI 切片已跑通，真实 WebView E2E（apply→undo、cleanup 演练、取消回滚）通过；目标结构见 `docs/design/12-前端与桌面壳架构.md`（v0.4）。
- 24 条自动化语料的**生成脚本**已就绪，但逐条断言 harness 尚未实现；CI 当前做生成、范围核对、哈希/幂等校验与单元测试，不能称为“语料回归零失败”。CI 已六 job 全绿（run 40872864062，2026-10-29，含 Windows 单元测试与 deny/SBOM）。
- 已补齐的文档：错误码表（已冻结 v1.2）、云端服务端草案、授权签发草案、i18n 草案、日志诊断草案、发布更新草案、前端/壳架构草案、AI 助手与工具调用草案；**07–13 为草案（12 已升 v0.4），14 已冻结 v1；01–06 已冻结或已评审**，07–13 待评审后才可作为实现冻结依据。
- 未闭环事项统一登记在 `docs/OPEN-QUESTIONS.md`，不要在代码注释或新文档里另立数值/结论。
- **F15 AI 助手**：已收编产品方案 **v1.19 第 14 章**与 **T v1.12**（F15 行＋B7），目标为全量可计划功能＋多计划、Phase 1 只启用 4 个功能；`设计 14 v1` 已冻结，实现未开始。
