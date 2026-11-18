//! BatchDesk Web 形态的 WASM 导出面（Wave 5 首块骨架，设计 05 §2）。
//!
//! 三条边界（设计 05 §0 结论）：
//! 1. Web 版强制力只存在于云端服务端；本地模式对授权/配额**零强制力**——不拦截、不放行、不判档。
//! 2. `web/` 不含判定逻辑，只按内核/服务端已给的结果做显示。
//! 3. **本导出面不得出现授权/配额判定入口**（许可证校验、层级判定、`consume`/`release` 公开绑定）。
//!
//! 因此这里只导出两类东西：
//! - **处理入口**：`plan_*`（只读产出计划，不落盘、不执行）；
//! - **能力查询**：`browser_capabilities`（只返回环境事实）。
//!
//! 导出面由 `crates/wasm/tests/export_surface.rs` 断言（出现禁止符号即失败），
//! 编译期 feature `web-zero-force` 裁掉 kernel 的授权阻断路径。

use wasm_bindgen::prelude::*;

/// 环境能力事实（只读，JSON 字符串）。
///
/// 字段刻意只含环境事实；**不含授权/配额**（那些由服务端下发，不经此入口）。
#[wasm_bindgen]
pub fn browser_capabilities() -> String {
    let capabilities = serde_json::json!({
        "form": "web",
        "localMode": true,
        "exportVersion": env!("CARGO_PKG_VERSION"),
    });
    capabilities.to_string()
}

/// C01 转码计划（只读）：入参/出参均为 JSON 字符串。
///
/// 失败时返回 `Err(JsValue)`，错误码沿用 kernel 的 `E1xxx–E9xxx`（设计 06 码位）。
#[wasm_bindgen]
pub fn plan_c01(json: &str) -> Result<String, JsValue> {
    let input: kernel::plan::PlanC01Input = serde_json::from_str(json)
        .map_err(|error| JsValue::from_str(&format!("E1002: {error}")))?;
    let plan = kernel::plan::plan_c01(input)
        .map_err(|error| JsValue::from_str(&format!("{}: {}", error.code, error.message)))?;
    serde_json::to_string(&plan).map_err(|error| JsValue::from_str(&format!("E1002: {error}")))
}

/// F12 改名计划（只读）：入参/出参均为 JSON 字符串。
///
/// 目标名合法性规则由调用方（服务端下发或按目标卷选择）传入；**缺省＝POSIX 语义**
/// （保守：只拦 `/` 与 NUL，不额外推断平台保留名）。这是**平台事实**而非授权判定
/// （不变量 11、设计 05 §2）。
#[wasm_bindgen]
pub fn plan_f12(json: &str) -> Result<String, JsValue> {
    let input: kernel::rename::WebPlanF12Input = serde_json::from_str(json)
        .map_err(|error| JsValue::from_str(&format!("E1002: {error}")))?;
    let input: kernel::rename::PlanF12Input = input
        .try_into()
        .map_err(|error: serde_json::Error| JsValue::from_str(&format!("E1002: {error}")))?;
    let plan = kernel::rename::plan_f12(input)
        .map_err(|error| JsValue::from_str(&format!("{}: {}", error.code, error.message)))?;
    serde_json::to_string(&plan).map_err(|error| JsValue::from_str(&format!("E1002: {error}")))
}
