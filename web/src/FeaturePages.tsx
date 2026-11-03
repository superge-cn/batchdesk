import { useEffect, useState } from "react";
import { Info, Loader2, Undo2 } from "lucide-react";
import {
  applyPlan,
  getTask,
  isTauri,
  listenTaskDone,
  listenTaskProgress,
  listenTaskState,
  planOperation,
  renamePreview,
  cancelTask,
  retryTask,
  toIpcError,
  undoDryRun,
  undoPlan
} from "./lib/ipc";
import { DangerConfirm } from "./DangerConfirm";
import { ErrorBanner } from "./ErrorBanner";
import { VirtualList } from "./VirtualList";
import { useI18n, type MessageKey } from "./i18n";
import type { Translate } from "./labels";
import { phaseLabel } from "./labels";
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
  /** apply 任务才可重试失败项；undo 任务由壳层按 E1002 拒绝。 */
  kind: "apply" | "undo";
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
  // 这是"表头 有|无"输入框的取值判定，不是界面文案。
  return normalized === "无" || normalized === "no" || normalized === "false"; // i18n-exempt
}

function statusBadge(t: Translate, status: string) {
  if (status === "normal") return <span className="badge badge-safe">{t("feature.status.normal")}</span>;
  if (status === "conflict") return <span className="badge badge-danger">{t("feature.status.conflict")}</span>;
  if (status === "invalid") return <span className="badge badge-danger">{t("feature.status.invalid")}</span>;
  if (status === "too-long") return <span className="badge badge-warn">{t("feature.status.tooLong")}</span>;
  if (status === "protected") return <span className="badge badge-danger">{t("feature.status.protected")}</span>;
  return <span className="badge">{status}</span>;
}

const FEATURE_TITLES: Record<View, MessageKey> = {
  f01: "feature.title.f01",
  f02: "feature.title.f02",
  f03: "feature.title.f03",
  f04: "feature.title.f04",
  c01: "feature.title.c01",
  c02: "feature.title.c02",
  c03: "feature.title.c03",
  f05: "feature.title.f05",
  f06: "feature.title.f06",
  f07: "feature.title.f07",
  f08: "feature.title.f08",
  f09: "feature.title.f09",
  f10: "feature.title.f10",
  c04: "feature.title.c04",
  c11: "feature.title.c11",
  c12: "feature.title.c12",
  c13: "feature.title.c13",
  c14: "feature.title.c14",
  c07: "feature.title.c07",
  c08: "feature.title.c08",
  c10: "feature.title.c10",
  f11: "feature.title.f11",
  f12: "feature.title.f12"
};

interface FieldSpec {
  name: string;
  labelKey: MessageKey;
  /** 纯技术示例（路径/编码/枚举）原样展示；含中文的提示走 placeholderKey。 */
  placeholder?: string;
  placeholderKey?: MessageKey;
  wide?: boolean;
  kind?: "textarea";
  hintKey?: MessageKey;
}

const FILE_LIST: FieldSpec = {
  name: "files",
  labelKey: "feature.field.files",
  kind: "textarea",
  placeholder: "/path/a.csv\n/path/b.csv\n"
};

