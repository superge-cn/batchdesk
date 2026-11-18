# 0098. WASM 导出面骨架与零强制力 feature

- 日期：2026-11-18
- 状态：已落地（Wave 5 首块骨架）
- 相关：设计 05《Web 强制力边界契约》§0/§2/§6；AGENTS 不变量 11；方案 §13.5/§13.8

## 背景

Wave 5 要开 Web/WASM 形态。设计 05 §0 的三条结论要求：

1. Web 版强制力只在云端服务端，本地模式对授权/配额**零强制力**；
2. `web/` 不含判定逻辑；
3. **WASM 公开导出面不得暴露授权/配额判定入口**。

设计 05 §2 进一步要求把「约定不调用」升级为**编译期 feature 裁掉**——使 WASM 产物在二进制层面不存在授权阻断路径，验收从「审代码」升级为「审导出面与符号表」。

## 决策

### 1. 新增 `crates/wasm`（workspace 成员）

- 目标 `wasm32-unknown-unknown`，`crate-type = ["cdylib", "rlib"]`；
- 只读处理能力用 **JSON 字符串接口**（`fn(&str) -> Result<String, JsValue>`），
  避开逐字段 `wasm-bindgen` 绑定——输入 DTO 链复杂（`NameRules` 含 `&'static [char]`
  这类不可反序列化字段，强行绑定会把契约细节拖进导出面）。

### 2. 首批导出面（本骨架）

| 入口 | 类别 | 说明 |
|---|---|---|
| `browser_capabilities()` | 能力查询 | 返回 `{form, localMode, exportVersion}`；**刻意不含授权/配额字段** |
| `plan_c01(json)` | 处理入口（只读） | C01 转码计划；不落盘、不执行 |

**未导出**（有意）：`plan_f12`——其输入链含 `NameRules { illegal_chars: &'static [char] }`
等不可反序列化字段，需先设计「平台规则如何进入 Web 形态」（本地规则内置？还是服务端下发？），
属下一批。**不为了凑数把契约细节塞进导出面**。

### 3. `web-zero-force` feature

- kernel 新增 feature（`crates/kernel/Cargo.toml`），wasm crate 转发；
- 启用后编译期裁掉基于 `LicenseService` 的阻断分支（设计 05 §2 的验收升级）；
- **裁剪点已落地（2026-11-18 续）**：`apply.rs` 的 `capability(C09)` 与 `consume(C09,1)` 两处按 feature 门控（`#[cfg(...)]` 分支）；`release` 保持调用（Web 形态注入的实现为 no-op）。
  影响面：`capability` 的 Deny/Quota 分支（E4001/E4004）与 `consume` 的错误路径；其余 5 处 `release` 不受影响。
  行为断言：新增 `web_zero_force_ignores_deny_license`（feature 下 DenyLicense 不阻断，条目 `Ok`）；
  既有 `denied_license_does_not_touch_the_file` 以 `#[cfg(not(feature = "web-zero-force"))]` 门控（语义相反）。

### 4. 导出面断言（两层）

`crates/wasm/tests/export_surface.rs`：

1. **源码层**：白名单（只允许登记入口）＋ 黑名单（`consume`/`release`/`license`/`quota`/
   `activation`/`signature`/`entitlement` 等出现在非注释代码即失败）；
2. **产物层**：用 Node 的 `WebAssembly.Module.exports()` 读**真实 wasm 导出表**，
   断言只含白名单入口（产物缺失时跳过，native `cargo test` 不受影响）。

实测（debug 产物）：非绑定内部导出仅 `browser_capabilities_*` 与 `plan_c01_*`，**无授权/配额符号**。

## 影响面

- `crates/kernel/src/plan.rs`：`PlanC01File`/`PlanC01Input` 补 `serde::Serialize/Deserialize`
  （原本没有；其余嵌套类型如 `TargetEncoding`/`CaseSense`/`ProtectedDir` **已有** serde，无需改）；
- `Cargo.toml`：workspace 成员 + `web-zero-force` 依赖转发；
- 新增：`crates/wasm/{Cargo.toml,src/lib.rs,tests/export_surface.rs}`、本 ADR。

## 验证

- `cargo test -p batchdesk-wasm --test export_surface` **4 项全过**；
- `cargo build -p batchdesk-wasm --target wasm32-unknown-unknown --features web-zero-force` 通过（34.5 MB debug）；
- `cargo test --workspace` **15 个二进制全绿**；
- fmt 通过；clippy 对本次 crate 无新增警告（`replace_content.rs` 的两条为**既有**问题，
  stash 后在 HEAD 上复现同样 2 条——本地 clippy 版本新于 CI，以 CI 注解为准）。

## 下一批

1. `apply.rs` 授权分支按 `web-zero-force` 门控（影响面清单：`apply.rs` 65 处 license 相关调用）；
2. F12 等其余功能的导出（先定「平台规则如何进入 Web 形态」）；
3. Worker + `web/` 侧接入（设计 02/05 的会话/配额由服务端下发）。
