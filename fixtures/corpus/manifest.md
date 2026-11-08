# 语料清单（28 条）

> **核对状态**：**已与 T v1.13 §3.2 原文逐条核对通过**（初核 2026-10-26）。核对项：编号、类别、构造方式摘要、套件归属——28 条全部一致，无差异。**历史复核**（2026-10-28，T v1.2 时期）：§3.2 曾修订 K08 出处列（5.1→5.2、5.3），本表核对项不受影响；当前锚点为 T v1.13。
>
> **判定权威**：本表只作索引与进度跟踪；每条语料的"预期行为"以 T v1.13 §3.2 对应行为准，本表不复制其判定文本。
>
> **脚本状态**：`就绪` = 已有 `gen_{编号}.py`；`待补` = 尚未落地；**`人工` = CI 显式排除，走人工跟踪台账**（见 §人工跟踪与 `manual-only.txt`）。
>
> 补充说明：T v1.13 §3.2 的"特征"列未逐条复制到本表（本表以"构造方式摘要"定位语料）；如需按特征检索，请直接查 §3.2 对应行。
>
> **范围口径（已确认）**：CI 的语料步骤**只处理"自动化"子集**，`人工` 的 4 条**显式排除**并单列跟踪。**当前 CI 已执行 K01–K08、T01–T08、R01–R04/R06、Z01/Z02、E03 的断言（24 asserted／0 partial）**；因此 CI 绿灯仍不能表述为"自动化语料零失败"，更不能表述为 T v1.13 §1.3 字面的"语料回归零失败"——后者由**自动化 + 人工两栏共同构成**，两栏都过才计入阶段级验收（T v1.13 §4.2 的"回归通过"）。

