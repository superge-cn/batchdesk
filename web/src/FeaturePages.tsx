import { useEffect, useState } from "react";
import { Loader2, Undo2 } from "lucide-react";
import {
  applyPlan,
  getTask,
  isTauri,
  listenTaskDone,
  listenTaskProgress,
  listenTaskState,
  planOperation,
  renamePreview,
  toIpcError,
  undoDryRun,
  undoPlan
} from "./lib/ipc";
import type {
  IpcError,
  PlanDocument,
  PreviewStatus,
  ProgressPhase,
  RenamePreview,
  TaskState,
  UndoDryRun
} from "./lib/ipc/types";

type View =
  | "f01"
  | "f02"
  | "f03"
  | "f04"
  | "c01"
  | "c02"
  | "c03"
  | "f05"
  | "f06"
  | "f07"
  | "f08"
  | "f09"
  | "f10"
  | "f11"
  | "f12"
  | "c04"
  | "c11"
  | "c12"
  | "c13"
  | "c14"
  | "c07"
  | "c08"
  | "c10";
type Busy = "plan" | "apply" | "undo" | null;

interface TaskView {
  taskId: string;
  state: TaskState;
  phase: ProgressPhase;
  done: number;
  total: number;
  message?: string;
}

function planOperationFor(
  view: View,
  form: Record<string, string>
): Promise<PlanDocument> {
  if (view === "f05") {
    return planOperation({
      kind: "f05",
      params: {
        file: form.file,
        outputDir: form.outputDir,
        ...(form.targetBytes ? { targetBytes: Number(form.targetBytes) } : {}),
        ...(form.rows ? { rows: Number(form.rows) } : {}),
        ...(form.delimiter ? { delimiter: form.delimiter } : {})
      },
      sourcePaths: []
    } as never);
  }
  if (view === "f06" || view === "f07" || view === "f08" || view === "f09") {
    const csvParams: Record<string, unknown> = {
      files: form.files ? form.files.split("\n").filter(Boolean) : [],
      target: form.target,
      sampleWindow: Number(form.sampleWindow || 4096),
      sampleBlocks: Number(form.sampleBlocks || 0)
    };
    if (form.delimiter) csvParams.delimiter = form.delimiter;
    if (form.source) csvParams.source = form.source;
    if (isNoHeader(form.header)) csvParams.hasHeader = false;
    if (view === "f06") {
      csvParams.output = form.output;
      if (form.sourceColumnName) {
        csvParams.sourceColumn = true;
        csvParams.sourceColumnName = form.sourceColumnName;
      }
      if (form.strict) csvParams.strict = true;
    } else {
      if (form.output) csvParams.output = form.output;
      if (form.outputDir) csvParams.outputDir = form.outputDir;
    }
    if (view === "f07") {
      csvParams.columns = form.columns
        ? form.columns.split(",").map((value) => value.trim()).filter(Boolean)
        : [];
      csvParams.columnIndexes = form.columnIndexes
        ? form.columnIndexes
            .split(",")
            .map((value) => Number(value.trim()))
            .filter((value) => Number.isFinite(value) && value > 0)
        : [];
    }
    if (view === "f08") {
      csvParams.window = form.window === "last" ? "last" : "first";
      csvParams.count = Number(form.count || 0);
    }
    if (view === "f09") {
      csvParams.startRow = Number(form.startRow || 0);
      csvParams.endRow = Number(form.endRow || 0);
    }
    return planOperation({ kind: view, params: csvParams, sourcePaths: [] } as never);
  }
  if (view === "f10") {
    return planOperation({
      kind: "f10",
      params: {
        roots: form.roots ? form.roots.split("\n").filter(Boolean) : [],
        ...(form.output ? { output: form.output } : {}),
        ...(form.outputDir ? { outputDir: form.outputDir } : {})
      },
      sourcePaths: []
    } as never);
  }
  if (view === "c07") {
    return planOperation({
      kind: "c07",
      params: {
        manifest: form.manifest,
        sourceRoot: form.sourceRoot,
        targetRoot: form.targetRoot,
        operation: form.operation === "copy" ? "copy" : "move",
        missing: form.missing === "error" ? "error" : "skip",
        ...(form.delimiter ? { delimiter: form.delimiter } : {})
      },
      sourcePaths: []
    } as never);
  }
  if (view === "c08") {
    return planOperation({
      kind: "c08",
      params: {
        sourceRoot: form.sourceRoot,
        targetRoot: form.targetRoot,
        rules: form.rules ? form.rules.split("\n").filter(Boolean) : [],
        recursive: form.recursive === "yes",
        ...(form.unclassifiedDir ? { unclassifiedDir: form.unclassifiedDir } : {})
      },
      sourcePaths: []
    } as never);
  }
  if (view === "c10") {
    const compress = form.mode === "compress";
    return planOperation({
      kind: "c10",
      params: {
        mode: compress ? "compress" : "extract",
        ...(compress
          ? {
              output: form.output,
              sources: form.sources ? form.sources.split("\n").filter(Boolean) : [],
              ...(form.compression ? { compression: form.compression } : {})
            }
          : {
              archive: form.archive,
              outputDir: form.outputDir,
              maxTotalBytes: Number(form.maxTotalBytes || 0),
              maxRatio: Number(form.maxRatio || 0)
            })
      },
      sourcePaths: []
    } as never);
  }
  if (view === "c04") {
    return planOperation({
      kind: "c04",
      params: {
        dir: form.dir,
        find: form.find,
        replace: form.replace ?? "",
        mode: form.mode || "plain",
        scope: form.scope || "full"
      },
      sourcePaths: []
    } as never);
  }
  if (view === "c11" || view === "c12" || view === "c13" || view === "c14") {
    const opParams: Record<string, unknown> = {
      files: form.files ? form.files.split("\n").filter(Boolean) : [],
      target: form.target,
      sampleWindow: Number(form.sampleWindow || 4096),
      sampleBlocks: Number(form.sampleBlocks || 0)
    };
    if (form.output) opParams.output = form.output;
    if (form.outputDir) opParams.outputDir = form.outputDir;
    if (form.delimiter) opParams.delimiter = form.delimiter;
    if (form.source) opParams.source = form.source;
    if (isNoHeader(form.header)) opParams.hasHeader = false;
    if (view === "c11") {
      opParams.keyColumns = splitList(form.keyColumns);
      opParams.keyColumnIndexes = splitNumbers(form.keyColumnIndexes);
    }
    if (view === "c12") {
      if (form.column) opParams.column = form.column;
      if (form.columnIndex) opParams.columnIndex = Number(form.columnIndex);
      opParams.op = form.op;
      opParams.value = form.value ?? "";
      opParams.filterMode = form.filterMode === "drop" ? "drop" : "keep";
    }
    if (view === "c13") {
      opParams.sortColumns = splitList(form.sortColumns);
      opParams.sortColumnIndexes = splitNumbers(form.sortColumnIndexes);
      opParams.order = form.order === "desc" ? "desc" : "asc";
      if (form.externalChunkRows)
        opParams.externalChunkRows = Number(form.externalChunkRows);
    }
    if (view === "c14") {
      if (form.splitColumn) opParams.splitColumn = form.splitColumn;
      if (form.splitColumnIndex)
        opParams.splitColumnIndex = Number(form.splitColumnIndex);
      if (form.splitSeparator) opParams.splitSeparator = form.splitSeparator;
      if (form.splitRegex) opParams.splitRegex = form.splitRegex;
      opParams.mergeColumns = splitList(form.mergeColumns);
      opParams.mergeColumnIndexes = splitNumbers(form.mergeColumnIndexes);
      if (form.mergeJoinWith !== undefined) opParams.mergeJoinWith = form.mergeJoinWith;
      if (form.mergeName) opParams.mergeName = form.mergeName;
    }
    return planOperation({ kind: view, params: opParams, sourcePaths: [] } as never);
  }
  if (view === "f11") {
    return planOperation({
      kind: "f11",
      params: { list: form.list, root: form.root },
      sourcePaths: []
    } as never);
  }
  if (view === "f01" || view === "f02" || view === "f03") {
    return planOperation({
      kind: view,
      params: {
        file: form.file,
        outputDir: form.outputDir,
        ...(view === "f01" && form.targetBytes
          ? { targetBytes: Number(form.targetBytes) }
          : {}),
        ...(view === "f02" && form.linesPerPart
          ? { linesPerPart: Number(form.linesPerPart) }
          : {}),
        ...(view === "f03" && form.parts ? { parts: Number(form.parts) } : {})
      },
      sourcePaths: []
    } as never);
  }
  if (view === "f04") {
    return planOperation({
      kind: "f04",
      params: {
        files: form.files ? form.files.split("\n").filter(Boolean) : [],
        output: form.output,
        target: form.target,
        ...(form.source ? { source: form.source } : {})
      },
      sourcePaths: []
    } as never);
  }
  if (view === "c01") {
    return planOperation({
      kind: "c01",
      params: {
        files: form.files ? form.files.split("\n").filter(Boolean) : [],
        target: form.target,
        sampleWindow: Number(form.sampleWindow || 4096),
        sampleBlocks: Number(form.sampleBlocks || 0)
      },
      sourcePaths: []
    } as never);
  }
  if (view === "c02") {
    return planOperation({
      kind: "c02",
      params: {
        files: form.files ? form.files.split("\n").filter(Boolean) : [],
        target: form.target,
        sampleBytes: Number(form.sampleBytes || 4096)
      },
      sourcePaths: []
    } as never);
  }
  if (view === "c03") {
    return planOperation({
      kind: "c03",
      params: {
        files: form.files ? form.files.split("\n").filter(Boolean) : [],
        mode: form.mode || "plain",
        find: form.find,
        replacement: form.replacement,
        sampleBytes: Number(form.sampleBytes || 4096)
      },
      sourcePaths: []
    } as never);
  }
  return planOperation({
    kind: "f12",
    params: {
      dir: form.dir,
      ...(form.prefix ? { prefix: form.prefix } : {}),
      ...(form.suffix ? { suffix: form.suffix } : {}),
      ...(form.find ? { find: form.find } : {}),
      replace: form.replace,
      scope: form.scope
    },
    sourcePaths: []
  } as never);
}

