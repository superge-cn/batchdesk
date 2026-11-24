//! Web 执行沙箱的端到端用例（ADR-0108）。
//!
//! 与 `export_surface.rs` 的分工：那边审**导出面**，这边审**执行语义**——
//! 计划是否真被执行、产物字节是否等于内核算出来的切片、撤销是否真把产物收走。
//! 全部走真实内核（`plan_f03` + `generic_apply` + `undo`），不 mock 执行器。

use batchdesk_wasm::exec::Sandbox;
use kernel::apply::ApplyState;
use kernel::fs::CaseSense;
use kernel::split::{plan_f03, PlanF01File, PlanF03Input, SplitAccumulator};

const PLAN_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const GROUP_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

/// 造一份「按份数分片」的真实计划：4 行文本 → 2 片，每片 2 行。
fn split_plan(source: &str, parts: u64) -> kernel::plan::PlanDocument {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut accumulator =
        SplitAccumulator::new_quotas(kernel::split::part_line_quotas(lines.len() as u64, parts))
            .unwrap();
    for line in &lines {
        accumulator.push_line(line.len() as u64, line.ends_with('\n'));
    }
    let layout = accumulator.finish(source.len() as u64);
    plan_f03(PlanF03Input {
        plan_id: PLAN_ID.to_string(),
        parts,
        output_dir: "/out".to_string(),
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        existing_targets: Vec::new(),
        files: vec![PlanF01File {
            path: "notes.txt".to_string(),
            canonical_path: None,
            group_id: GROUP_ID.to_string(),
            layout,
        }],
    })
    .unwrap()
}

fn stage_one(sandbox: &mut Sandbox, path: &str, content: &[u8]) {
    let manifest = serde_json::json!([{ "path": path, "length": content.len() }]).to_string();
    sandbox.stage(&manifest, content).unwrap();
}

fn apply_request(plan: &kernel::plan::PlanDocument) -> String {
    serde_json::json!({
        "plan": plan,
        "options": { "onConflict": "skip" },
        "now": "2026-11-22T10:00:00Z",
    })
    .to_string()
}

#[test]
fn split_plan_executes_in_the_sandbox_and_undo_removes_the_products() {
    let content = b"aaa\nbbb\nccc\nddd\n";
    let plan = split_plan("aaa\nbbb\nccc\nddd\n", 2);
    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "notes.txt", content);

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(
        applied["outcome"]["state"], "completed",
        "真实执行应完成：{applied}"
    );
    let products = applied["products"].as_array().unwrap();
    assert_eq!(products.len(), 2, "4 行分 2 片：{applied}");
    assert_eq!(products[0]["path"], "/out/notes.1.txt");
    assert_eq!(products[1]["path"], "/out/notes.2.txt");

    // 产物字节就是 kernel 计划里的字节区间——不是占位、不是空文件。
    assert_eq!(
        sandbox.product("/out/notes.1.txt").unwrap(),
        b"aaa\nbbb\n".to_vec()
    );
    assert_eq!(
        sandbox.product("/out/notes.2.txt").unwrap(),
        b"ccc\nddd\n".to_vec()
    );
    // 源文件仍在沙箱里（只读引用的语义：执行不改源）。
    assert_eq!(sandbox.product("notes.txt").unwrap(), content.to_vec());

    let undone = sandbox.undo(PLAN_ID).unwrap();
    assert_eq!(undone.reverted, 2, "两条产物都应被撤销：{undone:?}");
    assert!(sandbox.products().is_empty());
    assert!(
        sandbox.product("/out/notes.1.txt").is_err(),
        "撤销后产物必须从沙箱消失"
    );
    assert_eq!(
        sandbox.product("notes.txt").unwrap(),
        content.to_vec(),
        "撤销不得动源文件"
    );
}

#[test]
fn missing_input_is_reported_not_silently_skipped() {
    let plan = split_plan("aaa\nbbb\nccc\nddd\n", 2);
    let mut sandbox = Sandbox::new();
    // 故意不 stage 源文件：执行器必须按 E2001 报失败，而不是产出空分片。
    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_ne!(
        applied["outcome"]["state"], "completed",
        "没有输入不可能完成：{applied}"
    );
    assert!(
        applied["products"].as_array().unwrap().is_empty(),
        "失败不得留下产物：{applied}"
    );
    let first = &applied["outcome"]["report"]["entries"][0];
    assert_eq!(first["status"], "failed");
    assert_eq!(first["error"]["code"], "E2001");
}

#[test]
fn stage_rejects_payload_that_does_not_match_the_manifest() {
    let mut sandbox = Sandbox::new();
    let manifest = serde_json::json!([{ "path": "a.txt", "length": 4 }]).to_string();
    let error = sandbox.stage(&manifest, b"abc").unwrap_err();
    assert_eq!(error.code, "E1002");
    assert!(
        error.message.contains("expects 4 bytes"),
        "错误要说清差多少：{}",
        error.message
    );
}

