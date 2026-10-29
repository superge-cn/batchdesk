# 0012. Windows 文件身份改用 WinAPI（windows-sys）

- **状态**：已采纳
- **日期**：2026-10-29
- **决策者**：开发代理（工程实现细节，不改变产品行为）
- **关联**：设计 [04 文件系统适配层契约](../design/04-文件系统适配层契约.md) §7；[PDR-001](../porting/PDR-001-C09-重复文件查找.md) T5/T8；ADR-0001（工作区结构）；AGENTS 不变量 #9（新增依赖须过 cargo-deny）

---

## 1. 问题背景

设计 04 §7 要求扫描适配器用 **volume serial + file index** 生成 `FileIdentity`，用于识别"同一文件多个入口"（硬链接），避免把一份文件误判成重复而清理。

触发条件：2026-10-29 CI 首轮在 Windows 上编译 `crates/platform` 时报
`error[E0658]: use of unstable library feature 'windows_by_handle'` ——
`std::os::windows::fs::MetadataExt::volume_serial_number` / `file_index` 仍在不稳定特性门控下，stable 不可用。此前 lint 只在 ubuntu 执行，Windows 分支从未被编译。

不做决定的代价：Windows 上所有重复组都因 `identity: None` 被标 `needs_manual_review`，
C09 清理计划恒为空（`stats.total = 0`）——MVP 主交付平台上的唯一纵向切片不可用
（CI run 40870967145 实证）。

## 2. 候选与结论

| 候选 | 一句话定义 | 结论 |
|---|---|---|
| A | 引入 `windows-sys`，调用 `GetFileInformationByHandle` 取 volume serial + file index | **选用** |
| B | Windows 上恒返回 `None`，全部走 `needs_manual_review` 回退 | 否决（功能不可用：Windows 是主平台，C09 将无法生成清理计划） |
| C | 用路径 + 大小 + 哈希近似身份，不取文件 ID | 否决（硬链接会被当成两份重复文件，存在误删风险，违反 PDR-001 T5 与不变量 7） |
| D | 等待 `windows_by_handle` 稳定 | 否决（无时间表；阻塞 MVP） |

## 3. 选择理由

- B 虽有"安全侧"性质，但它把**整个功能**降级为不可用，不是"个别异常文件跳过"，与设计 04 §7 的回退语义（"无法取得身份时"的例外路径）不符。
- C 明确不安全：硬链接在同一份数据上有多个路径，若无文件身份会进入同一哈希组并被判为重复，直接触发误清理——这是本项目最不能接受的失败模式。
- D 不可控：std 跟踪 issue 无稳定时间表，无法作为 MVP 的前置条件。
- A 是唯一既满足契约字段口径、又不引入不安全近似的路径；`windows-sys` 为 Microsoft 官方 crate，许可证 `MIT OR Apache-2.0`，已在 `deny.toml` 白名单内。

## 4. 后果与退路

- **后果**：`crates/platform` 新增一个 Windows 专属依赖（`[target.'cfg(windows)'.dependencies]`，非 Windows 构建不引入）；出现本项目第一处 `unsafe` FFI（仅 3 行，作用域限定在 `identity_from`，含 SAFETY 注释）；SBOM 增加 windows-sys 及其传递依赖；每次扫描需额外 `File::open` 一次以取句柄（只读、与哈希读取的 IO 成本相比可忽略）。
- **退路**：若 `windows_by_handle` 稳定化，可删除该依赖并改回 `MetadataExt`（约 5 行改动，无需改架构）；若上游 `windows-sys` 停止维护，可换 `windows` 官方 crate 或 `winapi`，字段口径不变。

## 5. 关联

- 设计 04 §7（Windows 文件身份口径）；PDR-001 T5（身份缺失不得自动清理）、T8（文件 ID 回退用例）
- 代码：[crates/platform/src/scan.rs](../../crates/platform/src/scan.rs) 的 `identity_from`；[crates/platform/Cargo.toml](../../crates/platform/Cargo.toml)
- 台账：[TRACEABILITY §1 04 行](../design/TRACEABILITY.md)、[DEVELOPMENT-PLAN W0-6](../DEVELOPMENT-PLAN.md)
- CI 证据：run 40870120156（E0658）、run 40870967145（`stats.total = 0`）
