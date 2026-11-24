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
    let allowed = [
        "browser_capabilities",
        "plan_c01",
        "plan_c02",
        "plan_c03",
        // A2-3：CSV 家族（ADR-0102）
        "plan_c11",
        "plan_c12",
        "plan_f06",
        "plan_f07",
        "plan_f08",
        "plan_f12",
        "plan_c14",
        "plan_f09",
        "plan_f01",
        "plan_f02",
        "plan_f03",
        "plan_f05",
        "plan_c07",
        "plan_c08",
        "plan_f10",
        "plan_f11",
        "plan_f13",
        "plan_c04",
        "plan_c09",
        "plan_c13",
        "plan_f04",
        // A2-7：C10 中央目录事实 → 解压计划（ADR-0102/0103）
        "plan_c10",
        // A3-5：C10 压缩侧（ADR-0107）
        "plan_c10_compress",
        // A3-4：C07 清单解析下沉内核，Web 走同一实现（ADR-0106）
        "parse_c07_manifest",
        // A3-6：Web 执行面（ADR-0108）——沙箱登记/执行/撤销/产物读取
        "web_exec_stage",
        "web_exec_apply",
        "web_exec_undo",
        "web_exec_product",
        "web_exec_reset",
        // A3-12：F10 的目录与目录事实登记（ADR-0114）——只登记事实，不碰用户磁盘
        "web_exec_stage_dirs",
        "web_exec_stage_listing",
    ];
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
    // 黑名单词（设计 05 §2：导出面不得出现授权/配额判定入口）。
    //
    // `quota` 原为裸子串匹配，但 A2-4 起 kernel 的**分片行配额**函数名
    // （`part_line_quotas` / `new_quotas`，与授权无关）也被命中——那是误判。
    // 现收窄为授权配额的**语义**符号：实现类型 `QuotaBased`（ADR-0101）与
    // LicenseService 的 `quota_exhausted` 判定。分片配额不在禁止之列。
    let forbidden = [
        "consume",
        "release",
        "license",
        "QuotaBased",
        "quota_exhausted",
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
    let allowed = [
        "browser_capabilities",
        "plan_c01",
        "plan_c02",
        "plan_c03",
        // A2-3：CSV 家族（ADR-0102）
        "plan_c11",
        "plan_c12",
        "plan_f06",
        "plan_f07",
        "plan_f08",
        "plan_f12",
        "plan_c14",
        "plan_f09",
        "plan_f01",
        "plan_f02",
        "plan_f03",
        "plan_f05",
        "plan_c07",
        "plan_c08",
        "plan_f10",
        "plan_f11",
        "plan_f13",
        "plan_c04",
        "plan_c09",
        "plan_c13",
        "plan_f04",
        // A2-7：C10 中央目录事实 → 解压计划（ADR-0102/0103）
        "plan_c10",
        // A3-5：C10 压缩侧（ADR-0107）
        "plan_c10_compress",
        // A3-4：C07 清单解析下沉内核，Web 走同一实现（ADR-0106）
        "parse_c07_manifest",
        // A3-6：Web 执行面（ADR-0108）——沙箱登记/执行/撤销/产物读取
        "web_exec_stage",
        "web_exec_apply",
        "web_exec_undo",
        "web_exec_product",
        "web_exec_reset",
        // A3-12：F10 的目录与目录事实登记（ADR-0114）——只登记事实，不碰用户磁盘
        "web_exec_stage_dirs",
        "web_exec_stage_listing",
    ];
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
    // 「必须有」只在**产物存在**时断言；否则上面已提前返回。
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
        "planId": "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
        "items": [{"path": "/dir/a.txt"}],
        "existingTargets": [],
        "protectedDirs": [],
        "caseSense": "sensitive",
        "longPathSupported": true,
        "accessFacts": [],
        "rule": {"prefix": "x", "suffix": ".bak", "replace": "", "scope": "Full"}
    });
    let web: kernel::rename::WebPlanF12Input =
        serde_json::from_value(payload).expect("WebPlanF12Input 可反序列化");
    let input: kernel::rename::PlanF12Input = web.try_into().expect("Web 输入可转内核输入");
    let plan = kernel::rename::plan_f12(input).expect("计划生成成功");
    assert_eq!(plan.entries.len(), 1, "一个输入一条 rename 条目");
    assert_eq!(plan.entries[0].target.as_deref(), Some("/dir/xa.txt.bak"));
}