#[test]
fn apply_state_serializes_as_kebab_case_for_the_web_contract() {
    // 契约锚点：前端按字符串判终态（`completed`/`partial-failed`/...），
    // 由内核枚举的 serde 形态保证，不在 TS 侧再写一份映射。
    let json = serde_json::to_string(&ApplyState::PartialFailed).unwrap();
    assert_eq!(json, "\"partial-failed\"");
}

// ── A3-7：C01/C02/C03 的执行臂（改写器端口在沙箱内构造） ──────────────────

/// 走 Web 的真实链路造计划：线协议 JSON → `plan_c0X_core`（与浏览器同一入口）。
///
/// 不用手工拼 `PlanDocument`：`from_plan` 读的是 `plan_c0X` 写进去的参数快照，
/// 手工拼的计划可能「参数对不上而照样能跑」，测不出真实链路的接缝。
fn plan_from_core(
    core: fn(&str) -> Result<String, String>,
    params: serde_json::Value,
) -> kernel::plan::PlanDocument {
    let json = core(&params.to_string()).expect("plan core should accept the wire payload");
    serde_json::from_str(&json).expect("plan JSON should parse back into a PlanDocument")
}

#[test]
fn c02_line_ending_plan_rewrites_in_place_and_undo_restores_it() {
    let content = b"a\r\nb\r\n";
    let plan = plan_from_core(
        batchdesk_wasm::plan_c02_core,
        serde_json::json!({
            "target": "lf",
            "structure": "auto",
            "files": [{
                "path": "a.txt",
                "fileSize": content.len(),
                "sample": content,
                "data": content,
            }],
        }),
    );
    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "a.txt", content);

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(applied["outcome"]["state"], "completed", "{applied}");
    let products = applied["products"].as_array().unwrap();
    assert_eq!(products.len(), 1, "原地改写只有一条产物：{applied}");
    // 原地改写的产物路径就是源路径（C02 没有 target），不是另存一份；
    // 产物清单统一给沙箱键（绝对形态），因此是 `/a.txt` 而不是计划的 `a.txt`。
    assert_eq!(products[0]["path"], "/a.txt");
    assert_eq!(sandbox.product("a.txt").unwrap(), b"a\nb\n".to_vec());

    let undone = sandbox.undo(&plan.plan_id).unwrap();
    assert_eq!(undone.reverted, 1, "{undone:?}");
    assert_eq!(
        sandbox.product("a.txt").unwrap(),
        content.to_vec(),
        "撤销必须把原字节（含 CRLF）写回去"
    );
}

#[test]
fn c03_replace_plan_rewrites_every_match() {
    let content = "foo baz foo\n";
    let plan = plan_from_core(
        batchdesk_wasm::plan_c03_core,
        serde_json::json!({
            "mode": "plain",
            "find": "foo",
            "replacement": "bar",
            "csvMode": "text",
            "files": [{
                "path": "notes.txt",
                "fileSize": content.len(),
                "data": content.as_bytes(),
                "readonly": false,
            }],
        }),
    );
    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "notes.txt", content.as_bytes());

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(applied["outcome"]["state"], "completed", "{applied}");
    assert_eq!(
        sandbox.product("notes.txt").unwrap(),
        b"bar baz bar\n".to_vec(),
        "两处匹配都要替换（不是只换第一处）"
    );
}

#[test]
fn c01_transcode_plan_writes_the_target_encoding() {
    let content = "中文\n";
    let plan = plan_from_core(
        batchdesk_wasm::plan_c01_core,
        serde_json::json!({
            "target": "gb18030",
            "source": "utf-8",
            "files": [{
                "path": "a.txt",
                "fileSize": content.len(),
                "sample": content.as_bytes(),
            }],
        }),
    );
    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "a.txt", content.as_bytes());

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(applied["outcome"]["state"], "completed", "{applied}");
    // GB18030 的「中文」是 D6 D0 CE C4——不是原样复制，也不是替换字符。
    assert_eq!(
        sandbox.product("a.txt").unwrap(),
        vec![0xD6, 0xD0, 0xCE, 0xC4, 0x0A],
        "转码后的字节必须是目标编码"
    );
}

