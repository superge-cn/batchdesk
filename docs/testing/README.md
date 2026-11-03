# 测试文档索引

> 判定权威是 T v1.12 与产品方案 v1.19；本目录只承载**执行载体**（用例、表单、记录），不复制基线判定文本。

## 1. 文档地图

| 文档 | 覆盖 | 状态 | 说明 |
|---|---|---|---|
| [测试用例库-TC-v0.4.md](./测试用例库-TC-v0.4.md) | **当前版**：单机版阶段 0（25 项＋C17a）＋横切 B1/B2/B3/B4/B6/B8＋C37/B9 阶段 1 | v0.4 | 2026-11-02 按两项人类裁决升版：§3 逐行补齐为「每槽一项、项数＝槽数、顺序＝序号」（OQ-32，实例总数仍 296）；S01 的 E○2 由「预检中取消」改写为「预检期间界面可继续操作、结果落地前不派发」（OQ-31）并落地断言 |
| [测试用例库-TC-v0.3.md](./测试用例库-TC-v0.3.md) | 历史版本（2026-10-29；§3 存在枚举数≠槽数的行，已由 v0.4 修正）｜单机版阶段 0（25 项＋C17a）＋横切 B1/B2/B3/B4/B6/B8＋C37/B9 阶段 1 | v0.3 | 296 条阶段 0 功能实例＋65 条 XCT＋C37 8＋B9 8＝377 条 |
| [Web测试分册-TW-v0.2.md](./Web测试分册-TW-v0.2.md) | Web 双通道、OPFS/配额、隐私承诺、云契约、降级链、零强制力 | v0.2 | TW-A…TW-G；TW-E 覆盖 8 个云端端点（含 GET session）；TW-G 为 F15 Phase 3 |
| [验收签署模板-v0.2.md](./验收签署模板-v0.2.md) | 功能级/阶段级/闭门/毕业签署、缺陷关闭、豁免、冒烟、人工语料 | v0.2 | 空白表单；判定口径回指 T §4/§5/§6 |
| [性能基准执行手册.md](./性能基准执行手册.md) | C-1…C-8 的执行 SOP、环境台账、结果表 | v0.1 | 不定义阈值；口径引方案 §9.1 |
| [AI助手测试分册-AI-v0.1.md](./AI助手测试分册-AI-v0.1.md) | F15 Phase 1/2/3 AI 助手/工具调用/多计划/提示注入 | v0.1 | AI-01…AI-26；`fixtures/ai/`；mock provider |
| [用例覆盖映射表-v0.2.md](./用例覆盖映射表-v0.2.md) ＋ [CSV](./用例覆盖映射表-v0.2.csv) | **TC §3 阶段 0 全 296 条实例逐条 → 载体/状态/锚点/缺口**（CSV 为数据源，md 放口径与汇总） | v0.2 | 每完成切片或 UI 接入即改 `scripts/gen_case_map.py` 重跑生成；**九项校验已并入 `scripts/check_docs.py`（CI lint job 必跑）**：296 条数、唯一 ID、锚点实名、禁省略式、✅ 不挂缺口、⛔ 不标 CI、✅／◐ 不标真人工、🔶 标 WebDriver、E4 ✅ 必须引用取消锚点 |
| [用例执行台账-v0.1.md](./用例执行台账-v0.1.md) ＋ [CSV](./用例执行台账-v0.1.csv) | **按实例回答「锚点这次真跑过吗」**：296 行逐条解析到实测清单（实测通过 262／待人工 34／缺锚点 0，2026-11-03 全载体重跑；需精确化与不可机器定位已清零）；覆盖判定仍以映射表为准 | v0.1 | 由 `scripts/run_case_ledger.py` 产出（CSV＋md＋`执行记录/` 清单同源），`check_docs.py` 门禁行数／词法／载体一致／硬缺陷为零——ADR-0051 |
| [用例覆盖映射表-v0.1.md](./用例覆盖映射表-v0.1.md) | 历史版本（仅 C05/C06/S01/S02/C09/C17a 的 68 条），已被 v0.2 取代、不再更新 | v0.1 | — |
| [`fixtures/corpus/manual-results.md`](../../fixtures/corpus/manual-results.md) | 4 条人工语料台账 | 待执行 | R05/E01/E02/E04；阶段末/闭门前执行 |

## 2. 自动化现状（2026-11-01）

