//! `loadRecipe` 的路径槽位表（OQ-40 裁决①：模板 id ＋**调用方交进来的路径参数**）。
//!
//! 为什么需要这张表：模板按不变量 13 **不保存任何路径**（保存与导入两条路都会按
//! `PATH_PARAM_KEYS` 剔除），所以「把模板变成计划」缺的正是路径那一段。用户裁的是①——
//! `loadRecipe` 的入参带上文件集，壳层再走与 `planOperation` **完全相同**的那条路由（同出口）。
//!
//! 键名不靠猜：每个功能的槽位都取自该路由**参数结构体的必填字段**，出处逐条标在下面。
//! 线协议名是 serde `camelCase` 后的形态（`output_dir` → `outputDir`、`source_root` → `sourceRoot`）。
//! 没登记的功能直接报错，不退化成"试几个键名看哪个通过"，也不按位置猜哪个是输入哪个是输出。
//!
//! 两类槽位：
//! - `required`：**每个都必须给**（输入侧，f11 的 `root`、c07 的 `sourceRoot` 也在这一类，
//!   因为它们在各自路由里就是必填字段，语义上属于"从哪儿读"）；
//! - `outputs`：**至少给其中一个**（输出侧的互斥组，如 `output`／`outputDir`；空组＝该功能没有输出路径）。

use kernel::recipe::RecipeDocument;
use serde_json::{Map, Value};

/// 一个路径槽位：线协议键 ＋ 是否多值。
#[derive(Clone, Copy, Debug)]
pub struct PathSlot {
    pub key: &'static str,
    pub many: bool,
}

const fn one(key: &'static str) -> PathSlot {
    PathSlot { key, many: false }
}

const fn many(key: &'static str) -> PathSlot {
    PathSlot { key, many: true }
}

/// 一个功能的路径槽位。
#[derive(Clone, Copy, Debug)]
pub struct FeaturePaths {
    pub feature: &'static str,
    pub required: &'static [PathSlot],
    /// 输出互斥组：给其中任意一个即可（空切片＝无输出路径）。
    pub outputs: &'static [PathSlot],
    /// c09 的扫描根走 `planOperation` 的 `sourcePaths` 入参而不是 params
    /// （`lib.rs:264` 用 `source_paths[0]` 当根，且要求恰好一个）。
    pub via_source_paths: bool,
}

