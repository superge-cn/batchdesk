# 测试文档索引

> 判定权威是 T v1.13 与产品方案 v1.21；本目录只承载**执行载体**（用例、表单、记录），不复制基线判定文本。

## 1. 文档地图

| 文档 | 覆盖 | 状态 | 说明 |
|---|---|---|---|
| [测试用例库-TC-v0.4.md](./测试用例库-TC-v0.4.md) | **当前版**：单机版阶段 0（25 项＋C17a）＋横切 B1/B2/B3/B4/B6/B8＋C37/B9 阶段 1 | v0.4 | 2026-11-02 按两项人类裁决升版：§3 逐行补齐为「每槽一项、项数＝槽数、顺序＝序号」（OQ-32，实例总数仍 296）；S01 的 E○2 由「预检中取消」改写为「预检期间界面可继续操作、结果落地前不派发」（OQ-31）并落地断言 |
| [测试用例库-TC-v0.3.md](./测试用例库-TC-v0.3.md) | 历史版本（2026-10-29；§3 存在枚举数≠槽数的行，已由 v0.4 修正）｜单机版阶段 0（25 项＋C17a）＋横切 B1/B2/B3/B4/B6/B8＋C37/B9 阶段 1 | v0.3 | 296 条阶段 0 功能实例＋65 条 XCT＋C37 8＋B9 8＝377 条 |
| [Web测试分册-TW-v0.2.md](./Web测试分册-TW-v0.2.md) | Web 双通道、OPFS/配额、隐私承诺、云契约、降级链、零强制力 | v0.2 | TW-A…TW-G；TW-E 覆盖 8 个云端端点（含 GET session）；TW-G 为 F15 Phase 3 |
| [验收签署模板-v0.2.md](./验收签署模板-v0.2.md) | 功能级/阶段级/闭门/毕业签署、缺陷关闭、豁免、冒烟、人工语料 | v0.2 | 空白表单；判定口径回指 T §4/§5/§6 |
| [性能基准执行手册.md](./性能基准执行手册.md) | C-1…C-8 的执行 SOP、环境台账、结果表 | v0.1 | 不定义阈值；口径引方案 §9.1 |
| [AI助手测试分册-AI-v0.1.md](./AI助手测试分册-AI-v0.1.md) | F15 Phase 1/2/3 AI 助手/工具调用/多计划/提示注入 | v0.1 | AI-01…AI-26；`fixtures/ai/`；mock provider |
| [Windows实机日检查清单-v0.2.md](./Windows实机日检查清单-v0.2.md) | **当前版**：六组分类不变（32 条），新增当日议程 B0…B6（开工准备／环境类／占用批量／大小写折叠／基准重测／C-6 大集／扩展块），同步 OQ-42 关闭后的正式档口径（T v1.13 C-2 编码两变体＋方案 §9.1 双分母）；W4 收编后只剩 C15-PRE-04 且归开发线 | v0.2 | 2026-11-07 排期版；不复制判定点；口径守 ADR-0051「Linux 预跑不得冒充」 |
| [人工裁决改判清单-2026-11-09.md](./人工裁决改判清单-2026-11-09.md) | 32 条「待人工执行」中 31 条的改判建议与证据回链（W1 17／W2 5／W5 7／W3 1／C09 C-6 1；建议 ✅12／◐19） | 待人工裁决 | 只做「证据 → 建议状态」对账，改判本身须签署（ADR-0051）；C15-PRE-04 不在内 |
| [Windows实机日检查清单-v0.1.md](./Windows实机日检查清单-v0.1.md) | 历史版本（2026-11-05 起草、11-06 更新；六组分类与环境准备被 v0.2 沿用，§9「不在范围」中的四项已由 v0.2 §9·B6 显式排入） | v0.1 | 已被 v0.2 取代、不再更新 |
| [用例覆盖映射表-v0.2.md](./用例覆盖映射表-v0.2.md) ＋ [CSV](./用例覆盖映射表-v0.2.csv) | **TC §3 阶段 0 全 296 条实例逐条 → 载体/状态/锚点/缺口**（CSV 为数据源，md 放口径与汇总） | v0.2 | 每完成切片或 UI 接入即改 `scripts/gen_case_map.py` 重跑生成；**九项校验已并入 `scripts/check_docs.py`（CI lint job 必跑）**：296 条数、唯一 ID、锚点实名、禁省略式、✅ 不挂缺口、⛔ 不标 CI、✅／◐ 不标真人工、🔶 标 WebDriver、E4 ✅ 必须引用取消锚点 |
| [用例执行台账-v0.1.md](./用例执行台账-v0.1.md) ＋ [CSV](./用例执行台账-v0.1.csv) | **按实例回答「锚点这次真跑过吗」**：296 行逐条解析到实测清单（实测通过 262／待人工 34／缺锚点 0，2026-11-03 全载体重跑；需精确化与不可机器定位已清零）；覆盖判定仍以映射表为准 | v0.1 | 由 `scripts/run_case_ledger.py` 产出（CSV＋md＋`执行记录/` 清单同源），`check_docs.py` 门禁行数／词法／载体一致／硬缺陷为零——ADR-0051 |
| [用例覆盖映射表-v0.1.md](./用例覆盖映射表-v0.1.md) | 历史版本（仅 C05/C06/S01/S02/C09/C17a 的 68 条），已被 v0.2 取代、不再更新 | v0.1 | — |
| [`fixtures/corpus/manual-results.md`](../../fixtures/corpus/manual-results.md) | 4 条人工语料台账 | 待执行 | R05/E01/E02/E04；阶段末/闭门前执行 |