#[test]
fn broken_plan_params_are_rejected_before_anything_is_written() {
    let content = b"a\r\nb\r\n";
    let plan = plan_from_core(
        batchdesk_wasm::plan_c02_core,
        serde_json::json!({
            "target": "lf",
            "files": [{
                "path": "a.txt",
                "fileSize": content.len(),
                "sample": content,
                "data": content,
            }],
        }),
    );
    // 拆掉 `target`：改写器构造必须失败，而不是「没有改写器 → 原样复制」。
    let mut value: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
    value["params"].as_object_mut().unwrap().remove("target");
    let broken: kernel::plan::PlanDocument = serde_json::from_value(value).unwrap();

    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "a.txt", content);
    let error = sandbox.apply(&apply_request(&broken)).unwrap_err();
    assert_eq!(error.code, "E1001", "缺参数：{}", error.message);
    assert_eq!(
        sandbox.product("a.txt").unwrap(),
        content.to_vec(),
        "拒绝发生在写盘之前，源文件必须原样"
    );
}

// ── A3-7：合并/分片族的执行臂（只需读写，不需要额外端口） ──────────────────

#[test]
fn f04_merge_plan_writes_the_combined_text() {
    let a = "A\n";
    let b = "B\n";
    let plan = plan_from_core(
        batchdesk_wasm::plan_f04_core,
        serde_json::json!({
            "target": "/out/m.txt",
            "targetEncoding": "utf-8",
            "files": [
                { "path": "a.txt", "fileSize": a.len(), "data": a.as_bytes() },
                { "path": "b.txt", "fileSize": b.len(), "data": b.as_bytes() },
            ],
        }),
    );
    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "a.txt", a.as_bytes());
    // 第二个输入：manifest 追加一项，字节接在后面。
    let manifest = serde_json::json!([{ "path": "b.txt", "length": b.len() }]).to_string();
    sandbox.stage(&manifest, b.as_bytes()).unwrap();

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(applied["outcome"]["state"], "completed", "{applied}");
    assert_eq!(
        sandbox.product("/out/m.txt").unwrap(),
        b"A\nB\n".to_vec(),
        "合并顺序按计划条目的输入顺序"
    );
}

#[test]
fn f05_csv_split_plan_keeps_the_header_in_every_part() {
    let content = "name,qty\nb,2\na,1\nc,3\n";
    let plan = plan_from_core(
        batchdesk_wasm::plan_f05_core,
        serde_json::json!({
            "basis": { "rows": 1 },
            "keepHeader": true,
            "outputDir": "/out",
            "files": [{
                "path": "t.csv",
                "fileSize": content.len(),
                "sample": content.as_bytes(),
                "data": content.as_bytes(),
                "hasHeader": true,
                "targetOccupied": false,
            }],
        }),
    );
    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "t.csv", content.as_bytes());

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(applied["outcome"]["state"], "completed", "{applied}");
    let products = applied["products"].as_array().unwrap();
    assert_eq!(products.len(), 3, "3 数据行按 1 行一片：{applied}");
    assert_eq!(
        sandbox.product("/out/t.1.csv").unwrap(),
        b"name,qty\nb,2\n".to_vec()
    );
    assert_eq!(
        sandbox.product("/out/t.3.csv").unwrap(),
        b"name,qty\nc,3\n".to_vec()
    );
}

#[test]
fn f06_csv_merge_plan_writes_one_header_only() {
    let a = "name,qty\na,1\n";
    let b = "name,qty\nb,2\n";
    let plan = plan_from_core(
        batchdesk_wasm::plan_f06_core,
        serde_json::json!({
            "target": "/out/m.csv",
            "targetEncoding": "utf-8",
            "addSourceColumn": false,
            "sourceColumnName": "source",
            "strict": false,
            "files": [
                { "path": "a.csv", "fileSize": a.len(), "sample": a.as_bytes(), "data": a.as_bytes(), "hasHeader": true, "targetOccupied": false },
                { "path": "b.csv", "fileSize": b.len(), "sample": b.as_bytes(), "data": b.as_bytes(), "hasHeader": true, "targetOccupied": false },
            ],
        }),
    );
    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "a.csv", a.as_bytes());
    let manifest = serde_json::json!([{ "path": "b.csv", "length": b.len() }]).to_string();
    sandbox.stage(&manifest, b.as_bytes()).unwrap();

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(applied["outcome"]["state"], "completed", "{applied}");
    assert_eq!(
        sandbox.product("/out/m.csv").unwrap(),
        b"name,qty\na,1\nb,2\n".to_vec(),
        "表头只写一次"
    );
}

// ── A3-8：CSV 行列变换族的执行臂（输出路径按表单输出目录派生） ──────────────

