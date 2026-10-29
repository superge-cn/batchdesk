# 0008. SBOM 工具选型：cargo-cyclonedx（CycloneDX JSON）

- **状态**：已采纳（2026-10-28 用户确认）
- **日期**：2026-10-28
- **决策者**：用户确认 2026-10-28
- **关联**：`CONTRIBUTING.md` §1②、`.github/workflows/ci.yml` deny job、ADR-0002（供应链与发布加固）、`docs/OPEN-QUESTIONS.md` OQ-09

---

## 1. 问题背景

CI 的 deny job 已有 `cargo-deny` 的许可证/漏洞检查，但 SBOM 步骤一直是占位：没有选定工具，发布时无法产出可交付的软件物料清单。触发条件：MVP 发布前需要 SBOM artifact，且后续客户/合规审计可能要求 CycloneDX 或 SPDX。

## 2. 候选与结论

| 候选 | 一句话定义 | 结论 |
|---|---|---|
| **A cargo-cyclonedx** | Cargo 原生生成 CycloneDX JSON | **选用** |
| B cargo-sbom | 生成 SPDX 文档 | 备选（客户明确要求 SPDX 时） |
| C 手写/脚本拼接 | 自建清单生成器 | 否决：维护成本高、易漏依赖 |

## 3. 选择理由

- **A 的核心理由**：与 Cargo 集成、输出 CycloneDX JSON、生态工具（Dependency-Track 等）可直接消费；需要 SPDX 时可用转换工具，不必现在绑定两种格式。
- **B 作为退路**：若客户合同明确要求 SPDX，再追加 `cargo-sbom` 或转换步骤；不影响本决策。
- **不做 C**：SBOM 的价值在依赖覆盖完整；自建脚本容易漏 workspace/dev/build 依赖，反而制造虚假合规。

## 4. 后果与退路

- **后果**：CI deny job 增加构建期工具（`cargo install cargo-cyclonedx --locked`），产物命名 `sbom/*.cdx.json` 并上传 artifact；工具仅构建期，不进运行时依赖。
- **退路**：若 cargo-cyclonedx 维护停滞或输出不满足客户格式，切到 B/转换工具；只改 CI 步骤，不影响产品代码。
- **不在本 ADR 冻结**：工具小版本、CLI 具体参数以首次接入时该版本文档为准；接入 PR 必须记录实际命令。

## 5. 执行清单

- [ ] CI deny job 安装并固定 cargo-cyclonedx 版本，替换现有 SBOM 占位；
- [ ] 生成 JSON 并上传 artifact，路径与 `upload-artifact` 一致；
- [ ] 在 `CONTRIBUTING.md` 与发布检查清单（设计 11 §6）补实际命令；
- [ ] 首次接入后把产物样例附在 PR，确认可被 CycloneDX 工具解析。