## 宿主无法分配 WASM 时（临时通道）

`vitest` / `vite build` / `playwright` 都依赖 V8 的 WASM（Vite 的 `es-module-lexer`、Node 的 `undici`）。
宿主内存严重超卖时（`Committed_AS` ≫ `CommitLimit`），内核会拒绝 V8 的 WASM 保护段预留，
三者一律死于 `RangeError: WebAssembly.instantiate(): Out of memory`——**与代码无关**，
`ulimit -v`、`--max-old-space-size`、`--jitless`、禁用 sandbox 都绕不过。

判定（10 秒）：`node -e "try{new WebAssembly.Memory({initial:1});console.log('WASM_OK')}catch(e){console.log('WASM_FAIL')}"`。

`WASM_FAIL` 时的替代通道：`web/.probe/`（esbuild 原生打包 ＋ jsdom，直接渲染真实组件）。
它复刻 `SettingsPage.test.tsx`／`App.uiState.test.tsx`／`App.cancel-review.test.tsx`／`errors.test.tsx` 的**用户可见行为**，八个入口共 **124 条断言**（设置页 7／App·UiState 11／取消回顾 12／错误文案 24／导航可达 16／集中报告 7／i18n 18／任务视图 29）；
用法与边界见 [`web/.probe/README.md`](../../web/.probe/README.md)。

**口径**：这是弱证据，不是测试套件的替代。CI 的 web job（Vitest＋Playwright）仍是权威；
本地跑不了时也可用 API 取 CI 结论（公开仓库免 token）：
`curl -s api.github.com/repos/superge-cn/batchdesk/actions/runs/<id>/jobs | ...` 读 `steps[].conclusion`。

## 2. 自动化现状（2026-11-01）