/// 逐条出处（2026-11-06 实测自各路由结构体）：
/// - f01／f02／f03：`plan_kinds.rs:62` `SplitParams{file, outputDir}` 两个都必填
/// - f04：`plan_kinds.rs:186` `MergeParams{files, output}` 必填
/// - f05：`lib.rs:887` `F05ShellParams{file, outputDir}` 必填
/// - f06：`plan_csv.rs:215` 明确 `requires output`；输入 `files`
/// - f07／f08／f09：`plan_csv.rs:115-134` `requires output or outputDir`；输入 `files`
/// - f10：`plan_csv.rs:31` `CsvPlanParams{roots}`（清单类功能按根目录扫描）；输出同上二选一
/// - f11：`lib.rs:989` `F11ShellParams{list, root}` 两个都必填（清单文件＋建档根）
/// - f12：`lib.rs:1081` `F12ShellParams{dir}` 必填，无输出路径
/// - c04：`plan_ops.rs:263` 该分支只用 `dir`
/// - c01／c02／c03：`plan_kinds.rs:280/357/430` 输入 `files`；`target` 是**编码**不是路径，
///   留在参数快照里，不进槽位
/// - c07：`plan_manifest.rs:58` `ManifestParams{manifest, sourceRoot, targetRoot}`
/// - c08：`plan_manifest.rs:208` `ArchivePlanParams{sourceRoot, targetRoot, rules}`；
///   `rules` 是规则文本，留在快照里
/// - c10：`plan_manifest.rs:344/383` 按 `mode` 分流——解压 `archive`＋`outputDir`、
///   压缩 `sources`＋`output`
/// - c11…c14：`plan_ops.rs:35` 输入 `files`；输出 `output`／`outputDir` 二选一
///   （`output` 单值还要求**恰好一个输入**，见 `plan_ops.rs:171`，这条由路由自己把关）
/// - c09：`C09Params`（`lib.rs:158`）没有任何路径字段，根来自 `sourcePaths`
pub const FEATURE_PATH_SLOTS: &[FeaturePaths] = &[
    FeaturePaths {
        feature: "c09",
        required: &[many("sourcePaths")],
        outputs: &[],
        via_source_paths: true,
    },
    FeaturePaths {
        feature: "f01",
        required: &[one("file")],
        outputs: &[one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f02",
        required: &[one("file")],
        outputs: &[one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f03",
        required: &[one("file")],
        outputs: &[one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f04",
        required: &[many("files")],
        outputs: &[one("output")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f05",
        required: &[one("file")],
        outputs: &[one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f06",
        required: &[many("files")],
        outputs: &[one("output")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f07",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f08",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f09",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f10",
        required: &[many("roots")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f11",
        required: &[one("list"), one("root")],
        outputs: &[],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "f12",
        required: &[one("dir")],
        outputs: &[],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c04",
        required: &[one("dir")],
        outputs: &[],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c01",
        required: &[many("files")],
        outputs: &[],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c02",
        required: &[many("files")],
        outputs: &[],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c03",
        required: &[many("files")],
        outputs: &[],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c07",
        required: &[one("manifest"), one("sourceRoot")],
        outputs: &[one("targetRoot")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c08",
        required: &[one("sourceRoot")],
        outputs: &[one("targetRoot")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c10",
        required: &[one("archive")],
        outputs: &[one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c11",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c12",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c13",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
    FeaturePaths {
        feature: "c14",
        required: &[many("files")],
        outputs: &[one("output"), one("outputDir")],
        via_source_paths: false,
    },
];

/// c10 压缩模式的槽位（解压态在上面的表里，因为快照里的 `mode` 才是分流依据）。
pub const C10_COMPRESS_SLOTS: &[PathSlot] = &[many("sources")];
pub const C10_COMPRESS_OUTPUTS: &[PathSlot] = &[one("output")];

pub fn slots_for(feature: &str) -> Option<&'static FeaturePaths> {
    FEATURE_PATH_SLOTS
        .iter()
        .find(|entry| entry.feature == feature)
}

/// 一次 `loadRecipe` 的结果：交给 `planOperation` 同一条路由的三个入参。
#[derive(Debug)]
pub struct RecipeInvocation {
    pub kind: String,
    pub params: Value,
    pub source_paths: Vec<String>,
}

fn is_compress(snapshot: &Map<String, Value>) -> bool {
    snapshot
        .get("mode")
        .and_then(Value::as_str)
        .map(|mode| mode == "compress")
        .unwrap_or(false)
}

/// 把调用方给的路径参数按槽位校验后注入参数快照。
///
/// 三条硬规则，都是为了"宁可报错也不猜"：
/// 1. 快照里**已经带**某个路径键 → 报错（不变量 13 说模板不该存路径；真存了说明这份
///    模板绕过了保存通道，静默覆盖等于把这条边界抹掉）；
/// 2. 必填槽缺失 → 报错并列出缺哪些；
/// 3. 给了该功能**没有的槽位键** → 报错，不静默忽略（忽略了就会让界面上填了却没生效变成静默失败）。
pub fn build_invocation(
    document: &RecipeDocument,
    path_params: &Map<String, Value>,
) -> Result<RecipeInvocation, String> {
    let (feature, step) = document
        .single_step_binding()
        .map_err(|error| error.message)?;
    let entry = slots_for(&feature)
        .ok_or_else(|| format!("feature {feature} has no registered path slots in this shell"))?;

    // c10 有两个模式，槽位不同：判据取快照里的 mode，不靠调用方另传一个开关
    let (required, outputs) = if feature == "c10" && is_compress(&step.params_snapshot) {
        (C10_COMPRESS_SLOTS, C10_COMPRESS_OUTPUTS)
    } else {
        (entry.required, entry.outputs)
    };

    let mut declared: Vec<&str> = required.iter().map(|slot| slot.key).collect();
    declared.extend(outputs.iter().map(|slot| slot.key));
    for key in path_params.keys() {
        if !declared.contains(&key.as_str()) {
            return Err(format!(
                "{feature} has no path slot {key}; declared slots are {}",
                declared.join(", ")
            ));
        }
    }

    let mut missing: Vec<&str> = Vec::new();
    for slot in required {
        if !path_params.contains_key(slot.key) {
            missing.push(slot.key);
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "{feature} needs the path slots: {}",
            missing.join(", ")
        ));
    }
    if !outputs.is_empty()
        && !outputs
            .iter()
            .any(|slot| path_params.contains_key(slot.key))
    {
        let options: Vec<&str> = outputs.iter().map(|slot| slot.key).collect();
        return Err(format!(
            "{feature} needs one of the output slots: {}",
            options.join(", ")
        ));
    }

    let mut params = step.params_snapshot.clone();
    let mut source_paths: Vec<String> = Vec::new();
    for slot in required.iter().chain(outputs.iter()) {
        let Some(value) = path_params.get(slot.key) else {
            continue;
        };
        // 单数槽收 1 个、多值槽收 ≥1 个：形状不对直接拒，不做"取第一个"的宽容处理
        let paths: Vec<String> = match value {
            Value::Array(items) => {
                if !slot.many {
                    return Err(format!(
                        "path slot {} of {feature} takes one value",
                        slot.key
                    ));
                }
                items
                    .iter()
                    .map(|item| {
                        item.as_str().map(|text| text.to_string()).ok_or_else(|| {
                            format!("path slot {} of {feature} is not a string", slot.key)
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
            Value::String(text) => {
                if slot.many {
                    return Err(format!("path slot {} of {feature} takes a list", slot.key));
                }
                vec![text.clone()]
            }
            _ => {
                return Err(format!(
                    "path slot {} of {feature} is empty or wrong type",
                    slot.key
                ))
            }
        };
        if paths.is_empty() || paths.iter().any(|path| path.trim().is_empty()) {
            return Err(format!(
                "path slot {} of {feature} must not be empty",
                slot.key
            ));
        }
        if entry.via_source_paths {
            if paths.len() != 1 {
                return Err("c09 requires exactly one source directory".to_string());
            }
            source_paths = paths;
            continue;
        }
        if params.contains_key(slot.key) {
            return Err(format!(
                "template params already carry {}; it must not store paths",
                slot.key
            ));
        }
        let injected = if slot.many {
            Value::Array(paths.into_iter().map(Value::String).collect())
        } else {
            Value::String(paths[0].clone())
        };
        params.insert(slot.key.to_string(), injected);
    }

    if entry.via_source_paths && source_paths.is_empty() {
        return Err("c09 requires exactly one source directory".to_string());
    }

    Ok(RecipeInvocation {
        kind: feature,
        params: Value::Object(params),
        source_paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::plan::PlanAction;
    use kernel::recipe::CURRENT_RECIPE_SCHEMA_VERSION;
    use serde_json::json;

    fn recipe(feature: &str, kind: PlanAction, snapshot: Map<String, Value>) -> RecipeDocument {
        RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "模板",
            kind,
            snapshot,
            "2026-11-06T00:00:00Z",
            Some(feature.to_string()),
        )
    }

    fn object(items: &[(&str, Value)]) -> Map<String, Value> {
        let mut map = Map::new();
        for (key, value) in items {
            map.insert((*key).to_string(), value.clone());
        }
        map
    }


    #[test]
    fn every_registered_feature_has_slots_and_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for entry in FEATURE_PATH_SLOTS {
            assert!(
                seen.insert(entry.feature),
                "功能码 {} 在槽位表里出现两次",
                entry.feature
            );
            assert!(!entry.required.is_empty(), "{} 没有必填槽位", entry.feature);
            for slot in entry.required.iter().chain(entry.outputs) {
                assert!(
                    kernel::recipe::PATH_PARAM_KEYS
                        .iter()
                        .any(|key| *key == slot.key.to_ascii_lowercase()),
                    "槽位 {} 不在路径键名单里＝它不是路径（会被快照带着走）",
                    slot.key
                );
            }
        }
        // 自校：槽位表覆盖的是壳层真能路由的功能码，缩水说明写法变了
        assert!(
            seen.len() >= 24,
            "槽位表只剩 {} 项，解析或写法变了（核对失效＝没有正例）",
            seen.len()
        );
    }

    #[test]
    fn slots_come_from_the_route_not_from_names() {
        // 这三条正是"凭记忆写会写错"的地方：c01 的输入是 files（不是 file）、
        // f10 是 roots（不是 root）、c08 的输出是 targetRoot。
        let entry = slots_for("c01").unwrap();
        assert_eq!(entry.required[0].key, "files");
        assert!(entry.outputs.is_empty());
        assert_eq!(slots_for("f10").unwrap().required[0].key, "roots");
        assert_eq!(slots_for("c08").unwrap().outputs[0].key, "targetRoot");
    }

    #[test]
    fn load_recipe_builds_the_same_route_call_as_plan_operation() {
        let invocation = build_invocation(
            &recipe(
                "f01",
                PlanAction::Split,
                object(&[("targetBytes", json!(1024))]),
            ),
            &object(&[("file", json!("/in/a.txt")), ("outputDir", json!("/out"))]),
        )
        .unwrap();
        assert_eq!(invocation.kind, "f01");
        assert_eq!(invocation.params["targetBytes"], json!(1024));
        assert_eq!(invocation.params["file"], json!("/in/a.txt"));
        assert_eq!(invocation.params["outputDir"], json!("/out"));
        assert!(invocation.source_paths.is_empty());
    }

    #[test]
    fn c09_uses_the_source_paths_channel_instead_of_params() {
        let invocation = build_invocation(
            &recipe(
                "c09",
                PlanAction::Delete,
                object(&[("keep", json!("earliest"))]),
            ),
            &object(&[("sourcePaths", json!(["/data"]))]),
        )
        .unwrap();
        assert_eq!(invocation.kind, "c09");
        assert_eq!(invocation.source_paths, vec!["/data".to_string()]);
        // 根不进 params：c09 的路由只读 source_paths[0]
        assert!(!invocation
            .params
            .as_object()
            .unwrap()
            .contains_key("sourcePaths"));

        let too_many = build_invocation(
            &recipe("c09", PlanAction::Delete, Map::new()),
            &object(&[("sourcePaths", json!(["/a", "/b"]))]),
        );
        assert!(too_many.unwrap_err().contains("exactly one"));
    }

    #[test]
    fn missing_and_extra_slots_are_both_rejected_by_name() {
        // 缺输出槽：报的是缺哪一个，不是含糊的"参数错误"
        let error = build_invocation(
            &recipe("f01", PlanAction::Split, Map::new()),
            &object(&[("file", json!("/in/a.txt"))]),
        )
        .unwrap_err();
        assert!(error.contains("outputDir"), "{error}");

        // 给了该功能没有的键：不静默忽略（否则界面上填了却没生效就是静默失败）
        let error = build_invocation(
            &recipe("f01", PlanAction::Split, Map::new()),
            &object(&[
                ("file", json!("/in/a.txt")),
                ("outputDir", json!("/out")),
                ("roots", json!(["/x"])),
            ]),
        )
        .unwrap_err();
        assert!(error.contains("no path slot roots"), "{error}");

        // 输出互斥组一个都没给
        let error = build_invocation(
            &recipe("c11", PlanAction::CsvDedup, Map::new()),
            &object(&[("files", json!(["/a.csv"]))]),
        )
        .unwrap_err();
        assert!(error.contains("one of the output slots"), "{error}");
    }

    #[test]
    fn slot_shapes_are_strict_about_single_and_list_values() {
        // 单数槽给数组、多值槽给字符串都要拒
        let error = build_invocation(
            &recipe("f01", PlanAction::Split, Map::new()),
            &object(&[("file", json!(["/a", "/b"])), ("outputDir", json!("/out"))]),
        )
        .unwrap_err();
        assert!(error.contains("takes one value"), "{error}");

        let error = build_invocation(
            &recipe("c01", PlanAction::EditContent, Map::new()),
            &object(&[("files", json!("/a.txt"))]),
        )
        .unwrap_err();
        assert!(error.contains("takes a list"), "{error}");

        let error = build_invocation(
            &recipe("c01", PlanAction::EditContent, Map::new()),
            &object(&[("files", json!(["  "]))]),
        )
        .unwrap_err();
        assert!(error.contains("must not be empty"), "{error}");
    }

    #[test]
    fn a_template_that_still_carries_paths_is_not_silently_overwritten() {
        // 不变量 13：模板不该带路径。真带了（绕过保存通道的手改文件）要报错，
        // 而不是让调用方给的值悄悄盖掉它——那等于把这条边界抹平。
        let snapshot = object(&[("file", json!("/leaked/a.txt"))]);
        let error = build_invocation(
            &recipe("f01", PlanAction::Split, snapshot),
            &object(&[("file", json!("/in/a.txt")), ("outputDir", json!("/out"))]),
        )
        .unwrap_err();
        assert!(error.contains("already carry file"), "{error}");
    }

    #[test]
    fn c10_slots_follow_the_mode_in_the_snapshot() {
        let extract = build_invocation(
            &recipe("c10", PlanAction::Extract, Map::new()),
            &object(&[("archive", json!("/a.zip")), ("outputDir", json!("/out"))]),
        )
        .unwrap();
        assert_eq!(extract.params["archive"], json!("/a.zip"));

        let compress = build_invocation(
            &recipe(
                "c10",
                PlanAction::Archive,
                object(&[("mode", json!("compress"))]),
            ),
            &object(&[
                ("sources", json!(["/a.txt"])),
                ("output", json!("/out.zip")),
            ]),
        )
        .unwrap();
        assert_eq!(compress.params["sources"], json!(["/a.txt"]));
        assert_eq!(compress.params["output"], json!("/out.zip"));

        // 压缩态误给解压键 → 拒
        let error = build_invocation(
            &recipe(
                "c10",
                PlanAction::Archive,
                object(&[("mode", json!("compress"))]),
            ),
            &object(&[("archive", json!("/a.zip")), ("output", json!("/o.zip"))]),
        )
        .unwrap_err();
        assert!(error.contains("no path slot archive"), "{error}");
    }

    #[test]
    fn unbound_and_multi_step_recipes_are_rejected_before_paths() {
        // 未绑定（一对多动作）：报的是"该重存"，不是"路径没给"
        let unbound = RecipeDocument::from_single_step(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            "未绑定",
            PlanAction::Split,
            Map::new(),
            "2026-11-06T00:00:00Z",
            None,
        );
        let error = build_invocation(&unbound, &Map::new()).unwrap_err();
        assert!(error.contains("re-save"), "{error}");

        let mut multi = recipe("f01", PlanAction::Split, Map::new());
        multi.schema_version = CURRENT_RECIPE_SCHEMA_VERSION;
        multi.steps.push(multi.steps[0].clone());
        let error = build_invocation(&multi, &Map::new()).unwrap_err();
        assert!(error.contains("multi-step"), "{error}");
    }

    #[test]
    fn every_feature_in_the_feature_table_has_path_slots() {
        // 内核那张「动作→功能码唯一」表里出现的功能码，都必须在这张槽位表里有位置——
        // 否则旧模板一迁移就变成"能套用但没有通道"的空承诺。
        for feature in [
            "c09", "f07", "f10", "f11", "c10", "c11", "c12", "c13", "c14", "c07",
        ] {
            assert!(
                slots_for(feature).is_some(),
                "功能码 {feature} 在唯一映射表里，却没有路径槽位"
            );
        }
    }
}
