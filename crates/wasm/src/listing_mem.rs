//! F10 目录清单在 Web 形态的 `DirectoryLister`（ADR-0114）。
//!
//! 为什么需要它：F10 的执行臂是「流式遍历目录 → 写 UTF-8 BOM CSV」，桌面由
//! `platform::listing::FsDirectoryLister` 直接读磁盘；浏览器没有磁盘遍历能力，
//! 目录事实由前端从 `input[webkitdirectory]` 的 `webkitRelativePath` + `size` +
//! `lastModified` 采集后送进来。执行器仍是同一个 `perform_listing`，本模块只充当
//! 那个端口——不碰 `MemoryFs`、不读文件内容。
//!
//! **能力边界（与桌面的差异，必须如实记录）**：
//! - 浏览器只给「文件」，不给「空目录」——空目录不会出现在清单里；
//! - 目录的修改时间浏览器不暴露：目录行该列写空串（不编造时间）；
//! - 占位文件（OneDrive 云占位等）与符号链接在 File API 里不可分辨，
//!   `placeholders`/`skipped` 恒为 0；
//! - `dirs` 与 `max_depth` 由**文件路径的各级前缀**推出（不是读到的目录项），
//!   因此与桌面的差别恰是「空目录」这一项。
//!
//! 排序与层级口径与桌面逐条一致：每一层按名称升序、目录行先输出再递归子树、
//! 根目录自身不出行、目录大小记 0、层级从 1 起。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use kernel::csv_merge::write_record;
use kernel::error::KernelError;
use kernel::listing::{DirectoryLister, ListingOptions, ListingStats, LISTING_HEADER};
use serde::Deserialize;

/// 一个文件的事实（相对根路径 + 大小 + 修改时间）。
///
/// `path` 用正斜杠、无前导 `/`；`mtime` 是 RFC 3339 或空串（浏览器拿不到时）。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingFactFile {
    pub path: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub mtime: String,
}

/// 一个根目录及其文件事实。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingFactRoot {
    pub root: String,
    #[serde(default)]
    pub files: Vec<ListingFactFile>,
}

/// 沙箱内的清单端口：按根目录持有文件事实，被 `perform_listing` 调用。
#[derive(Clone, Debug, Default)]
pub struct MemoryDirectoryLister {
    roots: BTreeMap<String, Vec<ListingFactFile>>,
}

impl MemoryDirectoryLister {
    /// 从线协议 JSON（`[{root, files:[{path,size,mtime}]}]`）建端口。
    pub fn from_json(json: &str) -> Result<Self, KernelError> {
        let roots: Vec<ListingFactRoot> = serde_json::from_str(json).map_err(|error| {
            KernelError::invalid_params(format!("invalid listing facts: {error}"))
        })?;
        let mut table = BTreeMap::new();
        for root in roots {
            let key = root_key(&root.root);
            table.entry(key).or_insert_with(Vec::new).extend(root.files);
        }
        Ok(Self { roots: table })
    }

    /// 是否有这个根目录的事实（`stage_listing` 的返回摘要用）。
    pub fn contains(&self, root: &str) -> bool {
        self.roots.contains_key(&root_key(root))
    }

    /// 已登记的根目录数（`stage_listing` 的回执用，不给调用方数 JSON 字段）。
    pub fn root_count(&self) -> usize {
        self.roots.len()
    }
}

impl DirectoryLister for MemoryDirectoryLister {
    fn write_listing(
        &self,
        root: &str,
        options: &ListingOptions,
        out: &mut dyn Write,
    ) -> Result<ListingStats, KernelError> {
        let files = self.roots.get(&root_key(root)).ok_or_else(|| {
            KernelError::new(
                "E2001",
                "listing root facts are not staged in the browser sandbox",
                Some(root.to_string()),
                false,
            )
        })?;

        out.write_all(b"\xEF\xBB\xBF")
            .map_err(|error| listing_error(root, &error))?;
        let header: Vec<String> = LISTING_HEADER.iter().map(ToString::to_string).collect();
        let mut line = String::new();
        write_record(&header, b',', &mut line);
        out.write_all(line.as_bytes())
            .map_err(|error| listing_error(root, &error))?;

        let tree = ListingTree::build(files);
        let mut stats = ListingStats::default();
        tree.emit("", 0, options, out, &mut stats, root)?;
        Ok(stats)
    }
}