| 编号 | 类别 | 构造方式摘要 | 脚本状态 | 备注 |
|---|---|---|---|---|
| K01 | 编码 | 以 GBK 写入含中文的表头与数据行，不写 BOM | **就绪**（示范） | 需冻结哈希：`sha256 99d618c5780f8d570461720b267eef51634fee61fac645e783b115fa9abcd7c0`（246 字节） |
| K02 | 编码 | 同一内容按 UTF-8 保存，关闭 BOM 选项 | **就绪** | |
| K03 | 编码 | 同 K02，保存时勾选 BOM | **就绪** | |
| K04 | 编码 | 按 UTF-16LE 保存后截去首部 BOM | **就绪** | |
| K05 | 编码 | 仅 ASCII，长度小于检测采样窗口 | **就绪** | |
| K06 | 编码 | 取图片或压缩包，改扩展名为 .txt | **就绪** | 位图样本改为脚本内嵌 67 字节固定 PNG，**无二进制入库**；若换真实位图须单独确认入库策略 |
| K07 | 编码 | 前半段按 GBK、后半段按 UTF-8 写中文后字节拼接 | **就绪** | |
| K08 | 编码 | 多行 CSV 按 UTF-16 带 BOM 保存 | **就绪** | |
| T01 | 结构 | 末条记录的引号不闭合，其后仍有数据 | **就绪** | 需冻结哈希：`sha256 41ebf608f0df4af364208539132c5b234c4105249ef1e03ffe866f9120c03c8b`（176 字节） |
| T02 | 结构 | 在某引号字段内键入真实换行 | **就绪** | 构造器复用 `bench/C-3-大CSV/gen_c3.py`（禁止另写一套） |
| T03 | 结构 | 造一条长度超过方案 §5.1 目标分片值的长行 | **就绪** | 阈值只引 §5.1；分片值属用户参数，脚本以 `--shard-bytes` 构造（默认 1 MiB，**非产品阈值**） |
| T04 | 结构 | 表头 3 列，数据行 2/4 列交替 | **就绪** | 当前样本列数为 1/2/3/4，覆盖"少"与"多"两侧 |
| T05 | 结构 | 新建空文件，不改动内容 | **就绪** | 需冻结哈希（0 字节）：`sha256 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| T06 | 结构 | 按方案 §4.8 超大文件档生成，全程不写换行 | **就绪** | 体积大**不入库**：CI 默认生成 16 MiB 结构等价样本；`--full`（> 2 GB）仅本地性能场景 |
| T07 | 结构 | 首行直接写数据，不含列名 | **就绪** | |
| T08 | 结构 | 表头写入重复列名，另一列名加首尾空格 | **就绪** | |
| R01 | 路径 | 深层目录＋长文件名拼至方案 §5.13 的超长分级区间 | **就绪** | 阈值只引 §5.13；只产三档边界路径串 + 浅层真实目录，**不在生成期创建超出安全档的真实路径** |
| R02 | 路径 | 同批两文件目标名设为 A.txt 与 a.txt | **就绪** | 依赖文件系统大小写行为（见方案 §4.7）；以"源文件 + 映射表"产样本，不建同名双文件 |
| R03 | 路径 | 将目标名设为 Windows 保留名 | **就绪** | 平台相关；保留名无法落盘，故以"安全源文件 + 映射表"产样本，宿主平台无关 |
| R04 | 路径 | 目标名末尾加空格或点 | **就绪** | 平台相关；同样以映射表产样本，不在磁盘创建该类名字 |
| R05 | 路径 | 任务执行中禁用网卡或停止共享服务 | **人工** | 构造依赖运行中的断线动作，非脚本可产出 → 见 manual-results.md |
| R06 | 路径 | 同一路径分别以带前缀与常规形式传入 | **就绪** | 前缀归一化由被测实现完成（不变量 8），样本只提供两种写法的路径对 |
| E01 | 环境 | 用 Excel/WPS 打开目标文件并保持 | **人工** | 占用动作需人工介入 → 见 manual-results.md |
| E02 | 环境 | 对文件执行"释放空间"，仅留在线副本 | **人工** | 依赖 OneDrive 环境，CI runner 不可复现 |
| E03 | 环境 | 去掉文件的写权限位 | **就绪** | `os.chmod` 跨平台：POSIX 去写位、Windows 映射只读属性 |
| E04 | 环境 | 用配额受限卷或小容量可移动盘承载任务 | **人工** | 依赖配额受限卷/小容量盘，CI runner 不可复现 |
| Z01 | 结构 | 用不写 UTF-8 标志位的打包方式，文件名按 GBK | **就绪** | `zipfile` 无法产出本样本（总写 UTF-8 标志），脚本按 ZIP 结构手写；需冻结哈希：`sha256 113faa19fc87c291839a9dad22574af7cc11e24c6c3ea1dbbe405378c6634abb`（199 字节） |
| Z02 | 结构 | 改写条目名为 `../` 形式后重新打包 | **就绪** | 手写 ZIP 结构（同 Z01）；需冻结哈希：`sha256 470357455f22fff86e1a7f48e61265c8b474a6d1dfb7a33457b119f177f151c4`（486 字节） |

## 进度汇总

| 状态 | 条数 | 说明 |
|---|---|---|
| 就绪（脚本进 CI） | **24**（K01 示范 + 本轮 23） | 已有脚本，CI 生成 + 范围核对；**逐条断言 harness**：24 asserted（K01–K08、T01–T08、R01–R04、R06、Z01、Z02、E03）／0 partial／0 pending，见 `crates/kernel/tests/assertions.tsv` |
| 待补 | **0** | — |
| **人工**（CI 显式排除） | 4（R05、E01、E02、E04） | 单列跟踪，见下 |
| **合计** | **28** | |

> **落地记录（2026-11-05，K08 收口）**：K08 由 partial 升 **asserted**——F05 支持 UTF-16 带 BOM 源直通分片：`CsvRecordScanner::new_utf16` 按码元投影扫描（结构字符只可能是 ASCII，宽字符高位字节恰为 `0A`/`2C` 时按字节扫会切断记录），表头区间含 BOM 故首片自带；不保留表头时第 2 片起由 `PlanCsvSplit.shardBom` 标前缀、`copy_ranges_atomic(prefix)` 一次原子写出，`expected.size` 计入前缀。断言：`corpus::k08_utf16_bom_sample_detects_transcodes_and_splits_with_a_bom_per_part`、`apply_flow::k08_utf16_csv_parts_each_start_with_a_bom`（真实 FS 读回三片，逐片 FF FE 开头且能解出表头）、`apply_flow::k08_utf16_csv_parts_without_header_still_start_with_a_bom`（第 2 片起走补写路径）。无 BOM 的 UTF-16 仍 E3001（不猜测重写形态），ADR-0070。
> **落地记录（2026-11-04，E03 收口）**：执行期语义按 OQ-37 裁决①落地——改名族（f12/f13/c04）与 C03 遇 `E2002`/`E2003` 记 **Skipped 并报告**（可重试的 `E2003` 先重打一次；`E2002` 按设计 06 不可重试），任务终态 `completed`、Journal 零记录；其余写臂仍 `Failed`。E03 因此升 **asserted**。真实文件系统那条断言限 `#[cfg(unix)]`：Windows 的只读是文件属性位而非目录权限，首版没门控时被 Windows job 判红（`left: "ok" right: "skipped"`），已如实改为平台限定并把 Windows 属性表现归入实机日。
> **落地记录（2026-11-04，E03 标记半程）**：E03 由 pending 升 **partial**——`corpus::e03_readonly_target_is_marked_by_preflight_and_named_in_preview` 先自证样本确为只读（否则断言退化为空转），再断言 ① S01 预检发 `readonly` Warn 项携 `E2002` 且 `task_blocked=false`（任务不整体失败）；② F12 与 C03 的计划告警点名该只读路径（事实由 `platform::preflight::path_check_light`/`is_read_only` 采集，见 [ADR-0063](../../docs/adr/0063-只读与占用事实的分档采集.md)）；③ 真实 CLI 出口 `apply_flow::f12_and_c03_name_read_only_files_in_plan_warnings`。**仍缺**：方案 §5.13 的「执行时重试一次后按策略跳过」——把 `E2002/E2003` 由 `Failed` 改判 `Skipped` 连带影响 C16 重试失败项集合与「部分失败」统计，属产品可见语义，登记 OQ-37 等人类裁决，裁决前不改执行器。
> **落地记录（2026-10-31，C10）**：Z01/Z02 转 asserted——platform `ZipArchiveService` 按通用标志位 bit 11 判定 UTF-8，未设置且非 ASCII 时回退 GB18030（Z01 解码「中文资料.txt」）；kernel `plan_c10` 跳过 `../` 越界条目（Z02），CLI c10 仅解压 `safe/ok.txt` 并可用 undo 清理；台账 **19 asserted／4 partial／1 pending**（仅 E03）。
>
> **落地记录（2026-10-31，C02）**：K06 魔数探测接入 `plan_c02`——PNG 样本在计划期 blocked E3001（binary file signature），不进入文本管道；台账 K06 由 partial 转 asserted（17 asserted／4 partial／3 pending）。
>
> **落地记录（2026-10-31）**：重命名族语料断言收口——R01（长路径三级）接入 `plan_f12` 计划级判定：200 无告警、259 发 `long-path-caution`、260 在长路径支持关闭时 `resolution=skip`＋`long-path-disabled`、开启时正常条目；R02（仅大小写冲突）经 `scan_conflicts` 读样本映射表断言；R03（保留名）与 R04（尾部空格/点）逐条 `plan_f12` 断言；R06（普通/扩展写法）经 `try_normalize` 与计划冲突去重断言。R04 样本修正：原 `年度报表 .txt` 属词干空格（整名以 `.txt` 结尾，Windows 合法），改为真正的尾空格 `年度报表.txt `；首部空格行按 §5.13 不在拦截清单、断言为不阻断。台账 16 asserted／5 partial／3 pending。
>
> **落地记录（2026-10-30）**：断言 harness 起步——`crates/kernel/tests/corpus.rs` 直接读取 `corpus/{编号}/out` 样本断言 K01–K08（3 asserted／5 partial），并附台账覆盖测试（24 条自动化语料与磁盘一一对应、人工项不得登记、非 asserted 必须写缺口）；16 条 pending 的缺口按条登记在 `crates/kernel/tests/assertions.tsv`。K03 首跑暴露 `transcode` 对 UTF-8 源 `had_bom_input` 恒 false 的缺陷，已修复。
>
> **落地记录（2026-10-28）**：23 条脚本一次补齐，全部幂等且带自校验；`expected.md` 逐条落地（判定权威一律回指 T v1.13 §3.2 对应行，不复制判定文本）。落地中发现的构造约束已写回上表备注：T03 的分片值属用户参数（脚本以 `--shard-bytes` 构造，不写死产品阈值）、T06 大体积样本按需生成、R01 不在生成期创建超出安全档的真实路径、R02/R03/R04 以"源文件 + 映射表"产样本、Z01/Z02 手写 ZIP 结构。