/// A1 事实采集层的**形状对拍**（web/src/lib/web/facts.ts 的 `c01FileFact`）。
///
/// 前端采出的事实必须能原样反序列化成 WASM 的**线协议入参**——否则 `plan_c01`
/// 会在第一步就 E1002。这里用与前端同形的 JSON 真实跑一遍内核，钉住两侧字段名一致。
///
/// 注意参数用**字符串形态**（`"utf-8"`）：内核 `TargetEncoding` 的 serde 是内部
/// 结构体变体（`{"utf8":{"bom":false}}`），与 CLI/桌面/浏览器共用的 `as_str()`
/// 字符串形态不是同一套——转换在 WASM 边界（`WebC01Params`）完成。
#[test]
fn c01_facts_from_browser_shape_feed_the_kernel_plan() {
    let payload = serde_json::json!({
        "target": "utf-8",
        "invalid": "replace-and-count",
        "source": null,
        "files": [{
            "path": "报告.txt",
            "canonicalPath": null,
            "fileSize": 12,
            "sample": [231, 161, 139, 230, 170, 161]
        }]
    });
    // 走同一层转换：WASM 的 WebC01Params（camelCase 线协议）→ 内核输入。
    let params: batchdesk_wasm::WebC01Params =
        serde_json::from_value(payload).expect("浏览器形状的事实必须能反序列化");
    assert_eq!(params.files.len(), 1);
    assert_eq!(params.files[0].path, "报告.txt");
    assert_eq!(params.files[0].file_size, 12);
    assert_eq!(params.target.as_deref(), Some("utf-8"));

    // **反向严格性**：字段名拼错（`file_size` 而非 `fileSize`）必须导致反序列化失败，
    // 而不是被 serde 静默忽略后产出一个空壳计划——那样测试就形同虚设。
    let wrong = serde_json::json!({ "target": "utf-8", "files": [{ "path": "a", "file_size": 1, "sample": [] }] });
    assert!(
        serde_json::from_value::<batchdesk_wasm::WebC01Params>(wrong).is_err(),
        "必填字段名不符必须报错，不得静默通过"
    );
}

/// A1 端到端：浏览器采样的 C01 事实 → WASM `plan_c01` → 真实计划。
///
/// 这条与 `c01_facts_from_browser_shape_feed_the_kernel_plan` 的区别：
/// 前者只验「形状能反序列化」，这条走**完整的导出函数**（含编码解析、事实注入、
/// 内核计划生成），是「Web 形态真的能出计划」的最小证据。
#[test]
fn browser_sampled_c01_facts_produce_a_real_plan() {
    let payload = serde_json::json!({
        "target": "utf-8",
        "invalid": "replace-and-count",
        "files": [{
            "path": "报告.txt",
            "canonicalPath": null,
            "fileSize": 12,
            // UTF-8 的「报告」两字，用于钉住事实真的进了内核判定。
            "sample": [231, 161, 139, 230, 170, 161, 231, 161, 139, 230, 170, 161]
        }]
    });
    let raw = serde_json::to_string(&payload).unwrap();
    // 直接调用 wasm_bindgen 标注的函数（native 下同样可执行）。
    let out = batchdesk_wasm::plan_c01_core(&raw).expect("浏览器形状的事实必须能产出计划");
    let plan: serde_json::Value = serde_json::from_str(&out).expect("计划可反序列化");
    assert_eq!(plan["kind"], "c01");
    assert!(
        plan["entries"].as_array().map_or(0, Vec::len) >= 1,
        "至少一条转码条目"
    );
}