/// 相对根的路径口径：反斜杠转正斜杠、去掉两端多余的 `/`。
fn root_key(root: &str) -> String {
    root.replace('\\', "/").trim_matches('/').trim().to_string()
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// 由文件事实推出的目录树（父路径 → 子项名，自动排序去重）。
struct ListingTree<'a> {
    /// 目录的相对路径集合（由文件路径的各级前缀推出）。
    dirs: BTreeSet<String>,
    files: BTreeMap<String, &'a ListingFactFile>,
    children: BTreeMap<String, BTreeSet<String>>,
}

impl<'a> ListingTree<'a> {
    fn build(files: &'a [ListingFactFile]) -> Self {
        let mut tree = Self {
            dirs: BTreeSet::new(),
            files: BTreeMap::new(),
            children: BTreeMap::new(),
        };
        for file in files {
            let normalized = file.path.replace('\\', "/");
            let parts: Vec<&str> = normalized
                .split('/')
                .filter(|part| !part.is_empty())
                .collect();
            if parts.is_empty() {
                continue;
            }
            let mut parent = String::new();
            for (index, part) in parts.iter().enumerate() {
                let path = join(&parent, part);
                tree.children
                    .entry(parent.clone())
                    .or_default()
                    .insert((*part).to_string());
                if index + 1 < parts.len() {
                    tree.dirs.insert(path.clone());
                }
                parent = path;
            }
            tree.files.insert(parent, file);
        }
        tree
    }

    /// 深度优先写出：每层按名字升序，目录行先出行再递归（与桌面 `walk_dir` 同序）。
    fn emit(
        &self,
        parent: &str,
        level: u32,
        options: &ListingOptions,
        out: &mut dyn Write,
        stats: &mut ListingStats,
        root: &str,
    ) -> Result<(), KernelError> {
        let Some(names) = self.children.get(parent) else {
            return Ok(());
        };
        for name in names {
            let path = join(parent, name);
            let child_level = level + 1;
            if self.dirs.contains(&path) {
                stats.dirs += 1;
                stats.max_depth = stats.max_depth.max(child_level);
                if options.include_dirs {
                    // 目录大小记 0、修改时间留空（浏览器不暴露目录属性）。
                    write_row(out, &path, name, "目录", 0, "", child_level, root)?;
                }
                self.emit(&path, child_level, options, out, stats, root)?;
            } else if let Some(file) = self.files.get(&path) {
                stats.files += 1;
                stats.max_depth = stats.max_depth.max(child_level);
                if options.include_files {
                    write_row(
                        out,
                        &path,
                        name,
                        "文件",
                        file.size,
                        &file.mtime,
                        child_level,
                        root,
                    )?;
                }
            }
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn write_row(
    out: &mut dyn Write,
    path: &str,
    name: &str,
    kind: &str,
    size: u64,
    mtime: &str,
    level: u32,
    root: &str,
) -> Result<(), KernelError> {
    let fields = vec![
        path.to_string(),
        name.to_string(),
        kind.to_string(),
        size.to_string(),
        mtime.to_string(),
        level.to_string(),
    ];
    let mut line = String::new();
    write_record(&fields, b',', &mut line);
    out.write_all(line.as_bytes())
        .map_err(|error| listing_error(root, &error))
}

fn listing_error(root: &str, error: &std::io::Error) -> KernelError {
    KernelError::new("E2008", error.to_string(), Some(root.to_string()), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(json: &str) -> MemoryDirectoryLister {
        MemoryDirectoryLister::from_json(json).unwrap()
    }

    fn render(lister: &MemoryDirectoryLister, root: &str) -> (String, ListingStats) {
        let mut output = Vec::new();
        let stats = lister
            .write_listing(root, &ListingOptions::default(), &mut output)
            .unwrap();
        assert_eq!(&output[..3], b"\xEF\xBB\xBF");
        (String::from_utf8(output[3..].to_vec()).unwrap(), stats)
    }

    #[test]
    fn writes_bom_header_and_nested_rows_in_desktop_order() {
        let lister = facts(
            r#"[{"root":"photos","files":[
                {"path":"b.txt","size":2,"mtime":"2024-01-02T00:00:00Z"},
                {"path":"sub/a.txt","size":1,"mtime":"2024-01-01T00:00:00Z"},
                {"path":"sub/deep/c.txt","size":3,"mtime":""}
            ]}]"#,
        );
        let (text, stats) = render(&lister, "photos");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "相对路径,名称,类型,大小,修改时间,层级");
        // 每层按名称升序：b.txt 先于 sub；sub 目录行先出、再递归其子树。
        assert_eq!(lines[1], "b.txt,b.txt,文件,2,2024-01-02T00:00:00Z,1");
        assert_eq!(lines[2], "sub,sub,目录,0,,1");
        assert_eq!(lines[3], "sub/a.txt,a.txt,文件,1,2024-01-01T00:00:00Z,2");
        assert_eq!(lines[4], "sub/deep,deep,目录,0,,2");
        assert_eq!(lines[5], "sub/deep/c.txt,c.txt,文件,3,,3");
        assert_eq!(stats.files, 3);
        assert_eq!(stats.dirs, 2);
        assert_eq!(stats.max_depth, 3);
        assert_eq!(stats.placeholders, 0);
        assert_eq!(stats.skipped, 0);
    }