## 人工跟踪（CI 显式排除的 4 条）

**为什么排除**：这 4 条的构造方式依赖人工操作或特定环境（见上表备注），CI runner 无法复现。
**排除方式**：CI 的语料步骤读取 `manual-only.txt` 显式排除，并由 `check_scope.py` 强制"自动化与人工两集合互斥且无遗漏"。
**跟踪方式**：每次执行记一行到 [`manual-results.md`](./manual-results.md)，字段含语料编号、**环境组合**（T v1.13 §6.1 的 M1–M8）、日期、结论、失败时指向的缺陷编号。

| 语料 | 建议环境组合（T v1.13 §6.1） | 触发条件 |
|---|---|---|
| R05 | M3 或 M6（机械盘/SSD ＋ UNC） | 任务执行中断线 |
| E01 | M1 或 M5（SSD ＋ 本地） | 文件被 Excel/WPS 占用 |
| E02 | M2 或 M7（OneDrive 占位） | 占位文件被判为按需下载 |
| E04 | M1 或 M8（题述容量受限环境） | 目标盘剩余空间不足 |

**与验收的关系**：人工项**不进 CI 通过率**，但**进阶段级验收**——T v1.13 §4.2 的"回归通过"要求自动化与人工两栏都通过。当前自动化栏为 24 asserted／0 partial／0 pending（见 `crates/kernel/tests/assertions.tsv`），CI 绿灯仍不能代表"自动化语料零失败"。

