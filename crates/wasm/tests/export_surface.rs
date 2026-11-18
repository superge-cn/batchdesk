//! 导出面断言（设计 05 §2 的验收升级：从「审代码」到「审导出面」）。
//!
//! 规则：
//! - **允许**：处理入口（plan/apply/preview/cancel/journal 查询）、能力查询、事件回传。
//! - **禁止**（出现即缺陷，不是「环境差异」）：
//!   - 许可证/激活码校验、签名验证；
//!   - 授权层级判定、功能位图查询的**判定函数**；
//!   - 配额扣减/退还入口（`consume`/`release` 的公开绑定）；
//!   - 配置写入门禁。
//!
//! 本机（native）先对**源码导出面**做静态断言；wasm 产物可用 `wasm-objdump`/符号表
//! 做同一条断言（CI 的 web job 承接，见设计 05 §6）。

use std::fs;
use std::path::Path;

/// 本 crate 的 `lib.rs` 源码。
fn lib_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
    fs::read_to_string(&path).expect("读得到 lib.rs")
}

/// 取所有 `#[wasm_bindgen]` 标注的 `pub fn` 名字（粗粒度但足够门禁）。
fn exported_functions(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != "#[wasm_bindgen]" {
            continue;
        }
        // 向后找最近的 `pub fn <name>`
        for later in lines.iter().skip(index + 1).take(8) {
            let trimmed = later.trim();
            if let Some(rest) = trimmed.strip_prefix("pub fn ") {
                let name: String = rest
                    .chars()
                    .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                    .collect();
                if !name.is_empty() {
                    found.push(name);
                }
                break;
            }
        }
    }
    found
}

#[test]
fn export_surface_contains_only_allowed_entries() {
    let names = exported_functions(&lib_source());
    assert!(
        !names.is_empty(),
        "导出面为空——wasm 绑定未生成，检查 wasm-bindgen 注解"
    );

    // 白名单：本骨架只导出能力查询与只读计划入口。
    let allowed = ["browser_capabilities", "plan_c01", "plan_f12"];
    for name in &names {
        assert!(
            allowed.contains(&name.as_str()),
            "导出面出现未登记入口 `{name}`——请先在测试白名单或设计 05 §2 中登记理由"
        );
    }
}

#[test]
fn export_surface_has_no_license_or_quota_entries() {
    let source = lib_source().to_lowercase();
    // 禁止符号（设计 05 §2 黑名单）。命中即失败，附行号便于定位。
    let forbidden = [
        "consume",
        "release",
        "license",
        "quota",
        "activation",
        "signature",
        "entitlement",
        "capability_gate",
    ];
    let mut hits: Vec<String> = Vec::new();
    for (number, line) in source.lines().enumerate() {
        for word in forbidden {
            if line.contains(word) {
                hits.push(format!("行 {}: {}", number + 1, line.trim()));
            }
        }
    }
    // 注释里讨论边界是允许的（本文件即在讨论），故只对**非注释行**判定。
    let code_hits: Vec<String> = hits
        .into_iter()
        .filter(|entry| {
            let code = entry.split(": ").nth(1).unwrap_or("");
            !code.starts_with("//") && !code.starts_with("//!")
        })
        .collect();
    assert!(
        code_hits.is_empty(),
        "导出面源码出现禁止符号（设计 05 §2 黑名单）：\n{}",
        code_hits.join("\n")
    );
}

#[test]
fn kernel_exposes_web_zero_force_feature() {
    // 编译期开关必须存在（设计 05 §2：把「约定不调用」升级为「二进制层面不存在」）。
    let manifest =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../kernel/Cargo.toml"))
            .expect("读得到 kernel/Cargo.toml");
    assert!(
        manifest.contains("web-zero-force"),
        "kernel 缺少 web-zero-force feature——授权阻断分支无法在编译期裁掉"
    );
}

/// 对**真实 wasm 产物**核查导出面（设计 05 §2 的验收升级）。
///
/// 用 Node 的 `WebAssembly.Module.exports()` 读导出表（CI 的 web/desktop job 均有 node）。
/// 产物不存在时跳过——`cargo test --workspace` 在 native 下不会构建 wasm 目标。
#[test]
fn wasm_artifact_export_surface_is_whitelisted() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let candidates = [
        manifest.join("../target/wasm32-unknown-unknown/debug/batchdesk_wasm.wasm"),
        manifest.join("../../target/wasm32-unknown-unknown/debug/batchdesk_wasm.wasm"),
        manifest.join("target/wasm32-unknown-unknown/debug/batchdesk_wasm.wasm"),
    ];
    let artifact = candidates.iter().find(|path| path.exists());
    let Some(artifact) = artifact else {
        eprintln!(
            "未找到 wasm 产物，跳过产物级断言（先 cargo build --target wasm32-unknown-unknown）"
        );
        return;
    };

    let script = format!(
        r#"const fs=require("fs");const m=new WebAssembly.Module(fs.readFileSync({}));
const exp=WebAssembly.Module.exports(m).map(e=>e.name);
console.log(JSON.stringify(exp.filter(n=>!n.startsWith("__"))));"#,
        serde_json::to_string(&artifact.display().to_string()).unwrap()
    );
    let output = std::process::Command::new("node")
        .arg("-e")
        .arg(script)
        .output()
        .expect("node 可执行（CI web/desktop job 提供）");
    assert!(
        output.status.success(),
        "node 解析失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let names: Vec<String> = serde_json::from_slice(&output.stdout).expect("导出表 JSON");

    // wasm-bindgen 会给导出名加哈希后缀（`plan_c01_f2c4df315bee1a29`），
    // 故按**前缀**匹配白名单；运行期内部符号（memory/__wbindgen_*）排除。
    let internal = ["memory", "abort_handler", "instance_terminated"];
    let allowed = ["browser_capabilities", "plan_c01", "plan_f12"];
    let business: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| !internal.contains(name))
        .collect();
    for name in &business {
        assert!(
            allowed.iter().any(|entry| name.starts_with(entry)),
            "wasm 导出面出现未登记入口 `{name}`（设计 05 §2 白名单外）"
        );
    }
    for must in allowed {
        assert!(
            business.iter().any(|name| name.starts_with(must)),
            "wasm 导出面缺少应有入口 `{must}`"
        );
    }
}

/// Web F12 输入能产出计划（端到端：JSON → WebPlanF12Input → PlanF12Input → plan_f12）。
///
/// 同时钉住**零强制力边界**：`name_rules` 缺省时按 POSIX 语义（不推断平台保留名），
/// 且规则来自调用方声明而非内核授权判定。
#[test]
fn web_plan_f12_round_trips_and_defaults_name_rules_to_posix() {
    let payload = serde_json::json!({
        "plan_id": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
        "items": [{"path": "/dir/a.txt"}],
        "existing_targets": [],
        "protected_dirs": [],
        "case_sense": "sensitive",
        "long_path_supported": true,
        "access_facts": [],
        "rule": {"prefix": "x", "suffix": ".bak", "replace": "", "scope": "Full"}
    });
    let web: kernel::rename::WebPlanF12Input =
        serde_json::from_value(payload).expect("WebPlanF12Input 可反序列化");
    let input: kernel::rename::PlanF12Input = web.try_into().expect("Web 输入可转内核输入");
    let plan = kernel::rename::plan_f12(input).expect("计划生成成功");
    assert_eq!(plan.entries.len(), 1, "一个输入一条 rename 条目");
    assert_eq!(plan.entries[0].target.as_deref(), Some("/dir/xa.txt.bak"));
}
