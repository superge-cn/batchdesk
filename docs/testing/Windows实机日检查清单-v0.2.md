# Windows 实机日检查清单

> 状态：v0.2（2026-11-07 排期版）｜用途：把「待人工执行」的用例拆成可排期的动作，并给出**当天怎么跑**（§9 议程 B0…B6）
> 相对 v0.1 的变化：① 同步 OQ-42 关闭后的正式档口径——C-2 编码两变体（T v1.13 附录 C）与吞吐双分母（方案 v1.20 §9.1），W5 的 Windows 重测按正式档执行；② W4 收编——`C10-EXE-04` 已随 ADR-0074 转 ◐（CI 载体），W4 只剩 `C15-PRE-04` 且属开发线、不占实机日；③ v0.1 §9 里「不在范围」的 S03 崩溃演练、C09 T 全量 UI 验收、UNC/长路径抽样、CSP 真 WebView 核对本版排入议程 B6；④ 新增 B0 物料/机器清单与分块时长估算。
> 权威：判定点只在 T v1.13 与产品方案 v1.21；本文件不复制判定文本，只规定**在哪台机器上、按什么顺序、用什么手段**制造条件。
> 数据源：`docs/testing/用例覆盖映射表-v0.2.csv`（状态与缺口）、`docs/testing/用例执行台账-v0.1.csv`（判定）、`fixtures/corpus/manual-only.txt`（人工语料）。条数以两份 CSV 为准（2026-11-07 重跑：实测通过 264／待人工执行 32）。
> 上一版：[v0.1](./Windows实机日检查清单-v0.1.md)（2026-11-05 起草、11-06 更新）。

## 1. 这份清单要解决的问题

台账里「待人工执行」的行，载体一栏统一写着「真人工（Windows 实机/受控环境）」，看起来像同一类事。实际上它们分成六组，其中**只有三组真的需要 Windows**：共享冲突语义、Windows 特色环境（OneDrive／配额卷／UNC）、NTFS 大小写折叠。另外两组（实现缺口、大输入实测）在 Linux 上就能推到底——它们是「功能没做完」或「数值没测」，不是「平台差异测不到」。

把这个区别写下来有两个用处：

1. 不在 Linux 上做无意义的等待——能推的先推，实机日只做复核；
2. 不把 Linux 的预跑当成验收结论。ADR-0051 的口径是明文禁止的：**待人工行的判定只能是「待人工执行」，Linux 预跑不得冒充**。本清单里完成的任何一项，都不会自动改变映射表的 ⛔ 状态——改判的唯一依据是实机执行后的记录（§8 落点）。

## 2. 六组分类（基准：2026-11-07 台账 32 条）

| 组 | 性质 | 条数 | 实例 | 实机日之外能推进到哪 |
|---|---|---|---|---|
| W1 | 撤销时目标被占用（Windows 共享冲突） | 17 | F01/F02/F03/F04/F05/F06/F07/F08/F09/F10/F11/F13/C01/C02/C03/C07/C10 的 `RBK-03` | 不能：POSIX 允许删除/改名打开中的文件，测不到共享冲突；只读半边已由语料 E03 覆盖（Linux，asserted） |
| W2 | Windows 特色环境 | 5 | `F10-EXE-02`／`C09-EXE-03`（OneDrive 占位）、`F11-EXE-02`（E04 配额）、`C07-EXE-04`（E01 目标占用）、`C07-EXE-05`（R05 UNC 断线） | 不能：占位文件、配额卷、SMB 断线都是 Windows/网络侧事实 |
| W3 | 文件系统大小写折叠 | 1 | `C06-EXE-02`（@R02 折叠恢复） | 不能：本机 ext4 大小写敏感；计划层折叠判定已由语料 R02 断言，缺的是真实不敏感卷上的撤销 |
| W4 | 实现缺口（功能未做） | 1 | `C15-PRE-04`（改并发后旧预览失效） | 属**开发线**（OQ-35 改判 ⛔：运行中调参与面板重算 UI 未实现），不占实机日；做完后仍需实机走一遍交互 |
| W5 | 大输入实测（C-1/C-2 档） | 7 | `F01`／`F02`／`F03`／`F10`／`C01`／`C02`／`C03` 的 `EXE-05` | Linux 已出正式档读数（§3 参照表）；实机日按同一夹具重测 |
| W6 | 受控基准（C-6 大集） | 1 | `C09-EXE-05` | C-6 的 `assert_c09.py` 断言需 Unix 主机，Linux 已可跑；Windows 当天跑「大集实测」（归正式基准 OQ-16） |