#[test]
fn c11_dedup_plan_writes_to_the_output_dir_and_leaves_the_source_alone() {
    let content = b"name,qty\na,1\na,1\nb,2\n";
    let plan = plan_from_core(
        batchdesk_wasm::plan_c11_core,
        serde_json::json!({
            "target": "utf-8",
            "outputDir": "/out",
            "key": [{ "name": "name" }],
            "files": [{
                "path": "d.csv",
                "fileSize": content.len(),
                "sample": content,
                "data": content,
                "hasHeader": true,
                "targetOccupied": false,
            }],
        }),
    );
    // 计划的目标必须落在输出目录，**不是**源文件自身——否则执行时目标已存在，
    // `skip` 会自我跳过、`overwrite` 等于原地覆盖源。
    assert_eq!(
        plan.entries[0].target.as_deref(),
        Some("/out/d.csv"),
        "target 应由 outputDir 派生"
    );

    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "d.csv", content);
    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(applied["outcome"]["state"], "completed", "{applied}");
    assert_eq!(
        sandbox.product("/out/d.csv").unwrap(),
        b"name,qty\na,1\nb,2\n".to_vec(),
        "重复行只留首次出现"
    );
    assert_eq!(
        sandbox.product("d.csv").unwrap(),
        content.to_vec(),
        "源文件必须原样（不是原地改写）"
    );
}

#[test]
fn csv_plans_normalize_the_output_dir_and_default_to_the_sandbox_out() {
    // 反斜杠、尾部斜杠、缺前导斜杠：`MemoryFs` 眼里都是同一个目录。
    let content = b"name,qty\na,1\n";
    let f07 = plan_from_core(
        batchdesk_wasm::plan_f07_core,
        serde_json::json!({
            "targetEncoding": "utf-8",
            "outputDir": "out\\sub\\",
            "selections": [{ "index": 2 }],
            "files": [{
                "path": "a.csv",
                "fileSize": content.len(),
                "sample": content,
                "data": content,
                "hasHeader": true,
                "targetOccupied": false,
            }],
        }),
    );
    assert_eq!(
        f07.entries[0].target.as_deref(),
        Some("/out/sub/a.csv"),
        "反斜杠与尾部斜杠都要归一成 `/out/sub`"
    );

    // 不传 outputDir：缺省 `/out`，**不能**回落到源文件。
    let f08 = plan_from_core(
        batchdesk_wasm::plan_f08_core,
        serde_json::json!({
            "targetEncoding": "utf-8",
            "window": "first",
            "count": 1,
            "files": [{
                "path": "x.csv",
                "fileSize": content.len(),
                "sample": content,
                "data": content,
                "hasHeader": true,
                "targetOccupied": false,
            }],
        }),
    );
    assert_eq!(f08.entries[0].target.as_deref(), Some("/out/x.csv"));
    assert_ne!(
        f08.entries[0].target.as_deref(),
        Some("x.csv"),
        "缺省输出不得回落到源文件本身"
    );

    // 输出目录写成根：落在根下，不留 `//`。
    let rooted = plan_from_core(
        batchdesk_wasm::plan_f08_core,
        serde_json::json!({
            "targetEncoding": "utf-8",
            "outputDir": "/",
            "window": "first",
            "count": 1,
            "files": [{
                "path": "x.csv",
                "fileSize": content.len(),
                "sample": content,
                "data": content,
                "hasHeader": true,
                "targetOccupied": false,
            }],
        }),
    );
    assert_eq!(rooted.entries[0].target.as_deref(), Some("/x.csv"));
}

// ── A3-9：按相对路径归档/同步族（C07/C08） ─────────────────────────────────
//
// 这两个视图的源路径带目录层级：计划里写的是 `源根/相对路径`，源根在浏览器里
// 就是**所选目录名**（`sourceRoot` 字段是目录选择器，没有文本值可填）。staging
// 必须按同一规则拼键，否则执行器看到的是「每条都源缺失」。

#[test]
fn c07_manifest_move_uses_relative_paths_and_moves_the_source_away() {
    let content = b"report";
    let plan = plan_from_core(
        batchdesk_wasm::plan_c07_core,
        serde_json::json!({
            "operation": "move",
            "missing": "skip",
            "sourceRoot": "资料",
            "targetRoot": "/dst",
            "rows": [{ "name": "sub/年度报告.docx", "subDir": "2024" }],
            "facts": [{ "exists": true, "size": content.len() }],
        }),
    );
    // 计划的源是源根 + 清单里的相对路径（不是裸文件名）。
    assert_eq!(plan.entries[0].source, "资料/sub/年度报告.docx");
    assert_eq!(
        plan.entries[0].target.as_deref(),
        Some("/dst/2024/年度报告.docx")
    );

    let mut sandbox = Sandbox::new();
    // staging 键与计划的源逐字一致（前端 `stagedPath` 的同一规则）。
    stage_one(&mut sandbox, "资料/sub/年度报告.docx", content);

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(applied["outcome"]["state"], "completed", "{applied}");
    let products = applied["products"].as_array().unwrap();
    assert_eq!(products.len(), 1, "{applied}");
    assert_eq!(products[0]["path"], "/dst/2024/年度报告.docx");
    assert_eq!(
        sandbox.product("/dst/2024/年度报告.docx").unwrap(),
        content.to_vec()
    );
    assert!(
        sandbox.product("资料/sub/年度报告.docx").is_err(),
        "move 必须把源移走（撤销才靠 Journal 放回）"
    );

    let undone = sandbox.undo(&plan.plan_id).unwrap();
    assert_eq!(undone.reverted, 1, "{undone:?}");
    assert_eq!(
        sandbox.product("资料/sub/年度报告.docx").unwrap(),
        content.to_vec(),
        "撤销要把源放回原位"
    );
}

