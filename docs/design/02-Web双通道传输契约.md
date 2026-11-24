# 02 Web 双通道传输契约

> 状态：已评审·实现冻结 **v1.17**（2026-11-24 A3-18：**staging 内存峰值与跨线程复制收口**——`stage_inputs` 由「一次带全部拼接字节」改为**逐文件登记**（峰值只与最大单文件相关）、字节走 **transferable** 不再结构化克隆、「先算键再登记」保证不留下半批输入；同批修 `clearPersisted` 把任何异常当成功。协议形状不变，仍是「清单 + 一段字节」，只是清单固定一项，[ADR-0118](../adr/0118-Web端staging内存峰值与跨线程复制收口.md)。）｜v1.16（2026-11-24 A3-17：**F13 正则改名接入 UI**——F13 此前只有内核与 CLI（「切片完成，无 UI」），桌面壳无路由、前端无视图；本批补壳层 `plan_f13_shell`＋导航/字段/双语与 Web 计划执行通道，计划面与执行面各 **22 → 23 个视图**，`unsupported` 收 2（只剩 C09／F11）；同时把 F12/F13 的目录采集提成共用实现（跳符号链接＋按名排序，与 CLI 同口径——F12 壳层此前两项都没做），[ADR-0117](../adr/0117-F13正则改名接入UI面.md)。）｜v1.15（2026-11-23 A3-15：**wasm32 时间源修复**——`std::time::SystemTime::now()` 在 `wasm32-unknown-unknown` 上调用即 panic，而 `ulid_like_id()` 被 28 个 plan 入口共用，导致「浏览器里真选文件走 WASM」从 A2 起必崩；改为 wasm32 走 `js_sys::Date::now()`，防线放在真 WASM 的 e2e 而非语料（语料跑在 native 永远绿），[ADR-0116](../adr/0116-WASM侧时间源与真实浏览器防线.md)。）｜v1.14（2026-11-23 A3-13：① **C09 归因纠正**——`content-hash` → `file-identity`：哈希在 WASM 里本可算（`blake3` 已是 wasm 依赖、`plan_c09` 已导出），真正缺的是 `FileIdentity`（卷序列号＋文件 ID），`plan_c09` 对身份未知的组整组排除，直连只会得到 0 条目的假绿计划；② **执行后搬运**：产物在执行成功后异步落 OPFS（`batchdesk/products/<planId>/`），内核 `FsAdapter` 同步契约零改动——与 staging 的「异步前移」对称，[ADR-0115](../adr/0115-Web产物执行后搬运与OPFS落盘.md)。）｜v1.13（2026-11-22 A3-12：计划面与执行面各 21 → **22 个视图**——F10 目录清单接上：执行动作是 `listing`、缺的是 `DirectoryLister` 端口而非「目录树」（沿用 A3-9 的归因教训），端口由 `MemoryDirectoryLister` 在 WASM 内读浏览器注入的目录事实（`webkitRelativePath`）按桌面同口径写 BOM CSV；新增 `stage_dirs`／`stage_listing` 两个 op；剩余 3 个不可计划视图：C09／F11／F13，[ADR-0114](../adr/0114-F10目录清单执行面与浏览器目录事实.md)。）｜v1.12（2026-11-22 A3-11：计划面与执行面各 19 → **21 个视图**——C04 名称替换／F12 批量改名接上：条目＝目录**第一层**名字（与桌面 `read_dir` 单层同口径），staging 登记整棵子树，产物是改名后的文件、可下载可撤销；`WebPlanF12Input` 补 `camelCase`，F12 的 `planId` 由前端生成 26 字符 ULID；档位表删掉前端没有视图入口的 F13 死条目，[ADR-0113](../adr/0113-Web改名族执行面与目录第一层条目.md)。）｜v1.11（2026-11-22 A3-10：执行面 18 → **19 个视图**——C10 解压/压缩在浏览器里能真执行了：`MemoryArchiveService` 在 WASM 内实现 `ArchiveService`（与桌面同一个 `zip` 9.1.3、同一份名字解码、同一套 E3003/E3008/E3009），沙箱按 `plan.kind == c10` 走 `apply_archive_plan`；解密密码是**执行期秘密**（`ApplyRequest.password`，不进计划/Journal/沙箱），缺密码整单 E3008；`WebApplyGap` 删 `port-missing`，只剩未知视图的 `not-wired`，[ADR-0112](../adr/0112-C10归档执行面与WASM内ArchiveService.md)。）｜v1.10（2026-11-22 A3-9：执行面 16 → **18 个视图**——C07/C08 的源是 `sourceRoot/相对路径`，staging 键改按**计划的源口径**登记（目录选择类视图拼「根名/相对路径」，其余仍裸文件名）；C07 只登记源目录槽、清单不进沙箱；`WebApplyGap` 删 `relative-paths` 与 F10 死条目，只剩 C10 `port-missing`，[ADR-0111](../adr/0111-Web执行面按相对路径登记输入与C07-C08接入.md)。）｜v1.9（2026-11-22 A3-8：执行面 9 → **16 个视图**——7 个 CSV 计划入口接受 `outputDir`、目标路径由 wire 层派生（缺省沙箱 `/out`，不回落到源文件）；`WebApplyGap` 删 `target-in-place`，剩 C07 `relative-paths` 与 F10/C08/C10 `port-missing`，[ADR-0110](../adr/0110-Web计划输出路径派生与CSV族执行面.md)。）｜v1.8（2026-11-22 A3-7：执行面从 3 个视图扩到 **9 个**——C01/C02/C03 的改写器端口在沙箱内按 `plan.kind` 用 `from_plan` 构造（三者全在 kernel），F04–F06 的 `merge`／`split` 动作本就只要读写；未接的 10 个按 `target-in-place`／`relative-paths`／`port-missing` 分类拒绝，见 ADR-0109。）｜v1.7（2026-11-22 A3-6：Web **执行面**接上——`EXEC_OPS` 4 个 op（`stage_inputs`/`apply_plan`/`undo_plan`/`reset_sandbox`）→ 5 个导出（`web_exec_product` 是取值器）；执行跑在**会话内内存文件系统**（`kernel::memfs::MemoryFs`，`FsAdapter` 第三个实现）上，`generic_apply`/`apply_plan`/`undo` 全走内核原路径，产物从 Journal forward 取落盘路径后以 Blob 交回主线程下载；执行能力按**端口齐备度**分档（仅 F01/F02/F03），未接线视图按钮前即禁用；撤销 `products-only`（已下载副本收不回，界面照实说）；OPFS 路线降级为可选后续，见 [ADR-0108](../adr/0108-Web执行沙箱与产物下载.md)／§3.1.1。）｜v1.6（2026-11-22 A3-5：C10 **压缩侧**接上——WASM 导出 `plan_c10_compress`（`EXTRA_PLAN_OPS`，与解压共用 C10 视图但入参事实不同）；Web 侧 `mode=compress` 用 `sources` 多文件槽、`sourceFileCount`/`sourceBytes` 必填，`targetExists` 恒 false；`archive-compress` 缺口枚举删除。视图档位不变 19／6。）｜v1.5（2026-11-22 A3-4：`native` 扩到 **19 个**——C07 清单同步接上：清单解析下沉 `kernel::c07::parse_manifest`（与 CLI/桌面同一实现）、Web 侧按字段名分槽（`manifest`／`sourceRoot` 两份独立输入）、字节型导出 `parse_c07_manifest`（`BYTE_OPS`，原始字节不可先过 `Blob.text()`）、清单编码 `source` 为必需入口；`unsupported` 收窄到 **6 个**（C04、C09、F10–F13）。）｜v1.4（2026-11-21 A3-3：`native` 扩到 18 个——C08 归档要求**选择整个目录**（相对路径的唯一来源，`webkitdirectory`），C10 解压按 ZIP 中央目录出计划、**压缩侧**（`mode=compress`）按 `archive-compress` 显式拒绝；`unsupported` 收窄到 7 个。）｜v1.3（2026-11-21 A3-2 追加 **计划能力分档**（ADR-0105，§2.1）：25 个计划入口按事实齐备度二分——**16 个 `native`**（C01–C03、F01–F09、C11–C14）走 `File`→事实→`plan_*`；**9 个 `unsupported`**（C04、F10–F13 缺目录树；C09 缺全量哈希；C07/C08/C10 待接线）在**调用前**抛 `E5001`，不产弱化计划；未选文件时的 mock 计划标「演示数据」。）｜v1.2（2026-11-21 A3-1 追加**本地通道的计划接线**（ADR-0104）：浏览器功能页以 File API 采集事实（§2），经 WASM `plan_*` 出真计划——落点 `web/src/lib/web/planChannel.ts`，C02 已通；Web 无 OPFS 执行器（§3.1）→ 真计划在界面显式禁用执行/撤销/预演/存模板，不以 mock 冒充执行。）｜v1.1（2026-10-27 评审；2026-10-28 增量修订，用户确认） ｜ 适用：web/、crates/kernel（WASM 面）、云端服务端（后置） ｜ 日期：2026-10-27
>
> 上游：[01-内核接口与IPC契约](01-内核接口与IPC契约.md)（命令名、IpcError、PlanDocument、Task 状态机沿用篇 01，不重复定义）。
> 权威：方案 §13.1（D9 双通道定型）、§13.2（能力边界）、§13.3（浏览器能力表）、§13.5（复用边界）、§13.6（性能）、§13.7（云端处理）、§13.9（不承诺项）、§13.10（阈值与选择器）。数值一律引用 §13.10.2 单点表，本文不设数值。