合计 17＋5＋1＋1＋7＋1＝32，与台账「判定＝待人工执行」的行数一致（条数以 CSV 为准，本表只做分组）。~~`C10-EXE-04`（密码包）~~ 已于 2026-11-05 随 ADR-0074 落地转 ◐（载体 CI），不在其中。

## 3. W5：Linux 参照读数与 Windows 重测清单

Linux 侧已出的正式档读数（`fixtures/bench/results.md`）是 Windows 重测的**配对参照**，不是验收结论（ADR-0051）：

| 实例 | 场景 | Linux 参照行 |
|---|---|---|
| `F10-EXE-05` | C-1 十万文件清单 | R-01／R-02／R-03 |
| `F01-EXE-05` | C-2 按字节分片（`--split bytes`） | R-04／R-05／R-06 |
| `F02-EXE-05` | C-2 按行数分片（`--split lines`，含行数预扫描） | R-07 |
| `F03-EXE-05` | C-2 按份数分片（`--split parts`，双遍读盘） | R-08 |
| `C01-EXE-05` | C-2 整档转码 ASCII（`c2rw --case c01`） | R-19（83.0 MiB/s；修复前 R-09…R-11 保留为历史）；整档配对 R-32（84.0 MiB/s） |
| `C01-EXE-05` | **GB18030 多字节变体（正式档，T v1.13 附录 C，OQ-42 裁决落地）** | **R-33**（源口径 22.6／产物口径 31.6 MiB/s，双分母；256 MiB 方向性读数 R-29 保留作历史参照，不再增引） |
| `C02-EXE-05` | C-2 整档换行改写（`c2rw --case c02`） | R-20（63.1 MiB/s；修复前 R-12／R-13／R-18 保留为历史）；子档 R-25（89.5 MiB/s） |
| `C03-EXE-05` | C-2 查找替换三臂（`c2rw --case c03-plain｜c03-regex｜c03-cell`） | R-21（plain 51.0）／R-22（regex 14.0）／R-23（cell 9.8 MiB/s）；子档 R-26／R-27／R-28 |

Windows 重测执行要点（协议全文见性能基准执行手册 §2–§4，此处只列实机日易踩的四条）：

1. **先填 C-4 环境台账**（`fixtures/bench/C-4-介质固定集/env.md`），再跑任何测量；每条结果行引用 `C-4/记录N`。
2. **转换类一律双分母**：源字节口径与产物字节口径两个读数都记、分别判定，只记一个分母＝记录不完整按作废处理（手册 §2-6，方案 v1.20 §9.1）。
3. **配对同窗口**：GB18030 变体臂必须与 ASCII 对照臂**背靠背同一窗口**跑；Linux 的教训（R-31）是 ASCII 臂对宿主负载极敏感、跨窗口倍数不可比——Windows 侧同样成立。开测/收测各记一次负载快照；Windows 没有 loadavg，先以 CPU% 快照（任务管理器或 `typeperf "\Processor(_Total)\% Processor Time"`）＋后台进程清单替代并在轮次记录里写明替代口径（手册下次修订时同步此点）。
4. **先跑保险丝预检**：`--rss-limit-mib 1024`（缺省值）超限即 kill 并停测、不产出结果行——这是合法结论（"这一档在本机测不了"），不得写成"未测"。