#[test]
fn c08_archive_rules_keep_the_relative_tree_intact() {
    let jpg = b"img";
    let txt = b"note";
    let plan = plan_from_core(
        batchdesk_wasm::plan_c08_core,
        serde_json::json!({
            "sourceRoot": "src",
            "targetRoot": "/dst",
            "rules": [{
                "subDir": "images",
                "condition": { "kind": "extensions", "value": ["jpg"] },
            }],
            // 未命中项留在原地（不产生条目，也不动文件）。
            "unmatched": "keep",
            "files": [
                { "relPath": "a.jpg", "size": jpg.len(), "exists": true },
                { "relPath": "sub/b.txt", "size": txt.len(), "exists": true },
            ],
        }),
    );
    // 只有命中规则的条目进计划；源是 `源根/相对路径`，子目录不丢。
    assert_eq!(plan.entries.len(), 1, "{plan:?}");
    assert_eq!(plan.entries[0].source, "src/a.jpg");
    assert_eq!(plan.entries[0].target.as_deref(), Some("/dst/images/a.jpg"));

    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "src/a.jpg", jpg);
    let manifest =
        serde_json::json!([{ "path": "src/sub/b.txt", "length": txt.len() }]).to_string();
    sandbox.stage(&manifest, txt).unwrap();

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(applied["outcome"]["state"], "completed", "{applied}");
    assert_eq!(sandbox.product("/dst/images/a.jpg").unwrap(), jpg.to_vec());
    assert!(
        sandbox.product("src/sub/b.txt").is_ok(),
        "未命中规则的文件必须留在原地（unmatched=keep）"
    );
}

// ── A3-10：归档族（C10 解压/压缩走真 ZIP 编解码） ──────────────────────────────

use batchdesk_wasm::archive_mem::MemoryArchiveService;
use kernel::archive::{
    plan_c10, plan_c10_compress, ArchiveCompression, ArchiveService, PlanC10CompressInput,
    PlanC10Input,
};
use kernel::conflict::NameRules;
use kernel::memfs::MemoryFs;
use std::io::{Cursor, Read, Write};

/// 造一个真实的 ZIP（deflate）：不手写 ZIP 字节——测试要验的是「真包能解」。
fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// 造一个 WinZip AES-256 加密的 ZIP（固定盐：测试要确定性，不需要随机盐）。
fn encrypted_zip_bytes(name: &str, data: &[u8], password: &str) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .with_aes_encryption_and_salt(password.as_bytes(), zip::AesSalt::Aes256([7u8; 16]));
    writer.start_file(name, options).unwrap();
    writer.write_all(data).unwrap();
    writer.finish().unwrap().into_inner()
}

/// 用**归档端口自己的 inspect** 取事实再出计划（浏览器里这份事实来自中央目录读取，
/// 这里直接问端口，保证「列出来的」与「解得出的」是同一份）。
fn c10_extract_plan(archive: &[u8], password_provided: bool) -> kernel::plan::PlanDocument {
    let fs = MemoryFs::new();
    fs.insert_file("pkg.zip", archive.to_vec()).unwrap();
    let inspect = MemoryArchiveService::new(&fs).inspect("pkg.zip").unwrap();
    plan_c10(PlanC10Input {
        plan_id: PLAN_ID.to_string(),
        archive: "pkg.zip".to_string(),
        archive_size: inspect.archive_size,
        entries: inspect.entries,
        output_dir: "/out".to_string(),
        max_total_bytes: 1_000_000,
        max_ratio: 100.0,
        existing_targets: Vec::new(),
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        name_rules: NameRules::posix(),
        password_provided,
    })
    .unwrap()
}

