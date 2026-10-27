# AGENTS.md
> 本文件供 AI 编码代理读取。人类协作者请读 README.md 与 CONTRIBUTING.md。
> 基线文档引用一律"文档名+章节号"，数值以基线为准，禁止在代码注释中另立数值。

## 快速事实
- 栈：Rust workspace（crates/kernel 领域引擎 + src-tauri 桌面壳）+ TypeScript/shadcn 前端（web/）
- 内核需可编译为 WASM 复用于 Web 版（方案 13.5）——kernel 不得依赖 Tauri/桌面 API
- 包管理：Cargo + pnpm；CI：GitHub Actions（四 job：lint/deny/test/bench）

## 仓库地图
| 位置 | 职责 | 文档 |
|---|---|---|
| crates/kernel | 全部领域引擎（纯函数变换器层在此） | 方案 4.2/4.3 |
| src-tauri | 桌面壳、任务队列、文件系统适配、授权注入 | 方案 4.4 |
| web/ | 表现层，无业务逻辑 | 方案 4.2 / U v1.1 |
| fixtures/corpus | 28 条异常语料（回归测试输入） | T 3.2 |
| fixtures/bench | 性能基准数据集 C-1…C-8 | T 附录 C |
| docs/adr/ | 工程决策记录 | docs/adr/README |

## 常用命令
- 构建：cargo build --workspace
- 语料回归前的准备（**必须先跑**，否则语料回归无输入）：python fixtures/corpus/check_scope.py（范围门禁）→ 依次执行 fixtures/corpus/*/gen_*.py（幂等生成样本）
- 全量测试（含语料回归）：cargo test --workspace
  - ⚠️ 注意：`cargo test` 本身**不生成语料**。未先跑上一步时，语料回归等于没跑——不要据此判定"语料回归已通过"。
  - ⚠️ 当前语料**尚未建全**：manifest.md 里标「待补」的条目还没有脚本；标「人工」的条目不由 CI 跑（清单见 fixtures/corpus/manual-only.txt）。CI 绿灯只代表"自动化语料零失败"。
- 单 crate 测试：cargo test -p kernel
- 门禁：cargo fmt --check；cargo clippy --workspace --all-targets -- -D warnings；cargo deny check
- 基准：cargo bench（仅主分支对比基线）

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

## 修改完成的定义（逐项自检后才算完成）
- [ ] fmt --check 与 clippy -D warnings 零警告
- [ ] 已跑 check_scope.py 并通过，且 `cargo test --workspace` 全绿（**先确认语料已生成**，见「常用命令」的注意事项）
- [ ] 新增/改动批量写功能：有 plan/apply 结构与对应测试
- [ ] 涉及工程实现决策：已写 docs/adr/NNNN（判据见其 README）
- [ ] 修复 P0/P1 缺陷：已在 fixtures/corpus/manifest.md 挂新语料条目，并同步 manual-only.txt（如标为人工）——两处不同步会被 check_scope.py 拦下
- [ ] 未复制基线文档内容进代码注释或新文档

## 何时停下来问人（禁止自行决定）
- 产品行为、功能范围、优先级调整 → 提请人类走方案 11.6 流程
- 任何数值阈值（性能/大小/保留期/并发）→ 只引用方案 9.1/13.10.2 与 12.4 既定值
- 许可证判断 → 悬置并标注待人工判定，不猜测
- U v1.1 缺所需 token → 用占位 token 标 TODO，不发明新视觉值

## 前端附加约束（web/）
- 颜色/间距/字阶只用 U v1.1 语义 token，禁止裸色值与裸像素间距
- 大列表一律虚拟滚动，禁止整表渲染（方案 6.2）