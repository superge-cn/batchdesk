# 0010. Plan/Apply/Journal 的 JSON 序列化选用 serde

- **状态**：已采纳（2026-10-29，用户“按建议实施”；随 CLI apply/undo 框架落地）
- **日期**：2026-10-29
- **决策者**：用户（产品所有者）／工程实现
- **关联**：设计 01 §2.1、§5、§8；ADR-0004；PDR-001

---

## 1. 问题背景

C09 的计划预览需要落成可复核的 `PlanDocument` 文件，apply 必须消费同一份计划；apply 还要写 Journal JSONL 并输出 `ApplyReport`。设计 01 §2.1 已约定 Rust 侧使用 serde camelCase，但仓库此前没有任何序列化依赖。

触发条件：CLI apply/undo 要跨进程传递计划与 Journal；不做决定只能在单进程内内存传递，无法满足“先预览、后执行”和可撤销要求。

## 2. 候选与结论

| 候选 | 一句话定义 | 结论 |
|---|---|---|
| A | kernel DTO 派生 serde，CLI 用 serde_json 读写 | **选用** |
| B | 手写最小 JSON 解析/输出器 | 否决：转义、嵌套、可选字段与后续 DTO 演进都会重复造轮子，且易产生契约漂移 |
| C | 计划与 Journal 只留进程内内存 | 否决：无法先预览后执行、无法跨进程撤销 |

## 3. 选择理由

- 设计 01 明确要求 Rust serde 结构与 camelCase 线协议，serde 是该契约的直接落地；
- `PlanDocument`/`ApplyReport`/`JournalEntry` 都是可序列化 DTO，派生 serde 不引入手写字段映射；
- CLI 只需要 `serde_json` 做文件读写与 JSONL，不把 JSON 解析器下沉到业务逻辑；
- kernel 仍不读盘、不取时钟；serde 只提供类型转换，不破坏纯函数边界；
- 手写解析器在可选字段、转义和后续字段追加场景下不可控，维护成本高于依赖成本。

## 4. 后果与退路

- **后果**：kernel 新增 `serde`（derive）依赖，CLI 新增 `serde_json`；均由 Cargo.lock/SBOM 记录，并须通过 `cargo-deny`（MIT/Apache-2.0 均在白名单内）；后续 DTO 字段仍需遵守设计 01 的 camelCase 与可选字段规则。
- **后果**：`PlanParamValue` 采用 untagged 枚举；新增参数类型时必须补序列化/反序列化测试，避免 JSON 形状漂移。
- **退路**：若将来 ADR-0004 的 specta 生成链要求替换序列化层，保留 DTO 字段与 JSON 形状，更换 derive/生成器即可；若 serde 被排除，需先设计替代序列化契约并提升 01 版本。

## 5. 关联

- 设计 01 §2.1（编码/命名/serde）、§5（Plan/Apply DTO）、§8（Journal JSONL）；
- ADR-0004（类型契约单一来源）；
- PDR-001（C09 计划/apply/undo 实现）；
- `crates/kernel/src/plan.rs`、`apply.rs`、`journal.rs`；`crates/cli/src/journal.rs`。