## 0. 权威来源索引

| 议题 | 权威章节 | 本文角色 |
|---|---|---|
| 双通道并存/锁定/默认本地 | §13.1 三条硬约束 | 落地为任务字段与传输流程 |
| 通道能力差异（写回/下载） | §13.2 | 约束 FsAdapter Web 实现边界 |
| OPFS/配额/下载 | §13.3 | 落地为配额命令与产物下载契约 |
| 内核复用、FsAdapter 不复用但同接口 | §13.5 | 落地为浏览器适配器映射表 |
| 主线程保护、WASM 开销 | §13.6 | 落地为 Worker 边界与事件要求 |
| 云端隔离/白名单/留存/日志边界 | §13.7 | 落地为云端 REST 草案与错误码 |
| 撤销仅本页产物、日志仅会话内 | §13.4 / §13.8 / §11.4-13 | 声明契约口径差异 |
| 模式选择器/体检/引导/配额自检 | §13.10.1–§13.10.5 | 对应命令与字段 |

## 1. 总原则

1. **同一命令面**：篇 01 的命令名与 DTO 在 Web 形态不变；通道差异收敛在传输层与 FsAdapter 实现，不新增并行命令体系（§13.5）。
2. **通道定型于任务**：`TaskSummary` 增加可选只读字段 `channel: "local" | "cloud"`（Web 形态必填、桌面省略），随任务创建锁定，契约层禁止迁移命令（§13.1 硬约束 2；字段已回写篇 01 §3.2）。
3. **默认本地**：会话初始化返回 `lastChoice` 供界面恢复"上次选择"，但 `systemDefault` 恒为 `local` 且不可被记忆覆盖（§13.1 硬约束 1）——该恢复逻辑属显示层，web/ 内不得产生放行/拦截语义（不变量 11）。
4. **零内嵌大 payload**：文件内容不进 JSON；一切数据引用走 `BlobRef` 句柄（§4）。

## 2. 数据引用与分块句柄

```ts
/** 一切文件内容的引用；plan/apply/progress 中的数据只以此出现 */
interface BlobRef {
  kind: "user-file" | "opfs" | "remote";
  ref: string;          // user-file: File API 会话内句柄 id；opfs: opfs://<bucket>/<path>；remote: 服务器对象 id
  size: number;         // 前置体检用（§13.10.3）
  mime: string;
  sha256?: string;      // 可选：小文件直读时计算，供去重/哈希类功能复用（C34）
}
```

- 分块是**传输实现细节**：`size` 上限、分块大小均引用 §13.10.2 配置项，不在 DTO 中固化；句柄消费方（kernel/WASM、服务端）只感知 `BlobRef`。
- `PlanEntry.source/target` 在 Web 形态使用 `blobref://<ref>` 字符串；桌面形态不出现该 scheme（篇 01 §2 契约的形态差异点，仅此一处）。

### 2.1 计划能力分档（A3-2，ADR-0105）

浏览器只能从 File API 采到「用户手动选中的文件」的字节/大小/mtime；目录树、目标占用、
与桌面同算法的全量哈希都采不到。缺这些事实时内核**不会报错**，它会照常产出一份结论
错误的计划（典型：C09 拿不到 `full_hash` → 得出「没有重复文件」）。

因此 Web 侧把 25 个计划入口分两档（`web/src/lib/web/planChannel.ts` 的 `webPlanSupport`）：

