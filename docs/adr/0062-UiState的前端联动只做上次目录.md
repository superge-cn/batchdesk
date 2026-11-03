# 0062. UiState 的前端联动：只做"上次目录"，用读改写而不是加命令

- **状态**：已采纳（2026-11-03，设计 03 §11 的第一处界面联动）
- **日期**：2026-11-03
- **决策者**：开发代理（工程实现决策；字段集合取自设计 03 §11.2，语义对齐内核 `UiState::remember_directory`）
- **关联**：设计 03 §11（UiState 定位/存储/schema/恢复规则）、设计 12 §2（`lib/state/` 目标目录）、ADR-0004（命令只经生成绑定）；`web/src/lib/uiState.ts`、`web/src/lib/ipc/index.ts`、`web/src/App.tsx`

---

## 1. 问题

设计 03 §11 把 UiState 定为"本机、可重置的界面/工作区上下文"，壳层与内核侧的存储、白名单 patch、`remember_directory` 早就实现了（12 条命令里有 `get/set/resetUiState`），但**前端一条都没接**——`lib/ipc` 里连 `getUiState` 都没有。结果是每次启动 C09 工作台的"扫描目录"都是空输入框，用户得一遍遍重选；`navigation.lastDirectory`／`recentDirectories` 这两个字段写了契约却没人用。

接的时候要回答四个问题：

1. 前端要不要加一条"记住目录"的专用命令？
2. `setUiState` 的 patch 是**顶层浅合并**——传 `navigation` 会整个替换，前端怎么不把兄弟字段（`lastSection`/`lastFeature`）擦掉？
3. `recentDirectories` 要不要设上限？
4. 上次目录已被删除/改名时怎么办（设计 03 §11.3 要求"不阻塞启动"）？

## 2. 候选与结论

| 议题 | 候选 | 结论 |
|---|---|---|
| 命令面 | a) 新加 `rememberDirectory` 命令；b) 复用 `getUiState`/`setUiState` 读改写 | **b**。`setUiState` 已支持白名单 patch，加专用命令等于为"一个字段的更新"扩命令面，还要同步 01/03 两份契约与壳层测试。读改写在 6 行内能写完 |
| patch 语义 | a) 只传 `{navigation: {lastDirectory}}`；b) 传完整 navigation | **b**。壳层 `apply_patch` 是**顶层浅合并**：只给 `lastDirectory` 会把 `recentDirectories`（以及 `lastSection`/`lastFeature`）一起抹掉。所以前端必须保留一份内存态 navigation，改完整体回写 |
| 去重与置顶 | a) 前端自己定规则；b) 对齐内核 `remember_directory` | **b**。内核已有实现（去重后 `insert(0)`），前端照抄同一语义；两处不一致会出现"桌面列表顺序和契约不一致"这种无解 bug |
| 最近目录上限 | a) 前端截断到 N 条；b) 不设上限 | **b**。内核没设上限，设计 03 也没给数值。前端擅自截断会让"内核认为有 20 条、界面只显示 10 条"。真要限制该改契约（并在内核侧实现），不在前端偷偷做 |
| 上次目录失效 | a) 启动前先探测存在性（要多一条命令/每启动一次 stat）；b) 直接预填，选错了让 E2001 报出来 | **b**。§11.3 的要求是"不阻塞启动"；预填只是默认值，用户改一下即可，而真跑了不存在目录时 E2001 的文案已经是"路径不存在／确认文件或目录仍在原位置后重新预览"（ADR-0061） |
| 预填时机 | a) 无条件覆盖输入框；b) 只在输入框为空时填 | **b**。用户可能已经在敲；覆盖掉正在输入的内容是最容易被骂的"智能" |
| 写失败 | a) 弹错误横幅；b) 静默 | **b**。UiState 可丢（§11.1），失败后果只是"下次不预填"；此刻用户关心的是预览结果，弹错误反而抢注意力 |
| 候选列表 | a) 只预填不放候选；b) 输入框挂 `<datalist>` | **b**。已经是"最近目录"，给候选才用得上；`<datalist>` 是原生控件，不新增样式与依赖 |

## 3. 实现

- `web/src/lib/uiState.ts`：`rememberDirectory(navigation, dir)`（去重置顶、保留兄弟字段、空串不写）与 `preferredDirectory(navigation)`（只认 `lastDirectory`，空白视为无记录）。
- `web/src/lib/ipc/`：`getUiState`／`setUiState`／`resetUiState` 三个封装（Tauri 走 `invoke`，浏览器走 `mock.ts` 的内存态；mock 与壳层同口径拒绝 `uiStateVersion`／`updatedAt` 写入）。
- `web/src/App.tsx`：挂载时读 UiState → 记住 navigation、填候选、**仅当输入框为空**时预填上次目录；生成计划成功后 `void rememberRoot(root)` 读改写回写。
- 输入框挂 `list="c09-recent-dirs"`，候选来自 `recentDirectories`（没有记录时不渲染 `datalist`）。

## 4. 已接受的坑与边界

- **只接了"上次目录"一处**：`window`（窗口矩形与离屏 clamp）、`layout`（侧栏/面板宽度）、`tables`（列宽/排序）、`templatePrefs`（一键/逐步骤）仍未接。设计 03 §11.3 的"离屏→居中主屏"需要显示器信息，属壳层职责，本批没做。
- **没有"清除最近位置"的入口**：`resetUiState` 的封装已就位但没有界面按钮；设置区目前只管 ConfigDocument（`SettingsPage.tsx` 的注释里写明了这条边界）。要加得先想清楚它是否与会话中的"清理"混在一起（OQ-12 仍在）。
- **深链/多窗口未考虑**：现在只有主窗口一个消费者；将来多窗口同时写 UiState 时，读改写会丢更新（壳层没有版本号/ETag）。真要多窗口，得给 `setUiState` 加前置条件或改成字段级 patch——那是契约变更。
- **`<datalist>` 的行为随浏览器**：Chromium 上点箭头出候选、方向键可选；样式不受 CSS 控制。这是"零成本换可用性"的取舍，将来统一输入组件时再替换。
- **act() 警告**：预填发生在 `getUiState().then` 里，测试用 `waitFor` 等它落地；React 会打一条 act 警告（非失败）。要彻底消除需把首读移到 `I18nProvider` 那样的同步初始化，代价是每个测试都要 mock 异步——不值得。

## 5. 验证

- `src/lib/uiState.test.ts` 4 项：去重置顶、兄弟字段保留、空目录不写、`preferredDirectory` 的空/空白/正常三态。
- `src/App.uiState.test.tsx` 3 项：启动预填并挂候选、生成预览后回写（去重置顶且 `lastSection` 未被擦）、无记录时不预填不渲染候选。
- 既有 `App.preflight.test.tsx`／`App.cancel-review.test.tsx` 的 `vi.mock("./lib/ipc")` 补 `getUiState`／`setUiState` 两个 stub（App 现在会调它们）。
- `tsc --noEmit` 0 错；`vitest run` 11 → 13 文件、50 → 57 项全绿。
