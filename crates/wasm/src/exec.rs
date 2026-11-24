//! Web 执行沙箱：把真实的 `generic_apply`/`undo` 跑在会话内内存文件系统上（ADR-0108）。
//!
//! 为什么需要它：Web 形态没有 OPFS 执行器，而用户选中的源文件也不在 OPFS 里；
//! 浏览器能给的 File API 是**异步**的，内核执行器是**同步**的。所以执行前先把
//! 输入字节登记进沙箱（`stage`），执行器对沙箱跑与桌面同一套代码，产物再由
//! 主线程下载。**不是模拟执行**：计划、冲突裁决、Journal、撤销全走内核原路径。
//!
//! 边界：
//! - 沙箱不碰用户磁盘；产物只以字节形式交回调用方；
//! - 撤销只作用于沙箱内产物（`scope: products-only`，设计 02 §3.4）——
//!   已经下载到用户磁盘的副本无法被浏览器收回，界面必须照实说；
//! - C10 的归档端口由 `archive_mem::MemoryArchiveService` 提供（真 ZIP 编解码，
//!   与桌面同一个 `zip` crate 与同一套错误码，ADR-0112）；C07/C08 的源按相对路径
//!   登记（与前端 `stagedPath` 同一规则），键对不上时执行器逐条报 `E2001`；
//! - F10 目录清单的「目录」与「文件事实」分开登记：目录走 `stage_dirs`，文件事实
//!   （相对路径/大小/修改时间）走 `stage_listing`，执行时由 `MemoryDirectoryLister`
//!   按桌面同口径写成 CSV。**不登记文件字节**——清单本来就只读元数据（ADR-0114）；
//! - 解密密码是**执行期秘密**（ADR-0074）：只随本次请求进来，沙箱不留副本、
//!   Journal 与计划都不写它。

use kernel::apply::{
    apply_plan, undo_plan_with_ports, ApplyContext, ApplyOptions, UndoOutcome, UndoPorts,
};
use kernel::archive::{apply_archive_plan, ArchiveApplyContext, C10_KIND};
use kernel::encoding::TranscodeRewriter;
use kernel::error::KernelError;
use kernel::fs::FsAdapter;
use kernel::generic_apply::{apply_generic_plan, ContentRewriter, GenericApplyContext};
use kernel::journal::{FsOp, JournalStore, MemoryJournal};
use kernel::license::AlwaysAllowLicense;
use kernel::line_endings::{LineEndingRewriter, C02_KIND};
use kernel::memfs::MemoryFs;
use kernel::plan::{PlanDocument, C01_KIND, C09_KIND};
use kernel::replace_content::{ReplaceRewriter, C03_KIND};
use serde::Deserialize;

use crate::archive_mem::MemoryArchiveService;
use crate::listing_mem::MemoryDirectoryLister;

/// 沙箱默认时间戳：宿主没有传时钟事实时用它（Journal 只在会话内，ts 仅作展示）。
pub const DEFAULT_NOW: &str = "1970-01-01T00:00:00Z";

/// 登记清单里的一项（`stage` 的参数）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StagedFile {
    path: String,
    length: usize,
}

/// 只登记目录形态的清单项（`stage_dirs` 的参数；没有字节）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StagedDir {
    path: String,
}

/// 一次执行的请求（线协议，camelCase）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyRequest {
    pub plan: PlanDocument,
    #[serde(default)]
    pub options: Option<ApplyOptions>,
    #[serde(default)]
    pub now: Option<String>,
    /// 解密密码（执行期秘密，来自界面输入；不进计划、不进 Journal、不留在沙箱）。
    ///
    /// 计划里只有 `params.passwordProvided` 这个**事实**（ADR-0074）：计划说需要
    /// 密码而这里为空时，C10 执行器整单拒绝（`E3008`），不会解一半。
    #[serde(default)]
    pub password: Option<String>,
}

/// 会话沙箱：内存文件系统 + 内存 Journal。
pub struct Sandbox {
    fs: MemoryFs,
    journal: MemoryJournal,
    /// 最近一次执行的产物（沙箱内路径，按条目顺序）。
    products: Vec<String>,
    /// F10 的目录事实端口（`stage_listing` 登记；其它视图为 `None`）。
    listing: Option<MemoryDirectoryLister>,
}

impl Default for Sandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Sandbox {
    pub fn new() -> Self {
        Self {
            fs: MemoryFs::new(),
            journal: MemoryJournal::new(),
            products: Vec::new(),
            listing: None,
        }
    }