| 档位 | 视图 | 行为 |
|---|---|---|
| `native`（23 个） | C01–C04、**C07**、**C08**、**C10（解压＋压缩）**、F01–**F10**、**F12**、**F13**、C11–C14 | `File` → 事实（`facts.ts`）→ `plan_*`，与桌面同口径 |
| `unsupported`（2 个） | C09（缺**文件系统身份**：哈希可算，但 `plan_c09` 对身份未知的组整组排除——见 ADR-0115 同批纠正）；F11（缺「目标目录已有什么」；且「产物是空目录、浏览器无法交付」） | **调用前**抛 `E5001`，不准备事实、不调 WASM |

不设「有缺口但能算」的中间档：一旦允许，就要逐个解释偏差是否可接受，而计划文档
看起来完全一样、用户无从分辨。缺就拒绝——代价是功能少，收益是界面上的每份计划都是真的。

两个新增 `native` 各有**前提**，不是「能调就算」：

- **C08 归档**：相对路径只能从**目录选择**（`webkitdirectory` 的 `webkitRelativePath`）
  得到。普通多选时相对路径为空，此时拒绝（`E1001`）而**不**拿扁平文件名顶上——
  那会把「子目录里的 a.txt」当成「根目录下的 a.txt」，扩展名规则照样命中、产物路径却是错的。
  相对路径要剥掉浏览器自带的第一段（被选目录名），才与桌面 `collect_tree_files(source_root)`
  的 `rel_path` 同口径。
- **C10 解压**：表单的 `archive` 在浏览器里是**单文件选择器**（只有一个包才叫解压；
  桌面仍是路径输入）。条目事实来自 ZIP **中央目录**（[ADR-0103](../adr/0103-C10-Web侧条目清单零依赖中央目录读取.md)），
  只读元数据、不解压。`maxTotalBytes`/`maxRatio` 与 CLI/桌面同口径**必填**（替用户补默认值
  等于放行未知解压量）；密码只传「执行期会不会提供」这一位（ADR-0074）；
  `existingTargets` 不传（无 OPFS 列举），计划期不预判「目标已存在」——少一类冲突提示，
  不改变判定结论。

未选文件时仍走既有 mock（§6 的降级语义），但预览区必须标「演示数据」并说明如何得到
真计划：假计划可以存在，不能冒充真计划。

C10 的**压缩侧**（A3-5，[ADR-0107](../adr/0107-C10压缩侧接上与缺口枚举收敛.md)）单独接上，
不改视图档位（C10 本就 `native`），只是它不再只有解压一侧：

- **C10 压缩**：`mode=compress` 时输入槽换成 `sources`（多文件选择器）＋ `output`
  （输出包路径）。事实是「选中文件的相对路径 ＋ 逐个 size」，`sourceFileCount`／
  `sourceBytes` 为**必填**（缺省成 0 会产出「压缩 0 个文件、0 字节」这种看着正常、
  实际与所选文件无关的计划）。浏览器拿不到目录树，**整目录压缩不在能力内**；
  `targetExists` 恒 false（没有产物文件系统可查），计划不预判「目标已存在」。
  与解压同一条边界：**只出计划、不打包**——压缩执行臂要 `ArchiveService` 端口，Web 未装配（§3.1.1 执行分档）。

第三个新增 `native`（A3-4，[ADR-0106](../adr/0106-C07清单解析下沉内核与多路输入槽.md)）同样有前提：

- **C07 清单同步**：输入是**两份互不相干的东西**——一份清单 CSV ＋ 一个源目录，
  因此 Web 侧按字段名分槽（`manifest`／`sourceRoot`），不把两类文件混在一个数组里
  （混了只能靠扩展名猜谁是清单）。清单解析走 WASM 的 `parse_c07_manifest`：
  **原始字节**交给内核（`kernel::c07::parse_manifest`，与 CLI/桌面同一实现），
  因为编码检测必须在字节上做——`Blob.text()` 恒按 UTF-8 解，GB18030 清单会先被
  解成替换字符。与此配套：表单的 `source`（清单编码）是**必需入口**，中英混排的短
  清单会被统计检测猜成候选集外的编码（E3001），届时用户必须能指定 `gb18030`。
  - 边界：`targetOccupied` 恒 false——浏览器没有目标文件系统可查，计划因此**没有**
    「目标已存在」冲突提示（不影响条目与动作）。界面在目标根字段给浏览器专属提示。
  - 字节型导出与 JSON 型 `plan_*` 分开登记（`contract.ts` 的 `BYTE_OPS`），
    仍经同一 Worker 通道，`planHandler` 只认 `plan_` 前缀不受影响。

第四、五个新增 `native`（A3-11，[ADR-0113](../adr/0113-Web改名族执行面与目录第一层条目.md)）共用一套条目口径：

- **C04 名称替换 / F12 批量改名**：条目是目录的**第一层**名字（与桌面 `read_dir` 单层同口径，
  不递归）：根层文件 + 子目录名（相对路径的第一段，去重），键为 `根名/名字`。
  普通多选（无相对路径）在调 WASM 前 `E1001` 拒绝；两个目录一起选也拒绝（一次一个源根）。
  - 边界一：**空子目录看不到**（`webkitdirectory` 只给文件），符号链接与系统隐藏项同样拿不到
    —— 计划因此少那几条，界面在目录字段的浏览器专属提示里说明。
  - 边界二：`existingTargets` 恒空。浏览器的执行目标是会话沙箱，执行前本来就没有这些名字，
    所以「计划不报目标已存在」与执行环境一致，**不是**把计划打折。
  - 线协议：F12 的 `rule.scope` 是 `RenameScope` 枚举（`Full`/`Stem`/`Ext`），C04/F13 是字符串
    （`full`/`stem`/`ext`）——前端翻译一次；F12 的 `planId` 由调用方给（26 字符 ULID）。

## 3. 本地通道

### 3.1 FsAdapter 的浏览器实现（§13.5"同接口、不复用实现"）

> **A3-6 现状**：执行面落在**会话内内存沙箱**（§3.1.1，[ADR-0108](../adr/0108-Web执行沙箱与产物下载.md)）。下表「沙箱」列已落地；「OPFS」列是原设计口径，**尚未实现**，已降级为可选后续。两列都满足 §13.5「同接口、不复用实现」——`MemoryFs` 与桌面 `CliFsAdapter` 实现同一个 `FsAdapter` trait，执行器一行不改。