| 对象 | 已实现 | 未实现 |
|---|---|---|
| 语料生成 | 24 条自动化脚本、范围门禁、冻结哈希、幂等/确定性校验；**逐条 expected 断言 harness 已覆盖全部 24 条**（24 asserted／0 partial／0 pending，状态以 `crates/kernel/tests/assertions.tsv` 为准） | K08 已于 2026-11-05 收口（F05 每片写 BOM，ADR-0070）；剩余缺口在人工项与 Windows 实机，不在自动化断言 |
| TC 功能用例 | 设计量 377 条（296 阶段 0 功能实例＋65 XCT＋C37 8＋B9 8）；**阶段 0 的 296 条已逐条映射到执行载体**（v0.2 CSV：✅ 175／◐ 89／⛔ 32／🔶 0，自动化可达 264 条＝89.2%；数字以 CSV 实测为准），其判定所依赖的 plan/apply/undo 锚点大多已在 CI；E4 的"取消"已补到**十二臂**级（19 条 E4 行中 16 条 ✅，剩 C02 mid-stream／C09 Delete／C10 archive 三条）并由 Vitest 断言取消/重试入口可达；Playwright＋Chromium 层再断言真实浏览器下的面板语义（C05 预览渲染与刷新、S01 预检报告与「不存在跳过预检入口」负向断言、**方案 §7.3 的参数变更→旧计划失效阻断**、**保护目录命中时执行前的危险二次确认**（方案 §3.3/§7.11＋U §3.5）），D 类纯 UI 缺口清空（ADR-0046/0047/0048） | 逐条**签署记录**未开始（T §6.1 的 M1–M8 环境组合）；65 条 XCT 与 C37/B9 尚未做同级别映射 |
| TW Web 用例 | 设计量（A/B/C/D/E/F） | mock 服务端、Playwright/浏览器矩阵、真实 staging |
| 云契约 | 02 §4 的 8 端点契约 | mock 档自动化、staging 档 |
| AI 助手（F15 Phase 1/2/3） | `fixtures/ai/` 用例规范、设计 14 契约 | mock provider、真实 provider/staging、自动断言 harness |
| 性能 | C-1…C-8 数据集脚本/模板（C-7 阻塞）；C-6 的 C09 只读/计划预览断言（本地手动）；**测量夹具 `fixtures/bench/measure.py` ＋ 结果台账 `fixtures/bench/results.md`（C-1／C-2 首轮 Linux 预跑已入表）** | ~~Windows 实机重测~~ 已完成（2026-11-07 实机日，NVMe 平台，results.md 轮次 8：R-34…R-44；GB 变体/C03 regex/cell 未达 NVMe 档走 T §4.2 豁免）、C-4 其余介质记录、C-5／C-8、CI 基线对比脚本 |
| 代码与测试总览 | kernel 406 + 语料断言 24 + platform 101 + platform 集成 2（`archive_cancel`：C10 归档执行器的条目内取消） + CLI 7 单测 + 72 集成（合计 **612 项**，2026-11-04 复跑实测）＋ Vitest 组件测试 **73 项**＋Playwright（Chromium）UI 语义测试 **16 项**＋国际化门禁 `scripts/check_locales.py`（2 × 573 key；含第 ⑨ 项「不得渲染后端错误原文」与第 ⑩ 项「不得裸露线协议枚举」）；C-6 只读/计划预览；apply→undo、S02、保留期集成；plan→apply 内容再校验（E2010）；Windows 文件身份（WinAPI，ADR-0012）与长路径前缀收口；S01 preflight 9 项、S02 目标路径复核 2 项、S03 恢复 4 项（壳层）；C05 预览聚合 5 项（含冲突矩阵逐类断言）、C06 undo_dry_run 2 项 | TC S01/S02 的 UI 用例逐条执行（载体待 UI）；见用例覆盖映射表 |
| 配置/模板/UiState（C37/B9） | kernel `config`/`recipe`/`ui_state` 类型与迁移、`platform::config_store` 损坏兜底/原子写/模板 CRUD；单测 25+12 项；**设置区 UI 已建**（`web/src/SettingsPage.tsx` 五组读写＋来源标记＋E1007 只读＋§11.3「本机界面状态」重置入口，Vitest 7 项） | 人工执行（TC-C37 8 条、TC-XCT-B9 8 条待跑）；Web OPFS |
| 桌面 IPC/UI（C09＋23 功能视图＋任务与撤销） | 壳层队列/恢复/重试/合帧/审计/generic kind 路由/f10 清单/c11 去重/c10 ZIP/F11 逐级前缀事实/`TaskSummary.planId` 快照兼容/**CSP 与 capabilities 静态断言 6 项**/D1 自动暂停/D5 两阶段 Journal 测试 **66 项**；web `tsc --noEmit` + `vite build`；Tauri `cargo check`；浏览器 mock 流；**列表窗口化 `VirtualList`（C09 计划预览/执行结果/功能页计划预览/回滚预演四处）＋「任务与撤销」视图（`TaskHistory.tsx`：取消/重试/撤销/中断任务恢复）**；WebView E2E（apply→undo、cleanup 演练、取消回滚）；**中英双语**（`locales/` 505 key × 2＋`i18n.tsx` Provider，Vitest 7 项＋Playwright 1 项）＋**错误文案分层**（`errors.ts` 53 码位映射＋`ErrorBanner`，Vitest 8 项） | 各功能 C05 式预览面板、错误码文案资源化（`error.<code>.*`）、S03 实机崩溃演练、真 WebView e2e（Playwright 的 WebKit 不是 WebKitGTK，不能替代；需 tauri-driver/WebKitWebDriver）——Vitest 与 Playwright 已覆盖 23 视图＋任务视图＋设置页可达、取消/重试形状与面板语义、3000 条计划的 DOM 行数上界 |

