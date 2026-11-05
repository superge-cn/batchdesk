# fixtures/archive：归档格式的**互通性**夹具

不属于 T v1.12 §3.2 的 28 条语料体系（那套在 `../corpus/`），也不属于性能数据集（`../bench/`）。
这里放的是**必须由第三方实现产出**才能成立的样本：加密 ZIP 的密码学部分，Python 标准库
（`zipfile`）只能读、不能写，用自家 `zip` crate 造样本又会让"写"和"读"共用同一处缺陷，
测出来的是自洽而不是互通。

## 1. 入库策略（与 `fixtures/README.md` §4 的偏差及理由）

`fixtures/README.md` 的规则是"生成物不入库，只入库脚本与清单"。这两个包是**例外**：

| 理由 | 说明 |
|---|---|
| 只能由第三方工具生成 | `zip -P`（Info-ZIP，ZipCrypto）与 `7z -mem=AES256`（7-Zip，AES-256）；Python 无法产出加密 ZIP |
| 生成物必须字节固定 | 断言比的是"能不能用已知密码解出已知内容"；样本一变，失败原因就分不清是代码退化还是样本换了 |
| CI 不保证装有 7-Zip | 让 CI 在测试里 shell out 生成，会把"环境缺工具"变成红灯 |

体积：`zipcrypto.zip` 379 B、`aes256.zip` 391 B。

## 2. 内容与构造

两个包**内容完全相同**（同一条断言跑两遍算法）：

| 路径 | 内容 | 明文大小 |
|---|---|---|
| `a.txt` | `hello zip\n` | 10 B |
| `sub/b.txt` | `second\n` | 7 B |

密码一律 `secret123`（测试里是常量，只用于夹具，不是任何真实凭据）。

```bash
# ZipCrypto（传统加密，7-Zip / WinRAR 对 .zip 的默认档）
mkdir -p src/sub && printf 'hello zip\n' > src/a.txt && printf 'second\n' > src/sub/b.txt
(cd src && zip -q -P secret123 ../zipcrypto.zip a.txt sub/b.txt)

# AES-256（WinZip AE-2，7-Zip 勾选"加密强度"时产出）
(cd src && 7z a -tzip -psecret123 -mem=AES256 ../aes256.zip a.txt sub/b.txt)
```

两个包的 SHA-256（当前入库版本）：

```
cb2a80739d7cfa081bb6d5e16ea55855ba7d4f3b9826a77166c5fcb2c75f14dc  zipcrypto.zip
5902f5378225110a6c0e067d85a68a1bda1c8ab4ca227ceb3ab39232a4b79af7  aes256.zip
```

重新生成时哈希会变（ZIP 里带时间戳），语义不变即可；变更时请同步本节的哈希。
`src/` 只是生成时的临时工作目录（明文源文件），生成完即可删除，不入库。

## 3. 谁在用

| 测试 | 断言 |
|---|---|
| `crates/platform/tests/archive_password.rs::inspect_lists_encrypted_entries_without_a_password` | 无需密码即可 `inspect` 出条目名与 `encrypted=true`（走 `by_index_data`，只读中央目录） |
| `crates/platform/tests/archive_password.rs`（其余 5 项） | 对密码解出内容并逐文件进 Journal；错密码 `E3009` 且零产物；缺密码整单 `E3008`；空串等同缺密码；AES-256 同样通过 |
| `crates/cli/tests/apply_flow.rs`（3 项端到端） | 计划期 `--expect-password` 只记布尔（计划文件里搜不到密码）；执行期 `--password-stdin` 解出；错密码报 `E3009` |
| `src-tauri/src/tasks.rs::the_archive_password_never_reaches_the_task_snapshot` | 密码经壳层队列解出文件，而落盘快照里没有 `secret` 字段 |

契约与决策见 [ADR-0074](../../docs/adr/0074-C10密码包的秘密注入面.md)。