## 缺陷回归语料（D3/D4，不计入 28 条 T 基线）

> **口径**：以下两条来自 2026-11-08 Windows 实机日第二轮查实的实机缺陷，**不属于 T v1.13 §3.2 的 28 条语料**，因此不建 `corpus/D3|D4` 目录、不写 `gen_*.py`、不改 `EXPECTED_TOTAL = 28` 与 `manual-only.txt`。编号用 `D` 前缀，避免落入 `check_scope.py` 的 `[KTERZ]\d{2}` 行解析与目录核对。真实断线/满盘场景仍按人工语料 R05／E04 重跑。

| 缺陷 | 实机表现（2026-11-08） | 修复与回归锚点 | 状态 |
|---|---|---|---|
| D3 复制非原子 | R05 断线瞬间 `FsAdapter::copy` 留 0 字节目标；恢复后重放被 E2007「目标已存在」跳过，任务报 completed 但文件 0 字节（全量 SHA256 对账唯一不符） | 同目录临时文件 → 校验写入字节数 → `fs::rename` 原子替换；锚点 `platform::fs::tests::failed_copy_preserves_an_existing_target`、`platform::fs::tests::short_or_failed_copy_never_reaches_the_target_path`、`platform::fs::tests::copy_replaces_existing_target_and_cleans_the_temp_file`、`platform::fs::tests::failed_copy_leaves_no_zero_byte_target`（`#[cfg(unix)]`）；[ADR-0083](../../docs/adr/0083-实机缺陷D3D4的安全默认修复.md) | 已修（故障注入红→绿）；R05 人工重跑待做 |
| D4 磁盘预检只接 c09 | E04 剩 1 MiB 的卷上 F11 建档前 31 条成功、9 条 E2004；S01 未在预检期阻断（`preflight_gate` 仅 `plan.kind == "c09"` 时采集磁盘） | 按目标卷分组收集空间需求，kernel 每卷出一条 `disk-space`；锚点 `tasks::tests::preflight_gate_checks_disk_for_non_c09_write_plans`、`tasks::tests::disk_requirements_sum_within_a_volume`、`kernel::preflight::tests::each_affected_volume_gets_its_own_disk_item`、`kernel::preflight::tests::disk_pass_at_boundary_and_block_one_byte_below`；[ADR-0083](../../docs/adr/0083-实机缺陷D3D4的安全默认修复.md) | 已修（不可达余量下限红→绿）；E04 人工重跑待做 |


## 待补脚本的实施顺序建议

> **状态：本节已完结（2026-10-28）**——原本 23 条待补脚本已按下列顺序全部落地，保留本节仅作过程留痕。

1. **先补纯脚本可构造的**（K 全部、T 全部、R01/R02/R06、Z 全部）——这些已具备脚本，可在 CI 中生成并做范围核对；K01–K08 已有逐条断言，其余待实现；✅ 已落地
2. **再补环境相关但可脚本化的**（R03/R04 在 Windows runner 上、E03 改动权限位）；✅ 已落地（R03/R04 改为"映射表"形态后不再依赖 Windows runner，Windows runner 只影响**被测行为**，不影响**样本生成**）
3. **`人工` 的 4 条不再列为"待补脚本"**：它们不会产出脚本，按上节的人工跟踪执行；若后续某条变得可脚本化（例如 E01 出现可复现的占用模拟），改标"就绪"并从 `manual-only.txt` 移除即可——`check_scope.py` 会强制这一改动**双侧同步**。
