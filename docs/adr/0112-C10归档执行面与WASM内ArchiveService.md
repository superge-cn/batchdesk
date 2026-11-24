# 0112. C10 归档执行面：WASM 内的 ArchiveService 与执行期密码

- 日期：2026-11-22
- 状态：**已采纳**（A3-10 落地：`MemoryArchiveService` 在 `MemoryFs` 上实现 `ArchiveService`，沙箱按 `plan.kind == c10` 走 `apply_archive_plan`；执行档位 18 → **19 个视图**，`WebApplyGap` 只剩 `not-wired`）
- 相关：[ADR-0111](./0111-Web执行面按相对路径登记输入与C07-C08接入.md)（上一批把 C10 记为最后的 `port-missing`）、[ADR-0108](./0108-Web执行沙箱与产物下载.md)（沙箱执行与产物下载）、[ADR-0074](./0074-C10密码包的秘密注入面.md)（密码只在执行期）、[ADR-0103](./0103-C10-Web侧条目清单零依赖中央目录读取.md)（计划侧的中央目录读取）、设计 02 §3.1.1

## 背景

A3-9 收口时执行档位是 18／19，唯一剩下的是 C10（归档/解压）。缺的确实是端口：

- `ArchiveService` 的三个方法（`inspect`／`extract_entry`／`compress`）都要按**路径**读文件；
- 桌面实现 `platform::ZipArchiveService` 建在 `std::fs` 上（`fs::File::open`），而 `platform` 依赖 `rusqlite`／`windows-sys`，根本编不到 `wasm32-unknown-unknown`；
- 浏览器沙箱里的「文件」只是 `MemoryFs` 里的字节，没有可供 zip 库打开的文件句柄。

于是 C10 在 Web 上一直是「有计划、不能执行」：计划侧的条目事实来自 TypeScript 的中央目录读取（ADR-0103），执行侧一步也走不了。

## 决策

### 1. 端口实现放在 wasm crate，与 platform 对称

`crates/wasm/src/archive_mem.rs` 提供 `MemoryArchiveService<'a> { fs: &'a MemoryFs }`，把包整体读进 `Cursor` 后交给 `zip` 库。角色与桌面侧对称：`platform` 是「桌面壳层适配」，`wasm` 是「浏览器壳层适配」，两者都实现 kernel 定义的端口。

**不放进 kernel**：kernel 是纯函数层（不变量 1），ZIP 编解码属于适配层；把 `zip` 塞进 kernel 会让 CLI/桌面/WASM 三个消费者都被动依赖一个约 2 MB 的编解码栈。

### 2. 与桌面同一个 crate、同一套错误码、同一份名字解码

| 维度 | 桌面 | 浏览器 |
|---|---|---|
| 库 | `zip 9.1.3`（`deflate-flate2` + `aes-crypto`，miniz_oxide 纯 Rust 后端） | 同版本同特性 |
| 名字解码 | `kernel::archive::decode_entry_name`（本次从 platform 下沉） | 同一函数 |
| 错误码 | E3008 缺密码／E3009 密码错／E3003 读不了 | 同一映射 |
| 密码注入 | `ArchiveApplyContext.password` | 同一入口，值来自 `ApplyRequest.password` |

`decode_entry_name` 是纯函数（UTF-8 旗标优先、否则 GB18030 回退），下沉到 kernel 后两个端口实现共用——**同一份计划在两端必须解出同一个名字**，各写一遍就迟早分叉。`zip_error` 仍各留一份：桌面那份要把 `ZipError::Io` 里的网络断线保留成 E2005（让执行器的重试端口接住），而内存游标不会产生网络错误。

**可行性已实测**：`cargo check -p batchdesk-wasm --target wasm32-unknown-unknown` 在加入 `zip` 后通过；纯 Rust 后端（miniz_oxide／aes／sha1／pbkdf2）不需要 C 工具链。代价是 debug 产物 10.8 MB → 12.5 MB（发行构建另行剥离，见 ADR-0002）。

### 3. 执行期密码：随请求进来，不落任何持久面

`ApplyRequest` 增 `password`（可选）。它只在这一跳存在：计划的参数里只有 `passwordProvided` 这个**事实**（ADR-0074），Journal 不记密码，沙箱不缓存密码，`Sandbox::reset` 也不需要清理它。

计划说「执行期会提供密码」而请求没带时，执行器**整单**拒绝（E3008），不进条目循环——不消耗授权、不写 Journal、不留半截产物。这条在内核里已有，本次只是把 Web 的注入面接上。