    #[test]
    fn only_file_rows_are_needed_parents_are_derived() {
        // 前端不必单独登记目录行：目录集合由路径前缀推出，漏登记也不会断树。
        let lister = facts(r#"[{"root":"r","files":[{"path":"x/y/z.txt","size":1}]}]"#);
        let (text, stats) = render(&lister, "/r/");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "表头 + 两级目录 + 文件");
        assert_eq!(lines[1], "x,x,目录,0,,1");
        assert_eq!(lines[2], "x/y,y,目录,0,,2");
        assert_eq!(lines[3], "x/y/z.txt,z.txt,文件,1,,3");
        assert_eq!(stats.dirs, 2);
    }

    #[test]
    fn empty_root_writes_only_the_header() {
        let lister = facts(r#"[{"root":"empty","files":[]}]"#);
        let (text, stats) = render(&lister, "empty");
        assert_eq!(text.lines().count(), 1);
        assert_eq!(stats.files + stats.dirs, 0);
    }

    #[test]
    fn unknown_root_is_e2001() {
        let lister = facts(r#"[{"root":"a","files":[]}]"#);
        let mut output = Vec::new();
        let error = lister
            .write_listing("b", &ListingOptions::default(), &mut output)
            .unwrap_err();
        assert_eq!(error.code, "E2001");
    }

    #[test]
    fn options_can_leave_out_dirs_but_stats_still_count_them() {
        // 与桌面 `walk` 同口径：统计与实际输出无关（`stats.dirs` 在输出前累加）。
        let lister = facts(r#"[{"root":"r","files":[{"path":"d/a.txt","size":1}]}]"#);
        let mut output = Vec::new();
        let stats = lister
            .write_listing(
                "r",
                &ListingOptions {
                    include_files: true,
                    include_dirs: false,
                },
                &mut output,
            )
            .unwrap();
        let text = String::from_utf8(output[3..].to_vec()).unwrap();
        assert_eq!(text.lines().count(), 2, "表头 + 文件（目录行被关掉）");
        assert_eq!(stats.dirs, 1);
    }

    #[test]
    fn commas_and_quotes_in_names_are_escaped_like_the_desktop_writer() {
        let lister = facts(r#"[{"root":"r","files":[{"path":"a,b.txt","size":1}]}]"#);
        let (text, _) = render(&lister, "r");
        assert!(
            text.contains("\"a,b.txt\""),
            "含逗号的名字必须被引号包住：{text}"
        );
    }
}
