# 0096. C10 逐文件报告：清单进 `ApplyEntryReport.detail`，压缩侧与产物同源采集

- **状态**：已采纳
- **日期**：2026-11-13
- **决策者**：开发代理（工程实现决策；产品行为与形态引方案 §5.11 的既定要求）
- **关联**：方案 v1.22 §5.11（「压缩完成后自动生成文件清单（可含哈希）」）、§6.4 输出表（压缩/解压类＝「包内清单＋体积估算」）；设计 01 §5（`ApplyEntryReport.detail`）、§580（「C10 压缩为后续切片」）；设计 06 §3（OQ-38 裁决③：条目级 detail 界面在显示）；ADR-0029（「每压缩包一条报告，不逐文件报告」）、ADR-0030（已知缺口「逐文件报告」）

---

## 1. 问题背景

TC-C10-EXE-05 的缺口里长期挂着「逐文件报告未做」。ADR-0029 写明「每压缩包一条报告（**不逐文件报告**/重试）」、ADR-0030 把「逐文件报告」列进已知缺口，设计 01 §580 进一步登记「C10 解压首版切片……**后续再按需拆 1:N**；**C10 压缩为后续切片**」。

实测确认：`c10 --compress` 的 `apply --json` 报告只有一条整包 entry（`status: ok`，无 `detail`），报告回答不了「这个包里有哪些文件、各多大」——而方案 §5.11 明确要求「压缩完成后自动生成文件清单」用于**衔接交付核验**。

## 2. 候选与否决理由

| 议题 | 候选 | 结论 |
|---|---|---|
| 交付形态 | a) 计划拆成 1:N 条目（每文件一条 entry）；b) **在既有 summary 条目的 `detail` 里放逐文件清单** | **b**：ADR-0029 的「不逐文件报告」指的是**报告结构**；拆 1:N 要动计划结构、Journal 语义、撤销分组与并发口径（§580 明确"后续再按需"），代价远大于本缺口。`ApplyEntryReport.detail`（设计 01 §5、ADR-0050）正是为「执行计数类技术行」留的载体 |
| 明细放哪 | a) `error.detail`／`IpcError.detail`；b) **`ApplyEntryReport.detail`** | **b**：设计 06 §3 明确两者不得混称——`IpcError.detail` 常含绝对路径与堆栈、**仅进诊断日志**；`ApplyEntryReport.detail` 是内核产出的执行技术行、**不含绝对路径、界面在显示**（OQ-38 裁决③）。ZIP 条目名是包内相对名，符合后者 |
| 清单从哪来 | a) 压缩后再解析一遍产物 ZIP；b) **在遍历源的同时采集** | **b**：a 对大压缩包是整整多一遍全量读（2 GiB 包就是多读 2 GiB）；`add_path` 本来就在逐个文件写盘，顺手登记即可 |
| 哈希 | a) 默认算；b) **默认不算** | **b**：方案写的是「**可含**哈希」；对每个源文件额外一遍全量读，代价与压缩本身同量级。留作后续可选项，不在本次默认路径上 |
| 界面 | a) 只进数据面；b) **功能页终态横幅渲染 `detail`** | **b**：设计 06 §3 说 `ApplyEntryReport.detail`「界面在显示」，而功能页此前只显示「执行完成」——清单只躺在 JSON 报告里对用户没价值 |
| 展示上限 | a) 全量；b) **200 条截断并注明剩余** | **b**：`detail` 是展示字段；C-1 级的十万文件包全量塞进界面会失控。完整清单以 ZIP 本体为准（任意解压工具可列） |

## 3. 决策

1. **数据契约**：`ArchiveService::compress` 的返回从 `Vec<u8>` 改为 `CompressedArchive { bytes, manifest }`；`manifest: Vec<ArchiveManifestItem { name, size }>`（未压缩体积、按写入顺序、不含目录条目）。
2. **格式化**：`format_archive_manifest(items, limit)` 产出
   `N 个文件，共 X 字节` ＋逐行 `名称 体积`；超 `limit`（200）截断并附 `… 其余 M 项略`。
3. **压缩侧**：`add_path` 在 `start_file` 之前登记（写失败时整体报错、清单随之丢弃，不留"半份清单"）。
4. **解压侧**：在执行循环里收集**真正写出的文件**（`selection.selected` 中被写入且未占用的那些），目录条目不计。
5. **界面**：`FeaturePages` 的终态横幅渲染 `task.report.entries[].detail`（新增 `.manifest` 样式：等宽＋`pre-line` 保留换行）；`FeaturePages.TaskView` 补 `report?: ApplyReport`，done 事件到达时一并 patch（报告随 `listenTaskDone` 的 `event.summary` 到达，不必等 `getTask` 回读）。
6. **导出**：CLI 的 `apply --json` 报告即导出通道（已含 `detail`）；任务快照亦保留。

## 4. 后果

- **内核**：`archive.rs` 加 `ArchiveManifestItem`／`CompressedArchive`／`format_archive_manifest` 与 `MANIFEST_DETAIL_LIMIT`；`apply_archive_plan` 两条成功路径填 `detail`。
- **平台**：`compress`／`add_path` 采集清单（`add_path` 增一个参数，`#[allow(clippy::too_many_arguments)]`）。
- **前端**：`FeaturePages.tsx` 终态渲染明细、`TaskView` 补 `report`；`app.css` 加 `.manifest`；新增 `FeaturePages.manifest.test.tsx` 两项。
- **调用点**：`compress` 的 4 处调用（1 处生产 + 3 处测试桩）同步改为 `.bytes`／新返回类型。
- **不变**：计划结构（仍是每压缩包一条 summary entry）、Journal/撤销语义、并发口径均未触碰——这是与「拆 1:N」的本质区别。

## 5. 残余

1. **1:N 条目拆分未做**（设计 01 §580 的"后续再按需"）——那要动计划/Journal/撤销分组，是另一个切片。
2. **哈希未默认计算**（方案「可含」）：需要时再加一个显式开关，避免默认路径上多一遍全量读。
3. **解压侧的"占用跳过"条目不进清单**：清单反映的是"实际写出了什么"，跳过的条目由报告的状态行表达。
