# ADR-0115：Web 产物的「执行后搬运」与 OPFS 落盘

- 状态：已采纳·A3-13 落地（2026-11-23）
- 相关：ADR-0108（Web 执行沙箱：本文推翻其 §1／§2 的「不引入 OPFS」结论）、ADR-0114（F10 清单：同为「按执行臂需要什么分档」）；设计 02 §3.1／§3.4、设计 04（`FsAdapter` 同步契约）、设计 05（零强制力）
- 影响范围：`web/src/lib/web/opfsStore.ts`（新增）、`web/src/lib/web/applyChannel.ts`（`productFileNames` 抽出）、`web/src/FeaturePages.tsx`（执行后落盘与面板提示）、`web/src/test/fakeOpfs.ts`（测试句柄）、语言包 3 个 key

## 1. 背景

ADR-0108 判定 OPFS 路线不可行，理由有两条：

1. **用户源文件不在 OPFS 里**——浏览器只能通过 File API 拿到只读引用，写不回原位置；「在 OPFS 上实现 FsAdapter」即使做完，也只覆盖「产物落在 OPFS」，输入仍要先 staging 读字节。
2. **OPFS 是异步的，内核执行器是同步的**——`FsAdapter` 的方法签名全同步（设计 04 已冻结），`generic_apply`／`apply_plan`／`undo` 都在同步栈上跑。

于是 ADR-0108 §4 把 OPFS 降级为「未采纳的可选后续」，并写明推翻条件：**若将来要支持「产物留在浏览器里跨会话」「目录授权直写」**，那是另一个量级的工程，届时应另开 ADR 推翻其 §1／§2。

A3-12 收口后，剩余缺口全部与「产物交付」有关（F11 产物是空目录、C09 执行后无可下载产物）。用户明确要求推进 OPFS 落盘——触发的是推翻条件里的第一条（产物跨会话留存），不是第二条（目录授权直写）。

重新核对 ADR-0108 的两个理由，发现：

- 理由 1 仍然成立，但它反对的是「把 OPFS 当作输入来源」，不反对「把产物搬进 OPFS」——输入仍走 staging，产物本来就是新字节，写进 OPFS 不需要「写回原位置」。
- 理由 2 仍然成立，但它反对的是「执行**时**用 OPFS」；如果把 OPFS 写入放在**执行之后**，执行栈里一行异步代码都不需要。

## 2. 决策

### 2.1 异步后移到执行之后：与 staging 前移对称

```
stage_inputs（异步读 File → MemoryFs）        ← 异步前移（ADR-0108 §2）
        ↓
apply_plan（同步执行 → MemoryFs + Journal）    ← 内核同步栈，一行不改
        ↓
persistProducts（异步写 OPFS）                ← 本文：异步后移
```

内核与 `FsAdapter` 契约零改动；OPFS 只是执行完之后的**搬运**。这条管道与「下载」并列：下载给用户本机，OPFS 留一份在浏览器里。

### 2.2 目录形状 `batchdesk/products/<planId>/<名字>`

按 `planId` 分目录：

- 同一计划重跑时先清该目录，避免新旧产物混在一起（否则界面说不清「这次留下了什么」）；
- 撤销或清理可以整目录移除；
- 文件名由 `productFileNames` 统一给出（basename＋同名序号前缀），与下载**共用同一份算法**——两处各写一份必然漂移。

### 2.3 失败静默降级，不冒充承诺

零强制力（设计 05）：OPFS 不是安全边界，用户清站点数据即失效，配额也由浏览器自己管。因此：

- `opfsSupported()` 为假（旧 Safari、非安全上下文）→ 返回 `{saved: 0, failed: N}`，界面不显示落盘提示，只保留下载；
- 单个文件写失败不影响其余文件；
- 落盘结果只在 `saved > 0` 时显示，不给用户「已保存 0 个」的噪声；
- 「能不能落盘」不参与任何授权／配额判定。

### 2.4 撤销不收走 OPFS 副本，界面如实说

`undo` 只回滚沙箱（`products-only`），与「已下载的副本收不回」同理：已写进 OPFS 的字节也不会随撤销消失。界面文案直接写「撤销不收走这里」，并给一个独立的「清除浏览器中的产物」按钮——不把两种存储混成一句模糊的承诺。

### 2.5 本批不做目录授权直写

File System Access（`showDirectoryPicker`）能把产物写到**用户选中的真实目录**，能解 F11 的产物交付。但它需要：权限模型（句柄跨会话失效、重新授权）、Chromium-only 的形态差异、以及「写真实磁盘」的产品语义确认。本文只做 OPFS；目录授权直写留作独立 ADR，触发条件是产品确认「浏览器里要直接产出到用户目录」。