| 篇 01 §3 方法 | 沙箱实现（已落地） | OPFS 口径（设计，未实现） | 边界（§13.2/§13.3） |
|---|---|---|---|
| `read_bytes` / `read_ranges` | staged 字节切片 | File API slice 分块读 / OPFS read | 受内存建议档（§13.10.2）约束，超档**强提示后放行**（§13.6） |
| `write_atomic` | 临时键写入 + 一次替换，产物以字节交回主线程 | OPFS 临时名 + `move` 原子替换，对应 §4.3 | 产出**不落在用户磁盘**；交付只有下载 |
| `rename` / `move_path` / `copy` | 沙箱键空间内部 | 仅限 OPFS 内部对象 | **用户磁盘不可写**；用户文件只读引用 |
| `remove_to_quarantine` | `/.batchdesk-quarantine`（不变量 7 的 Web 等价实现） | OPFS 内隔离目录 | 会话结束即消失（§13.3 / §11.4-13） |
| `list_dir` | 按键前缀列举（只含 staged 输入与产物） | 目录授权句柄遍历 | 浏览器侧拿不到用户目录树（§13.9） |
| `stat` | 沙箱内 `mtime` 恒 `1970-01-01T00:00:00Z` | File API 元数据 | 沙箱内的 `mtime` 只服务一致性判定；用户文件的元数据仍走 File API，支撑前置体检（§13.10.3） |

- 桌面 `rename` 语义在 Web 由功能层降级实现（生成新名清单 + 打包下载，§13.4），FsAdapter 不提供"改用户磁盘文件名"的能力，从类型上杜绝误用。
- 判定规则对齐 §13.4：凡"直接改写用户磁盘"的功能，Web 侧 plan 的 `action` 产物一律导向 `download`。
- **沙箱不是模拟**：`generic_apply`／`apply_plan`／`undo` 跑的就是桌面那三个函数（不变量 2）——冲突二次裁决、目标路径保护、Journal 记录、反向执行全在；差别只在「文件系统」这一层是内存实现。

- **wasm32 没有 `std::time::SystemTime::now()`**（调用即 panic）：WASM 侧的计划 id 由 `epoch_millis()` 提供——wasm32 走 `js_sys::Date::now()`、其余走 `SystemTime`（ADR-0116）。这条路径覆盖全部 28 个 plan 入口，**native 测试与 vitest 都发现不了**，防线是 `web/e2e/browser-storage.spec.ts` 里真 WASM 的那条用例。

### 3.1.1 会话沙箱执行（A3-6）

浏览器给不了内核一个**同步**的文件系统（File API 全异步，OPFS 也是异步 API），而内核执行器是同步的。解法是把异步部分**全部前移**，执行时全同步：

```text
stage_inputs（读 File → 清单 [{path,length}] + 该文件字节 → web_exec_stage；逐文件一次）
        ↓ 沙箱内已是同步文件系统
apply_plan（web_exec_apply → generic_apply / apply_plan，同步）
        ↓
web_exec_product（逐产物取字节 → 主线程 Blob → 下载）
undo_plan（web_exec_undo → 按 Journal 反向执行，回滚沙箱内产物）
reset_sandbox（换输入 / 换视图时清空）
```

| op（`EXEC_OPS`） | WASM 导出 | 说明 |
|---|---|---|
| `stage_inputs` | `web_exec_stage` | 清单长度与字节数不符 → `E1002`（不静默截断）；**逐文件调用**，每次清单一项（ADR-0118） |
| `apply_plan` | `web_exec_apply` | 返回 `{outcome, products:[{path,size}]}`；`C09_KIND` 走 `apply_plan`，其余 `apply_generic_plan` |
| `undo_plan` | `web_exec_undo` | 返回 `{reverted, failed}`；回滚后 Journal 保留（条目标记已回滚） |
| `reset_sandbox` | `web_exec_reset` | 清空沙箱与 Journal |
| —（取值器，不在 `EXEC_OPS`） | `web_exec_product` | 单个产物的字节；它不是「操作」，是取值 |

- **产物＝Journal forward 的落盘路径**：`WriteAtomic{path}`／`Rename{to}`／`Copy{to}`，按 seq 去重后用 `stat` 确认仍是文件。不按计划条目猜路径——猜出来的路径撤销时对不上。
- **执行能力按「动作需要什么」分档**（A3-7～A3-12，[ADR-0109](../adr/0109-Web执行面扩到九视图与改写器端口在沙箱内构造.md)／[ADR-0110](../adr/0110-Web计划输出路径派生与CSV族执行面.md)／[ADR-0111](../adr/0111-Web执行面按相对路径登记输入与C07-C08接入.md)／[ADR-0112](../adr/0112-C10归档执行面与WASM内ArchiveService.md)／[ADR-0113](../adr/0113-Web改名族执行面与目录第一层条目.md)／[ADR-0114](../adr/0114-F10目录清单执行面与浏览器目录事实.md)）：已接 **23 个视图**（19 个原有 ＋ C04／F12／F10／F13），按动作族分七类，全部只需 `FsAdapter` ＋ 授权（沙箱注入 `AlwaysAllowLicense`）：
  - **读写**：F01–F03（`split`）、F04／F06（`merge`，F06 带 `csvMerge` 载荷）、F05（`split` ＋ `csv` 载荷）；
  - **改写器**：C01/C02/C03（`edit-content`，`TranscodeRewriter`／`LineEndingRewriter`／`ReplaceRewriter` **都在 kernel 里**，沙箱按 `plan.kind` 用 `from_plan` 构造后交给执行器）；
  - **CSV 行列变换**：F07／F08／F09（`csv-extract`／`csv-rows`）、C11–C14（`csv-dedup`／`csv-filter`／`csv-sort`／`csv-columns`）——动作同样只要读写，A3-8 之前跑不了是**计划侧**的输出路径缺陷（下一条）。
  - **按相对路径归档/同步**：C07（清单驱动 move/copy）、C08（规则归档 move）——动作是 move，但源带目录层级（下下条）。
  - **归档编解码**：C10 解压/压缩（`extract`／`archive`）——执行臂要 `ArchiveService` 端口，Web 侧由 `MemoryArchiveService` 提供（WASM 内真 ZIP 读写，与桌面同一个 `zip` crate、同一份名字解码、同一套错误码；ADR-0112）。解密密码只随本次 `apply` 请求进来（`ApplyRequest.password`），计划/Journal/沙箱都不留；
  - **按相对路径改名**：C04（`rename`，名称替换）、F12（`rename`，前缀/后缀/查找替换）、F13（`rename`，正则＋捕获模板）——条目是目录第一层，但 staging 要登记**全部选中的文件**：目录条目的 rename 会搬走整棵子树，子树不进沙箱就只剩空目录（ADR-0113）。产物是改名后的文件，可下载、可撤销。
  - **只读元数据的清单**：F10（`listing`）——执行臂要 `DirectoryLister` 端口，沙箱内由 `MemoryDirectoryLister` 读 `stage_listing` 登记的目录事实（来自 `webkitRelativePath`）按桌面同口径写 BOM CSV；`stage_dirs` 只建根目录供预检 `stat`，不建假的子目录树。不登记文件字节：清单不读内容（ADR-0114）。