    /// 清空沙箱（换输入即失效：调用方在重新预览/换视图时重置）。
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// 登记输入字节：`manifest` 是 `[{path, length}]`，`data` 是按同序拼接的字节。
    ///
    /// 用「清单 + 一段连续字节」而不是逐文件传：wasm-bindgen 的数组入参要逐个
    /// 分配，拼接后只过一次边界；代价是调用方要自己算偏移（Worker 里本来就是
    /// 顺序拼的）。
    pub fn stage(&mut self, manifest: &str, data: &[u8]) -> Result<String, KernelError> {
        let entries: Vec<StagedFile> = serde_json::from_str(manifest).map_err(|error| {
            KernelError::invalid_params(format!("invalid stage manifest: {error}"))
        })?;
        let expected: usize = entries.iter().map(|entry| entry.length).sum();
        if expected != data.len() {
            return Err(KernelError::invalid_params(format!(
                "stage manifest expects {expected} bytes but got {}",
                data.len()
            )));
        }
        let mut offset = 0usize;
        for entry in &entries {
            let end = offset + entry.length;
            self.fs
                .insert_file(&entry.path, data[offset..end].to_vec())?;
            offset = end;
        }
        Ok(serde_json::json!({
            "staged": entries.len(),
            "bytes": data.len(),
        })
        .to_string())
    }

    /// 只登记**目录**（F10 的清单根与各级子目录；没有字节）。
    ///
    /// 目录必须是真实存在的键，否则执行器预检 `stat(源)` 会报「源缺失」——
    /// 而这些目录在沙箱里没有对应的文件条目可推。幂等：多根目录共享父级时重复登记。
    pub fn stage_dirs(&mut self, manifest: &str) -> Result<String, KernelError> {
        let entries: Vec<StagedDir> = serde_json::from_str(manifest).map_err(|error| {
            KernelError::invalid_params(format!("invalid stage dirs manifest: {error}"))
        })?;
        for entry in &entries {
            self.fs.insert_dir(&entry.path)?;
        }
        Ok(serde_json::json!({ "stagedDirs": entries.len() }).to_string())
    }

    /// 登记 F10 的目录事实（相对路径 + 大小 + 修改时间），供 `MemoryDirectoryLister` 写清单。
    ///
    /// 与 `stage_dirs` 的分工：目录决定「预检能不能 stat 到根」，事实决定「清单内容」。
    /// 两者都来自浏览器，但一个是路径集合、一个是逐文件元数据，混成一个入口会让
    /// 「只想登记目录」的调用方被迫编造大小。
    pub fn stage_listing(&mut self, json: &str) -> Result<String, KernelError> {
        let lister = MemoryDirectoryLister::from_json(json)?;
        let roots = lister.root_count();
        self.listing = Some(lister);
        Ok(serde_json::json!({ "listingRoots": roots }).to_string())
    }