const FEATURE_FIELDS: Partial<Record<View, FieldSpec[]>> = {
  f01: [
    { name: "file", labelKey: "feature.field.file", placeholder: "/path/a.txt", wide: true },
    { name: "targetBytes", labelKey: "feature.field.targetBytes", placeholder: "8" },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true }
  ],
  f02: [
    { name: "file", labelKey: "feature.field.file", placeholder: "/path/a.txt", wide: true },
    { name: "linesPerPart", labelKey: "feature.field.linesPerPart", placeholder: "100" },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true }
  ],
  f03: [
    { name: "file", labelKey: "feature.field.file", placeholder: "/path/a.txt", wide: true },
    { name: "parts", labelKey: "feature.field.parts", placeholder: "4" },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true }
  ],
  f04: [
    FILE_LIST,
    { name: "output", labelKey: "feature.field.output", placeholder: "/path/merged.txt", wide: true },
    { name: "target", labelKey: "feature.field.target", placeholder: "utf-8" }
  ],
  c01: [
    FILE_LIST,
    { name: "target", labelKey: "feature.field.target", placeholder: "utf-8", wide: true },
    { name: "sampleWindow", labelKey: "feature.field.sampleWindow", placeholder: "4096" }
  ],
  c02: [
    FILE_LIST,
    { name: "target", labelKey: "feature.field.target", placeholder: "crlf|lf|cr", wide: true },
    { name: "sampleBytes", labelKey: "feature.field.sampleBytes", placeholder: "4096" }
  ],
  c03: [
    FILE_LIST,
    { name: "mode", labelKey: "feature.field.mode", placeholder: "plain" },
    { name: "find", labelKey: "feature.field.find", placeholderKey: "feature.placeholder.find", wide: true },
    {
      name: "replacement",
      labelKey: "feature.field.replacement",
      placeholderKey: "feature.placeholder.replacement",
      wide: true
    }
  ],
  f05: [
    { name: "file", labelKey: "feature.field.file", placeholder: "/path/list.csv", wide: true },
    {
      name: "targetBytes",
      labelKey: "feature.field.targetBytes",
      placeholderKey: "feature.placeholder.byRows"
    },
    { name: "rows", labelKey: "feature.field.rows", placeholderKey: "feature.placeholder.byBytes" },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true },
    { name: "delimiter", labelKey: "feature.field.delimiter", placeholder: "," }
  ],
  f06: [
    FILE_LIST,
    { name: "output", labelKey: "feature.field.output", placeholder: "/path/merged.csv", wide: true },
    { name: "target", labelKey: "feature.field.target", placeholder: "utf-8" },
    { name: "delimiter", labelKey: "feature.field.delimiter", placeholder: "," },
    {
      name: "header",
      labelKey: "feature.field.header",
      placeholderKey: "feature.placeholder.headerYes"
    },
    {
      name: "sourceColumnName",
      labelKey: "feature.field.sourceColumnName",
      placeholder: "source",
      wide: true
    },
    {
      name: "strict",
      labelKey: "feature.field.strict",
      placeholderKey: "feature.placeholder.strictYes"
    },
    { name: "sampleWindow", labelKey: "feature.field.sampleWindow", placeholder: "4096" }
  ],
  f07: [
    FILE_LIST,
    {
      name: "columns",
      labelKey: "feature.field.columns",
      placeholder: "name,dept",
      wide: true
    },
    { name: "columnIndexes", labelKey: "feature.field.columnIndexes", placeholder: "1,3" },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true },
    { name: "target", labelKey: "feature.field.target", placeholder: "utf-8" },
    { name: "delimiter", labelKey: "feature.field.delimiter", placeholder: "," },
    {
      name: "header",
      labelKey: "feature.field.header",
      placeholderKey: "feature.placeholder.headerYes"
    },
    { name: "sampleWindow", labelKey: "feature.field.sampleWindow", placeholder: "4096" }
  ],
  f08: [
    FILE_LIST,
    { name: "window", labelKey: "feature.field.window", placeholder: "first" },
    { name: "count", labelKey: "feature.field.count", placeholder: "10" },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true },
    { name: "target", labelKey: "feature.field.target", placeholder: "utf-8" },
    { name: "delimiter", labelKey: "feature.field.delimiter", placeholder: "," },
    {
      name: "header",
      labelKey: "feature.field.header",
      placeholderKey: "feature.placeholder.headerYes"
    },
    { name: "sampleWindow", labelKey: "feature.field.sampleWindow", placeholder: "4096" }
  ],
  f09: [
    FILE_LIST,
    { name: "startRow", labelKey: "feature.field.startRow", placeholder: "2" },
    { name: "endRow", labelKey: "feature.field.endRow", placeholder: "5" },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true },
    { name: "target", labelKey: "feature.field.target", placeholder: "utf-8" },
    { name: "delimiter", labelKey: "feature.field.delimiter", placeholder: "," },
    {
      name: "header",
      labelKey: "feature.field.header",
      placeholderKey: "feature.placeholder.headerYes"
    },
    { name: "sampleWindow", labelKey: "feature.field.sampleWindow", placeholder: "4096" }
  ],
  f10: [
    {
      name: "roots",
      labelKey: "feature.field.roots",
      kind: "textarea",
      placeholder: "/path/root-a\n/path/root-b\n",
      hintKey: "feature.field.f10.hint"
    },
    { name: "outputDir", labelKey: "feature.field.listingOutputDir", placeholder: "/path/out", wide: true }
  ],
  c04: [
    { name: "dir", labelKey: "feature.field.dir", placeholder: "/path/dir", wide: true },
    { name: "mode", labelKey: "feature.field.mode", placeholder: "plain" },
    { name: "scope", labelKey: "feature.field.scope", placeholder: "full" },
    {
      name: "find",
      labelKey: "feature.field.find",
      placeholderKey: "feature.placeholder.literalOrRegex",
      wide: true
    },
    {
      name: "replace",
      labelKey: "feature.field.replace",
      placeholderKey: "feature.placeholder.replaceText",
      wide: true
    }
  ],
  c11: [
    FILE_LIST,
    {
      name: "keyColumns",
      labelKey: "feature.field.keyColumns",
      placeholder: "id,user",
      wide: true
    },
    { name: "keyColumnIndexes", labelKey: "feature.field.keyColumnIndexes", placeholder: "1" },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true },
    { name: "target", labelKey: "feature.field.target", placeholder: "utf-8" },
    { name: "delimiter", labelKey: "feature.field.delimiter", placeholder: "," },
    {
      name: "header",
      labelKey: "feature.field.header",
      placeholderKey: "feature.placeholder.headerYes"
    },
    { name: "sampleWindow", labelKey: "feature.field.sampleWindow", placeholder: "4096" }
  ],
  c12: [
    FILE_LIST,
    { name: "column", labelKey: "feature.field.column", placeholder: "amount" },
    { name: "columnIndex", labelKey: "feature.field.columnIndex", placeholder: "3" },
    { name: "op", labelKey: "feature.field.op", placeholder: "gt", wide: true },
    { name: "value", labelKey: "feature.field.value", placeholder: "100" },
    { name: "filterMode", labelKey: "feature.field.filterMode", placeholder: "keep" },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true },
    { name: "target", labelKey: "feature.field.target", placeholder: "utf-8" },
    { name: "sampleWindow", labelKey: "feature.field.sampleWindow", placeholder: "4096" }
  ],
  c13: [
    FILE_LIST,
    { name: "sortColumns", labelKey: "feature.field.sortColumns", placeholder: "amount", wide: true },
    { name: "sortColumnIndexes", labelKey: "feature.field.sortColumnIndexes", placeholder: "" },
    { name: "order", labelKey: "feature.field.order", placeholder: "asc" },
    {
      name: "externalChunkRows",
      labelKey: "feature.field.externalChunkRows",
      placeholder: "5000"
    },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true },
    { name: "target", labelKey: "feature.field.target", placeholder: "utf-8" },
    { name: "sampleWindow", labelKey: "feature.field.sampleWindow", placeholder: "4096" }
  ],
  c14: [
    FILE_LIST,
    { name: "splitColumn", labelKey: "feature.field.splitColumn", placeholder: "tags" },
    { name: "splitColumnIndex", labelKey: "feature.field.splitColumnIndex", placeholder: "" },
    { name: "splitSeparator", labelKey: "feature.field.splitSeparator", placeholder: ";" },
    { name: "splitRegex", labelKey: "feature.field.splitRegex", placeholder: "[;,]" },
    {
      name: "mergeColumns",
      labelKey: "feature.field.mergeColumns",
      placeholder: "first,last",
      wide: true
    },
    { name: "mergeJoinWith", labelKey: "feature.field.mergeJoinWith", placeholder: "-" },
    { name: "mergeName", labelKey: "feature.field.mergeName", placeholder: "fullName" },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true },
    { name: "target", labelKey: "feature.field.target", placeholder: "utf-8" },
    { name: "sampleWindow", labelKey: "feature.field.sampleWindow", placeholder: "4096" }
  ],
  c07: [
    {
      name: "manifest",
      labelKey: "feature.field.manifest",
      placeholder: "/path/manifest.csv",
      wide: true
    },
    { name: "sourceRoot", labelKey: "feature.field.sourceRoot", placeholder: "/path/src", wide: true },
    { name: "targetRoot", labelKey: "feature.field.targetRoot", placeholder: "/path/dst", wide: true },
    { name: "operation", labelKey: "feature.field.operation", placeholder: "move" },
    { name: "missing", labelKey: "feature.field.missing", placeholder: "skip" },
    { name: "delimiter", labelKey: "feature.field.delimiter", placeholder: "," }
  ],
  c08: [
    { name: "sourceRoot", labelKey: "feature.field.sourceRoot", placeholder: "/path/src", wide: true },
    { name: "targetRoot", labelKey: "feature.field.targetRoot", placeholder: "/path/dst", wide: true },
    {
      name: "rules",
      labelKey: "feature.field.rules",
      kind: "textarea",
      placeholder:
        "images|ext:jpg,png\n2024|mtime:2024-01-01..2024-12-31\nbig|size:1048576..\nlogs|name:log\n",
      hintKey: "feature.field.c08.hint"
    },
    { name: "recursive", labelKey: "feature.field.recursive", placeholder: "yes" },
    {
      name: "unclassifiedDir",
      labelKey: "feature.field.unclassifiedDir",
      placeholder: "unsorted",
      wide: true
    }
  ],
  c10: [
    { name: "mode", labelKey: "feature.field.mode", placeholder: "extract" },
    { name: "archive", labelKey: "feature.field.archive", placeholder: "/path/a.zip", wide: true },
    {
      name: "outputDir",
      labelKey: "feature.field.extractOutputDir",
      placeholder: "/path/out",
      wide: true
    },
    { name: "maxTotalBytes", labelKey: "feature.field.maxTotalBytes", placeholder: "104857600" },
    { name: "maxRatio", labelKey: "feature.field.maxRatio", placeholder: "100" },
    {
      name: "output",
      labelKey: "feature.field.compressOutput",
      placeholder: "/path/bundle.zip",
      wide: true
    },
    {
      name: "sources",
      labelKey: "feature.field.sources",
      kind: "textarea",
      placeholder: "/path/src\n/path/other.txt\n"
    },
    { name: "compression", labelKey: "feature.field.compression", placeholder: "deflate" }
  ],
  f11: [
    { name: "root", labelKey: "feature.field.targetRoot", placeholder: "/path/root", wide: true },
    {
      name: "list",
      labelKey: "feature.field.list",
      kind: "textarea",
      placeholder: "folder-a\nfolder-b/sub\n",
      hintKey: "feature.field.f11.hint"
    }
  ],
  f12: [
    { name: "dir", labelKey: "feature.field.renameDir", placeholder: "/path/dir", wide: true },
    {
      name: "prefix",
      labelKey: "feature.field.prefix",
      placeholderKey: "feature.placeholder.prefixAppend"
    },
    {
      name: "suffix",
      labelKey: "feature.field.suffix",
      placeholderKey: "feature.placeholder.suffixAppend"
    },
    {
      name: "find",
      labelKey: "feature.field.find",
      placeholderKey: "feature.placeholder.literalFind"
    },
    {
      name: "replace",
      labelKey: "feature.field.replace",
      placeholderKey: "feature.placeholder.replaceText"
    },
    { name: "scope", labelKey: "feature.field.scope", placeholder: "full" }
  ]
};

