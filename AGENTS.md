# AGENTS.md
> 本文件供 AI 编码代理读取。人类协作者请读 README.md 与 CONTRIBUTING.md。
> 基线文档引用一律"文档名+章节号"，数值以基线为准，禁止在代码注释中另立数值。

## 快速事实
- 栈：Rust workspace（crates/kernel 领域引擎 + crates/platform 共享壳层 + src-tauri 桌面壳）+ TypeScript/React 前端（web/；当前 C09 切片使用 U token 自定义组件，shadcn 接入待后续）
- 内核需可编译为 WASM 复用于 Web 版（方案 13.5）——kernel 不得依赖 Tauri/桌面 API
- 包管理：Cargo + pnpm；CI：GitHub Actions（六 job：lint/deny/test/web/desktop/bench）

## 仓库地图
| 位置 | 职责 | 文档 |
|---|---|---|
| crates/kernel | 领域引擎（纯函数变换器层在此） | 方案 4.2/4.3 |
| crates/platform | 共享壳层适配：扫描/哈希、原生 FsAdapter、FileJournal、保留期清理 | 设计 01/04、方案 4.7 |
| src-tauri | 桌面壳：C09 任务队列/`task://*` 事件、S03 任务快照持久化与 `recoverTask`、S01 `preflightPlan`、IPC 命令、应用数据路径、计划暂存；授权注入待补 | 方案 4.4、设计 01 §4.3–§6、设计 12 |
| web/ | 表现层：C09 参数/预览/执行/撤销/保留期 UI；事件驱动进度/暂停/取消；无业务与授权逻辑 | 方案 4.2、U v1.3、设计 12 |
| fixtures/corpus | 28 条异常语料（24 条自动化脚本已就绪；断言 harness 已覆盖 K01–K08＋T01–T08＋R01–R04/R06（含 partial），其余 pending 登记在 `crates/kernel/tests/assertions.tsv`） | T 3.2 |
| fixtures/bench | 性能基准数据集 C-1…C-8 | T 附录 C |
| fixtures/migrations | 配置 schema 历史样本（不属于 28 条语料） | 设计 03 §7 |
| docs/design | 设计契约 01–13 | docs/design/README |
| docs/testing | 用例库、Web 分册、验收表单 | docs/testing/README |
| docs/DEVELOPMENT-PLAN.md | Wave 顺序与完成度台账（先做什么/后做什么） | 该文件 |
| docs/OPEN-QUESTIONS.md | 待决事项登记 | 该文件 |
| scripts/ | 文档/契约门禁（check_docs.py） | 该目录 |
| rust-toolchain.toml | 工具链统一（stable + rustfmt/clippy） | 该文件 |
| deny.toml | 依赖许可证/来源策略（OQ-13） | 该文件 |
| docs/adr/ | 工程决策记录 | docs/adr/README |