#[test]
fn c10_extract_unpacks_into_the_sandbox_and_undo_clears_it() {
    let archive = zip_bytes(&[("docs/a.txt", b"AAA"), ("docs/sub/b.txt", b"BBB")]);
    let plan = c10_extract_plan(&archive, false);
    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "pkg.zip", &archive);

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(
        applied["outcome"]["state"], "completed",
        "真解压应完成：{applied}"
    );
    // 目录层级按包内路径还原，内容逐字节一致。
    assert_eq!(sandbox.product("/out/docs/a.txt").unwrap(), b"AAA".to_vec());
    assert_eq!(
        sandbox.product("/out/docs/sub/b.txt").unwrap(),
        b"BBB".to_vec()
    );
    let products = applied["products"].as_array().unwrap();
    assert_eq!(products.len(), 2, "两个文件条目各算一件产物：{applied}");

    sandbox.undo(PLAN_ID).unwrap();
    assert!(
        sandbox.product("/out/docs/a.txt").is_err(),
        "撤销后产物必须收走"
    );
}

#[test]
fn c10_compress_writes_a_real_zip_that_round_trips() {
    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "a.txt", b"alpha");
    stage_one(&mut sandbox, "sub/b.txt", b"beta");

    let plan = plan_c10_compress(PlanC10CompressInput {
        plan_id: PLAN_ID.to_string(),
        output: "/out/pkg.zip".to_string(),
        sources: vec!["a.txt".to_string(), "sub/b.txt".to_string()],
        compression: ArchiveCompression::Deflate,
        target_exists: false,
        source_file_count: 2,
        source_bytes: 9,
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
    })
    .unwrap();

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(
        applied["outcome"]["state"], "completed",
        "真压缩应完成：{applied}"
    );
    assert_eq!(
        applied["products"].as_array().unwrap().len(),
        1,
        "压缩只产出一个包：{applied}"
    );

    // 产物能被第三方 zip 库读回：条目名、内容、清单三者一致。
    let bytes = sandbox.product("/out/pkg.zip").unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut names = Vec::new();
    for index in 0..zip.len() {
        names.push(
            zip.by_index_data(index)
                .unwrap()
                .name()
                .unwrap_or_default()
                .to_string(),
        );
    }
    // 条目名＝源的**文件名**（与桌面 `ZipArchiveService::compress` 同口径：目录源才
    // 保留层级，文件源按文件名入包，重名由执行器报 E1002）。
    assert_eq!(names, vec!["a.txt", "b.txt"], "条目名与桌面同口径");
    let mut entry = zip.by_index(0).unwrap();
    let mut text = String::new();
    entry.read_to_string(&mut text).unwrap();
    assert_eq!(text, "alpha");

    let detail = applied["outcome"]["report"]["entries"][0]["detail"]
        .as_str()
        .unwrap_or_default();
    assert!(
        detail.contains("2 个文件") && detail.contains("b.txt"),
        "清单要能回答包里有哪几个文件：{detail}"
    );
}

#[test]
fn c10_encrypted_archive_needs_the_password_at_execution_time() {
    let archive = encrypted_zip_bytes("secret/a.txt", b"TOP", "pw123");
    let plan = c10_extract_plan(&archive, true);
    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "pkg.zip", &archive);

    // 计划说「执行期会提供密码」但请求没带：整单拒绝，不留半截产物。
    let error = sandbox.apply(&apply_request(&plan)).unwrap_err();
    assert_eq!(error.code, "E3008", "缺密码要能补密码，不是格式不支持");
    assert!(
        sandbox
            .files()
            .iter()
            .all(|path| !path.starts_with("/out/")),
        "拒绝时不得写出任何条目"
    );

    // 带上密码就真解出来（密码只随请求进来，计划里没有它）。
    let mut request: serde_json::Value = serde_json::from_str(&apply_request(&plan)).unwrap();
    request["password"] = serde_json::json!("pw123");
    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&request.to_string()).unwrap()).unwrap();
    assert_eq!(
        applied["outcome"]["state"], "completed",
        "给了密码应解出来：{applied}"
    );
    assert_eq!(
        sandbox.product("/out/secret/a.txt").unwrap(),
        b"TOP".to_vec()
    );
}

// ── A3-11：改名族的执行面（C04/F12/F13 在沙箱里真改名、真撤销） ──────────────

