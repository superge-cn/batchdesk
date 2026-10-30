# 0013. C17a 操作日志存储选型：rusqlite（bundled）＋单表最小 schema＋user_version 迁移

- **状态**：已采纳（2026-10-30 用户确认推荐方案：① 引入 rusqlite；② 拆分列恒写、完整路径列受 `logFullPaths` 控制；③ 任务收尾时批量事务写）
- **日期**：2026-10-30
- **决策者**：产品所有者（按工程代理推荐确认）
- **关联**：方案 §10.2（C17a 字段集与保留期，v1.9/11.6-13）、§9.6/§9.7（脱敏与隐私）、§7.8（可撤销窗口）；设计 10 §1/§2/§3/§4/§7；设计 03 §4.1（`logging.logFullPaths`/`logging.retentionDays`）；ADR-0010（serde 同一选型）；硬性不变量 9（新依赖过 cargo-deny 并说明理由）

---

## 1. 问题背景

设计 10 §1 已冻结操作日志（审计轨）载体为"本地数据库（桌面 SQLite）"，但 §7 留待决项：**数据库 schema/索引与迁移策略**。C17a 最小日志是阶段 0 验收基本盘的组成部分（方案 §11：每阶段"预览＋回滚＋最小日志"覆盖所有批量功能），Wave 2 批量上功能前必须补上，否则每个新功能都缺验收依据。

触发条件：Wave 1 收尾只剩本项工程工作。不决定的代价：Wave 2 功能持续在缺审计轨的状态下开发，事后补录需要回改每个功能的接线。

## 2. 候选与结论

| 候选 | 一句话定义 | 结论 |
|---|---|---|
| A | `rusqlite`（bundled 静态编译 SQLite）＋单表最小 schema＋`PRAGMA user_version` 手写迁移 | **选用** |
| B | 先用 JSONL 落操作日志，SQLite 后补 | 否决：违背设计 10 §1 既定载体（"桌面 SQLite"），且二次迁库是纯返工 |
| C | sqlx / diesel / sea-orm 等带运行时或 ORM 的方案 | 否决：引入 async 运行时或编译期检查宏，与同步任务队列模型不符，超出最小需要 |
| D | KV 存储（redb/sled） | 否决：KV 失去 SQL 反查能力（C06 撤销反查、C58 报表），且"数据库（SQLite）"是契约字面 |
| E | 暂不做，直接进 Wave 2 | 否决：验收基本盘缺口随功能数累积 |

## 3. 选择理由

- **排除 B**：契约载体已冻结为 SQLite，JSONL 是偏离契约的实现捷径；C06 的"按计划反查操作记录"需要索引查询而非顺序扫描。
- **排除 C/D**：本仓库线程模型是同步 worker（`tasks.rs`），不需要 async；ORM/迁移框架与 KV 都把"最小"做成了"更重"。
- **选 A 的关键参数**：
  - 依赖面：`rusqlite 0.41` + `libsqlite3-sys 0.39`（bundled）+ `cc`/`pkg-config`，均为 MIT OR Apache-2.0，落现有 deny 白名单（已实跑 `cargo deny check` 验证）；
  - **schema（user_version=1，单表 `operation_log`）**：`id`(ULID 主键)/`ts`/`task_id`/`plan_id`/`entry_id`(可空)/`op_kind`/`phase`/`status`/`code`(可空)/`dir_name`/`old_name`/`new_name`/`before_path`(可空)/`after_path`(可空)/`overall_completed`/`detail`(可空)；索引 `plan_id`（C06 反查）、`task_id`（任务追溯）、`ts`（保留期清理）；
  - **脱敏调和**（用户确认项②）：方案 §10.2 字段集含"旧/新完整路径"，而设计 10 §3 规定 `logFullPaths` 默认 false——拆分列（`dir_name`/`old_name`/`new_name`）恒写保证字段集与撤销反查能力，完整路径两列默认 NULL、开关打开才写；
  - **迁移**：`PRAGMA user_version` 顺序执行、只增不改（追加列/表）；库版本高于代码 → E1007 只读拒绝；文件损坏 → 保留 `.corrupt.<ts>` 副本并重建空库（E5009，对齐 `config_store` 语义与设计 10 §4"不得静默清空"）；
  - **写入粒度**（用户确认项③）：任务收尾（`finish` 语义点）单事务批量写"批次 start/finish＋逐条 entry"，数据全部取自 `ApplyReport` 与计划/Journal；执行热路径不碰数据库；
  - **保留期**：`logging.retentionDays`（默认 7、可配 1–30，同 §7.8 撤销窗口）经 `cleanupRetention` 以 `DELETE WHERE ts <= cutoff` 清理；不把日志保留期当撤销窗口（设计 10 §4）。

## 4. 后果与退路

- **后果**：桌面二进制增大约 1MB（静态 SQLite）；Windows 构建多一个 C 编译单元（CI MSVC 可编译；本地 windows 交叉检查需与 blake3 相同的绕过手段）；审计写入为尽力而为——数据库不可用时降级 stderr 诊断并继续执行，**不得**因审计轨故障阻断批处理（Journal 才是撤销依据）。
- **退路**：若未来更换载体（如云端归档），操作日志的追加式 JSON 导出（设计 10 §5）是中间格式；单表 schema 只增不改，推翻本决策的迁移成本集中在 `platform::oplog` 一个模块。

## 5. 关联

- 代码落点：`crates/platform/src/oplog.rs`（存储与迁移）、`src-tauri/src/tasks.rs`（事务写入）、`platform::retention`/`cleanupRetention`（保留期清理）；
- 本 ADR 同时解决设计 10 §7 第 1 项待决（schema/索引/迁移策略）；第 2 项（诊断日志轮转参数）与本 ADR 无关，另行决策；
- CLI 侧暂不落操作日志（OQ-30 实验命令，设计 13 v0.7 §2.1）。