    /// 执行计划：`c10` 走归档执行器，`c09` 走专用执行器，其余走通用执行器
    /// （与 CLI 的 kind 路由同口径）。
    pub fn apply(&mut self, request: &str) -> Result<String, KernelError> {
        let request: ApplyRequest = serde_json::from_str(request).map_err(|error| {
            KernelError::invalid_params(format!("invalid apply request: {error}"))
        })?;
        let plan = request.plan;
        let options = request.options.unwrap_or_default();
        let now = request.now.unwrap_or_else(|| DEFAULT_NOW.to_string());
        let password = request.password.filter(|value| !value.is_empty());
        let license = AlwaysAllowLicense;
        let outcome = if plan.kind.as_str() == C10_KIND {
            // 归档端口的实现只看沙箱字节；`fs` 与 `archive` 借用同一个 `MemoryFs`
            // （都是共享借用），`journal` 是另一块字段的可变借用。
            let archive = MemoryArchiveService::new(&self.fs);
            let mut context = ArchiveApplyContext {
                fs: &self.fs,
                license: &license,
                journal: &mut self.journal,
                archive: &archive,
                task_id: format!("web-{}", plan.plan_id),
                now_rfc3339: now,
                protected_dirs: Vec::new(),
                case_sense: self.fs.case_sensitivity(),
                name_rules: self.fs.name_rules(),
                password: password.as_deref(),
                progress: None,
                cancel: None,
                network_retry: None,
            };
            apply_archive_plan(&plan, &options, &mut context)?
        } else if plan.kind.as_str() == C09_KIND {
            let mut context = ApplyContext {
                fs: &self.fs,
                license: &license,
                journal: &mut self.journal,
                task_id: format!("web-{}", plan.plan_id),
                now_rfc3339: now,
                protected_dirs: Vec::new(),
                case_sense: self.fs.case_sensitivity(),
                name_rules: self.fs.name_rules(),
                progress: None,
                cancel: None,
                network_retry: None,
            };
            apply_plan(&plan, &options, &mut context)?
        } else {
            // C01/C02/C03 的改写器全在 kernel 里（与 CLI／桌面同一实现），沙箱只负责
            // 把它们交给执行器。`from_plan` 读的是 `plan_c0X` 写入的参数快照，因此
            // 计划与执行不会各拿一个口径。
            let rewriter: Option<Box<dyn ContentRewriter>> = if plan.kind == C01_KIND {
                Some(Box::new(TranscodeRewriter::from_plan(&plan)?))
            } else if plan.kind == C02_KIND {
                Some(Box::new(LineEndingRewriter::from_plan(&plan)?))
            } else if plan.kind == C03_KIND {
                Some(Box::new(ReplaceRewriter::from_plan(&plan)?))
            } else {
                None
            };
            let mut context = GenericApplyContext {
                fs: &self.fs,
                license: &license,
                journal: &mut self.journal,
                task_id: format!("web-{}", plan.plan_id),
                now_rfc3339: now,
                protected_dirs: Vec::new(),
                case_sense: self.fs.case_sensitivity(),
                progress: None,
                cancel: None,
                network_retry: None,
                content_editor: None,
                content_rewriter: rewriter.as_deref(),
                // F10 的清单端口（`stage_listing` 登记过才在；其它视图为 None，
                // 落到 `perform_listing` 的 E5001）。
                directory_lister: self
                    .listing
                    .as_ref()
                    .map(|lister| lister as &dyn kernel::listing::DirectoryLister),
            };
            apply_generic_plan(&plan, &options, &mut context)?
        };
        self.products = product_paths(&self.fs, &self.journal, &plan.plan_id);
        let products: Vec<serde_json::Value> = self
            .products
            .iter()
            .map(|path| {
                let size = self.fs.stat(path).map(|stat| stat.size).unwrap_or(0);
                serde_json::json!({ "path": path, "size": size })
            })
            .collect();
        Ok(serde_json::json!({
            "outcome": outcome,
            "products": products,
        })
        .to_string())
    }

    /// 撤销最近一次执行涉及的计划：只回滚沙箱内已落盘条目。
    pub fn undo(&mut self, plan_id: &str) -> Result<UndoOutcome, KernelError> {
        let license = AlwaysAllowLicense;
        let outcome = undo_plan_with_ports(
            plan_id,
            &self.fs,
            &license,
            &mut self.journal,
            UndoPorts::default(),
        )?;
        self.products.clear();
        Ok(outcome)
    }

    /// 产物路径清单（`apply` 之后）。
    pub fn products(&self) -> &[String] {
        &self.products
    }

    /// 读产物字节（产物不存在报 E2001）。
    pub fn product(&self, path: &str) -> Result<Vec<u8>, KernelError> {
        self.fs.read_file(path)
    }

    /// 诊断用：沙箱内全部文件路径。
    pub fn files(&self) -> Vec<String> {
        self.fs.file_paths()
    }
}

/// 从 Journal 的 forward 操作里取「产物」：落盘路径即产物。
///
/// 只认 `write_atomic` / `rename(to)` / `copy(to)`——删除与建目录不产生可下载
/// 文件。按 seq 顺序去重：同一条目重试后可能留下多行，下载只该给最终路径。
fn product_paths(fs: &MemoryFs, journal: &MemoryJournal, plan_id: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let Ok(entries) = journal.entries_for_plan(plan_id) else {
        return paths;
    };
    for entry in entries {
        let path = match &entry.forward {
            FsOp::WriteAtomic { path, .. } => path.clone(),
            FsOp::Rename { to, .. } | FsOp::Copy { to, .. } => to.clone(),
            FsOp::Quarantine { .. } | FsOp::CreateDir { .. } | FsOp::RemoveDir { .. } => continue,
        };
        // 产物路径统一成**沙箱键**（绝对形态）：不同执行臂记录的是各自看到的路径——
        // 改名族记 `stat.final_path`（已绝对），F10 清单记计划里的原始 target
        // （可能是 `out/x.csv` 相对形态）。界面按它显示与下载，两种形态混着出
        // 会让同一份产物清单口径不一。
        let path = kernel::memfs::normalize_key(&path);
        if paths.contains(&path) {
            continue;
        }
        // 撤销过的条目（status=Reverted）不再算产物；但 apply 刚结束时都是 Applied。
        if fs.stat(&path).map(|stat| stat.kind).ok() != Some(kernel::fs::EntryKind::File) {
            continue;
        }
        paths.push(path);
    }
    paths
}