/// F12/F13/C04 的共用执行断言：源在沙箱里被改名为目标，产物可取、撤销后回退。
fn assert_renamed_and_undone(
    sandbox: &mut Sandbox,
    plan: &kernel::plan::PlanDocument,
    source: &str,
    target: &str,
    expected_bytes: &[u8],
) {
    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(plan)).unwrap()).unwrap();
    assert_eq!(
        applied["outcome"]["state"], "completed",
        "真改名应完成：{applied}"
    );
    // 产物＝改名后的文件（内容不变）：下载得到的就是改名后的副本。
    assert_eq!(
        sandbox.product(target).unwrap(),
        expected_bytes.to_vec(),
        "改名后的目标必须取得到字节"
    );
    assert!(sandbox.product(source).is_err(), "改名后源路径不应再存在");
    // 沙箱键一律是绝对路径：计划里的相对 target 在执行时按工作根规范（`素材/x` →
    // `/素材/x`），所以产物清单里是带前导斜杠的形态。
    let expected_product = format!("/{target}");
    assert!(
        applied["products"]
            .as_array()
            .unwrap()
            .iter()
            .any(|product| product["path"] == expected_product.as_str()),
        "产物清单要点名改名目标 {expected_product}：{applied}"
    );

    // 用计划自己的 id：C04/F13 的计划 id 由 wire 层生成（`plan_c04_core` 内部），
    // 不是测试里的 `PLAN_ID`——拿常量去撤销会得到「零条可撤」的假成功。
    sandbox.undo(plan.plan_id.as_str()).unwrap();
    assert_eq!(
        sandbox.product(source).unwrap(),
        expected_bytes.to_vec(),
        "撤销必须把源放回原位"
    );
    assert!(sandbox.product(target).is_err(), "撤销后目标不应残留");
}

#[test]
fn f12_prefix_rename_runs_in_the_sandbox() {
    let payload = serde_json::json!({
        "planId": PLAN_ID,
        "items": [{ "path": "素材/a.txt" }, { "path": "素材/sub/b.txt" }],
        "existingTargets": [],
        "protectedDirs": [],
        "caseSense": "sensitive",
        "longPathSupported": true,
        "accessFacts": [],
        "rule": { "prefix": "new_", "replace": "", "scope": "Full" }
    })
    .to_string();
    let plan: kernel::plan::PlanDocument =
        serde_json::from_str(&batchdesk_wasm::plan_f12_core(&payload).unwrap()).unwrap();

    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "素材/a.txt", b"AAA");
    stage_one(&mut sandbox, "素材/sub/b.txt", b"BBB");
    assert_renamed_and_undone(&mut sandbox, &plan, "素材/a.txt", "素材/new_a.txt", b"AAA");
}

#[test]
fn c04_name_replace_runs_in_the_sandbox() {
    let payload = serde_json::json!({
        "items": [{ "path": "素材/nb-01.txt" }],
        "rule": { "mode": "plain", "find": "nb-", "replace": "note-", "scope": "stem" }
    })
    .to_string();
    let plan: kernel::plan::PlanDocument =
        serde_json::from_str(&batchdesk_wasm::plan_c04_core(&payload).unwrap()).unwrap();

    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "素材/nb-01.txt", b"note body");
    assert_renamed_and_undone(
        &mut sandbox,
        &plan,
        "素材/nb-01.txt",
        "素材/note-01.txt",
        b"note body",
    );
}

#[test]
fn f13_regex_rename_runs_in_the_sandbox() {
    let payload = serde_json::json!({
        "items": [{ "path": "素材/img 1.png" }],
        "rule": { "pattern": "\\s+", "replacement": "_", "scope": "stem" }
    })
    .to_string();
    let plan: kernel::plan::PlanDocument =
        serde_json::from_str(&batchdesk_wasm::plan_f13_core(&payload).unwrap()).unwrap();

    let mut sandbox = Sandbox::new();
    stage_one(&mut sandbox, "素材/img 1.png", b"PNG");
    assert_renamed_and_undone(
        &mut sandbox,
        &plan,
        "素材/img 1.png",
        "素材/img_1.png",
        b"PNG",
    );
}

// ── A3-12：F10 目录清单的执行面（目录事实走端口，产物是 BOM CSV） ────────────

/// 走 Web 的真实链路造 F10 计划：线协议 JSON → `plan_f10_core`（与浏览器同一入口）。
fn f10_plan(roots: serde_json::Value) -> kernel::plan::PlanDocument {
    let payload = serde_json::json!({ "roots": roots }).to_string();
    serde_json::from_str(&batchdesk_wasm::plan_f10_core(&payload).unwrap()).unwrap()
}

/// F10 的目录事实（`stage_listing` 的入参形状）。
fn f10_facts(root: &str, files: serde_json::Value) -> String {
    serde_json::json!([{ "root": root, "files": files }]).to_string()
}

