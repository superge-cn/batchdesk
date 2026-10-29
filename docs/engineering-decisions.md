# D-R 系列工程约定登记

> 本文件登记仓库内被 README／CONTRIBUTING／ADR／fixtures／CI 引用的 **D-R1…D-R5**。
> 它们是**仓库级工程约定**，不是四份产品基线的一部分；涉及产品行为时仍走方案 §11.6。

| 编号 | 约定 | 现状 | 主要引用点 |
|---|---|---|---|
| D-R1 | 文档语言：中文文档＋中文注释＋英文标识符 | 已执行 | `CONTRIBUTING.md`、`docs/adr/README.md` |
| D-R2 | ADR 采用自建五要素模板（问题／候选／理由／后果与退路／关联） | 已执行 | `docs/adr/0000-template.md`、`docs/adr/README.md` |
| D-R3 | fixtures 生成脚本统一用 Python 3 | 已执行 | `README.md`、`fixtures/README.md`、`fixtures/corpus/check_scope.py` |
| D-R4 | ADR 决策者为项目负责人（产品所有者确认产品行为） | 已执行 | `docs/adr/0001`…`0003` 头部 |
| D-R5 | CI 平台为 GitHub Actions；job 结构与命令保持可移植 | 已执行 | `.github/workflows/ci.yml` 文件头 |

## 变更规则

1. D-R 是工程约定，改动走 ADR 或项目负责人书面确认；不直接改本表。
2. 若某项约定实际改变产品行为/数值/视觉，按方案 §11.6 或 U 的流程处理，**不能**以 D-R 名义绕开基线。
3. 新增 D-R 编号只在确有仓库级约定时使用；日常实现细节不进本表。
4. 本表不复制基线内容；引用基线时只写章节号。