export function FeaturePages({ view, onBack }: { view: View; onBack: () => void }) {
  const { t } = useI18n();
  const title = t(FEATURE_TITLES[view] ?? "nav.c09");
  const [form, setForm] = useState<Record<string, string>>({});
  const [plan, setPlan] = useState<PlanDocument | null>(null);
  const [preview, setPreview] = useState<RenamePreview | null>(null);
  const [dryRun, setDryRun] = useState<UndoDryRun | null>(null);
  const [task, setTask] = useState<TaskView | null>(null);
  const [error, setError] = useState<IpcError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  /** 上次成功预览时锁定的表单快照：任一字段变化即视为旧计划失效（T 配方 P4）。 */
  const [plannedForm, setPlannedForm] = useState<string | null>(null);
  const [canUndo, setCanUndo] = useState(false);
  /** 保护目录命中的危险二次确认（方案 §3.3＋§7.11）。 */
  const [dangerOpen, setDangerOpen] = useState(false);

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
    setPlannedForm(null);
    setDangerOpen(false);
    try {
      const next = await planOperationFor(view, form);
      setPlan(next);
      setPlannedForm(JSON.stringify(form));
      if (view === "f12") {
        try {
          setPreview(await renamePreview(next.planId));
        } catch (value) {
          setPreview(null);
        }
      }
      setNotice(t("feature.notice.planDone", { count: next.entries.length }));
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
        kind: "apply",
        state: "queued",
        phase: "queued",
        done: 0,
        total: plan.entries.length
      });
      setNotice(t("feature.notice.queued"));
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
        kind: "undo",
        state: "queued",
        phase: "queued",
        done: 0,
        total: 0
      });
      setNotice(t("feature.notice.undoQueued"));
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setBusy(null);
    }
  };

  const onCancel = async () => {
    if (!task) return;
    setError(null);
    try {
      await cancelTask(task.taskId);
      setNotice(t("feature.notice.cancelRequested"));
    } catch (value) {
      setError(toIpcError(value));
    }
  };

  const onRetryFailed = async () => {
    if (!task) return;
    setBusy("apply");
    setError(null);
    try {
      const handle = await retryTask(task.taskId);
      setTask({
        taskId: handle.taskId,
        kind: "apply",
        state: "queued",
        phase: "queued",
        done: 0,
        total: task.total
      });
      setNotice(t("feature.notice.retryQueued"));
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
  // 参数改动后旧计划立即失效：执行／撤销／回滚预演都不得沿用旧 planId。
  // 内核侧仍有 E2010 指纹与保护名单 E1002 兜底，这里是入口层阻断（方案 §7.3）。
  const stale =
    plan !== null && plannedForm !== null && JSON.stringify(form) !== plannedForm;
  // 保护命中条目：E2009 由 kernel 判定，界面只做危险确认，不放宽也不改变该判定。
  const protectionHits = (plan?.entries ?? []).filter((entry) => entry.blocked?.code === "E2009");

  const field = (
    label: string,
    name: string,
    placeholder: string,
    wide = false
  ) => (
    // key 由调用方在列表里透传（字段网格按 spec 逐项渲染，缺 key 会触发 React 警告）。
    <div className={`field${wide ? " wide" : ""}`} key={name}>
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
      {error && <ErrorBanner error={error} onClose={() => setError(null)} />}
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
                <p className="card-sub">{t("feature.subtitle")}</p>
              </div>
              <button className="button button-ghost" type="button" onClick={onBack}>
                {t("common.backC09")}
              </button>
            </div>

            <div className="field-grid">
              {(FEATURE_FIELDS[view] ?? []).map((spec) =>
                spec.kind === "textarea" ? (
                  <div className="field wide" key={spec.name}>
                    <label htmlFor={`${view}-${spec.name}`}>{t(spec.labelKey)}</label>
                    <textarea
                      id={`${view}-${spec.name}`}
                      className="textarea"
                      value={form[spec.name] ?? ""}
                      onChange={(event) => setField(spec.name, event.target.value)}
                      placeholder={
                        spec.placeholderKey ? t(spec.placeholderKey) : spec.placeholder
                      }
                      rows={6}
                      spellCheck={false}
                    />
                    {spec.hintKey && <div className="hint">{t(spec.hintKey)}</div>}
                  </div>
                ) : (
                  field(
                    t(spec.labelKey),
                    spec.name,
                    spec.placeholderKey ? t(spec.placeholderKey) : spec.placeholder ?? "",
                    spec.wide
                  )
                )
              )}
            </div>

            {stale && (
              <div className="banner banner-warn" role="status" data-testid="plan-stale">
                <div>{t("feature.stale")}</div>
              </div>
            )}

            {dangerOpen && (
              <DangerConfirm
                confirmLabel={t("c09.danger.confirm")}
                items={protectionHits.map((entry) =>
                  t("c09.danger.item", {
                    path: entry.target ?? entry.source,
                    reason: entry.blocked?.reason ?? t("c09.danger.protectedReason")
                  })
                )}
                title={t("c09.danger.title")}
                body={t("c09.danger.body", { count: protectionHits.length })}
                onCancel={() => setDangerOpen(false)}
                onConfirm={() => {
                  setDangerOpen(false);
                  void onApply();
                }}
              />
            )}

            <div className="toolbar">
              <span className="hint">{t("feature.plan.hint")}</span>
              <button
                className="button button-primary"
                type="button"
                onClick={onPlan}
                disabled={busy !== null || active}
              >
                {busy === "plan" ? <Loader2 size={16} className="spin" /> : null}
                {busy === "plan" ? t("feature.plan.busy") : t("feature.plan.cta")}
              </button>
            </div>
          </section>

          {plan && (
            <section className="card" aria-labelledby="feature-plan">
              <div className="card-header">
                <div>
                  <h2 className="card-title" id="feature-plan">{t("feature.preview.title")}</h2>
                  <p className="card-sub">
                    {t("feature.preview.summary", {
                      count: plan.entries.length,
                      blocked: plan.stats.blocked,
                      warning: plan.stats.warning
                    })}
                  </p>
                </div>
                <span className="plan-id" title={plan.planId}>{plan.planId}</span>
              </div>
              <div className="table">
                <div className="row row-head">
                  <span>{t("feature.table.source")}</span>
                  <span>{t("feature.table.target")}</span>
                </div>
                {/* 方案 6.2：按目录处理的计划动辄上千条，只挂窗口内的行。 */}
                <VirtualList
                  label={t("feature.preview.list")}
                  items={plan.entries}
                  getKey={(entry) => entry.entryId}
                  renderRow={(entry) => (
                    <div className="row">
                      <span className="path" title={entry.source}>{entry.source}</span>
                      <span className="path" title={entry.target ?? entry.reason}>
                        {entry.target ?? entry.reason}
                        {entry.blocked ? ` · ${entry.blocked.code}` : ""}
                      </span>
                    </div>
                  )}
                />
              </div>
              {/* 集中报告：无效行/重复行/父路径为文件等都在计划级 warnings 里，功能页必须逐项点名，
                  只有计数会让用户看不到"哪几行被排除、为什么"。 */}
              {plan.warnings.length > 0 && (
                <div className="warnings">
                  {plan.warnings.map((warning, index) => (
                    <div className={`warning ${warning.riskLevel}`} key={`${warning.kind}-${index}`}>
                      <Info size={14} />
                      <span>
                        <strong>{warning.kind}</strong>
                        {warning.suggestion ? ` · ${warning.suggestion}` : ""}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </section>
          )}
        </div>

        <div className="stack">
          <section className="card" aria-labelledby="feature-execute">
            <div className="card-header">
              <div>
                <h2 className="card-title" id="feature-execute">{t("feature.execute.title")}</h2>
                <p className="card-sub">{t("feature.execute.sub")}</p>
              </div>
            </div>
            <div className="button-row">
              <button
                className="button button-primary"
                type="button"
                onClick={() => {
                  if (protectionHits.length > 0) {
                    setDangerOpen(true);
                    return;
                  }
                  void onApply();
                }}
                disabled={
                  !plan || busy !== null || active || plan.entries.length === 0 || stale
                }
              >
                {busy === "apply" ? <Loader2 size={16} className="spin" /> : null}
                {active ? t("feature.execute.applying") : t("feature.execute.apply")}
              </button>
              {active && (
                <button
                  className="button button-secondary"
                  type="button"
                  data-testid="cancel-task"
                  onClick={onCancel}
                  disabled={task?.state === "cancelling"}
                >
                  {task?.state === "cancelling"
                    ? t("common.cancelling")
                    : t("common.cancelAction")}
                </button>
              )}
              {task !== null &&
                !active &&
                task.kind === "apply" &&
                ["partial-failed", "failed"].includes(task.state) && (
                  <button
                    className="button button-secondary"
                    type="button"
                    data-testid="retry-failed"
                    onClick={onRetryFailed}
                    disabled={busy !== null}
                  >
                    {t("common.retryFailed")}
                  </button>
                )}
              <button
                className="button button-secondary"
                type="button"
                onClick={onUndoDryRun}
                disabled={!plan || stale}
              >
                {t("feature.execute.dryRun")}
              </button>
              <button
                className="button button-secondary"
                type="button"
                onClick={onUndo}
                disabled={!plan || busy !== null || active || stale}
              >
                {busy === "undo" ? <Loader2 size={16} className="spin" /> : <Undo2 size={16} />}
                {t("common.undo")}
              </button>
            </div>
            {task && (
              <div className="task-panel">
                <div className="task-head">
                  <span className="task-id" title={task.taskId}>{task.taskId}</span>
                  <span className="badge">{task.state}</span>
                </div>
                <div className="progress" role="progressbar" aria-label={t("common.progress")}>
                  <div
                    className="progress-fill"
                    style={{
                      width: `${task.total > 0 ? Math.min(100, Math.round((task.done / task.total) * 100)) : 0}%`
                    }}
                  />
                </div>
                <div className="hint">
                  {phaseLabel(t, task.phase)} · {task.done}/{task.total}
                  {task.message ? ` · ${task.message}` : ""}
                </div>
              </div>
            )}
            {dryRun && (
              <div className="banner banner-info" role="status">
                <div>
                  <strong>{t("feature.dryRun.title")}</strong> ·{" "}
                  {t("feature.dryRun.summary", {
                    conflict: dryRun.conflictCount,
                    missing: dryRun.missingCount
                  })}
                  <VirtualList
                    label={t("feature.dryRun.title")}
                    items={dryRun.entries}
                    getKey={(entry) => entry.entryId}
                    renderRow={(entry) => (
                      <div className="muted">
                        {entry.entryId} · {entry.status}
                        {entry.detail ? ` · ${entry.detail}` : ""}
                      </div>
                    )}
                  />
                </div>
              </div>
            )}
            {terminalOk && (
              <div className="banner banner-info" role="status">
                <div>{t("feature.execute.done")}</div>
              </div>
            )}
          </section>

          {view === "f12" && preview && (
            <section className="card" aria-labelledby="c05-preview">
              <div className="card-header">
                <div>
                  <h2 className="card-title" id="c05-preview">{t("feature.c05.title")}</h2>
                  <p className="card-sub">
                    {t("feature.c05.summary", {
                      process: preview.counts.toProcess,
                      skipped: preview.counts.skipped,
                      conflicts: preview.counts.conflicts,
                      blocked: preview.counts.blocked
                    })}
                  </p>
                </div>
              </div>
              <div className="table">
                <div className="table-scroll">
                  <div className="row row-head">
                    <span>{t("feature.c05.table.status")}</span>
                    <span>{t("feature.c05.table.rename")}</span>
                  </div>
                  {preview.rows.map((row) => (
                    <div className="row" key={row.entryId}>
                      <span>
                        {statusBadge(t, row.status as PreviewStatus)}
                        {row.twoPhase ? t("feature.c05.twoPhase") : ""}
                      </span>
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
                <h2 className="card-title">{t("feature.form.title")}</h2>
                <p className="card-sub">
                  {isTauri() ? t("feature.form.desktop") : t("feature.form.web")}
                </p>
              </div>
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
