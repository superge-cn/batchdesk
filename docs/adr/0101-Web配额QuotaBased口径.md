# 0101. Web 配额 QuotaBased 口径

- 日期：2026-11-18
- 状态：已落地（接口＋口径；服务端持久化待设计 07）
- 相关：方案 §13.8（QuotaBased）、设计 02 §4（403/429）、设计 05 §0、AGENTS 不变量 5/11
- **裁决人**：产品所有者（2026-11-18，本口径为人类裁决，非 AI 自定）

## 裁决口径（四条）

| # | 项 | 裁决 |
|---|---|---|
| 1 | **计量单位** | **按操作次数**（每条 1 次） |
| 2 | **免费额度** | 每设备 **10 次**（`QUOTA_FREE_LIMIT`） |
| 3 | **超额行为** | **服务端拦截**——本地返回 `Capability::Quota`，服务端按设计 02 §4 的 403/429 拒绝 |
| 4 | **配额归属** | **按设备**——**登录不重置**（设备级计数；账号只用于识别，不改写配额） |

## 实现

`crates/kernel/src/license.rs` 新增 `QuotaBasedLicense`：

- `Mutex<HashMap<device, used>>` ＋ `limit`（默认 10，`with_limit` 可注入）；
- 实现 `LicenseService`（**同一接口，只换实现**——不变量 5）；
- `capability_for(device)`：未超额 `Allow`；已超额 `Quota{used,limit}`；
- `consume_for(device, n)`：超额返回 **E4004**（与桌面同一码位），**且不产生副作用**；
- `release_for(device, n)`：撤销退还，`saturating_sub` 不下溢；
- 登录/登出**不触碰计数**（设备级，天然满足「登录不重置」）。

> 订阅（商业授权）**不在本结构内**：本实现只表达**免费配额**；订阅状态由服务端在更大系统中裁决。

## 边界（不变量 11）

- **Web/WASM 零强制力**：`web-zero-force` feature 下 `apply` 的 capability/consume
  分支被裁掉（ADR-0098）——本地**不**因超额而阻断；
- **服务端拦截**：真实拦截在服务端（设计 07），本结构是**服务端/测试的进程内镜像**，
  真实服务端需自行持久化，但**同一 trait、同一口径**；
- 前端只**显示** `quota:{used,limit}` 并置灰，不判「能不能用」。

## 验证

`cargo test -p batchdesk-kernel quota_tests` **5 项全过**：

1. 免费额度＝10 次；
2. 超额返回 E4004 且不累加；
3. 按设备归属＋**登录不重置**；
4. 撤销退还不低于 0；
5. 经 `dyn LicenseService` 走 trait（不变量 5）。

## 下一批

- 服务端持久化与设备标识下发（设计 07）；
- `web/` 侧把 `quota:{used,limit}` 接到界面（显示/置灰，不判定）。
