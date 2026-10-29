# 基线文档清单（版本与哈希）

> 本文件记录 `docs/baseline/` 四份基线的**版本、落位文件名与快照哈希**，用于发现“文件名对了、内容被静默改动”的情况。
>
> 哈希不是权威，只做漂移检测：基线升版后，按 `docs/baseline/README.md` 的流程整体替换文件，并同步更新本表与根 README 的版本表。

| 文档 | 版本 | 文件 | sha256 | 快照日期 |
|---|---|---|---|---|
| 《办公辅助工具产品方案》 | v1.19 | `产品方案-v1.19.md` | `32a8ca43153e6cde5777bb9ed3620c4029a58dc207f7b6f6462e80c85ed85065` | 2026-10-28 |
| 《功能测试方案与验收标准》 | T v1.12 | `功能测试方案与验收标准-T-v1.12.md` | `10adbc7146dffa631db254bc1f247dfd815ba5e8d1370e748a9ad391522259fa` | 2026-10-28 |
| 《UI 视觉规范》 | U v1.3 | `UI视觉规范-U-v1.3.md` | `9541732716d2a10bdea673af4338b855ca89c05219fdab1d43e798d4747a17c2` | 2026-10-28 |
| 《原型走查包》 | 独立附件 | `原型走查包.md` | `884b695f0ed09165237ad3a55c71ac68c981f49e080cb5c68f99693fbe2b6500` | 2026-10-28 |

## 校验方式

```bash
sha256sum docs/baseline/产品方案-v1.19.md \
          docs/baseline/功能测试方案与验收标准-T-v1.12.md \
          docs/baseline/UI视觉规范-U-v1.3.md \
          docs/baseline/原型走查包.md
```

## 升版纪律

1. 只替换 `docs/baseline/` 内的文件，不在本仓库编辑基线正文；
2. 更新本表、`docs/baseline/README.md` 的版本表与根 `README.md` 的基线文档地图；
3. 按 `docs/baseline/README.md` §2 核对所有引用章节号的文档（含 `docs/design/` 与 `docs/testing/`）。