### 2.6 读回闭环放在设置页的「浏览器存储」卡片

只写不读等于没做：关掉页面之后用户找不回产物，「跨会话留存」就是一句空话。A3-14 补上
`BrowserStorageCard`（设置页）：列出 `batchdesk/products/*/*`、单件重新下载、整体清理。
下载复用 `defaultProductSaver`（与执行面板同一份实现），不复写「先 revoke 还是后 revoke」
这类细节。

**为什么不放在执行结果面板**：那个面板只在本次执行后存在；刷新页面就没有了，正好是
「跨会话」要解决的场景。设置页是常驻入口。

## 3. 替代方案与取舍

| 方案 | 结论 |
|---|---|
| 把 `FsAdapter` 改成 `async`，执行时直写 OPFS | **拒绝**：破坏已冻结契约（设计 04），波及 kernel／platform／cli／src-tauri 全部调用点；且 OPFS 只解决「产物存哪」，不解决「输入怎么进来」 |
| 引入阻塞桥（`Atomics.wait` + SharedArrayBuffer）在同步栈里调异步 OPFS | **拒绝**：需要 COOP/COEP 响应头，且把执行器切成状态机；收益与代价不成比例 |
| 落盘放在 Worker 内（WASM 侧） | **拒绝**：产物字节已经随 `apply_plan` 响应回到主线程（Blob），再绕回 Worker 只会多一次拷贝 |
| 不做 OPFS，只保留下载 | **不采纳**：用户明确要求「产物跨会话留存」；且下载是「用户主动动作」，OPFS 是「执行后自动留一份」，两者互补 |
| 本批一并做 File System Access 直写真实目录 | **不采纳**：需要权限模型与产品语义确认，见 §2.5 |

## 4. 后果

- Web 执行面在**下载**之外多了一条**浏览器内留存**通道；产物按 `planId` 分组，可列出、可读回、可整体清除。
- **不解 F11**：F11 的产物是空目录，OPFS 里建空目录对用户同样不可交付（浏览器无法把目录给用户）；F11 仍需 File System Access 或桌面版。
- **不解 C09**：C09 的阻塞是 `FileIdentity`（ADR-0114 同批纠正为 `file-identity`），与产物存哪无关。
- 配额风险：OPFS 由浏览器配额约束，写满时 `createWritable` 抛错 → 静默降级，界面不显示落盘成功。
- 推翻条件：若产品要求「产物直接落到用户磁盘目录」，需另开 ADR 做 File System Access 直写（权限模型 + Chromium-only 降级）；若内核将来改成异步 `FsAdapter`，本文的「执行后搬运」可以退化为「执行时直写」，但那时要重审 ADR-0108 的全部结论。

## 5. 证据

- `vitest`：41 文件／**354 项**（新增 `opfsStore.test.ts` **8 项**：分目录落盘／重跑清残留／列出／读回与缺失 null／清理幂等／不支持时不抛／空产物不建目录／单文件失败不连坐；`FeaturePages.webPlan.test.tsx` 13→**14 项**：执行后 OPFS 里真有该 `planId` 目录与产物字节）。
- `tsc --noEmit`／`vite build`／`check_locales`（810 key）／`check_docs`／`check_dist_csp` 全 0。
- 反向验证（3 条实测红、均已回滚）：断掉 UI 的落盘调用（UI 用例红）；`persistProducts` 不清残留（重跑用例红）；`listPersisted` 不收集条目（列用例红）。
- A3-14 读回闭环：`BrowserStorageCard.test.tsx` **4 项**（不支持形态如实说明／空态／列出＋单件下载断言 OPFS 字节／整体清理后目录真的没了）；vitest **42 文件／358 项**。
- **真实浏览器 e2e**（`playwright test`，Chromium＋mock IPC）：**27 项全绿**。同批修掉 6 个真实回归——A3-11 把 F12 的「改名目录」改成目录选择器后，3 个 spec（`plan-warnings`／`rename-preview`／`stale-plan`）还在 `fill()` 一个 `<input type="file">`；这些用例验证的是 mock 计划的面板语义（ADR-0046 边界），改为不选文件、留在 mock IPC 路径。
- `cargo deny check`：advisories／bans／licenses／sources 全 ok（依赖树本批未新增 crate）。
- 同批 C09 归因纠正：`WebPlanGap` 的 `content-hash` → `file-identity`，`planChannel.test.ts` 70 项与语言包 810 key 同步（详见台账 A3-13）。