## 常用命令
- 构建：cargo build --workspace
- 语料生成前的准备（**必须先跑**，否则没有输入样本）：python fixtures/corpus/check_scope.py（范围门禁）→ 依次执行 fixtures/corpus/*/gen_*.py（幂等生成样本）
- 全量测试：`cargo test --workspace`（kernel/platform/CLI 单测 + CLI 集成 + `crates/kernel/tests/corpus.rs` 语料断言；**K01–K08＋T01–T08＋R01–R04/R06＋Z01/Z02 已接断言（含 partial）**）
  - ⚠️ `cargo test` 不生成语料；生成脚本只准备输入，不代表回归已执行。
  - ⚠️ 语料现状：**24 条自动化脚本已就绪、0 条待补**；4 条标「人工」的语料不由 CI 跑（R05/E01/E02/E04，见 fixtures/corpus/manual-only.txt 与 manual-results.md）。**断言 harness 覆盖 K01–K08＋T01–T08＋R01–R04/R06＋Z01/Z02（24 条中 19 条 asserted／4 条 partial），其余 1 条（E03）在 `crates/kernel/tests/assertions.tsv` 标 pending**：CI 绿灯仍只代表"生成 + 范围核对 + 单元测试 + 已接入的语料断言通过"，不能表述为"自动化语料零失败"，更不能表述为"语料回归零失败"。
- 单 crate 测试：cargo test -p kernel / -p batchdesk-platform
- Web/壳：pnpm install（根，Tauri CLI）→ (cd web && pnpm install) → pnpm build；pnpm tauri dev；cargo check --manifest-path src-tauri/Cargo.toml（独立 workspace，需 WebKitGTK/WebView2；不随根 cargo test 编译）
- 桌面壳任务队列测试：cargo test --manifest-path src-tauri/Cargo.toml --offline
- WebView E2E：先启动 Vite 与 tauri-driver（前置条件见 scripts/e2e-tauri.mjs 文件头），再 `node scripts/e2e-tauri.mjs`
- 语料生成物校验：python fixtures/corpus/verify_corpus.py --hashes（加 `--rebuild` 校验幂等）
- 文档/契约门禁：python scripts/check_docs.py；python fixtures/migrations/check_schema.py
- 门禁：cargo fmt --check；cargo clippy --workspace --all-targets -- -D warnings；cargo deny check（白名单见 `deny.toml`；项目许可证 OQ-11 待法务）
  - ⚠️ **门禁结论一律以命令退出码为准**：禁止 `cargo clippy … | grep …` / `| head` 等管道判断通过——管道会吞掉上游退出码，grep 无输出 ≠ 通过。用法为 `cmd > /tmp/x.log 2>&1; echo "EXIT=$?"`，或显式读 `${PIPESTATUS[0]}`。
  - ⚠️ **本地工具链可能滞后于 CI**：CI 用最新 stable（`dtolnay/rust-toolchain@stable`），会引入本地尚未生效的新 lint；本地 clippy 绿灯不等于 CI 绿灯，以 CI 注解（`::error::`）为准。
- 基准：cargo bench（仅主分支；基线对比脚本待实现）

## 硬性不变量（违反 = 打回）
1. kernel 的变换器层是纯函数：禁止任何磁盘 I/O、时钟、环境读取；输入输出全走参数与返回值（方案 4.3）
2. 批量写操作必须 plan/apply 两段：plan 为纯函数绝不落盘，apply 按计划执行并上报进度（方案 7.1）
3. 错误只走 E1xxx–E9xxx 分段（方案 4.6）；禁止把 anyhow 字符串直接透出为用户提示
4. 撤销/回滚只走平台框架（映射表 + Journal，方案 7.8）；功能内自建撤销 = 不合格
5. 业务代码只依赖 LicenseService 抽象接口（方案 12.4）；禁止直读许可证文件、禁止内嵌层级判断；实现按形态注入，**接口不变、只换实现**（方案 13.8）：单机版注入 AlwaysAllowLicense、商业版注入完整授权实现、Web 版注入配额型实现（QuotaBased）——禁止为任一形态另造一套授权判断
6. kernel 禁止任何网络调用与遥测；联网仅存在于授权校验与（默认关闭的）更新检查（方案 4.10/9.7）
7. 安全默认值：默认不覆盖、占位文件默认跳过、删除一律进隔离目录（方案 4.5）
8. 路径合法性、大小写折叠比较、原子改名、长路径前缀统一走文件系统适配层（方案 4.7）；禁止业务代码自拼 \\?\ 前缀
9. 新增依赖必须先过 cargo-deny 并在 PR 说明理由；GPL/AGPL 源不得作为移植蓝本（方案 4.11.4）
10. 移植代码与自写代码同标准：**自动化语料全量必过**（方案 4.11.2a④），不因"来自成熟项目"豁免；人工项（清单见 fixtures/corpus/manual-only.txt）无法在 CI 跑，按 fixtures/corpus/manual-results.md 台账执行、由人记录
11. 强制力边界：授权与功能判定的强制力只存在于编译产物内的执行路径——kernel 执行层按操作类别复核，不信任壳层／入口门禁（方案 8.5）；**web/ 与 WASM 产物零强制力**：授权相关元素只做显示／隐藏，不得含放行／拦截逻辑，WASM 公开导出面不得暴露授权／配额判定入口；浏览器内一切判定均可被绕过、不得当作安全边界（方案 13.5/13.8）；Web 版强制力归属以 13.8 的单独设计定稿为准（加固与混淆边界见 docs/adr/0002）
12. AI 助手边界（F15）：AI 只生成意图/计划（含多计划束），**不得直接 apply**；工具调用只走 allowlist（只读＋`planOperation`）；文件内容/文件名默认不出端；模型输出不得作为授权/安全判定；多计划按依赖顺序、每步预览/确认，跨计划冲突必须暴露；功能可见性只由 FunctionCatalog 决定；Web/WASM 零强制力同样适用（设计 14）
13. 处理模板边界（C37）：模板只保存声明式参数与适用扩展名，**不保存脚本/宏/凭据/文件内容/绝对源路径**；应用模板必须重新预览，不得直接 apply；每步 `OperationKind` 仍由 LicenseService 复核；多步骤本机新建默认一键、导入默认逐步骤，仅低风险且可回滚步骤可自动（§7.4 信任模式），冲突/警告/高风险/授权失败立即降级逐步骤；单步 G1、多步 G2（方案 §15）
14. 持久化边界：用户设置只进 ConfigDocument；窗口/布局/最近目录/列宽/排序/模板运行偏好只进本机 UiState；任务/进度走 S03、撤销走 Journal、日志/授权/AI 对话各走专用存储，**不得写入 Config/UiState**；设置变更即保存，UiState 不导出、可重置；Web 端尽力而为、零强制力（方案 §15.3、设计 03 §11）

## 修改完成的定义（逐项自检后才算完成）
- [ ] fmt --check 与 clippy -D warnings 零警告（**以退出码判定**：禁止用管道 grep 判断；本地工具链可能滞后于 CI，新 lint 以 CI 注解为准）
- [ ] 已跑 check_scope.py 并通过，且 `cargo test --workspace` 全绿（**先确认语料已生成**，见「常用命令」的注意事项）
- [ ] 新增/改动批量写功能：有 plan/apply 结构与对应测试
- [ ] 新增/改动处理模板或持久化：有 C37/T B9 覆盖，模板不保存敏感/路径数据，UiState 不混入 Config
- [ ] 涉及 L2a 移植：PDR/台账行已批准，来源/版本/许可证/移植部件/排除项齐全，文件头/NOTICE/SBOM 与 T 语料回归通过
- [ ] 涉及 web/src-tauri：`pnpm build` 通过；`cargo check --manifest-path src-tauri/Cargo.toml` 通过（本机 WebView 依赖可用时）
- [ ] 涉及工程实现决策：已写 docs/adr/NNNN（判据见其 README）
- [ ] 修复 P0/P1 缺陷：已在 fixtures/corpus/manifest.md 挂新语料条目，并同步 manual-only.txt（如标为人工）——两处不同步会被 check_scope.py 拦下
- [ ] 已按协作纪律第 4 条在 `docs/DEVELOPMENT-PLAN.md` + `docs/design/TRACEABILITY.md`（及对应 PDR/测试/CHANGELOG/ADR）标注状态、证据与剩余缺口
- [ ] 未复制基线文档内容进代码注释或新文档

## 何时停下来问人（禁止自行决定）
- 产品行为、功能范围、优先级调整 → 提请人类走方案 11.6 流程
- 任何数值阈值（性能/大小/保留期/并发）→ 只引用方案 9.1/13.10.2 与 12.4 既定值
- 许可证判断 → 悬置并标注待人工判定，不猜测
- U v1.3 缺所需 token → 用占位 token 标 TODO，不发明新视觉值

## 协作纪律（AI 开发四条铁律）
1. **引用回核**：凡答复/文档/注释中写"依据 方案 x.y"（含 T/U 章节号），必须是当次会话打开基线原文核对过；核对不了的一律标注"未核对，待人工确认"，禁止凭记忆引用章节号、数值与码位——补全即幻觉。
2. **缺陷修复五步闭环**（缺一步不算修复完成）：① 先红后绿——复现先固化为单测或新语料，禁止未复现直接改码；② 影响面清单——动手前列出会触碰的调用方/契约/文档，修改时逐处同步；③ 最小改动半径——禁止顺手重构；④ 全量回归——按「常用命令」先生成语料再 `cargo test --workspace`（断言尚未覆盖的语料条目只做生成/范围核对），fmt/clippy/deny 门禁全过；⑤ 沉淀——P0/P1 按「修改完成的定义」挂 manifest，教训按第 3 条分流。
3. **经验沉淀分流**（按类型归位，不全塞一筐）：输入/异常场景类 bug → fixtures/corpus 新语料（CI 自动回归）；工程实现决策 → docs/adr/NNNN（判据见其 README）；工作方式/流程教训 → 先记会话记忆，人工评审确认后升格进本文件或基线文档；不满足任何一类的不沉淀。
4. **完成即标注**（项目规则）：每完成一个任务、功能或切片，必须在**相关文档的相应位置**写明状态与证据，禁止只在会话里说“做完了”。至少同步：
   - `docs/DEVELOPMENT-PLAN.md`：对应 Wave/任务改为“完成/切片/部分/未开始/阻塞”，写证据与剩余缺口；
   - `docs/design/TRACEABILITY.md`：对应契约行更新“当前实现/测试锚点/主要缺口”；
   - 涉及移植 → PDR §8/§9 + 来源台账；涉及决策 → `OPEN-QUESTIONS`/`PENDING-DECISIONS` + 基线修订；用户可见变化 → `CHANGELOG`；
   - 涉及测试/语料 → TC/T 对应行、`fixtures/corpus/manifest.md`（P0/P1 缺陷必挂新语料）；
   - 涉及工程选型 → `docs/adr/NNNN`。
   “完成”必须有可复核证据（测试命令/结果、门禁、日期、文件/章节回链）；没有证据只能写“切片/部分”，**不得把“切片完成”写成“产品完成”**。标注完成后，台账应能直接回答“谁/何时/哪条命令证明的/还差什么”。

## 前端附加约束（web/）
- 颜色/间距/字阶只用 U v1.3 语义 token，禁止裸色值与裸像素间距
- 大列表一律虚拟滚动，禁止整表渲染（方案 6.2）