- **目标路径由 wire 层按表单输出目录派生**（A3-8）：七个 CSV 计划入口接受 `outputDir`，目标＝`输出目录 + 源文件名`；空/缺省＝沙箱默认 `/out`。**不得**回落到源文件路径：`target == source` 时 `skip` 会自我跳过（成功但零产物）、`overwrite` 等于原地覆盖源。目录口径与 `MemoryFs` 键空间一致（反斜杠换正斜杠、去尾斜杠、补前导斜杠），**路径归一只有这一处**——前端不再自己拼目标路径。
- **staging 键按计划的源口径登记**（A3-9）：沙箱是内存文件系统，执行器只按计划里的 `source` 字符串找文件，键差一个字就是 `E2001`。目录选择类视图（C07/C08）在浏览器里以**所选目录名**为 `sourceRoot`（该字段是目录选择器，没有文本值），计划源是 `根名/相对路径`，staging 键必须用同一规则拼；普通多选视图的源事实是 `displayPath(file.name)`，键就是裸文件名。C07 的清单文件只参与计划（`parse_c07_manifest` 已把行解析进内存），**不登记**进沙箱。
- **C10 的两个输入槽口径不同**（A3-10）：解压的源是包自己的文件名（`archive` 槽），压缩的源是 `pickedRelativePath(source) || source.name`（`sources` 槽）——`stagedPairs` 因此把槽名带着走。压缩条目名与桌面同口径：**文件源按文件名入包**，目录源才保留层级；Web 只能选文件，所以浏览器产出的包不含目录条目，重名按 `E1002` 拒绝。
- **执行档位只剩未知视图的兜底**：`webApplySupport` 对任何未登记的视图返回 `{ level: "unsupported", gap: "not-wired" }`（界面统一显示 `feature.execute.webOnly`）。23 个已接视图全部 `native`，不再有按缺口细分的档位——`port-missing`／`relative-paths`／`target-in-place` 三档都随对应实现落地而删除（枚举不留永不出现的值）。
- 界面依据档位**在按钮之前**禁用并说明缺口，不在执行到一半才报端口缺失；未接线的视图不会得到「尽力执行」的半成品。
- **撤销范围 `products-only`**（§3.4）：沙箱内的写入可回滚，用户已下载的副本不可。
- **§13.10.2 建议档只提示不阻断**：执行前用 `File.size` 统计（不读内容），单文件 > 100 MB 或总量 > 500 MB 弹确认，确认后放行；数值只写在 `web/src/lib/web/applyChannel.ts` 的两个常量里。这是建议不是配额（零强制力，设计 05 §2）。
- **零强制力**：`web_exec_*` 五个导出不含授权 / 配额 / 试用期判定入口（设计 05 §2 白名单）。

### 3.2 Worker 与事件（§13.6）

- WASM 内核、解析、哈希运行于 Web Worker；主线程仅渲染与转发事件。
- 篇 01 事件（`task://progress` 等）经 Worker → 主线程 → 框架事件桥接，事件名与字段不变；任何超一屏等待的操作界面必须具备骨架屏与取消入口（§13.6，属前端实现约束）。

### 3.3 本地通道新增命令

| 命令 | 入参 | 出参 | 依据 |
|---|---|---|---|
| `registerUserFiles` | `{handles: FileSystemFileHandle[]}` | `BlobRef[]`（会话内句柄登记） | §13.3 File API |
| `requestDirectoryAccess` | — | `DirGrant \| null`（用户拒绝返回 null，不报错） | §13.3 目录授权 |
| `getStorageQuota` | — | `{usage, quota}`（`navigator.storage.estimate()` 原样透出） | §13.10.5 配额自检 |
| `downloadProducts` | `{planId}` | `void`（触发浏览器下载清单，ZIP 打包重命名类产物） | §13.2 产出只下载、§13.4 降级口径 |
| `getChannelDefaults` | — | `{systemDefault: "local", cloudAvailable: boolean, lastChoice?: Channel}` | §13.1 / §13.10.1 / §6.3 降级 |

- `type Channel = "local" | "cloud"`（§1 总原则 2 的任务字段同型）；`cloudAvailable: false` 时云端选项置灰但保留能力预期说明（§13.1 硬约束 3、§6.3），`lastChoice` 恢复属显示层逻辑（总原则 3）。
- `type DirGrant = { grantId: string; displayName?: string };` —— 浏览器适配层的会话内目录授权句柄，不落盘、不跨会话复用。
- **F15-lite 不做 Web**；阶段 2 若开放云端 AI，只走云端通道、仍零强制力（05 §3/§4），并与设计 14 对齐。

### 3.4 撤销与日志的 Web 口径（§13.4 / §13.8 / §11.4-13）

- `undoPlan` 仅作用于**本页产物**（沙箱内对象，§3.1.1）；对用户源文件的引用无从回滚，契约层在 `UndoablePage` 条目上以 `scope: "products-only"` 显式标注，界面须同步声明"不等同桌面版留存/回滚"（§13.8 一致性约束）。**已下载到用户磁盘的副本浏览器收不回来**——这不是缺陷，是平台事实；产物面板必须写明「撤销只回滚浏览器内产物」。
- **OPFS 落盘是执行之后的异步搬运**（A3-13，[ADR-0115](../adr/0115-Web产物执行后搬运与OPFS落盘.md)）：`FsAdapter` 保持同步（设计 04），产物写 `batchdesk/products/<planId>/<名字>`（文件名与下载共用 `productFileNames`）。它与下载并列、互不替代：下载给用户本机，OPFS 留一份在浏览器里。**已写进 OPFS 的副本同样不随撤销消失**（与已下载副本同理），界面给独立「清除浏览器中的产物」入口并如实写明。落盘失败一律静默降级（`opfsSupported()` 为假、配额满、单文件写失败），不阻断执行、不改计划、不参与授权或配额判定（零强制力，设计 05）。读回入口是**设置页的「浏览器存储」卡片**（A3-14）：列出 `batchdesk/products/*/*`、单件重新下载、整体清理；下载复用与执行面板同一份保存器。
- 操作日志仅会话内留存 + 导出（§11.4-13）；`listLogs` 返回 `ephemeral: true`，禁止写持久化存储。