#[test]
fn f10_listing_writes_a_bom_csv_and_undo_removes_it() {
    let plan = f10_plan(serde_json::json!([{
        "path": "素材",
        "target": "out/素材.csv",
        "files": 3,
        "dirs": 2,
        "maxDepth": 3
    }]));
    assert_eq!(plan.kind, "f10");
    assert_eq!(plan.entries[0].blocked, None, "计划的源与目标不冲突");

    let mut sandbox = Sandbox::new();
    // 目录要先建出来：执行器预检 `stat(源)` 要求源是目录（`stage_dirs`）。
    sandbox
        .stage_dirs(
            &serde_json::json!([
                { "path": "素材" },
                { "path": "素材/sub" },
                { "path": "素材/sub/deep" }
            ])
            .to_string(),
        )
        .unwrap();
    // 清单内容来自浏览器采到的事实；**不登记文件字节**——清单只读元数据。
    sandbox
        .stage_listing(&f10_facts(
            "素材",
            serde_json::json!([
                { "path": "a.txt", "size": 3, "mtime": "2024-01-01T00:00:00Z" },
                { "path": "sub/b.txt", "size": 5, "mtime": "" },
                { "path": "sub/deep/c.txt", "size": 7, "mtime": "2024-01-03T00:00:00Z" }
            ]),
        ))
        .unwrap();

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_eq!(
        applied["outcome"]["state"], "completed",
        "真实执行应完成：{applied}"
    );
    let products = applied["products"].as_array().unwrap();
    assert_eq!(products.len(), 1, "一个根一条清单：{applied}");
    assert_eq!(products[0]["path"], "/out/素材.csv");

    // 产物就是 UTF-8 BOM + 桌面同口径的表头与行（不是占位文件）。
    let csv = sandbox.product("/out/素材.csv").unwrap();
    assert_eq!(&csv[..3], b"\xEF\xBB\xBF", "清单必须是 UTF-8 BOM");
    let text = String::from_utf8(csv[3..].to_vec()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "相对路径,名称,类型,大小,修改时间,层级");
    assert_eq!(lines[1], "a.txt,a.txt,文件,3,2024-01-01T00:00:00Z,1");
    assert_eq!(lines[2], "sub,sub,目录,0,,1");
    assert_eq!(lines[3], "sub/b.txt,b.txt,文件,5,,2");
    assert_eq!(lines[4], "sub/deep,deep,目录,0,,2");
    assert_eq!(
        lines[5],
        "sub/deep/c.txt,c.txt,文件,7,2024-01-03T00:00:00Z,3"
    );

    // 撤销：清单进隔离目录（不是「删了就完了」），产物随之消失。
    sandbox.undo(plan.plan_id.as_str()).unwrap();
    assert!(
        sandbox.product("/out/素材.csv").is_err(),
        "撤销后产物不应残留"
    );
}

#[test]
fn f10_without_staged_directories_reports_e2001_per_entry() {
    // 只登记事实、不建目录：预检 `stat(源)` 失败——必须逐条报出来，
    // 而不是产出一份「空清单」骗过界面。
    let plan = f10_plan(serde_json::json!([{
        "path": "素材",
        "target": "out/素材.csv",
        "files": 1,
        "dirs": 0,
        "maxDepth": 1
    }]));
    let mut sandbox = Sandbox::new();
    sandbox
        .stage_listing(&f10_facts(
            "素材",
            serde_json::json!([{ "path": "a.txt", "size": 3, "mtime": "" }]),
        ))
        .unwrap();

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    assert_ne!(applied["outcome"]["state"], "completed");
    assert!(
        applied["products"].as_array().unwrap().is_empty(),
        "失败不得留下产物：{applied}"
    );
    let first = &applied["outcome"]["report"]["entries"][0];
    assert_eq!(first["status"], "failed");
    assert_eq!(first["error"]["code"], "E2001");
}

#[test]
fn f10_without_listing_facts_reports_the_missing_port() {
    // 目录建了、事实没登记：端口缺失走 E5001（能力缺失 ≠ 文件不存在），
    // 两种失败必须分得开，否则界面会把「接线漏了」显示成「用户文件没了」。
    let plan = f10_plan(serde_json::json!([{
        "path": "素材",
        "target": "out/素材.csv",
        "files": 1,
        "dirs": 0,
        "maxDepth": 1
    }]));
    let mut sandbox = Sandbox::new();
    sandbox
        .stage_dirs(&serde_json::json!([{ "path": "素材" }]).to_string())
        .unwrap();

    let applied: serde_json::Value =
        serde_json::from_str(&sandbox.apply(&apply_request(&plan)).unwrap()).unwrap();
    let first = &applied["outcome"]["report"]["entries"][0];
    assert_eq!(first["status"], "failed");
    assert_eq!(first["error"]["code"], "E5001");
}

#[test]
fn f10_listing_target_inside_the_root_is_refused_by_the_plan() {
    // 清单不得自包含：目标落在被列出的根目录内，计划期就 blocked（E1002）。
    let plan = f10_plan(serde_json::json!([{
        "path": "素材",
        "target": "素材/清单.csv",
        "files": 1,
        "dirs": 0,
        "maxDepth": 1
    }]));
    let blocked = plan.entries[0].blocked.as_ref().expect("必须被拦下");
    assert_eq!(blocked.code, "E1002");
}