fn bytes_json(data: &[u8]) -> serde_json::Value {
    serde_json::Value::Array(data.iter().map(|b| serde_json::json!(b)).collect())
}

/// A2-2 端到端（C03）：浏览器事实 → `plan_c03` → 真实计划。
#[test]
fn browser_facts_produce_a_real_c03_plan() {
    let text = "alpha\r\nbeta\r\ngamma";
    let payload = serde_json::json!({
        "mode": "plain",
        "find": "beta",
        "replacement": "BETA",
        "csvMode": "text",
        "sampleBytes": 4096,
        "files": [{
            "path": "note.txt",
            "canonicalPath": null,
            "fileSize": text.len(),
            "data": bytes_json(text.as_bytes()),
            "readonly": false,
            "locked": false
        }]
    });
    let out = batchdesk_wasm::plan_c03_core(serde_json::to_string(&payload).unwrap().as_str())
        .expect("C03 应产出计划");
    let plan: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(plan["kind"], "c03");
    assert!(plan["entries"].as_array().map_or(0, Vec::len) >= 1);
}

/// A2-2 端到端（C02）：换行统计走下沉后的 `counts_from_bytes`，出真实计划。
#[test]
fn browser_facts_produce_a_real_c02_plan() {
    let text = "a\r\nb\nc";
    let payload = serde_json::json!({
        "target": "lf",
        "structure": "auto",
        "files": [{
            "path": "log.txt",
            "canonicalPath": null,
            "fileSize": text.len(),
            "sample": bytes_json(text.as_bytes()),
            "data": bytes_json(text.as_bytes())
        }]
    });
    let out = batchdesk_wasm::plan_c02_core(serde_json::to_string(&payload).unwrap().as_str())
        .expect("C02 应产出计划");
    let plan: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(plan["kind"], "c02");
    assert!(plan["entries"].as_array().map_or(0, Vec::len) >= 1);
}

