# C-4 介质固定集：环境基线记录

> 用途：为 C-1／C-2／C-3 提供**介质变量固定**的前提；四项缺一不可（见 [README](./README.md)）。
> 本文件是**人工填写台账**，不是数据脚本；每换一次测试机或重装系统，新增一条记录并注明日期。

## 记录 1

| 项 | 值 | 备注 |
|---|---|---|
| 记录日期 | 2026-11-02 | 本轮 C-1／C-2 预跑用此记录 |
| 主机标识 | `chaoge-PC` | 仅主机名，无个人信息 |
| 操作系统与版本 | Deepin 25（crimson）／内核 6.18.34-amd64-desktop-rolling | **非 Windows**：方案 11.6-13 已确认 MVP 平台范围＝仅 Windows，故本轮为**非验收环境预跑**，§9.1 的吞吐档位仍可按介质比较，但验收测量须在 Windows 上重跑 |
| 介质类型 | SATA SSD（`lsblk` ROTA=0，非旋转） | 对应 §9.1：目录清单指标注明 SSD；大文件吞吐的 ≥200 MB/s 档写的是 NVMe，本机是 SATA SSD，故吞吐结论只作**下限参考**不作达标判定 |
| 介质型号 | Samsung SSD 870 EVO 1TB（`/dev/sda`，分区 `/dev/sda5`） | `lsblk -d -o NAME,ROTA,TRAN,MODEL` 实测 |
| 文件系统 | ext4（`/dev/sda5` 挂 `/home`，`rw,relatime`） | 与 Windows/NTFS 的长路径与硬链接语义不同：R01/R02 类结论在此机不可判定，属 Windows 实机日 |
| 测试前磁盘剩余空间 | 149,139,048 KiB ≈ 142.2 GiB（`df -T /home`） | 远大于本轮数据集体积（C-1 约 395 MiB；C-2 全量档 2 GiB＋64 MiB，输出另需同体积，落在 `/dev/sda4` 的 84 GiB 空余上），满足 §9.1 的 S01 安全余量判定式 |
| 磁盘压缩 | 已关闭（ext4 挂载项无压缩，仅 `rw,relatime`） | `findmnt -no OPTIONS /home` |
| 索引服务 | 已关闭（`baloo_file` inactive；未安装 `updatedb`／`mlocate`） | 索引会污染目录遍历的缓存结论 |
| 实时杀软 | 未启用实时防护（无 clamd 进程；`clamav-freshclam` inactive） | **注意**：§9.1 的"批量写每 100 项暂停 200ms"是为规避 Windows 实时杀软叠加扫描；本机无杀软，故该节流的收益无法在此机测出，属 Windows 实机日 |

## 记录 2