function splitList(value: string | undefined): string[] {
  return (value ?? "")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

function splitNumbers(value: string | undefined): number[] {
  return splitList(value)
    .map((item) => Number(item))
    .filter((item) => Number.isFinite(item) && item > 0);
}

function isNoHeader(value: string | undefined) {
  const normalized = (value ?? "").trim().toLowerCase();
  return normalized === "无" || normalized === "no" || normalized === "false";
}

function statusBadge(status: string) {
  if (status === "normal") return <span className="badge badge-safe">正常</span>;
  if (status === "conflict") return <span className="badge badge-danger">冲突</span>;
  if (status === "invalid") return <span className="badge badge-danger">非法</span>;
  if (status === "too-long") return <span className="badge badge-warn">超长</span>;
  if (status === "protected") return <span className="badge badge-danger">保护</span>;
  return <span className="badge">{status}</span>;
}

const FEATURE_TITLES: Record<View, string> = {
  f01: "TXT 按字节分割（F01）",
  f02: "TXT 按行数分割（F02）",
  f03: "TXT 按份数分割（F03）",
  f04: "TXT 合并（F04）",
  c01: "编码转换（C01）",
  c02: "换行符转换（C02）",
  c03: "查找替换（C03）",
  f05: "CSV 分割（F05）",
  f06: "CSV 合并（F06）",
  f07: "CSV 抽取指定列（F07）",
  f08: "CSV 抽取前/后 N 行（F08）",
  f09: "CSV 抽取位置行区间（F09）",
  f10: "生成目录清单（F10）",
  c04: "名称查找替换（C04）",
  c11: "CSV 去重（C11）",
  c12: "CSV 过滤（C12）",
  c13: "CSV 排序（C13）",
  c14: "CSV 列拆分/合并（C14）",
  c07: "清单驱动移动/复制（C07）",
  c08: "按规则归档（C08）",
  c10: "ZIP 解压/压缩（C10）",
  f11: "清单建档（F11）",
  f12: "批量改名（F12）"
};

interface FieldSpec {
  name: string;
  label: string;
  placeholder?: string;
  wide?: boolean;
  kind?: "textarea";
  hint?: string;
}

const FILE_LIST: FieldSpec = {
  name: "files",
  label: "文件列表（每行一个路径）",
  kind: "textarea",
  placeholder: "/path/a.csv\n/path/b.csv\n"
};

const FEATURE_FIELDS: Partial<Record<View, FieldSpec[]>> = {
  f01: [
    { name: "file", label: "源 TXT 文件", placeholder: "/path/a.txt", wide: true },
    { name: "targetBytes", label: "目标分片字节", placeholder: "8" },
    { name: "outputDir", label: "输出目录", placeholder: "/path/out", wide: true }
  ],
  f02: [
    { name: "file", label: "源 TXT 文件", placeholder: "/path/a.txt", wide: true },
    { name: "linesPerPart", label: "每片行数", placeholder: "100" },
    { name: "outputDir", label: "输出目录", placeholder: "/path/out", wide: true }
  ],
  f03: [
    { name: "file", label: "源 TXT 文件", placeholder: "/path/a.txt", wide: true },
    { name: "parts", label: "目标份数", placeholder: "4" },
    { name: "outputDir", label: "输出目录", placeholder: "/path/out", wide: true }
  ],
  f04: [
    FILE_LIST,
    { name: "output", label: "合并产物路径", placeholder: "/path/merged.txt", wide: true },
    { name: "target", label: "目标编码", placeholder: "utf-8" }
  ],
  c01: [
    FILE_LIST,
    { name: "target", label: "目标编码", placeholder: "utf-8", wide: true },
    { name: "sampleWindow", label: "采样窗口字节", placeholder: "4096" }
  ],
  c02: [
    FILE_LIST,
    { name: "target", label: "目标换行符 (crlf|lf|cr)", placeholder: "lf", wide: true },
    { name: "sampleBytes", label: "采样字节", placeholder: "4096" }
  ],
  c03: [
    FILE_LIST,
    { name: "mode", label: "模式 (plain|regex)", placeholder: "plain" },
    { name: "find", label: "查找文本", placeholder: "查找…", wide: true },
    { name: "replacement", label: "替换为", placeholder: "替换…", wide: true }
  ],
  f05: [
    { name: "file", label: "源 CSV 文件", placeholder: "/path/名单.csv", wide: true },
    { name: "targetBytes", label: "分片目标字节（二选一）", placeholder: "留空则用行数" },
    { name: "rows", label: "分片行数（二选一）", placeholder: "留空则用字节" },
    { name: "outputDir", label: "输出目录", placeholder: "/path/out", wide: true },
    { name: "delimiter", label: "分隔符（可留空嗅探）", placeholder: "," }
  ],
  f06: [
    FILE_LIST,
    { name: "output", label: "合并产物路径", placeholder: "/path/merged.csv", wide: true },
    { name: "target", label: "目标编码", placeholder: "utf-8" },
    { name: "delimiter", label: "分隔符（可留空嗅探）", placeholder: "," },
    { name: "header", label: "表头 有|无", placeholder: "有（默认）" },
    { name: "sourceColumnName", label: "来源列名（留空则不加来源列）", placeholder: "来源文件", wide: true },
    { name: "strict", label: "严格列对齐 strict", placeholder: "填 yes 启用" },
    { name: "sampleWindow", label: "采样窗口字节", placeholder: "4096" }
  ],
  f07: [
    FILE_LIST,
    { name: "columns", label: "列名（逗号分隔，按选择顺序）", placeholder: "姓名,部门", wide: true },
    { name: "columnIndexes", label: "列序号（逗号分隔，1-based）", placeholder: "1,3" },
    { name: "outputDir", label: "输出目录", placeholder: "/path/out", wide: true },
    { name: "target", label: "目标编码", placeholder: "utf-8" },
    { name: "delimiter", label: "分隔符（可留空嗅探）", placeholder: "," },
    { name: "header", label: "表头 有|无", placeholder: "有（默认）" },
    { name: "sampleWindow", label: "采样窗口字节", placeholder: "4096" }
  ],
  f08: [
    FILE_LIST,
    { name: "window", label: "窗口 first|last", placeholder: "first" },
    { name: "count", label: "行数 N（0 合法）", placeholder: "10" },
    { name: "outputDir", label: "输出目录", placeholder: "/path/out", wide: true },
    { name: "target", label: "目标编码", placeholder: "utf-8" },
    { name: "delimiter", label: "分隔符（可留空嗅探）", placeholder: "," },
    { name: "header", label: "表头 有|无", placeholder: "有（默认）" },
    { name: "sampleWindow", label: "采样窗口字节", placeholder: "4096" }
  ],
  f09: [
    FILE_LIST,
    { name: "startRow", label: "起始数据行 X（1-based）", placeholder: "2" },
    { name: "endRow", label: "结束数据行 Y（≥X）", placeholder: "5" },
    { name: "outputDir", label: "输出目录", placeholder: "/path/out", wide: true },
    { name: "target", label: "目标编码", placeholder: "utf-8" },
    { name: "delimiter", label: "分隔符（可留空嗅探）", placeholder: "," },
    { name: "header", label: "表头 有|无", placeholder: "有（默认）" },
    { name: "sampleWindow", label: "采样窗口字节", placeholder: "4096" }
  ],
  f10: [
    {
      name: "roots",
      label: "根目录（每行一个）",
      kind: "textarea",
      placeholder: "/path/root-a\n/path/root-b\n",
      hint: "UTF-8 BOM CSV，列：相对路径/名称/类型/大小/修改时间/层级；不读文件内容。"
    },
    { name: "outputDir", label: "清单输出目录", placeholder: "/path/out", wide: true }
  ],
  c04: [
    { name: "dir", label: "目录", placeholder: "/path/dir", wide: true },
    { name: "mode", label: "模式 plain|regex", placeholder: "plain" },
    { name: "scope", label: "作用范围 full|stem|ext", placeholder: "full" },
    { name: "find", label: "查找文本", placeholder: "字面或正则", wide: true },
    { name: "replace", label: "替换为（支持 $1/\${name}）", placeholder: "替换文本", wide: true }
  ],
  c11: [
    FILE_LIST,
    { name: "keyColumns", label: "键列名（逗号分隔，留空＝整行）", placeholder: "订单号,客户", wide: true },
    { name: "keyColumnIndexes", label: "键列序号（逗号分隔，1-based）", placeholder: "1" },
    { name: "outputDir", label: "输出目录", placeholder: "/path/out", wide: true },
    { name: "target", label: "目标编码", placeholder: "utf-8" },
    { name: "delimiter", label: "分隔符（可留空嗅探）", placeholder: "," },
    { name: "header", label: "表头 有|无", placeholder: "有（默认）" },
    { name: "sampleWindow", label: "采样窗口字节", placeholder: "4096" }
  ],
  c12: [
    FILE_LIST,
    { name: "column", label: "条件列名", placeholder: "金额" },
    { name: "columnIndex", label: "条件列序号（与列名二选一）", placeholder: "3" },
    { name: "op", label: "算子 contains|not-contains|eq|ne|gt|ge|lt|le", placeholder: "gt", wide: true },
    { name: "value", label: "比较值", placeholder: "100" },
    { name: "filterMode", label: "选向 keep|drop", placeholder: "keep" },
    { name: "outputDir", label: "输出目录", placeholder: "/path/out", wide: true },
    { name: "target", label: "目标编码", placeholder: "utf-8" },
    { name: "sampleWindow", label: "采样窗口字节", placeholder: "4096" }
  ],
  c13: [
    FILE_LIST,
    { name: "sortColumns", label: "排序键列名（逗号分隔）", placeholder: "金额", wide: true },
    { name: "sortColumnIndexes", label: "排序键列序号（逗号分隔）", placeholder: "" },
    { name: "order", label: "顺序 asc|desc", placeholder: "asc" },
    { name: "externalChunkRows", label: "外排块行数（留空＝内存排序）", placeholder: "5000" },
    { name: "outputDir", label: "输出目录", placeholder: "/path/out", wide: true },
    { name: "target", label: "目标编码", placeholder: "utf-8" },
    { name: "sampleWindow", label: "采样窗口字节", placeholder: "4096" }
  ],
  c14: [
    FILE_LIST,
    { name: "splitColumn", label: "拆分源列名", placeholder: "标签" },
    { name: "splitColumnIndex", label: "拆分源列序号（与列名二选一）", placeholder: "" },
    { name: "splitSeparator", label: "拆分分隔符", placeholder: ";" },
    { name: "splitRegex", label: "拆分正则（与分隔符二选一）", placeholder: "[;,]" },
    { name: "mergeColumns", label: "合并源列名（逗号分隔，与拆分互斥）", placeholder: "姓,名", wide: true },
    { name: "mergeJoinWith", label: "合并连接符", placeholder: "-" },
    { name: "mergeName", label: "合并新列名", placeholder: "姓名" },
    { name: "outputDir", label: "输出目录", placeholder: "/path/out", wide: true },
    { name: "target", label: "目标编码", placeholder: "utf-8" },
    { name: "sampleWindow", label: "采样窗口字节", placeholder: "4096" }
  ],
  c07: [
    { name: "manifest", label: "清单 CSV（两列：名称,目标子目录）", placeholder: "/path/manifest.csv", wide: true },
    { name: "sourceRoot", label: "源根目录", placeholder: "/path/src", wide: true },
    { name: "targetRoot", label: "目标根目录", placeholder: "/path/dst", wide: true },
    { name: "operation", label: "操作 move|copy", placeholder: "move" },
    { name: "missing", label: "缺失项 skip|error", placeholder: "skip" },
    { name: "delimiter", label: "清单分隔符（可留空嗅探）", placeholder: "," }
  ],
  c08: [
    { name: "sourceRoot", label: "源目录", placeholder: "/path/src", wide: true },
    { name: "targetRoot", label: "归档目标目录", placeholder: "/path/dst", wide: true },
    {
      name: "rules",
      label: "规则链（每行 <子目录>|<type>:<params>，首中生效）",
      kind: "textarea",
      placeholder: "images|ext:jpg,png\n2024|mtime:2024-01-01..2024-12-31\nbig|size:1048576..\nlogs|name:log\n",
      hint: "四类条件：ext / mtime(from..to) / size(字节 min..max) / name 关键字；未命中默认留在原位。"
    },
    { name: "recursive", label: "递归 yes", placeholder: "yes" },
    { name: "unclassifiedDir", label: "未分类去向（留空＝留在原位）", placeholder: "unsorted", wide: true }
  ],
  c10: [
    { name: "mode", label: "模式 extract|compress", placeholder: "extract" },
    { name: "archive", label: "压缩包路径（解压）", placeholder: "/path/a.zip", wide: true },
    { name: "outputDir", label: "解压输出目录（须已存在）", placeholder: "/path/out", wide: true },
    { name: "maxTotalBytes", label: "解压总量上限字节（必填）", placeholder: "104857600" },
    { name: "maxRatio", label: "解压比例上限（必填，如 100）", placeholder: "100" },
    { name: "output", label: "压缩产物路径（压缩模式）", placeholder: "/path/bundle.zip", wide: true },
    {
      name: "sources",
      label: "压缩源（每行一个路径，压缩模式）",
      kind: "textarea",
      placeholder: "/path/src\n/path/other.txt\n"
    },
    { name: "compression", label: "压缩算法（压缩模式）", placeholder: "deflate" }
  ],
  f11: [
    { name: "root", label: "目标根目录", placeholder: "/path/root", wide: true },
    {
      name: "list",
      label: "清单（每行一个相对路径）",
      kind: "textarea",
      placeholder: "folder-a\nfolder-b/sub\n",
      hint: "路径相对目标根目录；禁止 .. 逃逸（E1003）。"
    }
  ],
  f12: [
    { name: "dir", label: "改名目录", placeholder: "/path/dir", wide: true },
    { name: "prefix", label: "前缀", placeholder: "追加到词根前" },
    { name: "suffix", label: "后缀", placeholder: "追加到词根后" },
    { name: "find", label: "查找文本", placeholder: "字面查找" },
    { name: "replace", label: "替换为", placeholder: "替换文本" },
    { name: "scope", label: "作用范围 full|stem|ext", placeholder: "full" }
  ]
};

export function FeaturePages({ view, onBack }: { view: View; onBack: () => void }) {
  const title = FEATURE_TITLES[view] ?? view;
  const [form, setForm] = useState<Record<string, string>>({});
  const [plan, setPlan] = useState<PlanDocument | null>(null);
  const [preview, setPreview] = useState<RenamePreview | null>(null);
  const [dryRun, setDryRun] = useState<UndoDryRun | null>(null);
  const [task, setTask] = useState<TaskView | null>(null);
  const [error, setError] = useState<IpcError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  const [canUndo, setCanUndo] = useState(false);

  const setField = (name: string, value: string) =>
    setForm((previous) => ({ ...previous, [name]: value }));

  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    const track = (subscription: Promise<() => void>) => {
      subscription
        .then((unlisten) => unlisteners.push(unlisten))
        .catch(() => undefined);
    };
    const patch = (taskId: string, update: Partial<TaskView>) =>
      setTask((current) =>
        current && current.taskId === taskId ? { ...current, ...update } : current
      );
    track(
      listenTaskProgress((event) => {
        const entryProgress =
          event.phase === "queued" || event.phase === "executing"
            ? { done: event.done, total: event.total }
            : {};
        patch(event.taskId, { phase: event.phase, ...entryProgress, message: event.message });
      })
    );
    track(listenTaskState((event) => patch(event.taskId, { state: event.state })));
    track(
      listenTaskDone((event) => {
        getTask(event.taskId)
          .then((detail) =>
            patch(event.taskId, {
              state: detail.summary.state,
              phase: detail.summary.phase,
              done: detail.summary.done,
              total: detail.summary.total,
              message: detail.summary.message
            })
          )
          .catch(() => undefined);
      })
    );
    return () => unlisteners.forEach((unlisten) => unlisten());
  }, []);

  const onPlan = async () => {
    setBusy("plan");
    setError(null);
    setNotice(null);
    setPlan(null);
    setPreview(null);
    setDryRun(null);
    setCanUndo(false);
    try {
      const next = await planOperationFor(view, form);
      setPlan(next);
      if (view === "f12") {
        try {
          setPreview(await renamePreview(next.planId));
        } catch (value) {
          setPreview(null);
        }
      }
      setNotice(`计划已生成：${next.entries.length} 条。核对预览后执行。`);
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setBusy(null);
    }
  };

  const onApply = async () => {
    if (!plan) return;
    setBusy("apply");
    setError(null);
    setNotice(null);
    try {
      const handle = await applyPlan(plan.planId, {
        onConflict: "skip",
        overwriteConfirmed: false
      });
      setTask({
        taskId: handle.taskId,
        state: "queued",
        phase: "queued",
        done: 0,
        total: plan.entries.length
      });
      setNotice("任务已加入队列；进度通过 task:// 事件推送。");
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setBusy(null);
    }
  };

  const onUndo = async () => {
    if (!plan) return;
    setBusy("undo");
    setError(null);
    setNotice(null);
    try {
      const handle = await undoPlan(plan.planId);
      setTask({
        taskId: handle.taskId,
        state: "queued",
        phase: "queued",
        done: 0,
        total: 0
      });
      setNotice("撤销任务已加入队列；回滚进度通过 task:// 事件推送。");
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setBusy(null);
    }
  };

  const onUndoDryRun = async () => {
    if (!plan) return;
    setError(null);
    try {
      setDryRun(await undoDryRun(plan.planId));
    } catch (value) {
      setError(toIpcError(value));
    }
  };

  const active = task !== null && !["completed", "partial-failed", "failed", "cancelled"].includes(task.state);
  const terminalOk = task !== null && ["completed", "partial-failed"].includes(task.state);

  const field = (
    label: string,
    name: string,
    placeholder: string,
    wide = false
  ) => (
    <div className={`field${wide ? " wide" : ""}`}>
      <label htmlFor={`${view}-${name}`}>{label}</label>
      <input
        id={`${view}-${name}`}
        className="input"
        value={form[name] ?? ""}
        onChange={(event) => setField(name, event.target.value)}
        placeholder={placeholder}
        spellCheck={false}
      />
    </div>
  );

  return (
    <div className="content">
      {error && (
        <div className="banner banner-danger" role="alert">
          <div>
            <strong>{error.code}</strong> · {error.message}
          </div>
          <button className="button button-ghost" type="button" onClick={() => setError(null)}>
            关闭
          </button>
        </div>
      )}
      {notice && (
        <div className="banner banner-info" role="status">
          <div>{notice}</div>
        </div>
      )}

      <div className="content-grid">
        <div className="stack">
          <section className="card" aria-labelledby="feature-params">
            <div className="card-header">
              <div>
                <h2 className="card-title" id="feature-params">{title}</h2>
                <p className="card-sub">先预览、再执行、可撤销（C18 干跑：plan 命令即零落盘预览）。</p>
              </div>
              <button className="button button-ghost" type="button" onClick={onBack}>
                返回 C09
              </button>
            </div>

            <div className="field-grid">
              {(FEATURE_FIELDS[view] ?? []).map((spec) =>
                spec.kind === "textarea" ? (
                  <div className="field wide" key={spec.name}>
                    <label htmlFor={`${view}-${spec.name}`}>{spec.label}</label>
                    <textarea
                      id={`${view}-${spec.name}`}
                      className="textarea"
                      value={form[spec.name] ?? ""}
                      onChange={(event) => setField(spec.name, event.target.value)}
                      placeholder={spec.placeholder}
                      rows={6}
                      spellCheck={false}
                    />
                    {spec.hint && <div className="hint">{spec.hint}</div>}
                  </div>
                ) : (
                  field(spec.label, spec.name, spec.placeholder ?? "", spec.wide)
                )
              )}
            </div>

            <div className="toolbar">
              <span className="hint">plan 命令即干跑预览（C18）：零落盘、重复预览幂等。</span>
              <button
                className="button button-primary"
                type="button"
                onClick={onPlan}
                disabled={busy !== null || active}
              >
                {busy === "plan" ? <Loader2 size={16} className="spin" /> : null}
                {busy === "plan" ? "正在生成计划…" : "生成计划预览"}
              </button>
            </div>
          </section>

          {plan && (
            <section className="card" aria-labelledby="feature-plan">
              <div className="card-header">
                <div>
                  <h2 className="card-title" id="feature-plan">计划预览</h2>
                  <p className="card-sub">
                    {plan.entries.length} 条 ·受阻 {plan.stats.blocked} ·警告 {plan.stats.warning}
                  </p>
                </div>
                <span className="plan-id" title={plan.planId}>{plan.planId}</span>
              </div>
              <div className="table">
                <div className="table-scroll">
                  <div className="row row-head">
                    <span>源</span>
                    <span>目标 / 说明</span>
                  </div>
                  {plan.entries.map((entry) => (
                    <div className="row" key={entry.entryId}>
                      <span className="path" title={entry.source}>{entry.source}</span>
                      <span className="path" title={entry.target ?? entry.reason}>
                        {entry.target ?? entry.reason}
                        {entry.blocked ? ` · ${entry.blocked.code}` : ""}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            </section>
          )}
        </div>

        <div className="stack">
          <section className="card" aria-labelledby="feature-execute">
            <div className="card-header">
              <div>
                <h2 className="card-title" id="feature-execute">执行与撤销</h2>
                <p className="card-sub">C06：撤销前可回滚预演；回滚为终态。</p>
              </div>
            </div>
            <div className="button-row">
              <button
                className="button button-primary"
                type="button"
                onClick={onApply}
                disabled={!plan || busy !== null || active || plan.entries.length === 0}
              >
                {busy === "apply" ? <Loader2 size={16} className="spin" /> : null}
                {active ? "执行中…" : "执行"}
              </button>
              <button
                className="button button-secondary"
                type="button"
                onClick={onUndoDryRun}
                disabled={!plan}
              >
                回滚预演
              </button>
              <button
                className="button button-secondary"
                type="button"
                onClick={onUndo}
                disabled={!plan || busy !== null || active}
              >
                {busy === "undo" ? <Loader2 size={16} className="spin" /> : <Undo2 size={16} />}
                撤销
              </button>
            </div>
            {task && (
              <div className="task-panel">
                <div className="task-head">
                  <span className="task-id" title={task.taskId}>{task.taskId}</span>
                  <span className="badge">{task.state}</span>
                </div>
                <div className="progress" role="progressbar" aria-label="进度">
                  <div
                    className="progress-fill"
                    style={{
                      width: `${task.total > 0 ? Math.min(100, Math.round((task.done / task.total) * 100)) : 0}%`
                    }}
                  />
                </div>
                <div className="hint">
                  {task.phase} · {task.done}/{task.total}
                  {task.message ? ` · ${task.message}` : ""}
                </div>
              </div>
            )}
            {dryRun && (
              <div className="banner banner-info" role="status">
                <div>
                  <strong>回滚预演</strong> · 冲突 {dryRun.conflictCount} · 缺失 {dryRun.missingCount}
                  {dryRun.entries.map((entry) => (
                    <div className="muted" key={entry.entryId}>
                      {entry.entryId} · {entry.status}
                      {entry.detail ? ` · ${entry.detail}` : ""}
                    </div>
                  ))}
                </div>
              </div>
            )}
            {terminalOk && (
              <div className="banner banner-info" role="status">
                <div>执行完成；如需恢复可点击「撤销」（回滚为终态，不可二次回滚）。</div>
              </div>
            )}
          </section>

          {view === "f12" && preview && (
            <section className="card" aria-labelledby="c05-preview">
              <div className="card-header">
                <div>
                  <h2 className="card-title" id="c05-preview">C05 预览与冲突检测</h2>
                  <p className="card-sub">
                    将处理 {preview.counts.toProcess} · 跳过 {preview.counts.skipped} · 冲突{" "}
                    {preview.counts.conflicts} · 异常 {preview.counts.blocked}
                  </p>
                </div>
              </div>
              <div className="table">
                <div className="table-scroll">
                  <div className="row row-head">
                    <span>状态</span>
                    <span>旧名 → 新名</span>
                  </div>
                  {preview.rows.map((row) => (
                    <div className="row" key={row.entryId}>
                      <span>{statusBadge(row.status as PreviewStatus)}{row.twoPhase ? " (两阶段)" : ""}</span>
                      <span className="path">
                        {row.oldPath} → {row.newPath}
                        {row.detail ? ` · ${row.detail}` : ""}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
              {preview.highlights.length > 0 && (
                <div className="warnings">
                  {preview.highlights.map((highlight, index) => (
                    <div className="warning caution" key={index}>
                      <span>{highlight}</span>
                    </div>
                  ))}
                </div>
              )}
            </section>
          )}

          <section className="card">
            <div className="card-header">
              <div>
                <h2 className="card-title">形态</h2>
                <p className="card-sub">
                  {isTauri() ? "桌面壳：本地文件系统" : "浏览器预览 · mock IPC（无真实计划）"}
                </p>
              </div>
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