> **CI 口径**：当前 `cargo test --workspace --all-features` **683 项**（kernel 448 + 语料断言 24 + platform 119 + platform 集成 2 + 1 + 6 + CLI 7 单测 + 76 集成），另有 `pnpm test` **81 项 / 16 文件**与 `pnpm test:e2e` **17 项**，以及国际化门禁 `python3 scripts/check_locales.py`（2 × 626 key、源码引用 615 key、设计 06 §2 的 53 个错误码逐码对齐码表、设计 06 §2A 的 61 个预警类别逐类对齐、界面 17 个源码文件零后端错误原文与零枚举裸露）与产物 CSP 断言 `python3 scripts/check_dist_csp.py`（web job 构建后跑：无内联脚本/样式、无远端引用）；C-6 的 C09 断言是本地手动、CI 不生成 C-6（覆盖只读检测与计划预览）；`crates/cli/tests/apply_flow.rs` 覆盖真实文件系统的 apply→隔离→undo、S02 保护目录与 cleanup 保留期及 C01–C05/C10/C11/C12 与 F01–F13 plan→apply→undo、C05 预览与 C06 撤销预演/终态/跨批次；壳层任务队列/中断恢复由独立 `cargo test --manifest-path src-tauri/Cargo.toml` 覆盖（**66 项**，含 generic kind 队列路由、f10 清单写出与撤销、c11 去重、c10 ZIP 压缩 apply→undo、S01 预检门 5 项、CSP／capabilities 静态断言 6 项、窗口恢复规则 `window_state` 7 项、D1 自动暂停 1 项；D1 补全的 undo 重试/映射盘锚点见 kernel `apply::tests::undo_network_disconnect_is_retried_three_times_then_pauses`／`undo_network_disconnect_aborts_the_whole_task_without_a_pause_host`／`mapped_network_drive_apply_and_undo_are_retried` 3 项与 `platform::fs::tests::network_path_detection_keeps_unc_and_rejects_local_drive`；edit-content 重试复用备份的锚点 `generic_apply::tests::edit_content_network_retry_reuses_a_single_quarantine_backup`；transcode 执行路径 `transcode_entries_run_the_in_place_rewrite_and_undo_restores_the_bytes`／`transcode_without_a_content_editor_is_rejected_before_touching_files`；undo 预演重试 `apply::tests::undo_dry_run_retries_network_stat_and_does_not_report_missing`／`undo_dry_run_surfaces_e2005_when_the_host_cannot_pause`），WebView E2E 由 `scripts/e2e-tauri.mjs` 本地手动执行（尚未进 CI）；语料步骤执行生成 + 范围核对 + 哈希/幂等 + 24 条自动化语料的逐条断言（24 asserted／0 partial／0 pending）；TC v0.3 已展开 C37/B9 实例，人工执行尚未开始；C05/C06/S01/S02/C09/C17a 实例的执行载体映射见《用例覆盖映射表》（C05/C06 无必须人工的 CLI 条目）；CI 绿灯不能表述为“语料回归零失败”或“377 条用例通过”；**CI 结果链（最新在前，均经 GitHub Actions API 逐 job／逐 step 核对）**：run 42489526338（2026-11-08，head `f76f0de`＝transcode/undo 预演补全批收尾；七 job 全 success、95 步零 skipped 零失败，bench 11／desktop 15／deny 12／lint 10／test·ubuntu 16／web 15／test·windows 16；其前 run 42489189813 仅 lint 的桌面壳 rustfmt 步骤红，格式化后转绿） → run 42487267357（2026-11-08，head `749fe58`＝D1 再补全批：edit-content/transcode 重试＋备份复用；七 job 全 success、95 步零 skipped 零失败，bench 11／desktop 15／deny 12／lint 10／test·ubuntu 16／web 15／test·windows 16） → run 42485549965（2026-11-08，head `8af89c7`＝D1 补全批：undo 重试＋映射盘识别；七 job 全 success、95 步零 skipped 零失败，bench 11／desktop 15／deny 12／lint 10／test·ubuntu 16／web 15／test·windows 16） → run 42475364987（2026-11-08，head `2692392`＝C10 归档执行器接入批收尾（`86899b7` 的 C10 重试接线＋`2692392` 的 Rust 1.99 `try_update` 适配）；七 job 全 success、95 步零 skipped 零失败，bench 11／desktop 15／deny 12／lint 10／test·ubuntu 16／web 15／test·windows 16；其前 run 42471860342 仅 lint 红——1.99 弃用 `fetch_update`，修复后转绿） → run 42449989630（2026-11-08，head `2a145bf`＝D1/D2/D5 修复批；七 job 全 success、95 步零 skipped 零失败，bench 11／desktop 15／deny 12／lint 10／test·ubuntu 16／web 15／test·windows 16） → run 42444212091（2026-11-08，head `f76c520`＝D3/D4 修复批；七 job 全 success、95 步零 skipped 零失败，bench 11／desktop 15／deny 12／lint 10／test·ubuntu 16／web 15／test·windows 16） → run 42439125925（2026-11-08，head `2c47f22`＝Windows 实机日第二轮执行批；七 job 全 success、95 步零 skipped 零失败） → run 42390288263（2026-11-07，head `6e798ce`＝Windows 实机日批；七 job 全 success、95 步零 skipped 零失败） → run 41991099699（run 163，2026-11-05，head `24b475b`＝C37 处理模板前端批；**这一次覆盖 6 个提交**——`de219ab`…`24b475b` 一次性推送，只对 tip 出一个 run；lint 首次实跑新增的「rustfmt（桌面壳独立 workspace）」步骤） → run 41957511340（run 160，head `3634d6f`＝导航目录单一来源批） → run 41954459249（run 159，head `d8bfa9b`＝门禁步骤改名批） → run 41953709161（run 158，head `9579aaf`＝预警类别对照源批） → run 41941034270（run 157，head `37295a8`＝上一批的台账回填） → run 41940462594（run 156，head `8f4dc16`＝任务类别 `job` 批） → run 41856701018（run 155，head `fa91702`＝枚举裸露门禁批） → run 41843517595（run 154，head `ecfe5f0`＝条目级状态本地化批） → run 41839550541（run 153，head `aa820c0`＝上一批的台账回填） → run 41837684027（run 152，head `32b428d`＝C05 执行链 e2e＋状态徽标本地化批） → run 41831178876（run 151，head `d08316e`＝C05 逐类断言批的台账回填） → run 41830536215（run 150，head `c862a95`＝C05 逐类断言批） → run 41828440576（run 149，head `bf3fbb5`＝界面错误原文收口批） → run 41785845637（run 148，head `72109ce`＝OQ-08 语料收口批的台账回填） → run 41785374801（run 146，head `1728409`＝OQ-08 三条编码语料收口批） → run 41778481455（run 144，head `0eb9a68`＝三批合验：C03 正则窗口化（ADR-0064）＋错误码 53 码位收口（ADR-0061 §7）＋UiState window 段（ADR-0065）） → run 41761314629（head `a29e05f`＝E03 标记半程批） → run 41745327722（head `bc8b065`＝§11.3 重置入口批（同时收口此前未推送的 ADR-0056…0062 六批与产物 CSP 门禁）） → run 41552314744（head `26fdc1a`＝ADR-0055 批的台账回填）。步骤清单：`rustfmt`＋`rustfmt（桌面壳独立 workspace）`＋`clippy（零容忍）`、ubuntu／windows 的 `文档一致性`／`国际化门禁`／`单元测试 + 语料断言`／`语料幂等复检`、web 的 `国际化门禁`／`类型检查与构建`／`产物 CSP 断言`／`前端组件测试（Vitest…）`／`UI 语义测试（Playwright…）`、desktop 的 `桌面壳 cargo check`＋`桌面壳单元测试`、deny 的根 workspace＋桌面壳独立 workspace＋SBOM 均 success。**判红同样留档**：run 41769780150 因真实 FS 只读断言未做平台门控被 windows job 判红，修复批 `ddd5d55` 转绿——本地绿不代表跨平台绿**。

