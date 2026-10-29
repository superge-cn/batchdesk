# K02 预期摘要

| 项 | 内容 |
|---|---|
| 类别 | 编码 |
| 样本特征 | UTF-8 无 BOM 中文 CSV（8 条数据行，CRLF） |
| 预期行为 | 判 UTF-8，不误判为 GBK（方案 §5.2 检测流水线） |
| 判定权威 | T v1.12 §3.2 的 **K02** 行（涉及功能与套件归属以该行为准） |

生成：`python fixtures/corpus/K02/gen_k02.py` → `out/K02.csv`（幂等）
自校验：合法 UTF-8、含非 ASCII、无 BOM