## 4. 云端通道（服务端可后置，接口先冻结）

### 4.1 传输流程（§13.7）

```text
前置体检(§13.10.3) → createUploadSession（服务端校验白名单+建沙箱）
  → uploadChunk*（分块、可断点续传，块序号+校验和；断线恢复时先 GET session 查缺失块）
  → completeUpload → 服务端执行（任务级沙箱，§13.7 隔离）
  → pollTask / task://progress（与本地同一事件语义）
  → downloadResult（结果保留至下载完成或 §13.7 的到期兜底，先到为准）
  → 任务终态；输入与中间产物服务端即时删除
```

- 强制 HTTPS、证书校验不可关闭（§13.7）；HTTP 明文一律在客户端拒绝（E5007）。
- 双重白名单校验：客户端预检（体验，非安全边界）＋服务端强制（扩展名+魔数，§13.7）。浏览器侧校验**不是**强制力（不变量 11），拒绝与否以服务端返回为准。

### 4.2 云端 REST 草案（本文新增，评审冻结后生效）

| 端点 | 方法 | 说明 |
|---|---|---|
| `/v1/sessions` | POST | 建 `uploadSessionId`；入参：文件元数据（名不上服务端日志，§13.7）、大小、类型、任务 kind 与参数快照；返回配额判定（QuotaBased，§13.8） |
| `/v1/sessions/{id}` | GET | 断点续传查询：返回 `expiresAt`、`receivedChunks` 与 `totalChunks`；过期/失效返回 E5005（对应 ADR-0006 P2） |
| `/v1/sessions/{id}/chunks` | PUT | 分块上传；入参：`index, checksum`；幂等重放安全 |
| `/v1/sessions/{id}/complete` | POST | 结算校验和 → 返回 `taskId`（对应 §13.7 沙箱启动） |
| `/v1/tasks/{id}` | GET | 轮询任务状态与 ApplyReport（结构与篇 01 §5/§6 同构） |
| `/v1/tasks/{id}/result` | GET | 下载结果；`410 Gone` = 已到期删除（结果码可查 `resultCode`，§13.7 仅留结果码） |
| `/v1/tasks/{id}/cancel` | POST | 中止执行中的云端任务 → 服务端中止沙箱并即时清理（取消映射，见 §4.3） |
| `/v1/tasks/{id}` | DELETE | 用户主动放弃 → 服务端即时清理并销毁沙箱（§13.7；终态后不可再取结果，亦即云端"撤销"的落点） |

**请求/响应 schema**（错误响应统一为 `{ error: IpcError }`，`IpcError` 结构与码位沿用篇 01 §2.1/§7；HTTP 状态码与 `IpcError.code` 并用，本文新增码位见 §5）：

```ts
// POST /v1/sessions
// 请求：
{ fileName: string;        // 仅用于白名单校验（扩展名+魔数），服务端日志不落名（§13.7）
  size: number; mime: string;
  kind: OperationKind;     // 篇 01 §3.2 冻结值
  params: Record<string, unknown>;  // 参数快照，与篇 01 PlanDocument.params 同构；服务端按 kind+params 执行
  totalChunks: number; chunkSize: number }  // 二值均引用 §13.10.2 配置项，服务端复核
// 201 响应：
{ uploadSessionId: string; expiresAt: string }   // 过期后重建会话（E5005）
// 拒绝：403 E1006（白名单外）；429 E4004（配额耗尽，QuotaBased）

// GET /v1/sessions/{id}
// 200 响应：{ uploadSessionId: string; expiresAt: string; receivedChunks: number[]; totalChunks: number }
// 410 响应：{ error: IpcError(E5005) }（会话过期/失效；客户端重建会话并按缺失块续传）

// PUT /v1/sessions/{id}/chunks
// 请求体：二进制分块；头：X-Chunk-Index、X-Chunk-Checksum（sha256，规格见 ADR-0006）
// 200 响应：{ received: number }（累计已收块数）；422 E5012（仅该块作废、会话保留，重传该块）

// POST /v1/sessions/{id}/complete
// 请求：无体（会话内元数据已齐）；201 响应：{ taskId: string; state: TaskState }

// GET /v1/tasks/{id}
// 200 响应：{ summary: TaskSummary; report?: ApplyReport; resultCode?: string }
//          TaskSummary/ApplyReport 结构沿用篇 01 §3.2/§5，不重复定义；轮询间隔引用 §13.10.2

// GET /v1/tasks/{id}/result
// 200 响应：结果二进制流（带 Content-Length 与 ETag 供续传校验）；410 → { error: IpcError(E5006) }

// POST /v1/tasks/{id}/cancel
// 202 响应：{ taskId: string; state: "cancelled" }
// 语义：云端无 Journal（沙箱内即时删除，§13.7），取消＝中止＋清理，不产生回滚；
//       与篇 01 §6 本地 cancelled（Journal 反向回滚后终态）是刻意差异，界面文案不得声称"已恢复原状"

// DELETE /v1/tasks/{id}
// 204 响应：无体；沙箱销毁、结果不可再取（再取即 E5006）
```

- 认证/配额按 §8.1 功能位图与 §11.4-12 账号取舍走，具体身份方案不在本篇（服务端契约后置细化时另立）。
- 服务端日志仅含任务 ID、大小、类型、耗时、结果码（§13.7），对齐 §9.7；本契约所有云端入参不得要求服务器记录文件名/内容。

### 4.3 云端与本地契约的差异点汇总

| 项 | 本地 | 云端 |
|---|---|---|
| BlobRef kind | `user-file` / `opfs` | `remote` |
| 进度来源 | Worker 内核推送 | 轮询 `/v1/tasks/{id}` 汇入同一 `task://progress` |
| 撤销 scope | `products-only`（会话沙箱，§3.1.1；已下载副本不可回滚） | `products-only`（服务端结果，下载前可撤）——**落点＝DELETE `/v1/tasks/{id}`**，服务端无 Journal、无回滚 |
| 取消（cancelTask） | 篇 01 §4.1，已执行部分按 Journal 反向回滚 | `POST /v1/tasks/{id}/cancel`：中止沙箱＋即时清理，不回滚（无本地副作用） |
| 断网 | 可用（§13.2） | 不可用：任务创建前置检查，失败 E5008 |
| 产物落地 | `downloadProducts` | `/result` 直接下载 |