### 4. 沙箱路由：`c10` 走归档执行器，其余照旧

```rust
let outcome = if plan.kind.as_str() == C10_KIND {
    let archive = MemoryArchiveService::new(&self.fs);
    let mut context = ArchiveApplyContext { fs: &self.fs, archive: &archive, .. };
    apply_archive_plan(&plan, &options, &mut context)?
} else if plan.kind.as_str() == C09_KIND { /* 专用执行器 */ } else { /* 通用执行器 */ }
```

`fs` 与 `archive` 借用同一个 `MemoryFs`（共享借用），`journal` 是另一块字段的可变借用——不需要 `Rc`／内部可变性包装。

### 5. 产物与撤销仍从 Journal 取

解压的每条文件都是 `WriteAtomic{path}`、目录是 `CreateDir{path}`，压缩是 `WriteAtomic{zip 路径}`。因此：

- 产物清单＝`Journal` forward 里的落盘路径（**能下载什么＝撤销能回滚什么**，ADR-0108 的同一条口径）；
- 撤销把解出来的文件搬进隔离目录，与桌面同一套 `undo_plan_with_ports`。

### 6. 前端 staging：C10 两槽两种口径

C10 的两个输入槽在计划里口径不同，staging 必须逐槽处理：

| 槽 | 计划的源 | staging 键 |
|---|---|---|
| `archive`（解压） | `trimmed(form,"archive") ?? archive.name` | 文件名 |
| `sources`（压缩） | `pickedRelativePath(source) \|\| source.name` | 相对路径（不含根名） |

`stagedPairs` 因此把槽名带着走；`webStagedFiles` 继续供界面算 §13.10.2 建议档，与执行共用一份筛选。

**压缩条目名与桌面同口径**：文件源按**文件名**入包（`platform::ZipArchiveService::compress` 就是这么做的），目录源才保留层级。Web 压缩只能选文件（拿不到目录树），所以浏览器产出的包不含目录条目；同名文件会被执行器按 `E1002` 拒绝——与桌面同一行为，不另立规则。

### 7. 档位表收口：19 native，`WebApplyGap` 只剩 `not-wired`

`port-missing` 一档随本次实现删除（同一纪律：枚举里不留永不出现的值）。未知视图仍按 `not-wired` 保守拒绝——将来加视图忘了接线时，界面会如实说「未接线」而不是猜成能执行。

## 被否的方案

- **在 TypeScript 里解压**：`apply` 必须是内核代码（不变量 2），前端解压等于把执行语义搬到表现层，计划与执行的冲突裁决、Journal、撤销全要重写一遍。
- **把 `zip` 加进 kernel、让 platform 与 wasm 都调 kernel 的字节级实现**：kernel 会被迫依赖编解码栈（含 AES），而 kernel 的定位是纯函数变换器层；`ArchiveService` 本来就是端口，端口实现放壳层才是设计意图。
- **把 `platform::ZipArchiveService` 泛型化成「任意 `Read + Seek` 源」以复用**：要动桌面的执行路径与 E2005 网络分支，风险大于收益；实际重复只有约 60 行适配代码，且两端共用 `decode_entry_name` 与同一份错误码语义。
- **Web 不接加密包（关掉 `aes-crypto`）**：界面本来就有密码字段（C10 与桌面同形），计划里也有 `passwordProvided` 事实；关掉只会让同一份计划在两端给出不同结论。测试用真 AES-256 包对拍，顺手把密码链路钉死。

## 后果

- **好的**：19 个视图在浏览器里全部能真执行、真出产物、真撤销；解压产物字节与桌面同一个库解出来的完全一致；加密包在两端行为相同（缺密码 E3008、密码错 E3009）。
- **代价**：debug wasm 体积 +1.7 MB；`MemoryArchiveService` 把**整个包**读进内存（`MemoryFs::read_file` 返回新 `Vec`），因此受 §13.10.2 单文件 ≤100 MB 的本地建议档约束（超档是强提示＋确认后放行，不是配额）。
- **仍未解决**：OPFS 落盘与目录授权未实现（ADR-0108 的可选后续）；Web 的「整目录压缩」不在能力内（只能选文件）；F10 的计划侧本身不可用（无目录树），执行面无从谈起。

## 推翻条件

若浏览器能给出可随机访问的文件句柄（File System Access API 的 `slice`／OPFS 文件句柄），`inspect`／`extract_entry` 可以改成流式读写，不必整包进内存；届时另开 ADR 推翻本文第 1、7 条的内存口径，端口签名（kernel 侧）不变。
