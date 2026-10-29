# 设计评审资产

## 液态玻璃样式示例（已收编 U v1.3）

- 确认结论（2026-10-28，用户）：浅色/深色两版都保留；由主题设置切换；**浅色为默认**。
- 浅色模式：[`light-liquid-glass-ui.png`](./light-liquid-glass-ui.png)（评审图）／[`light-liquid-glass-ui.html`](./light-liquid-glass-ui.html)（可改样式源文件）
- 深色模式：[`dark-liquid-glass-ui.png`](./dark-liquid-glass-ui.png)（评审图）／[`dark-liquid-glass-ui.html`](./dark-liquid-glass-ui.html)（可改样式源文件）
- 用途：已确认并收编到 U v1.3 的液态玻璃样式参考；实现以 U v1.3 §5.3 为准。
- 取色（由用户提供的 Deepin 风格壁纸截图取样）：
  - 亮部 `#78BCFC`、高光青 `#0EA9FC`、深部蓝 `#2359FC`；
  - 浅色模式用亮蓝/青做大面积环境光，乳白蓝玻璃面板；
  - 深色模式保留深蓝底，但比首版抬亮一档，减少近黑感；品牌主色 `#2F6BFF`（已确认），高光用 `#22C7FF`。
- 效果构成：半透明面板 + `backdrop-filter: blur()/saturate()`、双层高光描边、内阴影、背景液态曲线、细噪点。
- 核对要点：浅色模式是否过白/过灰；深色模式是否仍偏黑；两模式的蓝色是否与安全/警告/危险状态色混淆；表格与 AI 面板的可读性。
- 字号以 U v1.3 的 12/13/14/16/20 档位为主；少量审计/标注文字在示例中压缩，落实现时按 U token 复核。
- 基线与决策：OQ-24/D-U5 与 OQ-03/D-U3 已收编 U v1.3。`--brand-500 = #2F6BFF`；实心主按钮按 U v1.3 使用 brand-600 `#1C4FE0`，`#2F6BFF` 用于焦点环/选中/高光。

## OQ-03 品牌主色（已确认 #2F6BFF）

- 2026-10-28 用户直接指定 `--brand-500 = #2F6BFF`，不在原 A/B/C 三候选内；下表保留为历史对照。
- 文件：[`brand-color-candidates.png`](./brand-color-candidates.png)（版面示例）／[`brand-color-candidates.html`](./brand-color-candidates.html)（可改色源文件）
- 用途：OQ-03 审核留档；使用 U v1.3 的中性色与状态色，品牌色为**历史候选示例值**。
- 候选色（历史对照）：

| 方向 | brand-500 | brand-600 | brand-100 |
|---|---|---|---|
| A 沉稳蓝（推荐） | `#0B5CAD` | `#084A8C` | `#E6F0FA` |
| B 青绿 | `#00796B` | `#00665A` | `#E0F2EF` |
| C 深青灰 | `#2F4F4F` | `#263F3F` | `#E6EDED` |

- 已选 `#2F6BFF` 的核对：H≈222.7°，与 safe/warn/danger 的色相间隔约 71.5°/179.5°/140.5°，均 ≥30°；白字 on brand-500 约 4.499:1（略低于 4.5），实心主按钮按 U v1.3 使用 brand-600 `#1C4FE0`（6.47:1）。
- 注意：B 青绿与 safe 绿（`#17794A`）色相距离较近，U 已提示需按 ≥30° 规则核对；该方向未被采用。