## 5. 错误码位（延续篇 01 §7，仅追加）

| 码位 | 域 | 语义 | retryable |
|---|---|---|---|
| E1006 | 输入 | 类型白名单外（扩展名+魔数双校验失败，§13.7） | false |
| E5004 | 环境 | 云端任务超限（CPU/墙钟/内存，取值引用 §13.10.2 配置项） | false |
| E5005 | 环境 | 上传会话过期/失效 | false（重建会话） |
| E5006 | 环境 | 结果已到期删除（对应 HTTP 410） | false |
| E5007 | 环境 | 不安全传输（明文回退被拒） | false |
| E5008 | 环境 | 网络不可达/断网（云端通道前置检查） | true |
| E5012 | 环境 | 上传分块校验失败：仅该块作废、会话保留，客户端重传该块；验收性质见 [ADR-0006](../adr/0006-云端上传协议自研分块.md) P3 | true |

## 6. 预置与降级策略（§13.9"不做能力假设"）

1. 启动时运行**能力探测**（WASM、OPFS、目录授权、File System Access、Worker），结果汇总为 `BrowserCapabilities`；功能可用性由内核按能力决定，前端只负责显示/隐藏（不变量 11）。
2. 探测失败的功能项显示降级提示并引导桌面客户端（§13.10.4 超限引导卡复用此入口）。
3. 云端通道未上线时按 §13.1 硬约束 3 处理：置灰但保留能力预期说明，不得死链；契约层表现为 `getChannelDefaults().cloudAvailable: false`。

## 7. 测试锚点与待决

- [x] E02/R05 类语料（T v1.12 §3.2）Web 口径 → 已拍板（用户确认 2026-10-27）：占位文件/UNC 属桌面专属（§13.4/§13.9），Web 语料**不承接**此两条；测试方案补篇建立时直接引用本决策
- [x] 分块协议是否采用现成断点续传规范（如 tus）或自研 → 已定，见 [ADR-0006](../adr/0006-云端上传协议自研分块.md)：自研轻量分块（本篇 §4.2 规格冻结不变），排除 tus 的理由（互操作价值为零 + 与任务级沙箱耦合）与迁移退路（传输层单点替换，命令契约不动）均在该 ADR；已采纳（2026-10-27）
- [x] 云端身份与配额服务端完整契约（对齐 §8.1/§11.4-12）→ 已拍板：后置独立篇（云端服务端可后置，§13.1；身份方案依赖 §11.4-12 产品取舍）
- [x] `BrowserCapabilities` 探测项清单与降级映射表 → 已拍板：实现期细化（依赖真实浏览器差异实测，不做纸面清单）
- [x] 本轮补齐（2026-10-28）：REST 请求/响应 schema（§4.2）、`cancel` 端点与 cancelTask 云端映射、云端"撤销"落点＝DELETE 任务（§4.3）、`getChannelDefaults` 补 `cloudAvailable` 并补 `Channel` 类型定义（§3.3，消除与 §6.3 的不一致）

## 8. 修订记录