## 3. 执行流程

1. **功能完成当日**：按 TC 的 PRE/EXE/RBK 三类用例人工执行，签署功能级表单（TS 模板 A）。
2. **每次构建后 30 分钟内**：跑 T §6.2 的 S-A/S-B/S-C 冒烟（TS 模板 G）；不通过阻断提测。
3. **阶段末**：全量用例＋语料全量＋性能实测；自动化语料断言实现后，自动化/人工两栏都过才签阶段级（TS 模板 B）。
4. **缺陷**：按 T §5 判级与流转；P0/P1 修复必须沉淀语料（`fixtures/README.md` §5），并同步 `manual-only.txt`/`manifest.md`。
5. **Web**：按 TW §2 浏览器矩阵人工执行；TW-E 的 mock 档在 mock 服务端就绪后接入 CI。

## 4. 维护规则

- 新增功能：在 TC §3 追加一行并同步 T §2.2（基线改动走 T 升版）；实例序号从 01 起，作废不复用。
- 新增语料：TC §4.2 B2 追加行，同步 manifest/manual-only/expected.md；`check_scope.py` 强制。
- 判定点修订：T 基线升版后整库回核，不允许只改一处。
- 任何判定点必须有 T/方案章节号；无出处时标“阻塞”，不写预期。
- 提交前运行 `python scripts/check_docs.py`；语料相关变更再跑 `check_scope.py` 与 `verify_corpus.py`。
