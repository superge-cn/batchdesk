# K03 预期摘要

| 项 | 内容 |
|---|---|
| 类别 | 编码 |
| 样本特征 | UTF-8 带 BOM 中文 CSV（BOM 为 `EF BB BF`） |
| 预期行为 | BOM 优先命中，导出策略符合方案 §5.2 |
| 判定权威 | T v1.12 §3.2 的 **K03** 行（涉及功能与套件归属以该行为准） |

生成：`python fixtures/corpus/K03/gen_k03.py` → `out/K03.csv`（幂等）
自校验：以 UTF-8 BOM 开头；剥 BOM 后为合法 UTF-8 且首行为表头