| 版本 | 日期 | 变更 |
|---|---|---|
| v1.15 | 2026-11-23 | A3-15：**WASM 时间源修复（P0）**——`ulid_like_id()` 的 `SystemTime::now()` 在 wasm32 上 panic，28 个 plan 入口共用它；改为 `epoch_millis()`（wasm32 → `js_sys::Date::now()`，其余 → `SystemTime`），`js-sys` 只在 wasm32 target 引入（ADR-0116）。防线＝`e2e/browser-storage.spec.ts` 的真 WASM 用例（真实 Chromium 选文件→计划→执行→OPFS 落盘），并说明为何不挂语料（语料跑在 native）。同批补 OPFS 读回闭环的设置页入口（A3-14，见 v1.14 行）。 |
| v1.14 | 2026-11-23 | A3-13：① C09 归因纠正（ADR-0115 同批）——`WebPlanGap` 的 `content-hash` → `file-identity`；哈希可算（blake3 已是 wasm 依赖、`plan_c09` 已导出），缺的是 `FileIdentity`，`plan_c09` 对身份未知组整组排除，直连会得 0 条目假绿计划。② 产物**执行后搬运**到 OPFS（ADR-0115）：新增 `opfsStore.ts`（落盘／列出／读回／清理，全部静默降级），`productFileNames` 与下载共用去重算法；`FsAdapter` 同步契约零改动（异步后移，与 staging 前移对称）；撤销不收走 OPFS 副本，界面给独立清理入口；A3-14 补读回闭环：设置页「浏览器存储」卡片列出/单件下载/整体清理。 |
| v1.13 | 2026-11-22 | A3-12：F10 接上计划与执行面（ADR-0114）——执行动作是 `listing`、缺的是 `DirectoryLister` 端口而非「目录树」（沿用 A3-9 的归因教训）；端口由 `MemoryDirectoryLister` 在 WASM 内读浏览器注入的目录事实（`webkitRelativePath`）按桌面同口径写 BOM CSV；`MemoryFs::insert_dir` 支撑 `stage_dirs` 只建根目录供预检 `stat`，不建假子树；`stage_dirs`／`stage_listing` 两个新 op + 两个新导出；`listingTrees` 从文件路径一次推出摘要与清单行（计划/执行共用口径）；界面 `webDirectory` 让桌面保留多行 textarea、浏览器改目录选择。档位面 21 → **22 个视图**（计划与执行同步），`unsupported` 4 → 3（去 F10）。 |
| v1.12 | 2026-11-22 | A3-11：C04/F12 接上计划与执行面（ADR-0113）——条目口径＝目录**第一层**（根层文件＋子目录名，不递归，与桌面 `read_dir` 同口径），键为 `根名/名字`；staging 登记**全部选中的文件**（目录条目的 rename 会搬走整棵子树）；`existingTargets` 恒空（沙箱执行前为空，计划与执行环境一致）；`WebPlanF12Input` 补 `camelCase`、F12 的 `planId`由前端生成 ULID、`scope` 在 C04/F13（小写字符串）与 F12（`RenameScope` 枚举）之间翻译一次；档位面 19 → **21 个视图**（计划与执行两侧同步），档位表删除前端无视图入口的 F13。 |
| v1 | 2026-10-27 | 首版冻结（用户评审确认）。 |
| v1.1 | 2026-10-28 | 明确 `TaskSummary.channel` 并回写篇 01；补 `DirGrant` 定义；补 `GET /v1/sessions/{id}` 断点查询；结果保留时长改为引用 §13.7；与篇 01 的 `scope`/`ephemeral` 字段对齐。 |
| v1.2 | 2026-11-21 | 追加本地通道的**计划接线**（ADR-0104）：`File`→事实→`plan_*`，C02 纵切；真计划显式禁用执行链。 |
| v1.3 | 2026-11-21 | 追加**计划能力分档**（ADR-0105，§2.1）：16 个 `native`／9 个 `unsupported`，不支持者调用前拒绝；mock 计划标「演示数据」。 |
| v1.4 | 2026-11-21 | A3-3：C08（目录选择→相对路径）与 C10（中央目录→条目事实）转为 `native`（18 个）；C10 压缩侧按 `archive-compress` 拒绝；`unsupported` 收窄到 7 个。 |
| v1.5 | 2026-11-22 | A3-4：C07 转为 `native`（19 个／6 个）——清单解析下沉 `kernel::c07::parse_manifest`（三形态同一实现）、Web 侧按字段名分槽、字节型导出 `parse_c07_manifest`（`BYTE_OPS`）、清单编码 `source` 为必需入口；`targetOccupied` 恒 false 记为能力边界。 |
| v1.6 | 2026-11-22 | A3-5：C10 **压缩侧**接上（`plan_c10_compress`，ADR-0107）——视图档位不变（19 个），`mode=compress` 用 `sources` 多文件槽＋`sourceFileCount`/`sourceBytes` 必填事实；`archive-compress` 缺口枚举删除。 |
| v1.7 | 2026-11-22 | A3-6：Web **执行面**接上（ADR-0108，§3.1.1）——`MemoryFs`（`FsAdapter` 第三个实现）＋内存 Journal，`EXEC_OPS` 4 个 op／5 个导出；`stage_inputs` 先登记字节、`apply_plan` 同步执行、产物从 Journal forward 取路径后下载；执行能力按**端口齐备度**分档（仅 F01/F02/F03，其余 `not-wired` 按钮前禁用）；撤销 `products-only`；§13.10.2 建议档提示不阻断；OPFS 路线降级为可选后续。 |
| v1.8 | 2026-11-22 | A3-7：执行面扩到 **9 个视图**（ADR-0109）——C01/C02/C03 的改写器在沙箱内按 `plan.kind` 构造（`from_plan`，与 CLI/桌面同一实现）；F04–F06 的 `merge`／`split` 动作无需额外端口，实测可用；执行档位判据改为「动作需要什么」；未接的 10 个视图按 `target-in-place`（F07–F09／C11–C14，计划输出指向源文件）／`relative-paths`（C07）／`port-missing`（F10／C08／C10）分类拒绝。 |
| v1.9 | 2026-11-22 | A3-8：CSV 行列变换族接上执行面（ADR-0110）——`plan_f07`／`plan_f08`／`plan_f09`／`plan_c11`–`plan_c14` 的线协议增可选 `outputDir`，目标路径＝输出目录＋源文件名（空/缺省 `/out`，反斜杠/尾斜杠/前导斜杠归一），**不再回落源文件**（旧行为下 `skip` 自我跳过、`overwrite` 原地覆盖）；前端 7 个 mapper 补传 `outputDir`；执行档位 9 → 16 个视图，`WebApplyGap` 删 `target-in-place`。 |
| v1.11 | 2026-11-22 | A3-10：C10 接上执行面（ADR-0112）——`crates/wasm/src/archive_mem.rs` 用同一个 `zip` crate 在 `MemoryFs` 上实现 `ArchiveService`（inspect 只读中央目录、extract 按条目解、compress 按源打包）；`decode_entry_name` 从 platform 下沉 kernel 供两端共用；`ApplyRequest` 增执行期 `password`（不进计划/Journal/沙箱），缺密码整单 E3008；沙箱按 `plan.kind == c10` 走 `apply_archive_plan`；前端 `stagedPairs` 按槽分口径（`archive`＝文件名、`sources`＝相对路径）；执行档位 18 → **19 个视图**，`WebApplyGap` 只剩 `not-wired`。 |
| v1.10 | 2026-11-22 | A3-9：C07/C08 接上执行面（ADR-0111）——**staging 键按计划的源口径登记**：目录选择类视图（C07/C08）拼「根名/相对路径」，普通多选仍裸文件名；C07 只登记 `sourceRoot` 槽（清单只参与计划、不进沙箱）；无相对路径时在调 WASM 前 `E1001` 拒绝；`webStagedFiles` 同时供界面算 §13.10.2 建议档；执行档位 16 → **18 个视图**，`WebApplyGap` 删 `relative-paths` 与 F10 死条目（F10 计划本身不支持），只剩 C10 `port-missing`。 |
| v1.16 | 2026-11-24 | A3-17：**F13 正则改名接入 UI**（ADR-0117）——F13 从「只有内核与 CLI」补到桌面与 Web：壳层新增 `plan_f13_shell`（与 CLI `f13` 同口径采集：目录第一层、跳符号链接、按名排序），前端补导航/字段/双语/`plan_f13` 映射与执行档位；计划面与执行面各 22 → **23 个视图**，`unsupported` 3 → 2（只剩 C09／F11）；C05 对照预览扩到 F13（内核 `C05_PREVIEW_KINDS` 本就含它）。同批把 F12/F13 的目录采集提成共用实现，修掉 F12 壳层「不跳链接、不排序」与 CLI 不一致的既有偏差。 |
| v1.17 | 2026-11-24 | A3-18：**staging 的内存峰值收口**（ADR-0118）——旧实现把所有文件拼成一块再交 Worker：主线程峰值≈总量×2（拼接＋结构化克隆），再加 wasm 内的按值拷贝，500MB 建议档下会到 GB 级，与 §13.10.2 的建议档不自洽。改为**逐文件 `stage_inputs`**（峰值只与最大单文件相关）、字节用 **transferable** 转移（不再克隆）、**先把所有沙箱键算完再登记**（缺相对路径的 `E1001` 不会留下半批输入）。同批修 `clearPersisted`：只有 `NotFoundError` 视为「已是目标状态」，其它失败如实返回 `false`。 |