/// A2-3 端到端：浏览器字节 → 五个 CSV 功能各自出真实计划。
///
/// 统一钉住「同一组事实、五条导出都能产出计划」——任何一条喂不进参数都会在这里红。
#[test]
fn browser_csv_facts_produce_real_plans() {
    fn file(name: &str, text: &str) -> serde_json::Value {
        serde_json::json!({
            "path": name,
            "canonicalPath": null,
            "fileSize": text.len(),
            "sample": bytes_json(text.as_bytes()),
            "data": bytes_json(text.as_bytes()),
            "hasHeader": true,
            "targetOccupied": false
        })
    }
    let csv = "name,age\nann,30\nbob,25\n";

    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "plan_c11_core",
            serde_json::json!({
                "target": "utf-8",
                "key": [{"name": "name"}],
                "files": [file("a.csv", csv)]
            }),
        ),
        (
            "plan_c12_core",
            serde_json::json!({
                "target": "utf-8",
                "modeKeep": true,
                "conditionColumn": {"name": "age"},
                "conditionOp": "eq",
                "conditionValue": "30",
                "files": [file("a.csv", csv)]
            }),
        ),
        (
            "plan_f06_core",
            serde_json::json!({
                "target": "out.csv",
                "targetEncoding": "utf-8",
                "addSourceColumn": false,
                "sourceColumnName": "来源文件",
                "strict": false,
                "files": [file("a.csv", csv)]
            }),
        ),
        (
            "plan_f07_core",
            serde_json::json!({
                "targetEncoding": "utf-8",
                "selections": [{"name": "name"}],
                "files": [file("a.csv", csv)]
            }),
        ),
        (
            "plan_f08_core",
            serde_json::json!({
                "targetEncoding": "utf-8",
                "window": "first",
                "count": 1,
                "files": [file("a.csv", csv)]
            }),
        ),
        (
            "plan_f09_core",
            serde_json::json!({
                "targetEncoding": "utf-8",
                "startRow": 1,
                "endRow": 2,
                "files": [file("a.csv", csv)]
            }),
        ),
        (
            "plan_c04_core",
            serde_json::json!({
                "items": [{"path": "report-final.txt"}],
                "rule": {"mode": "plain", "find": "final", "replace": "draft", "scope": "stem"}
            }),
        ),
        (
            "plan_f04_core",
            serde_json::json!({
                "target": "all.txt",
                "targetEncoding": "utf-8",
                "files": [{"path": "a.txt", "canonicalPath": null, "fileSize": 6,
                           "data": bytes_json(b"aaaaaa")}]
            }),
        ),
        (
            "plan_c13_core",
            serde_json::json!({
                "keys": [{"name": "age"}],
                "order": "asc",
                "targetEncoding": "utf-8",
                "files": [file("a.csv", csv)]
            }),
        ),
        (
            "plan_c09_core",
            serde_json::json!({
                "keep": "earliest",
                "mode": "full",
                "hashAlgorithm": "blake3",
                "includeZeroByte": false,
                "files": [{"id": 1, "path": "a.txt", "canonicalPath": null,
                           "mtimeNanos": 1, "fullHash": [1,2,3]},
                          {"id": 2, "path": "b.txt", "canonicalPath": null,
                           "mtimeNanos": 2, "fullHash": [1,2,3]}],
                "report": {"groups": [{"size": 3, "hash": [1,2,3],
                                       "members": [1, 2], "confidence": "confirmed",
                                       "needsManualReview": false}],
                           "zeroByteIds": [], "hardlinkSkipped": [], "incomplete": []},
                "skippedPaths": []
            }),
        ),
        (
            "plan_c07_core",
            serde_json::json!({
                "operation": "move",
                "missing": "skip",
                "sourceRoot": "src",
                "targetRoot": "dst",
                "rows": [{"name": "a.txt", "subDir": "2026"},
                         {"name": "b.txt", "subDir": "2026"}],
                "facts": [{"exists": true, "size": 3}, {"exists": false, "size": 0}]
            }),
        ),
        (
            "plan_c08_core",
            serde_json::json!({
                "sourceRoot": "src",
                "targetRoot": "dst",
                "rules": [{"subDir": "2026",
                           "condition": {"kind": "extensions", "value": ["log", "txt"]}}],
                "unmatched": "keep",
                "files": [{"relPath": "a.log", "size": 12, "modifiedDate": "2026-11-20",
                           "exists": true}]
            }),
        ),
        (
            "plan_f10_core",
            serde_json::json!({
                "roots": [{"path": "dir", "canonicalPath": null, "target": "dir.txt",
                           "files": 3, "dirs": 1, "placeholders": 0, "skipped": 0,
                           "maxDepth": 2}]
            }),
        ),
        (
            "plan_f11_core",
            serde_json::json!({
                "root": "dst",
                "canonicalRoot": null,
                "rows": [{"line": 1, "path": "2026/11"},
                         {"line": 2, "path": "2026/12"}]
            }),
        ),
        (
            "plan_f13_core",
            serde_json::json!({
                "items": [{"path": "a-report.txt"}],
                "rule": {"pattern": "^a-(.*)$", "replacement": "$1", "scope": "full"}
            }),
        ),
        (
            "plan_f01_core",
            serde_json::json!({
                "targetBytes": 3,
                "outputDir": "out",
                "files": [{"path": "a.txt", "canonicalPath": null, "groupId": "g1",
                           "fileSize": 11, "data": bytes_json(b"aaa
bbb
ccc")}],
            }),
        ),
        (
            "plan_f02_core",
            serde_json::json!({
                "linesPerPart": 2,
                "outputDir": "out",
                "files": [{"path": "a.txt", "canonicalPath": null, "groupId": "g1",
                           "fileSize": 11, "data": bytes_json(b"aaa
bbb
ccc")}],
            }),
        ),
        (
            "plan_f03_core",
            serde_json::json!({
                "parts": 2,
                "outputDir": "out",
                "files": [{"path": "a.txt", "canonicalPath": null, "groupId": "g1",
                           "fileSize": 11, "data": bytes_json(b"aaa
bbb
ccc")}],
            }),
        ),
        (
            "plan_f05_core",
            serde_json::json!({
                "basis": {"bytes": 8},
                "keepHeader": true,
                "outputDir": "out",
                "files": [file("a.csv", csv)]
            }),
        ),
        (
            "plan_c14_core",
            serde_json::json!({
                "targetEncoding": "utf-8",
                "mode": "split",
                "split": {"source": {"name": "name"}, "separator": "-"},
                "files": [file("a.csv", csv)]
            }),
        ),
    ];

    for (name, payload) in cases {
        let raw = serde_json::to_string(&payload).unwrap();
        let out = match name {
            "plan_c11_core" => batchdesk_wasm::plan_c11_core(&raw),
            "plan_c12_core" => batchdesk_wasm::plan_c12_core(&raw),
            "plan_f06_core" => batchdesk_wasm::plan_f06_core(&raw),
            "plan_f07_core" => batchdesk_wasm::plan_f07_core(&raw),
            "plan_f08_core" => batchdesk_wasm::plan_f08_core(&raw),
            "plan_f09_core" => batchdesk_wasm::plan_f09_core(&raw),
            "plan_c14_core" => batchdesk_wasm::plan_c14_core(&raw),
            "plan_c04_core" => batchdesk_wasm::plan_c04_core(&raw),
            "plan_f04_core" => batchdesk_wasm::plan_f04_core(&raw),
            "plan_c13_core" => batchdesk_wasm::plan_c13_core(&raw),
            "plan_c09_core" => batchdesk_wasm::plan_c09_core(&raw),
            "plan_c07_core" => batchdesk_wasm::plan_c07_core(&raw),
            "plan_c08_core" => batchdesk_wasm::plan_c08_core(&raw),
            "plan_f10_core" => batchdesk_wasm::plan_f10_core(&raw),
            "plan_f11_core" => batchdesk_wasm::plan_f11_core(&raw),
            "plan_f13_core" => batchdesk_wasm::plan_f13_core(&raw),
            "plan_f01_core" => batchdesk_wasm::plan_f01_core(&raw),
            "plan_f02_core" => batchdesk_wasm::plan_f02_core(&raw),
            "plan_f03_core" => batchdesk_wasm::plan_f03_core(&raw),
            "plan_f05_core" => batchdesk_wasm::plan_f05_core(&raw),
            _ => unreachable!("{name} 未接入分发"),
        }
        .unwrap_or_else(|error| panic!("{name} 失败：{error}"));
        let plan: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(
            plan["entries"].as_array().map_or(0, Vec::len) >= 1,
            "{name} 未产出条目"
        );
    }
}

/// A2-7 端到端（C10）：浏览器中央目录事实 → `plan_c10` → 真实解压计划。
///
/// 事实形态必须与 `web/src/lib/web/zipIndex.ts` 的 `ZipEntryFact` 逐字段同形
/// （ADR-0103）：字段名错一个，`plan_c10` 会在第一步 E1002，而不是产出一个
/// 「条目全被跳过、看着却像正常」的计划。
#[test]
fn browser_central_directory_facts_produce_a_real_c10_plan() {
    let payload = serde_json::json!({
        "archive": "C:/downloads/bundle.zip",
        "archiveSize": 4096,
        "entries": [
            // GB18030 回退解出的中文名：应进 safeCount，并触发 gbk 告警。
            {
                "index": 0, "name": "报告.txt", "kind": "file", "size": 120,
                "compressedSize": 80, "gbkFallback": true, "encrypted": false, "supported": true
            },
            // 目录条目：参与选择但不计 files。
            {
                "index": 1, "name": "docs/", "kind": "dir", "size": 0,
                "compressedSize": 0, "gbkFallback": false, "encrypted": false, "supported": true
            },
            // 路径穿越：内核必须拦下并记入 unsafeNames。
            {
                "index": 2, "name": "../escape.txt", "kind": "file", "size": 999,
                "compressedSize": 10, "gbkFallback": false, "encrypted": false, "supported": true
            }
        ],
        "outputDir": "opfs://batchdesk-out",
        "maxTotalBytes": 104857600,
        "maxRatio": 100
    });
    let out = batchdesk_wasm::plan_c10_core(&serde_json::to_string(&payload).unwrap())
        .expect("中央目录事实必须能产出解压计划");
    let plan: serde_json::Value = serde_json::from_str(&out).expect("计划可反序列化");
    assert_eq!(plan["kind"], "c10");
    assert_eq!(plan["params"]["entryCount"], 3);
    assert_eq!(
        plan["params"]["safeCount"], 2,
        "穿越条目必须被拦下，只剩正常文件与目录"
    );
    assert!(
        plan["entries"][0]["blocked"].is_null(),
        "正常包不应被整包阻断"
    );
    assert_eq!(plan["entries"][0]["action"], "extract");
    let warning_kinds: Vec<&str> = plan["warnings"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|item| item["kind"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        warning_kinds.contains(&"unsafe-entry-skipped"),
        "路径穿越必须留下告警，实际：{warning_kinds:?}"
    );
    assert!(
        warning_kinds.contains(&"gbk-filename-fallback"),
        "GB18030 回退必须留下告警，实际：{warning_kinds:?}"
    );
}

/// A2-7：加密包的处置走「执行期会不会给密码」这一个事实（ADR-0074）。
///
/// 不给密码 → 整包 blocked `E3008`；给密码 → 出计划并在 `params.passwordProvided`
/// 记下事实（计划里没有密码本身）。两条都必须能真实触发，否则 Web 侧只会
/// 「解不开也没提示」。
#[test]
fn web_c10_password_fact_decides_whether_the_plan_is_blocked() {
    fn payload(password_provided: Option<bool>) -> String {
        let mut value = serde_json::json!({
            "archive": "C:/downloads/secret.zip",
            "archiveSize": 2048,
            "entries": [{
                "index": 0, "name": "secret.txt", "kind": "file", "size": 64,
                "compressedSize": 64, "gbkFallback": false, "encrypted": true, "supported": true
            }],
            "outputDir": "/out",
            "maxTotalBytes": 104857600,
            "maxRatio": 100
        });
        if let Some(flag) = password_provided {
            value["passwordProvided"] = serde_json::Value::Bool(flag);
        }
        serde_json::to_string(&value).unwrap()
    }

    let blocked: serde_json::Value =
        serde_json::from_str(&batchdesk_wasm::plan_c10_core(&payload(None)).unwrap()).unwrap();
    assert_eq!(blocked["entries"][0]["blocked"]["code"], "E3008");
    assert!(
        blocked["params"]["passwordProvided"].is_null(),
        "没给密码时不应记录 passwordProvided"
    );

    let planned: serde_json::Value =
        serde_json::from_str(&batchdesk_wasm::plan_c10_core(&payload(Some(true))).unwrap())
            .unwrap();
    assert!(
        planned["entries"][0]["blocked"].is_null(),
        "声明执行期给密码后不应整包阻断"
    );
    assert_eq!(planned["params"]["passwordProvided"], true);
}

/// A2-7 反向严格性：`supported` 缺字段必须报错。
///
/// 若给它 serde 默认值 `false`，整包条目会被静默当作「不支持」跳过，
/// 得到一个 safeCount=0、看起来却像正常的结果——比报错更危险。
#[test]
fn web_c10_entry_missing_supported_is_rejected() {
    let payload = serde_json::json!({
        "archive": "a.zip",
        "archiveSize": 1,
        "entries": [{
            "index": 0, "name": "a.txt", "kind": "file", "size": 1,
            "compressedSize": 1, "gbkFallback": false, "encrypted": false
        }],
        "outputDir": "/out",
        "maxTotalBytes": 1024,
        "maxRatio": 100
    });
    let error = batchdesk_wasm::plan_c10_core(&serde_json::to_string(&payload).unwrap())
        .expect_err("缺 supported 字段必须反序列化失败");
    assert!(
        error.starts_with("E1002:"),
        "错误码应为 E1002，实际：{error}"
    );
}

// ── A3-4：C07 清单解析（ADR-0106）──────────────────────────────────────

/// GB18030 编码的中英混排清单：`年度报告0.docx,归档目录`（两行）。
///
/// 用固定字节而不用例里现编码：这条用例要证明的是「浏览器拿到的原始字节
/// 交给内核后能正确解析」，编码来源本身不该由测试再引入一层依赖。
const GB18030_MANIFEST: &[u8] = &[
    0xC4, 0xEA, 0xB6, 0xC8, 0xB1, 0xA8, 0xB8, 0xE6, 0x30, 0x2E, 0x64, 0x6F, 0x63, 0x78, 0x2C, 0xB9,
    0xE9, 0xB5, 0xB5, 0xC4, 0xBF, 0xC2, 0xBC, 0x0A, 0xC4, 0xEA, 0xB6, 0xC8, 0xB1, 0xA8, 0xB8, 0xE6,
    0x31, 0x2E, 0x64, 0x6F, 0x63, 0x78, 0x2C, 0xB9, 0xE9, 0xB5, 0xB5, 0xC4, 0xBF, 0xC2, 0xBC, 0x0A,
];

/// Web 侧清单解析与桌面/CLI 同一条内核管线，且**显式编码是必需入口**。
///
/// 中英混排的文件名清单在统计检测下会被猜成候选集外的编码（EUC-KR 等）→ E3001。
/// 若没有指定编码的入口，这类清单在浏览器里永远解析不了——所以这里同时钉住
/// 「不指定会报 E3001」和「指定后能解析」，避免把前者当偶发。
#[test]
fn web_manifest_parsing_shares_the_kernel_pipeline_and_needs_an_explicit_encoding() {
    let error = batchdesk_wasm::parse_c07_manifest_core(GB18030_MANIFEST, None, None)
        .expect_err("中英混排的短清单不应被静默猜成某个编码");
    assert!(
        error.starts_with("E3001:"),
        "错误码应为 E3001，实际：{error}"
    );

    let rows = batchdesk_wasm::parse_c07_manifest_core(GB18030_MANIFEST, None, Some("gb18030"))
        .expect("指定 GB18030 后必须能解析");
    let rows: Vec<serde_json::Value> = serde_json::from_str(&rows).expect("行是 JSON 数组");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "年度报告0.docx");
    assert_eq!(
        rows[0]["subDir"], "归档目录",
        "线协议字段名必须是 camelCase"
    );
}

/// 少于两列的行必须被拒（E1004）而不是跳过：跳过会让用户以为该行已处理。
#[test]
fn web_manifest_parsing_rejects_single_column_rows() {
    let error = batchdesk_wasm::parse_c07_manifest_core(b"a.jpg,x\norphan.jpg\n", None, None)
        .expect_err("单列行必须失败");
    assert!(
        error.starts_with("E1004:"),
        "错误码应为 E1004，实际：{error}"
    );
}

/// 显式分隔符优先于嗅探（分号清单里字段自身含逗号时，嗅探会被带偏）。
#[test]
fn web_manifest_parsing_honours_an_explicit_delimiter() {
    let rows = batchdesk_wasm::parse_c07_manifest_core(b"a,1.jpg;x\n", Some(";"), None)
        .expect("显式分号可解析");
    let rows: Vec<serde_json::Value> = serde_json::from_str(&rows).unwrap();
    assert_eq!(rows[0]["name"], "a,1.jpg");
    assert_eq!(rows[0]["subDir"], "x");
}

/// 未知分隔符/编码必须报 E1002（而不是退回嗅探或检测，让用户以为参数生效了）。
#[test]
fn web_manifest_parsing_rejects_unknown_delimiter_and_encoding() {
    let error = batchdesk_wasm::parse_c07_manifest_core(b"a,b\n", Some("::"), None)
        .expect_err("未知分隔符必须失败");
    assert!(error.starts_with("E1002:"), "{error}");
    let error = batchdesk_wasm::parse_c07_manifest_core(b"a,b\n", None, Some("latin-9"))
        .expect_err("未知编码必须失败");
    assert!(error.starts_with("E1002:"), "{error}");
}

// ── A3-5：C10 压缩侧（ADR-0107）──────────────────────────────────────────

/// 压缩计划必须带 `mode=compress`：套用通道按它分流路径槽位（ADR-0080），
/// 缺了会被当成解压计划（去读一个不存在的压缩包）。
#[test]
fn web_c10_compression_plan_is_marked_both_ways_and_carries_the_facts() {
    let raw = batchdesk_wasm::plan_c10_compress_core(
        r#"{
            "output": "/out/bundle.zip",
            "sources": ["/src/a.txt", "/src/b.txt"],
            "compression": "store",
            "sourceFileCount": 2,
            "sourceBytes": 12
        }"#,
    )
    .expect("压缩计划生成成功");
    let plan: serde_json::Value = serde_json::from_str(&raw).expect("计划是 JSON");
    assert_eq!(plan["kind"], "c10");
    assert_eq!(plan["params"]["mode"], "compress");
    assert_eq!(plan["params"]["compression"], "store");
    assert_eq!(plan["params"]["sourceCount"], 2);
    assert_eq!(plan["params"]["sourceBytes"], 12);
    assert_eq!(plan["entries"][0]["action"], "archive");
    assert_eq!(plan["entries"][0]["target"], "/out/bundle.zip");
}

/// 输出与源同路径、或落在源目录内部必须阻断（E1002）——否则压缩产物会被下一次
/// 运行当作源，越压越大。
#[test]
fn web_c10_compression_blocks_an_output_that_overlaps_a_source() {
    for (output, sources) in [
        ("/src/a.txt", vec!["/src/a.txt"]),
        ("/src/dir/bundle.zip", vec!["/src/dir"]),
    ] {
        let payload = serde_json::json!({
            "output": output,
            "sources": sources,
            "sourceFileCount": 1,
            "sourceBytes": 3,
        });
        let raw = batchdesk_wasm::plan_c10_compress_core(&payload.to_string())
            .expect("计划仍应生成（阻断写在 entries[].blocked 上）");
        let plan: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            plan["entries"][0]["blocked"]["code"], "E1002",
            "output={output} 应与源重叠"
        );
        assert_eq!(plan["stats"]["blocked"], 1);
    }
}

/// 源文件计数/字节数缺失时必须报 E1001：默认成 0 会产出「压缩 0 个文件、0 字节」
/// 这种看着正常、实际与所选文件无关的计划。
#[test]
fn web_c10_compression_requires_source_counts() {
    let error = batchdesk_wasm::plan_c10_compress_core(
        r#"{"output": "/out/a.zip", "sources": ["/src/a.txt"]}"#,
    )
    .expect_err("缺 sourceFileCount/sourceBytes 必须失败");
    assert!(error.starts_with("E1001:"), "{error}");
}

/// 未知压缩级别与空源清单都必须拒绝（不退回 deflate、不生成空计划）。
#[test]
fn web_c10_compression_rejects_unknown_compression_and_empty_sources() {
    let error = batchdesk_wasm::plan_c10_compress_core(
        r#"{"output":"/out/a.zip","sources":["/src/a"],"compression":"brotli","sourceFileCount":1,"sourceBytes":1}"#,
    )
    .expect_err("未知压缩级别必须失败");
    assert!(error.starts_with("E1002:"), "{error}");
    let error = batchdesk_wasm::plan_c10_compress_core(
        r#"{"output":"/out/a.zip","sources":[],"sourceFileCount":0,"sourceBytes":0}"#,
    )
    .expect_err("空源清单必须失败");
    assert!(error.starts_with("E1001:"), "{error}");
}