| 对象 | 已实现 | 未实现 |
|---|---|---|
| 语料生成 | 24 条自动化脚本、范围门禁、冻结哈希、幂等/确定性校验 | **逐条 expected 断言 harness**（K01–K08＋T01–T08＋R01–R04/R06 已接；E03/Z01/Z02 待接） |
| TC 功能用例 | 设计量 377 条（296 阶段 0 功能实例＋65 XCT＋C37 8＋B9 8）；**阶段 0 的 296 条已逐条映射到执行载体**（v0.2 CSV：✅ 170／◐ 92／⛔ 34／🔶 0，自动化可达 262 条＝88.5%），其判定所依赖的 plan/apply/undo 锚点大多已在 CI；E4 的"取消"已补到**十二臂**级（19 条 E4 行中 16 条 ✅，剩 C02 mid-stream／C09 Delete／C10 archive 三条）并由 Vitest 断言取消/重试入口可达；Playwright＋Chromium 层再断言真实浏览器下的面板语义（C05 预览渲染与刷新、S01 预检报告与「不存在跳过预检入口」负向断言、**方案 §7.3 的参数变更→旧计划失效阻断**、**保护目录命中时执行前的危险二次确认**（方案 §3.3/§7.11＋U §3.5）），D 类纯 UI 缺口清空（ADR-0046/0047/0048） | 逐条**签署记录**未开始（T §6.1 的 M1–M8 环境组合）；65 条 XCT 与 C37/B9 尚未做同级别映射 |
| TW Web 用例 | 设计量（A/B/C/D/E/F） | mock 服务端、Playwright/浏览器矩阵、真实 staging |
| 云契约 | 02 §4 的 8 端点契约 | mock 档自动化、staging 档 |
| AI 助手（F15 Phase 1/2/3） | `fixtures/ai/` 用例规范、设计 14 契约 | mock provider、真实 provider/staging、自动断言 harness |
| 性能 | C-1…C-8 数据集脚本/模板（C-7 阻塞）；C-6 的 C09 只读/计划预览断言（本地手动）；**测量夹具 `fixtures/bench/measure.py` ＋ 结果台账 `fixtures/bench/results.md`（C-1／C-2 首轮 Linux 预跑已入表）** | Windows 实机重测、C-4 其余介质记录、C-5／C-8、CI 基线对比脚本 |
| 代码与测试总览 | kernel 390 + 语料断言 22 + platform 100 + platform 集成 2（`archive_cancel`：C10 归档执行器的条目内取消） + CLI 7 单测 + 70 集成（合计 **591 项**，2026-11-03 实测）＋ Vitest 组件测试 **63 项**＋Playwright（Chromium）UI 语义测试 **15 项**＋国际化门禁 `scripts/check_locales.py`（2 × 395 key）；C-6 只读/计划预览；apply→undo、S02、保留期集成；plan→apply 内容再校验（E2010）；Windows 文件身份（WinAPI，ADR-0012）与长路径前缀收口；S01 preflight 9 项、S02 目标路径复核 2 项、S03 恢复 4 项（壳层）；C05 预览聚合 4 项、C06 undo_dry_run 2 项 | TC S01/S02 的 UI 用例逐条执行（载体待 UI）；E03 语料断言；见用例覆盖映射表 |
| 配置/模板/UiState（C37/B9） | kernel `config`/`recipe`/`ui_state` 类型与迁移、`platform::config_store` 损坏兜底/原子写/模板 CRUD；单测 25+12 项；**设置区 UI 已建**（`web/src/SettingsPage.tsx` 五组读写＋来源标记＋E1007 只读，Vitest 5 项） | 人工执行（TC-C37 8 条、TC-XCT-B9 8 条待跑）；Web OPFS |
| 桌面 IPC/UI（C09＋23 功能视图＋任务与撤销） | 壳层队列/恢复/重试/合帧/审计/generic kind 路由/f10 清单/c11 去重/c10 ZIP/F11 逐级前缀事实/`TaskSummary.planId` 快照兼容/**CSP 与 capabilities 静态断言 6 项** 测试 **33 项**；web `tsc --noEmit` + `vite build`；Tauri `cargo check`；浏览器 mock 流；**列表窗口化 `VirtualList`（C09 计划预览/执行结果/功能页计划预览/回滚预演四处）＋「任务与撤销」视图（`TaskHistory.tsx`：取消/重试/撤销/中断任务恢复）**；WebView E2E（apply→undo、cleanup 演练、取消回滚）；**中英双语**（`locales/` 437 key × 2＋`i18n.tsx` Provider，Vitest 7 项＋Playwright 1 项）＋**错误文案分层**（`errors.ts` 21 码位映射＋`ErrorBanner`，Vitest 6 项） | 各功能 C05 式预览面板、错误码文案资源化（`error.<code>.*`）、S03 实机崩溃演练、真 WebView e2e（Playwright 的 WebKit 不是 WebKitGTK，不能替代；需 tauri-driver/WebKitWebDriver）——Vitest 与 Playwright 已覆盖 23 视图＋任务视图＋设置页可达、取消/重试形状与面板语义、3000 条计划的 DOM 行数上界 |

> **CI 口径**：当前 `cargo test --workspace --all-features` **589 项**（kernel 390 + 语料断言 22 + platform 100 + CLI 7 单测 + 70 集成），另有 `pnpm test` **63 项**与 `pnpm test:e2e` **15 项**，以及国际化门禁 `python3 scripts/check_locales.py`（2 × 437 key、源码引用 426 key、21 个错误码两段齐全）与产物 CSP 断言 `python3 scripts/check_dist_csp.py`（web job 构建后跑：无内联脚本/样式、无远端引用）；C-6 的 C09 断言是本地手动、CI 不生成 C-6（覆盖只读检测与计划预览）；`crates/cli/tests/apply_flow.rs` 覆盖真实文件系统的 apply→隔离→undo、S02 保护目录与 cleanup 保留期及 C01–C05/C10/C11/C12 与 F01–F13 plan→apply→undo、C05 预览与 C06 撤销预演/终态/跨批次；壳层任务队列/中断恢复由独立 `cargo test --manifest-path src-tauri/Cargo.toml` 覆盖（17 项，含 generic kind 队列路由、f10 清单写出与撤销、c11 去重、c10 ZIP 压缩 apply→undo），WebView E2E 由 `scripts/e2e-tauri.mjs` 本地手动执行（尚未进 CI）；语料步骤执行生成 + 范围核对 + 哈希/幂等 + 已接入的 22 项断言（19 asserted／4 partial；E03 仍 pending）；TC v0.3 已展开 C37/B9 实例，人工执行尚未开始；C05/C06/S01/S02/C09/C17a 实例的执行载体映射见《用例覆盖映射表》（C05/C06 无必须人工的 CLI 条目）；CI 绿灯不能表述为“语料回归零失败”或“377 条用例通过”；**CI 七 job 全绿（run 41297222857，2026-11-01，head 16d19e8，含 Windows 单元测试与 desktop/deny/SBOM；上一轮 UI 接入切片 run 41269629787）**。

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