命令形状（Windows 下用 `python`；产物在 `%TEMP%\batchdesk-bench\` 或脚本所示临时目录，测后清理、数据集不入库；就地改写类跑在数据集之外的工作副本上）：

```bat
cargo build --release
python fixtures\bench\measure.py c1
python fixtures\bench\measure.py c2 --split bytes --rss-limit-mib 1024
python fixtures\bench\measure.py c2 --split lines --rss-limit-mib 1024
python fixtures\bench\measure.py c2 --split parts --rss-limit-mib 1024
python fixtures\bench\measure.py c2rw --case c01 --rss-limit-mib 1024
python fixtures\bench\measure.py c2rw --case c02 --rss-limit-mib 1024
python fixtures\bench\measure.py c2rw --case c03-plain  --rss-limit-mib 1024
python fixtures\bench\measure.py c2rw --case c03-regex  --rss-limit-mib 1024
python fixtures\bench\measure.py c2rw --case c03-cell   --rss-limit-mib 1024
:: GB18030 正式变体：先固化数据集，再与 c01 ASCII 臂背靠背同窗口
python fixtures\bench\C-2-超大单文件\gen_c2.py --encoding gb18030
python fixtures\bench\measure.py c2rw --case c01 --gb-multibyte --size-mib 2048 --rss-limit-mib 1024
```

仍缺的（与实机日无关，如实登记）：

| 实例 | 还缺什么 | 为什么 |
|---|---|---|
| `C03-EXE-05` | 界定不出的正则（无界量词／锚点／`\b`／分组量化／零宽）的大档数值 | 计划段与改写段都仍整段读——ADR-0064 划的边界，不是漏做；要测需先做「正则左邻上下文」切片 |
| 全部 W5 行 | **Windows 实机数值** | NTFS／杀软／页缓存不同，Linux 读数不作验收证据（ADR-0051）——这正是 B4 块要产出的 |

## 4. W4：只剩 C15-PRE-04，且它不进实机日

`C10-EXE-04`（加密包密码三条路）已随 ADR-0074 落地，CI 有真文件断言，映射表转 ◐（载体 CI）——v0.1 时代它占的实机日名额已释放。剩下的 `C15-PRE-04`（改并发后旧预览失效）是**功能未实现**（OQ-35 改判 ⛔），解除条件是先做运行中调参与面板重算 UI，属开发线排期，不占实机日；做完后仍需实机走一遍「任务运行中改并发、再点执行」的整条交互，届时随对应切片的验收一起做。

## 5. W1：占用怎么造（以及为什么 Linux 替不了）

R3 判定点（TC §2 R3）：撤销时目标被占用／只读／已被第三方改动，要①给出明确失败原因且不清空映射、②不造成二次破坏、③提示满足三层结构。

制造 "占用" 的两种手段：

- **Excel/WPS 打开目标文件**（语料 E01 的定义方式）：最贴近用户报告的场景，也最容易被杀软和其他句柄干扰；
- **PowerShell 持句柄**：`[System.IO.File]::Open($p,'Open','Read','None')` 以 `FileShare.None` 打开，独占性最干净，适合批量跑 17 条时避免"Excel 没真占住"的假阳性。

为什么 Linux 不能代替：POSIX 下删除/改名打开中的文件是合法操作，进程持有 fd 时 unlink 只减少链接计数，文件要到最后一个句柄关闭才真正消失——所以在 Linux 上"占用中的文件无法撤销"这个条件根本造不出来，跑出来的绿是无意义的绿。只读半边（`chmod 0444`）已在 Linux 侧由语料 E03 覆盖并断言到执行期跳过与报告，不必重复。

## 6. 顺带修掉的文档漂移（历史记录，v0.1 遗留）

映射表的「缺口」列有处在功能实现之前就写下了，功能做完后没回头改。2026-11-05／11-06 两批修了 `TC-C01-EXE-05`／`TC-C02-EXE-05`／`TC-C03-EXE-05` 三行（状态仍 ⛔，改的是「缺什么」）；2026-11-07 的两条实现缺口修完后，`TC-C02-EXE-05`／`TC-C03-EXE-05` 的缺口栏已同步为"Windows 实机重测＋（C03）界定不出的正则仍整段"。本版无新的映射表改动。

## 7. W2／W3：环境准备

| 组 | 需要的条件 | 怎么准备 | 失败时的常见假象 |
|---|---|---|---|
| OneDrive 占位（W2） | 文件在云端、本地只剩占位 | 对目标文件右键「释放空间」，确认图标变云朵；关闭"始终保留在此设备" | 一打开资源管理器就触发按需下载，占位消失→测到的是普通文件 |
| 配额卷（W2） | 写入必然失败的卷 | 小容量 U 盘，或用 `diskpart` 建 VHD 并设 NTFS 配额 | 配额只限制用户不限制卷；用管理员跑会绕过 |
| UNC 断线（W2） | 执行中断开网络共享 | 映射到另一台机的共享，执行到一半 `net use /delete` 或禁用网卡 | 断线后系统自动重连，任务"看起来没失败" |
| 大小写折叠（W3） | 真不敏感卷上的改名/撤销 | NTFS 默认不敏感；若要敏感目录对照，用 `fsutil file setCaseSensitiveInfo` | 在 WSL 的 ext4 上测——那不是 Windows 的折叠语义 |
| 基准安静窗口（B4） | 无杀软扫描/索引/更新抢 IO 的窗口 | 把仓库与数据集目录加入杀软排除；暂停 Windows Update；关浏览器与同步盘 | 杀软对 release 二进制逐次扫描把首轮拖长 10 倍级——预热轮丢弃可吸收大部分，但整档轮仍会被拖 |

## 8. 每条执行完的落点

1. 用例结果 → 映射表状态列（⛔ → ✅ 或 ◐）＋「缺口／待办」列写实测结论（改 `scripts/gen_case_map.py` 后重新生成 CSV，不手编）；
2. 语料结果 → `fixtures/corpus/manual-results.md` 追加一行（R05／E01／E02／E04；环境组合按 `fixtures/corpus/manifest.md` 的建议取 M1–M8）；
3. 性能数值 → `fixtures/bench/results.md` 按手册 §4 追加（含 C-4/记录N 引用与全部 3 次原始值）；
4. 台账 → 重跑 `python scripts/run_case_ledger.py`（数字不手写，ADR-0051）；
5. 签署 → `docs/testing/验收签署模板-v0.2.md` 的对应表单。

## 9. 实机日议程（B0…B6）

> 时长为排期粗估，以实际为准；顺序依据：环境类准备成本最高且牵动四条人工语料的记录，先做；占用批量一次环境跑完；基准要安静窗口，避开其他块的活动期；C-6 大集最耗时，放最后；B6 是 W0-6／Wave 出口欠账，可同日下午或另排半日。

### B0｜开工准备（约 30–60 分钟，release 构建另需约 20 分钟）

| 项 | 内容 |
|---|---|
| 机器 | Windows 10/11 x64 实机（NTFS，含管理员权限：`diskpart`/`fsutil`/配额要用）；WebView2 Runtime（Win11 自带）；基准卷剩余空间 ≥ 15 GiB（双变体整档＋分片输出＋工作副本的排期估算，正式判定仍引手册 §2-4） |
| 工具链 | Rust（仓库 `rust-toolchain.toml` 自动钉版本）、Node＋pnpm、Python 3、Git；`cargo build --release`（根 workspace）；根目录 `pnpm install` → `web` 目录 `pnpm install && pnpm build`；桌面壳 `pnpm tauri dev`（B6 UI 走查用）或 `pnpm tauri build`。注：Windows 构建已由 CI windows job 实证，本块风险低 |
| 记录 | 手册 §2-1：记提交 SHA、release 二进制 sha256、Rust 工具链版本——所有后续结果行都要引用 |
| 物料 | Excel 或 WPS（E01）；小容量 U 盘或 VHD＋NTFS 配额（E04）；已登录 OneDrive 且含测试文件的账号（E02）；第二台机器或虚拟机的 SMB 共享（R05）；（可选）tauri-driver＋msedgedriver（B6 的真 WebView E2E） |
| 语料联动 | 四条人工语料 R05／E01／E02／E04 的执行记录在 B1/B2 内顺带落账，不单独排块 |

### B1｜W2 环境类 5 条（约 2–3 小时）

顺序：`F10-EXE-02`＋`C09-EXE-03`（OneDrive 占位，**落 E02 语料记录**）→ `F11-EXE-02`（配额卷，**落 E04 语料记录**）→ `C07-EXE-04`（Excel 真占用，**落 E01 语料记录**）→ `C07-EXE-05`（UNC 断线，**落 R05 语料记录**）。每条注意 §7 表的"常见假象"列。

### B2｜W1 占用批量 17 条（约 2–3 小时）

PowerShell `FileShare.None` 持句柄法（§5），逐功能的 apply→undo 走 `RBK-03`；一次准备环境，逐条只换目标文件与功能命令。逐条核对三点：明确失败原因呈现、映射表不清空、无二次破坏；条目文案按三层结构核对（判据回 T，不在本清单复述）。

### B3｜W3 大小写折叠 1 条（约 15 分钟）

`C06-EXE-02`：NTFS 不敏感卷上的折叠恢复；如需敏感对照再用 `fsutil file setCaseSensitiveInfo`。

### B4｜W5 基准重测（半天～1 天，需要安静窗口）

顺序与配对（§3）：C-1 → C-2 分片三臂 → `c2rw` 整档四臂（c01 ASCII／c02／c03×3）→ **GB18030 变体臂与 c01 ASCII 臂背靠背同窗口**；每场景预热 1＋正式 3 取中位、双分母记录、引用 C-4/记录N。结果行落 `fixtures/bench/results.md` 并在备注写明"Windows 实机"与窗口负载快照。

### B5｜W6 C-6 大集（约 1–2 小时）

`C09-EXE-05`：C-6 大集上的 C09 扫描实测（归正式基准 OQ-16）。注意 `assert_c09.py` 需 Unix 主机，Windows 当天跑的是大集行为与耗时实测，断言复核留在 Linux 侧；硬链接类在 NTFS 上受支持，断言类目可人工核对一遍。

### B6｜扩展块：W0-6 与 Wave 出口欠账（半天，可同日下午或另排半日）

| 项 | 动作 | 落点 |
|---|---|---|
| C09 T 全量 UI 验收（W0-6 剩余） | 桌面 app（`pnpm tauri dev` 或 release 包）真机走查：扫描→计划预览→危险二次确认→执行→进度/暂停/取消→撤销→保留期演练/永久清理；对照 TC v0.4 §3 的 C09 行逐条勾 | 映射表 C09 行＋功能级签署表单（TS 模板 A） |
| S03 实机崩溃演练 | 执行中强杀进程（任务管理器或 `taskkill /F`）→ 重启 → 验证中断任务 `interrupted` 提示与 resume/rollback/discard 三条路 | 壳层恢复切片已有单测（Linux）；实机已执行（2026-11-08 第二/三轮）：回滚 71/71 全 reverted 且零 Journal 孤儿、续跑 1131/1131 幂等跳过、断线自动暂停显示原因后继续 5135/5135；证据 `E:/batchdesk-shijiri/s03/`，台账见 DEVELOPMENT-PLAN §6.1 |
| UNC/长路径矩阵抽样 | >260 字符深树的扫描/改名/撤销、`\\?\` 前缀显示值、UNC 共享上的 canonicalize 行为 | 长路径计划级断言 R01/R06 已 asserted；实机抽样结论记台账＋映射表缺口栏 |
| CSP 真 WebView 核对 | release 构建加载，确认控制台无 CSP 违规报告；装了 tauri-driver 则跑 `node scripts/e2e-tauri.mjs` 的真 WebView 链路 | `scripts/e2e-tauri.mjs` 手动项清账；装不上驱动则手动核对留截图 |

## 10. 不在本清单范围内

- TC §4 的 65 条横切用例（XCT）与 C37/B9 的 16 条：尚未做实例级映射，属另一条工作线；
- Web 分册（TW）与云契约：mock 服务端未就绪；
- W4 的 `C15-PRE-04`：功能未实现（OQ-35），属开发线（§4）；
- C-7 授权状态集与 C-5/C-8 正式基准：C-7 阻塞于 OQ-14 后续，C-5/C-8 按 OQ-16 排期。