| 项 | 值 | 备注 |
|---|---|---|
| 记录日期 | 2026-11-07 | 本轮 Windows 实机日（检查清单 v0.2 §9·B4）用此记录 |
| 主机标识 | `WIN75K7S16U0E2E` | 仅主机名，无个人信息 |
| 操作系统与版本 | Windows 11 专业版 10.0.26200（Build 26200） | **Windows／NTFS**：MVP 平台范围（方案 11.6-13）内的验收环境 |
| 介质类型 | NVMe SSD（`Get-PhysicalDisk` BusType=NVMe，MediaType=SSD） | 对应 §9.1：大文件吞吐的 ≥200 MB/s 档（NVMe）在本机**适用**；目录清单的 SSD 档同样适用 |
| 介质型号 | Samsung SSD 990 EVO Plus 1TB（Disk 0，分区 E: 432 GB） | PowerShell `Get-PhysicalDisk`／`Get-Partition` 实测 |
| 文件系统 | NTFS（E:，`Get-Volume` Healthy） | 大小写不敏感卷：W3（C06-EXE-02 折叠恢复）在本机可判定；长路径前缀语义与 Linux 不同 |
| 测试前磁盘剩余空间 | 82.3 GB（`fsutil volume diskfree e:` ＝ 88,372,109,312 字节） | 覆盖 C-1（约 395 MiB）＋C-2 双变体（2 GiB×2＋GB 期望产物 ≈3 GiB）＋工作副本，满足 §9.1 的 S01 安全余量判定式 |
| 磁盘压缩 | 未启用（`compact /s:E:\` 无压缩文件） | NTFS 压缩默认关闭，实测一致 |
| 索引服务 | **WSearch Running（Automatic）＋ SysMain Running（Automatic）** | 与记录 1 的「索引已关闭」不同：Windows 默认开启，是本轮的既知扰动源；正式读数要求安静窗口（手册 §3），必要时先暂停两服务再跑并在此备注 |
| 实时杀软 | **在运行**：360 安全套装＋360 杀毒（SecurityCenter2 productState 335872）；Windows Defender 在注册列表（productState 393472，实时防护状态未单独确认） | §9.1 的「批量写每 100 项暂停 200 ms」节流在本机是**真实生效场景**；杀软对 release 二进制与产物文件的逐次扫描会拉长首轮——预热轮丢弃可吸收大部分；正式读数建议把基准目录加入杀软排除并在此备注 |
| CPU／RAM | AMD Ryzen 7 5800U（8 核 16 线程）／15.3 GB | 内存采样口径见 ADR-0082（WorkingSetSize，Windows 侧） |
| 内存采样口径 | `GetProcessMemoryInfo` 的 WorkingSetSize（ADR-0082） | 与记录 1 的 VmRSS 同为驻留物理内存口径但不等价，两平台读数不做直接比较 |

## 记录 3

| 项 | 值 | 备注 |
|---|---|---|
| 记录日期 | 2026-11-12 | EX-01 三臂**预演**用此记录（非补测，补测节点 2026-11-30） |
| 主机标识 | `WIN75K7S16U0E2E` | 与记录 2 同一台，未换机 |
| 操作系统与版本 | Windows 11 专业版 10.0.26200（Build 26200） | 同记录 2（MVP 平台范围内） |
| 介质类型 | NVMe SSD（`Get-PhysicalDisk` BusType=NVMe，MediaType=SSD） | §9.1 的 ≥200 MB/s（NVMe）档在本机**适用** |
| 介质型号 | Samsung SSD 990 EVO Plus 1TB（Disk 0，分区 E: 432 GB） | 同记录 2 |
| 文件系统 | NTFS（E:，`Get-Volume` Healthy） | 同记录 2 |
| 测试前磁盘剩余空间 | 42 GB（清理后 `df -h /e`；测量进行中一度逼近 0，逐臂清理后回到 36 GB） | **教训**：整档三臂串行时每臂峰值 4–6 GiB，工作目录缺省落 `E:\tmp\batchdesk-bench`（Python 的 `/tmp` 在 Windows 原生下＝当前盘根，非 Git Bash 的 `/tmp`）；必须逐臂清理并预留 ≥15 GiB，否则 `ENOSPC` 会让该臂整轮作废 |
| 磁盘压缩 | 未启用 | 同记录 2 |
| 索引服务 | WSearch Running ＋ SysMain Running | 同记录 2（Windows 默认开启） |
| 实时杀软 | 360 安全套装＋360 杀毒在运行 | 同记录 2 |
| CPU／RAM | AMD Ryzen 7 5800U（8 核 16 线程）／15.3 GB | 同记录 2 |
| 内存采样口径 | `GetProcessMemoryInfo` 的 WorkingSetSize（ADR-0082） | 与记录 1 的 VmRSS 同为驻留物理内存口径但不等价 |
| 页缓存条件 | **冷**：每轮工作副本是新写入的 2.2 GiB，源数据集的页在上一轮即被挤出 | 与记录 2 的「页缓存热窗口、绝对值偏乐观」不同，本轮读数偏保守（C01 ASCII 对照 231.2 vs R-38 的 486.9 MiB/s 即为口径差，非实现回退） |

## 记录 4

| 项 | 值 | 备注 |
|---|---|---|
| 记录日期 | 2026-11-13 | F04 合并臂（`c2merge`）实测用此记录 |
| 主机标识 | `WIN75K7S16U0E2E` | 与记录 2／3 同一台，未换机 |
| 操作系统与版本 | Windows 11 专业版 10.0.26200（Build 26200） | 同记录 2／3 |
| 介质类型 | NVMe SSD（`Get-PhysicalDisk` BusType=NVMe，MediaType=SSD） | §9.1 的 ≥200 MB/s（NVMe）档在本机**适用** |
| 介质型号 | Samsung SSD 990 EVO Plus 1TB（Disk 0，分区 E: 432 GB） | 同记录 2／3 |
| 文件系统 | NTFS（E:，`Get-Volume` Healthy） | 同记录 2／3 |
| 测试前磁盘剩余空间 | **3.2 GB**（`df -h /e`；当日构建与往轮残留把盘占到 100%） | **教训（再现记录 3 的 ENOSPC）**：本轮先按整档 1 GiB 跑，warm＋run1 各占约 1 GiB 时把盘写满，run2 报 `E3005 os error 112`（磁盘空间不足）整轮作废；清理 `E:	mpatchdesk-bench` 与临时目录后回到 3.2 GB，改用 **256 MiB 子档**（`--size-mib 256`）才跑通。整档测量前必须预留 ≥15 GiB（同记录 3） |
| 磁盘压缩 | 未启用 | 同记录 2／3 |
| 索引服务 | WSearch Running ＋ SysMain Running | 同记录 2／3 |
| 实时杀软 | 360 安全套装＋360 杀毒在运行 | 同记录 2／3 |
| CPU／RAM | AMD Ryzen 7 5800U（8 核 16 线程）／15.3 GB | 同记录 2／3 |
| 内存采样口径 | `GetProcessMemoryInfo` 的 WorkingSetSize（ADR-0082） | 同记录 2／3 |
| 页缓存条件 | **冷**：每轮工作副本是新写入的 256 MiB 子档 | 与记录 3 同口径（偏保守） |

## 使用要求

1. 每次跑 C-1／C-2／C-3 前先确认本记录与当前机器一致；不一致则**新增记录**，不要覆盖旧记录。
2. 结论记录时必须引用本文件的记录编号（例：`C-4/记录1`），便于把吞吐结果与介质对应起来。
3. 本文件不含任何真实客户数据。
