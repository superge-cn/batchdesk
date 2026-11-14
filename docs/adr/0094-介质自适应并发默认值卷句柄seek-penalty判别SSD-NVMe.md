# 0094. 介质自适应并发默认值：卷句柄 seek-penalty 判别 SSD/NVMe

- **状态**：已采纳
- **日期**：2026-11-13
- **决策者**：开发代理（工程实现决策；产品行为与阈值不变，取值直接引方案 §5.12 的既定决策）
- **关联**：方案 v1.22 §5.12（C15 并发控制）、§9.1（性能分档）；设计 04 §4.1（`FsAdapter` 能力）；设计 01 §4（`ApplyOptions.entryConcurrency`）；ADR-0033（§4 登记过"介质自适应自动档未落地"）；`crates/kernel/src/apply.rs`、`crates/kernel/src/fs.rs`、`crates/platform/src/fs.rs`、`src-tauri/src/tasks.rs`

---

## 1. 问题背景

方案 §5.12 有一条**已定的产品决策**：「并发的默认值（决策）：按介质自适应——I/O 密集类操作（复制/移动/解压）：SSD/NVMe 默认 4，机械盘与网络盘默认 2；CPU 密集类（哈希/编码检测/压缩）：默认核数减一，上限 8。用户可调范围 1–16。」

但实现里 `AutoOrNumber::Auto` 只是**透传**调用方的 `entryConcurrency`（`resolve_entry_concurrency` 的 `_ => requested` 分支），而所有调用方都不设该字段（恒为缺省 1）——所以自动档实际上等于"永远串行"。ADR-0033 §4 把这条登记为已知缺口，TC-C15-EXE-05 的缺口列写着「介质自适应默认值未做」。

## 2. 候选与否决理由

| 议题 | 候选 | 结论 |
|---|---|---|
| SSD/HDD 判别 | a) `GetDiskPerformanceInfo`；b) `IOCTL_STORAGE_QUERY_PROPERTY` 的 `StorageDeviceSeekPenaltyProperty`；c) 用 `GetDriveTypeW` 只分本地/网络，SSD 与机械盘同档 | **b**：c 拿不到 SSD 与机械盘的区别（那正是 4 与 2 的差别来源）；a 是 Win10+ 且只给吞吐不给介质类型，判据不如 seek-penalty 直接 |
| 开什么句柄 | a) **卷句柄** `\\.\X:`；b) 文件句柄 | **a**（实测）：本机 NVMe 上文件句柄一律 `ERROR_INVALID_PARAMETER(87)`（`StorageDeviceProperty` 与 `StorageDeviceSeekPenaltyProperty` **都**失败），卷句柄才返回描述符。卷句柄 `dwDesiredAccess` 传 0 即可，**不需要管理员权限** |
| 判别不了时 | a) 报错让用户处理；b) 保守取低档（2） | **b**：并发默认值只影响吞吐不影响正确性，静默降级到机械盘档（2）比阻断任务合理；返回 `MediumKind::Unknown` 并落到低档 |
| 端口 | a) 新增 `medium_kind` trait 方法；b) 在壳层直接写 Win32 | **a**：与既有的 `is_network_path` 同形（默认实现不猜、平台适配层覆写），kernel 与测试适配层拿到的确定值是 `Unknown`，便于全平台测 |
| 工作类别 | a) 按动作逐条判；b) **按计划 kind 判**（c10 按动作再细分） | **b**：并发的取值发生在**派发时**、整任务一个值，逐条判没有意义；kind 是现成的稳定分类 |
| 调用方显式要求更高 | a) 自适应值直接覆盖；b) 取两者较大 | **b**：`requested` 目前恒为缺省 1（无人设置），但保留"调用方要求更高则尊重"的出口，未来超大文件降并发的规则可以走这条 |

## 3. 决策

1. **端口**：`FsAdapter::medium_kind(&self, path) -> MediumKind`，缺省 `Unknown`；Windows 适配层覆写——先判 `is_network_path`（UNC＋映射盘）→ `Network`，否则开 `\\.\X:` 卷句柄问 seek-penalty：`IncursSeekPenalty == false` → `SsdNvme`，`true` → `Hdd`。
2. **纯函数**：`kernel::apply::adaptive_concurrency(medium, class, cpus)`——I/O 密集 SSD/NVMe→4、Hdd/Network/Unknown→2；CPU 密集 `cpus-1` 夹 1–8；结果统一夹进 1–16。
3. **工作类别**：`kernel::apply::plan_work_class(plan)`——CPU 密集＝C01（转码/编码检测）、C02、C03、C09（哈希）、C11（去重）；c10 按动作分（压缩＝CPU、解压＝I/O）；其余一律 I/O 主导。
4. **接线**：`resolve_entry_concurrency` 的 `Auto`／`None` 分支改为走自适应；`run_apply` 在**派发时**问一次介质（与并发设置同口径，方案 §7.10「运行中调整只影响尚未派发的条目」）。
5. **取值对象**：`plan_medium` 取计划里第一条可用路径的 **target**（写在哪块盘上决定并发代价），没有 target 才退到 source。

## 4. 后果

- **内核**：`fs.rs` 加 `MediumKind` 与 trait 方法；`apply.rs` 加 `WorkClass`／`adaptive_concurrency`／`plan_work_class`。
- **平台**：`fs.rs` 加 `volume_seek_penalty`（Windows 卷句柄）＋ trait 覆写；`windows-sys` 扩 `Win32_System_IO`／`Win32_System_Ioctl` 两个 feature（同一 crate，不新增依赖）。
- **壳层**：`resolve_entry_concurrency` 签名加 `plan`＋`fs`；新增 `adaptive_entry_concurrency`／`plan_medium`。
- **配置**：不动。缺省仍是 `auto`，语义从"透传请求值（＝1）"变成"按介质自适应"。
- **测试**：内核纯函数全档断言；壳层自动档接线；platform 加 `#[cfg(windows)]` 实机介质用例。
- **不变**：手动档（`Number(n)`）语义、1–16 契约范围、`MAX_ENTRY_CONCURRENCY` 均未触碰。

## 5. 残余

1. ~~**>2 GB 降并发的规则未做**~~ **已补**（2026-11-13，ADR-0095）：`plan_has_huge_entry`＋`huge_entry_cap`，作用在 `resolve_entry_concurrency` 最后一步、对手动档同样生效。当时 `requested` 无人设置，本 ADR 实现的"调用方要求更高则尊重"出口成了它的挂载点。
2. **macOS/Linux 的介质判别**：`volume_seek_penalty` 在非 Windows 返回 `Unknown`（保守 2）。Linux 上本可经 `ioctl(BLKROTATIONAL)` 判别，但当前切片只做 Windows（本机实机），其余平台按保守档运行。
3. **NVMe 与 SATA SSD 未细分**：两者都取 4。方案只分「SSD/NVMe」一档，不再细分。
