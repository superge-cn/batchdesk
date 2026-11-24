import { useEffect, useMemo, useRef, useState } from "react";
import { Info, Loader2, Save, Undo2 } from "lucide-react";
import {
  applyPlan,
  getConfig,
  getTask,
  isTauri,
  listenTaskDone,
  listenTaskProgress,
  listenTaskState,
  planOperation,
  renamePreview,
  cancelTask,
  retryTask,
  saveRecipe,
  toIpcError,
  undoDryRun,
  undoPlan
} from "./lib/ipc";
import { DangerConfirm } from "./DangerConfirm";
import { ErrorBanner } from "./ErrorBanner";
import { warningKindTitle } from "./warnings";
import { VirtualList } from "./VirtualList";
import type { FeatureView as View } from "./navCatalog";
import { createWasmClient } from "./lib/wasm/client";
import {
  WebPlanUnavailable,
  planWithPickedInputs,
  webPlanSupport,
  type WebPlanGap
} from "./lib/web/planChannel";
import { isNoHeader, splitList, splitNumbers } from "./lib/web/formValues";
import {
  applyInSandbox,
  productFileNames,
  resetSandbox,
  saveProducts,
  stageInputs,
  summarizeOutcome,
  undoInSandbox,
  webApplySupport,
  webSandboxFacts,
  webStagedFiles,
  type ExecCaller,
  type WebApplyResult,
  type WebSandboxFacts
} from "./lib/web/applyChannel";
import {
  clearPersisted,
  persistProducts,
  type PersistOutcome
} from "./lib/web/opfsStore";
import { formatBytes } from "./lib/format";
import { useI18n, type MessageKey } from "./i18n";
import type { Translate } from "./labels";
import { dryRunStatusLabel, phaseLabel, stateBadge } from "./labels";
import type {
  ApplyReport,
  IpcError,
  PlanDocument,
  PlanEntry,
  PreviewStatus,
  ProgressPhase,
  RecipeSampling,
  RenamePreview,
  RenamePreviewRow,
  SaveRecipeInput,
  TaskState,
  UndoDryRun
} from "./lib/ipc/types";
import { TableHead, columnVarsStyle, useUiTableState } from "./TableHead";
import { sortRows } from "./lib/uiState";

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
  /** 终态报告：条目级明细（设计 06 §3 的 `ApplyEntryReport.detail`）走这里上界面，
   *  C10 的逐文件清单即其一。与 App 侧的 `TaskView` 同名字段对齐。 */
  report?: ApplyReport;
}

/**
 * Web 计划通道的 WASM 客户端：一页一个 Worker（重复创建有成本）。
 * 产物未装载时 `createWasmClient()` 返回 null，调用方据此明确失败而不是出示假计划。
 */
let planWasmClient: ReturnType<typeof createWasmClient> | undefined;
function sharedWasmClient(): ReturnType<typeof createWasmClient> {
  if (planWasmClient === undefined) {
    planWasmClient = createWasmClient();
  }
  return planWasmClient;
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
    });
  }
  if (view === "f06" || view === "f07" || view === "f08" || view === "f09") {
    // 公共字段（四个功能都有）；各功能的专有字段在各自分支里按精确类型声明，
    // 不用 `Record<string, unknown>`——那样 `kind` 的判别力会被擦掉。
    const base = {
      files: form.files ? form.files.split("\n").filter(Boolean) : [],
      target: form.target,
      sampleWindow: Number(form.sampleWindow || 4096),
      sampleBlocks: Number(form.sampleBlocks || 0),
      ...(form.delimiter ? { delimiter: form.delimiter } : {}),
      ...(form.source ? { source: form.source } : {}),
      ...(isNoHeader(form.header) ? { hasHeader: false } : {})
    };
    if (view === "f06") {
      return planOperation({
        kind: "f06",
        params: {
          ...base,
          output: form.output,
          ...(form.sourceColumnName
            ? { sourceColumn: true as const, sourceColumnName: form.sourceColumnName }
            : {}),
          ...(form.strict ? { strict: true as const } : {})
        },
        sourcePaths: []
      });
    }
    if (view === "f07") {
      return planOperation({
        kind: "f07",
        params: {
          ...base,
          ...(form.output ? { output: form.output } : {}),
          ...(form.outputDir ? { outputDir: form.outputDir } : {}),
          columns: form.columns
            ? form.columns.split(",").map((value) => value.trim()).filter(Boolean)
            : [],
          columnIndexes: form.columnIndexes
            ? form.columnIndexes
                .split(",")
                .map((value) => Number(value.trim()))
                .filter((value) => Number.isFinite(value) && value > 0)
            : []
        },
        sourcePaths: []
      });
    }
    if (view === "f08") {
      return planOperation({
        kind: "f08",
        params: {
          ...base,
          ...(form.output ? { output: form.output } : {}),
          ...(form.outputDir ? { outputDir: form.outputDir } : {}),
          window: form.window === "last" ? ("last" as const) : ("first" as const),
          count: Number(form.count || 0)
        },
        sourcePaths: []
      });
    }
    return planOperation({
      kind: "f09",
      params: {
        ...base,
        ...(form.output ? { output: form.output } : {}),
        ...(form.outputDir ? { outputDir: form.outputDir } : {}),
        startRow: Number(form.startRow || 0),
        endRow: Number(form.endRow || 0)
      },
      sourcePaths: []
    });
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
    });
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
    });
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
    });
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
              maxRatio: Number(form.maxRatio || 0),
              // 密码只随计划请求进壳层内存表，计划文档里没有它（ADR-0074）。
              ...(form.password ? { password: form.password } : {})
            })
      },
      sourcePaths: []
    });
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
    });
  }
  if (view === "c11" || view === "c12" || view === "c13" || view === "c14") {
    const base = {
      files: form.files ? form.files.split("\n").filter(Boolean) : [],
      target: form.target,
      sampleWindow: Number(form.sampleWindow || 4096),
      sampleBlocks: Number(form.sampleBlocks || 0),
      ...(form.output ? { output: form.output } : {}),
      ...(form.outputDir ? { outputDir: form.outputDir } : {}),
      ...(form.delimiter ? { delimiter: form.delimiter } : {}),
      ...(form.source ? { source: form.source } : {}),
      ...(isNoHeader(form.header) ? { hasHeader: false } : {})
    };
    if (view === "c11") {
      return planOperation({
        kind: "c11",
        params: {
          ...base,
          keyColumns: splitList(form.keyColumns),
          keyColumnIndexes: splitNumbers(form.keyColumnIndexes)
        },
        sourcePaths: []
      });
    }
    if (view === "c12") {
      return planOperation({
        kind: "c12",
        params: {
          ...base,
          ...(form.column ? { column: form.column } : {}),
          ...(form.columnIndex ? { columnIndex: Number(form.columnIndex) } : {}),
          ...(form.op ? { op: form.op } : {}),
          value: form.value ?? "",
          filterMode: form.filterMode === "drop" ? ("drop" as const) : ("keep" as const)
        },
        sourcePaths: []
      });
    }
    if (view === "c13") {
      return planOperation({
        kind: "c13",
        params: {
          ...base,
          sortColumns: splitList(form.sortColumns),
          sortColumnIndexes: splitNumbers(form.sortColumnIndexes),
          order: form.order === "desc" ? ("desc" as const) : ("asc" as const),
          ...(form.externalChunkRows
            ? { externalChunkRows: Number(form.externalChunkRows) }
            : {})
        },
        sourcePaths: []
      });
    }
    return planOperation({
      kind: "c14",
      params: {
        ...base,
        ...(form.splitColumn ? { splitColumn: form.splitColumn } : {}),
        ...(form.splitColumnIndex
          ? { splitColumnIndex: Number(form.splitColumnIndex) }
          : {}),
        ...(form.splitSeparator ? { splitSeparator: form.splitSeparator } : {}),
        ...(form.splitRegex ? { splitRegex: form.splitRegex } : {}),
        mergeColumns: splitList(form.mergeColumns),
        mergeColumnIndexes: splitNumbers(form.mergeColumnIndexes),
        ...(form.mergeJoinWith !== undefined ? { mergeJoinWith: form.mergeJoinWith } : {}),
        ...(form.mergeName ? { mergeName: form.mergeName } : {})
      },
      sourcePaths: []
    });
  }
  if (view === "f11") {
    return planOperation({
      kind: "f11",
      params: { list: form.list, root: form.root },
      sourcePaths: []
    });
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
    });
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
    });
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
    });
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
    });
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
    });
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
  });
}

// 表单值解析（splitList／splitNumbers／isNoHeader）与 Web 计划通道共用，
// 定义在 `lib/web/formValues.ts`——两处各写一份必然漂移。

/**
 * 浏览器计划能力缺口的文案键：`webPlanSupport` 给的是原因枚举（lib 层不依赖语言包），
 * 界面在这里翻译成人话。`Record<WebPlanGap, ...>` 保证新增缺口时编译不过。
 */
const WEB_PLAN_GAP_KEYS: Record<WebPlanGap, MessageKey> = {
  "directory-walk": "feature.plan.gap.directoryWalk",
  "file-identity": "feature.plan.gap.fileIdentity",
  "pending-wiring": "feature.plan.gap.pendingWiring"
};

/**
 * 预览行状态徽章。`Record` 全覆盖＋语气色表：内核将来加一个状态时这里编译不过，
 * 而不是走兜底分支把线协议字符串（`too-long` 之类）原样印到界面上（09 §2）。
 */
const PREVIEW_STATUS_KEYS: Record<PreviewStatus, MessageKey> = {
  normal: "feature.status.normal",
  conflict: "feature.status.conflict",
  invalid: "feature.status.invalid",
  "too-long": "feature.status.tooLong",
  protected: "feature.status.protected"
};

const PREVIEW_STATUS_TONES: Record<PreviewStatus, string> = {
  normal: "badge badge-safe",
  conflict: "badge badge-danger",
  invalid: "badge badge-danger",
  "too-long": "badge badge-warn",
  protected: "badge badge-danger"
};

/** 排序取比较键而不是界面文案：文案随语言变，键不随语言变（设计 09）。 */
function featureSortKey(entry: PlanEntry, column: string): string | undefined {
  return column === "source" ? entry.source : entry.target ?? entry.reason;
}

/**
 * C05 预览表（F12 页内嵌的冲突检测表）：与功能计划预览**分表**存列宽/排序
 * （表 id `table.feature.c05`），否则拖 C05 的列会把 F12 计划表的列宽擦掉。
 */
const C05_PREVIEW_TABLE = "table.feature.c05";
const C05_PREVIEW_COLUMNS = ["status", "rename"];

/** 排序键按 `RenamePreviewRow` 的字段名取（AGENTS 引用回核：字段取自类型定义，不猜）。 */
function c05SortValue(row: RenamePreviewRow, column: string): string | number | undefined {
  if (column === "status") return row.status;
  if (column === "rename") return row.oldPath;
  return undefined;
}

function statusBadge(t: Translate, status: PreviewStatus) {
  return <span className={PREVIEW_STATUS_TONES[status]}>{t(PREVIEW_STATUS_KEYS[status])}</span>;
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
  /**
   * `files`：多文件选择器（File API）。
   * `file`：**单文件**选择器（C10 的压缩包：一次只解一个，多选没有语义）。
   * `directory`：**整个目录**选择器（`webkitdirectory`）——C08 需要每个文件的
   * 相对路径，只有目录选择才拿得到（`webkitRelativePath`）。
   * 桌面形态三者都仍渲染同一名字的路径输入——渲染按 `isTauri()` 分流，
   * 计划侧也分两条路（见 `onPlan`），字段名保持一致以免模板/上下文漂移。
   */
  kind?: "textarea" | "password" | "files" | "file" | "directory";
  /**
   * 桌面仍是 `kind` 的形态、浏览器改走**目录选择**。
   *
   * 与 `kind: "directory"` 的区别：那个连桌面也渲染成单行路径输入；F10 的 `roots`
   * 桌面是**多行**（多个根目录一行一个），不能为了 Web 的目录选择把桌面降级成单行。
   */
  webDirectory?: boolean;
  hintKey?: MessageKey;
  /**
   * 浏览器形态专属提示（覆盖 `hintKey`）。同一字段在桌面与浏览器的**能力**
   * 不同时用：桌面能查目标占用、浏览器不能，提示不能共用一条。
   */
  webHintKey?: MessageKey;
}

const FILE_LIST: FieldSpec = {
  name: "files",
  labelKey: "feature.field.files",
  // 浏览器渲染 File 选择器、桌面渲染路径文本域（渲染按 isTauri 分流，字段名一致）。
  kind: "files",
  placeholder: "/path/a.csv\n/path/b.csv\n"
};

const FEATURE_FIELDS: Partial<Record<View, FieldSpec[]>> = {
  f01: [
    {
      name: "file",
      labelKey: "feature.field.file",
      kind: "files",
      placeholder: "/path/a.txt",
      wide: true
    },
    { name: "targetBytes", labelKey: "feature.field.targetBytes", placeholder: "8" },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true }
  ],
  f02: [
    {
      name: "file",
      labelKey: "feature.field.file",
      kind: "files",
      placeholder: "/path/a.txt",
      wide: true
    },
    { name: "linesPerPart", labelKey: "feature.field.linesPerPart", placeholder: "100" },
    { name: "outputDir", labelKey: "feature.field.outputDir", placeholder: "/path/out", wide: true }
  ],
  f03: [
    {
      name: "file",
      labelKey: "feature.field.file",
      kind: "files",
      placeholder: "/path/a.txt",
      wide: true
    },
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
    // 浏览器形态用文件选择器；桌面形态是路径文本域（同一字段名，按 isTauri 分流）。
    { name: "files", labelKey: "feature.field.files", kind: "files" },
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
    {
      name: "file",
      labelKey: "feature.field.file",
      kind: "files",
      placeholder: "/path/list.csv",
      wide: true
    },
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
      // 桌面：多个根目录一行一个；浏览器：目录选择（清单的源是根目录，只有目录
      // 选择才拿得到相对路径与根名）。
      kind: "textarea",
      webDirectory: true,
      webHintKey: "feature.field.f10.webHint",
      placeholder: "/path/root-a\n/path/root-b\n",
      hintKey: "feature.field.f10.hint"
    },
    { name: "outputDir", labelKey: "feature.field.listingOutputDir", placeholder: "/path/out", wide: true }
  ],
  c04: [
    {
      name: "dir",
      labelKey: "feature.field.dir",
      // 浏览器里必须选**整个目录**：条目＝目录的第一层名字，只有目录选择才拿得到
      // 相对路径（普通多选没有 `webkitRelativePath`）。
      kind: "directory",
      webHintKey: "feature.field.dirWebHint",
      placeholder: "/path/dir",
      wide: true
    },
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
      // 清单与源目录是**两份独立输入**：浏览器形态各占一个槽，
      // 不把清单混进源目录的选中文件里（那样只能靠扩展名猜谁是清单）。
      kind: "file",
      webHintKey: "feature.field.manifest.webHint",
      placeholder: "/path/manifest.csv",
      wide: true
    },
    {
      name: "sourceRoot",
      labelKey: "feature.field.sourceRoot",
      kind: "directory",
      placeholder: "/path/src",
      wide: true
    },
    {
      name: "targetRoot",
      labelKey: "feature.field.targetRoot",
      webHintKey: "feature.field.c07.targetWebHint",
      placeholder: "/path/dst",
      wide: true
    },
    { name: "operation", labelKey: "feature.field.operation", placeholder: "move" },
    { name: "missing", labelKey: "feature.field.missing", placeholder: "skip" },
    { name: "delimiter", labelKey: "feature.field.delimiter", placeholder: "," },
    {
      name: "source",
      labelKey: "feature.field.source",
      webHintKey: "feature.field.source.webHint",
      placeholder: "utf-8"
    }
  ],
  c08: [
    {
      name: "sourceRoot",
      labelKey: "feature.field.sourceRoot",
      // 浏览器里必须选**整个目录**：归档规则要按相对路径判定归属，
      // 普通多选拿不到子目录结构（`planC08` 会拒绝而不是拿文件名凑数）。
      kind: "directory",
      placeholder: "/path/src",
      wide: true
    },
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
    {
      name: "mode",
      // C10 的取值是 extract|compress，与 C03/C04 的 plain|regex 不是一回事：
      // 标签必须专属，否则界面上会写「模式 (plain|regex)」而实际只收这两个值。
      labelKey: "feature.field.c10.mode",
      hintKey: "feature.field.c10.mode.hint",
      placeholder: "extract"
    },
    {
      name: "archive",
      labelKey: "feature.field.archive",
      // 浏览器里必须选一个压缩包：C10 的条目事实来自中央目录（ADR-0103），
      // 没有路径就等于没有输入。桌面仍渲染路径输入。
      kind: "file",
      placeholder: "/path/a.zip",
      wide: true
    },
    {
      name: "outputDir",
      labelKey: "feature.field.extractOutputDir",
      placeholder: "/path/out",
      wide: true
    },
    { name: "maxTotalBytes", labelKey: "feature.field.maxTotalBytes", placeholder: "104857600" },
    { name: "maxRatio", labelKey: "feature.field.maxRatio", placeholder: "100" },
    {
      name: "password",
      labelKey: "feature.field.password",
      kind: "password",
      hintKey: "feature.field.passwordHint",
      wide: true
    },
    {
      name: "output",
      labelKey: "feature.field.compressOutput",
      // 浏览器没有产物文件系统：目标是否已存在查不到，计划不会预判冲突。
      webHintKey: "feature.field.compressOutput.webHint",
      placeholder: "/path/bundle.zip",
      wide: true
    },
    {
      name: "sources",
      labelKey: "feature.field.sources",
      // 浏览器压缩只能选**文件**：拿不到目录树，「整目录压缩」不在 Web 能力内。
      kind: "files",
      webHintKey: "feature.field.sources.webHint",
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
    {
      name: "dir",
      labelKey: "feature.field.renameDir",
      // 同 C04：条目来自目录选择的相对路径，浏览器里只改第一层（与桌面 `read_dir` 同口径）。
      kind: "directory",
      webHintKey: "feature.field.dirWebHint",
      placeholder: "/path/dir",
      wide: true
    },
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

/// v6 变换正文（OQ-43②，ADR-0081）：f06/f07/c08/c12/c13/c14 的请求侧语义
/// （来源列/列选择/规则/条件/排序键/拆分合并规格）从不进 plan.params，保存时
/// 从表单采集进 `content` 块——取值形态与上方 plan 构造分支一致（同一批
/// split/Number 规则），路径键由内核 strip_paths 兜底剔除，白名单外的键由
/// 壳层拒绝（不静默丢）。
function contentFromForm(
  view: View,
  form: Record<string, string>
): Record<string, unknown> | undefined {
  const content: Record<string, unknown> = {};
  if (view === "f06") {
    // 界面用「来源列名是否填了」表达开关（与 plan 构造一致）：填了＝要来源列
    content.sourceColumn = Boolean(form.sourceColumnName);
    if (form.sourceColumnName) content.sourceColumnName = form.sourceColumnName;
  } else if (view === "f07") {
    content.columns = form.columns
      ? form.columns.split(",").map((value) => value.trim()).filter(Boolean)
      : [];
    content.columnIndexes = form.columnIndexes
      ? form.columnIndexes
          .split(",")
          .map((value) => Number(value.trim()))
          .filter((value) => Number.isFinite(value) && value > 0)
      : [];
  } else if (view === "c08") {
    content.rules = form.rules ? form.rules.split("\n").filter(Boolean) : [];
    content.recursive = form.recursive === "yes";
  } else if (view === "c12") {
    if (form.column) content.column = form.column;
    if (form.columnIndex) content.columnIndex = Number(form.columnIndex);
    content.op = form.op;
    content.value = form.value ?? "";
    content.filterMode = form.filterMode === "drop" ? "drop" : "keep";
  } else if (view === "c13") {
    content.sortColumns = splitList(form.sortColumns);
    content.sortColumnIndexes = splitNumbers(form.sortColumnIndexes);
    content.order = form.order === "desc" ? "desc" : "asc";
    if (form.externalChunkRows) content.externalChunkRows = Number(form.externalChunkRows);
  } else if (view === "c14") {
    if (form.splitColumn) content.splitColumn = form.splitColumn;
    if (form.splitColumnIndex) content.splitColumnIndex = Number(form.splitColumnIndex);
    if (form.splitSeparator) content.splitSeparator = form.splitSeparator;
    if (form.splitRegex) content.splitRegex = form.splitRegex;
    content.mergeColumns = splitList(form.mergeColumns);
    content.mergeColumnIndexes = splitNumbers(form.mergeColumnIndexes);
    if (form.mergeJoinWith !== undefined) content.mergeJoinWith = form.mergeJoinWith;
    if (form.mergeName) content.mergeName = form.mergeName;
  }
  return Object.keys(content).length > 0 ? content : undefined;
}


export function FeaturePages({
  view,
  onBack,
  initialPlan = null,
  onContextChange
}: {
  view: View;
  onBack: () => void;
  /** F15：AI 面板交接的计划；只进预览，执行仍由本页的确认按钮发起。 */
  initialPlan?: PlanDocument | null;
  /** F15：把当前功能页的功能码与表单上报，供 App 组装面板上下文（设计 14 §9）。 */
  onContextChange?: (feature: string, form: Record<string, string>) => void;
}) {
  const { t } = useI18n();
  const title = t(FEATURE_TITLES[view] ?? "nav.c09");
  // 列宽/排序把手（设计 03 §11.2 的 tables）：按功能页各留一份，表 id 用功能码，
  // 换功能页不会把上一张表的宽度带过来。
  const uiTables = useUiTableState();
  const previewTableId = `table.feature.${view}`;
  const [form, setForm] = useState<Record<string, string>>({});
  /**
   * 浏览器形态选中的本地文件，**按字段名分槽**（桌面形态恒空：路径来自表单，
   * 由壳层采集）。
   *
   * 为什么分槽：C07 需要两份互不相干的输入（清单文件 ＋ 源目录）。混在一个数组里
   * 就只能靠扩展名猜谁是清单，猜错会拿源目录里的某个 CSV 当清单解析。
   */
  const [pickedByField, setPickedByField] = useState<Record<string, File[]>>({});
  /** 所有槽的扁平视图：只用于「有没有选输入」与 AI 上下文的文件名清单。 */
  const pickedFiles = useMemo(
    () => Object.values(pickedByField).flat(),
    [pickedByField]
  );
  /**
   * 计划来源。`wasm` 表示这份计划由浏览器内核真实算出——但 Web 形态**没有**
   * OPFS 执行器（设计 02 §3.1 未实现），执行/撤销/回滚预演必须禁用；
   * 否则真计划配 mock 执行会造出「已执行」的假象。
   */
  const [planOrigin, setPlanOrigin] = useState<"ipc" | "wasm">("ipc");
  /** 浏览器沙箱的执行结果：真执行、产物以 Blob 交回，下载后才落到本机（ADR-0108）。 */
  const [webApply, setWebApply] = useState<WebApplyResult | null>(null);
  /**
   * 本次执行往浏览器（OPFS）里留了几份产物。
   *
   * 只有实际存下来才显示：不支持 OPFS 的浏览器与失败分支都不该给用户
   * 一个「已保存 0 个」的噪声（零强制力，落盘是加分项不是承诺）。
   */
  const [persisted, setPersisted] = useState<PersistOutcome | null>(null);
  /** 超出 §13.10.2 本地建议档时的确认事实；非空即弹确认（确认前不读内容）。 */
  const [sandboxAsk, setSandboxAsk] = useState<WebSandboxFacts | null>(null);
  /** 沙箱里是否已有执行痕迹：换输入/换视图时据此决定要不要清空（避免空转调用）。 */
  const sandboxUsed = useRef(false);
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
  /** 存为处理模板（方案 §15.2.1：保存入口在参数/预览页，不在独立表单）。 */
  const [templateOpen, setTemplateOpen] = useState(false);
  const [templateName, setTemplateName] = useState("");
  const [templateDescription, setTemplateDescription] = useState("");
  const [templateExtensions, setTemplateExtensions] = useState("");
  const [savingTemplate, setSavingTemplate] = useState(false);
  /** F15：已应用的 AI 计划 id；同一计划在切视图往返时不重复套用。 */
  const appliedAiPlan = useRef<string | null>(null);

  const setField = (name: string, value: string) =>
    setForm((previous) => ({ ...previous, [name]: value }));

  /**
   * 浏览器形态的「选好了文件」：多选与目录选择共用。
   *
   * 换输入即让旧计划失效（方案 §7.3）——否则界面上会留着一份按旧文件算出来的
   * 计划，看起来仍然可以执行。
   */
  const onPickFiles = (field: string, picked: FileList | null) => {
    const files = Array.from(picked ?? []);
    setPickedByField((previous) => ({ ...previous, [field]: files }));
    setPlan(null);
    setPlannedForm(null);
    setPlanOrigin("ipc");
    setWebApply(null);
    if (sandboxUsed.current) {
      sandboxUsed.current = false;
      resetPlanSandbox();
    }
  };

  // 换功能页即换输入：浏览器选择的文件与「计划来源」都不跨视图复用
  // （表单 field 仍按既有设计跨视图保留，那是路径文本，与 File 句柄不同）。
  useEffect(() => {
    setPickedByField({});
    setPlanOrigin("ipc");
    setWebApply(null);
    setSandboxAsk(null);
    if (sandboxUsed.current) {
      sandboxUsed.current = false;
      resetPlanSandbox();
    }
    // `resetPlanSandbox` 只读模块级 Worker 引用；不随渲染变化。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view]);

  // F15：把当前功能页的功能码与表单上报给 App，供 AI 面板上下文取「文件名清单」。
  // 只在视图或表单变化时上报；面板是否使用、是否发送由面板自己的同意流程决定。
  //
  // 浏览器形态的文件是 `File` 对象，**不在表单里**（表单只有路径文本域）。
  // 这里把文件名按同一字段合进去：否则用户明明选了文件，AI 上下文的
  // 「包含文件名」还是恒空（`fileNamesForFeature` 读的就是 `form.files`）。
  //
  // 合并结果必须 `useMemo` 缓存：每次渲染新建对象 → `setAiForm` 拿到新引用 →
  // App 重渲染 → 回调换引用 → 本 effect 重跑，会把自己转成死循环（实测卡死）。
  const reportedForm = useMemo(
    () =>
      pickedFiles.length > 0
        ? { ...form, files: pickedFiles.map((file) => file.name).join("\n") }
        : form,
    [form, pickedFiles]
  );
  // 回调本身放在 ref 里：`onContextChange` 是调用方的内联箭头函数，每次渲染都换
  // 引用；把它放进依赖会让「渲染 → effect → setState → 渲染」无谓地转圈。
  const contextChangeRef = useRef(onContextChange);
  useEffect(() => {
    contextChangeRef.current = onContextChange;
  }, [onContextChange]);
  useEffect(() => {
    contextChangeRef.current?.(view, reportedForm);
  }, [view, reportedForm]);

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
        // 报告随 done 事件到达（`event.summary` 就是 ApplyReport）——条目级明细
        // （C10 的逐文件清单）据此上界面，不必等 getTask 回读。
        patch(event.taskId, { report: event.summary });
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

  // F15：AI 面板交接的计划只落到既有预览（设计 14 §7.1），不触发任何执行。
  // 快照取当前表单，避免交接瞬间被 §7.3 的 stale 判定误伤；用户之后改参数仍会失效。
  useEffect(() => {
    if (!initialPlan || initialPlan.kind !== view) return;
    if (appliedAiPlan.current === initialPlan.planId) return;
    appliedAiPlan.current = initialPlan.planId;
    setPlan(initialPlan);
    setPlannedForm(JSON.stringify(form));
    setPreview(null);
    setDryRun(null);
    setCanUndo(false);
    setError(null);
    setDangerOpen(false);
    setNotice(t("feature.notice.aiPlan", { count: initialPlan.entries.length }));
    if (view === "f12") {
      renamePreview(initialPlan.planId)
        .then(setPreview)
        .catch(() => setPreview(null));
    }
  }, [initialPlan, view, form, t]);

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
      // 浏览器形态 + 已选文件 → 走 WASM 真算（A3-2：能算的视图全部接线）。
      // 事实不全的视图由 `planWithFiles` 明确拒绝（E5001），**不在这里退回 mock**：
      // 假计划可以是演示数据，但不能是「以为算过了」。
      let next: PlanDocument;
      if (!isTauri() && pickedFiles.length > 0) {
        const client = sharedWasmClient();
        if (client === null) {
          throw new WebPlanUnavailable(
            "WASM runtime is not available in this environment"
          );
        }
        next = await planWithPickedInputs(
          (request) => client.call(request),
          view,
          pickedByField,
          form
        );
        setPlanOrigin("wasm");
      } else {
        next = await planOperationFor(view, form);
        setPlanOrigin("ipc");
      }
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

/**
 * 清空浏览器执行沙箱（换输入／换视图／重新预览时）。
 *
 * 不提示失败：沙箱不可用时后续 apply 会自己报 E5002；这里再弹一次只是噪音。
 */
const resetPlanSandbox = () => {
  const client = sharedWasmClient();
  if (client === null) return;
  resetSandbox((request) => client.call(request)).catch(() => undefined);
};

/** 冲突策略取自设置（C19：`processing.onConflict`，缺省 skip＝安全默认）。 */
const readOnConflict = async (): Promise<"skip" | "sequence" | "overwrite"> => {
  try {
    const scope = await getConfig("processing");
    const value = scope.values["onConflict"]?.value;
    if (value === "skip" || value === "sequence" || value === "overwrite") {
      return value;
    }
  } catch {
    // 读不到设置时保持缺省 skip（不覆盖）。
  }
  return "skip";
};

/**
 * 浏览器沙箱执行（ADR-0108）：登记输入 → 内核执行 → 产物交回。
 *
 * 与桌面 `onApply` 的区别只在落点：桌面写用户磁盘并进任务队列；这里写 Worker
 * 内沙箱、产物等用户下载。**同一个 `generic_apply`**，不是模拟。
 */
const runSandboxApply = async (client: { call: ExecCaller }) => {
  if (!plan) return;
  setBusy("apply");
  setError(null);
  setNotice(null);
  setWebApply(null);
  setPersisted(null);
  try {
    const call: ExecCaller = (request) => client.call(request);
    await stageInputs(call, view, pickedByField);
    const result = await applyInSandbox(call, plan, {
      onConflict: await readOnConflict(),
      // 解密密码只随本次请求进沙箱（ADR-0074）：计划里只有「会提供密码」这个事实，
      // 界面上不填就不发，执行器按 E3008 整单拒绝而不是解一半。
      ...(form.password ? { password: form.password } : {})
    });
    sandboxUsed.current = true;
    setWebApply(result);
    setNotice(t("feature.notice.webApplied", { count: result.products.length }));
    // 落盘是**执行之后的异步搬运**（ADR-0115）：内核 FsAdapter 是同步契约，
    // OPFS 是异步 API，所以不把 OPFS 塞进执行栈。失败不阻断、不改计划——
    // 零强制力环境下它只是「多留一份」，不是安全边界。
    if (result.products.length > 0) {
      const names = productFileNames(result.products);
      void persistProducts(
        plan.planId,
        result.products.map((product, index) => ({
          name: names[index],
          blob: product.blob
        }))
      )
        .then((outcome) => {
          if (outcome.saved > 0) setPersisted(outcome);
        })
        .catch(() => undefined);
    }
  } catch (value) {
    setError(toIpcError(value));
  } finally {
    setBusy(null);
  }
};

  const onApply = async () => {
    if (!plan) return;
    // 浏览器形态：真计划 → 沙箱真执行（不再走 mock，也不再假装"只能看"）。
    if (!isTauri() && planOrigin === "wasm") {
      if (webApplySupport(view).level !== "native") return;
      const client = sharedWasmClient();
      if (client === null) {
        setError(
          toIpcError(
            new WebPlanUnavailable("WASM runtime is not available in this environment")
          )
        );
        return;
      }
      const facts = webSandboxFacts(webStagedFiles(view, pickedByField));
      if (facts.aboveGuidance) {
        // §13.10.2：超本地建议档是强提示＋确认后放行，不是阻断。
        setSandboxAsk(facts);
        return;
      }
      await runSandboxApply(client);
      return;
    }
    setBusy("apply");
    setError(null);
    setNotice(null);
    try {
      // C19：冲突策略取自设置（processing.onConflict，默认 skip）；选 overwrite 即视为已确认。
      const onConflict = await readOnConflict();
      const handle = await applyPlan(plan.planId, {
        onConflict,
        overwriteConfirmed: onConflict === "overwrite"
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
    if (!isTauri() && planOrigin === "wasm") {
      const client = sharedWasmClient();
      if (client === null || webApply === null) return;
      setBusy("undo");
      setError(null);
      setNotice(null);
      try {
        const result = await undoInSandbox(
          (request) => client.call(request),
          plan.planId
        );
        sandboxUsed.current = false;
        setWebApply(null);
        resetPlanSandbox();
        setNotice(t("feature.notice.webUndone", { count: result.reverted }));
      } catch (value) {
        setError(toIpcError(value));
      } finally {
        setBusy(null);
      }
      return;
    }
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

  /** 浏览器执行能力：只有 `native` 档才放开执行按钮（其余仍如实禁用）。 */
  const canWebApply =
    planOrigin === "wasm" && webApplySupport(view).level === "native";

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

  /**
   * 存为处理模板。
   *
   * `kind` 取**计划里实际出现的动作**（`PlanEntry.action`），不靠人维护一张
   * 「功能码 → 动作」对照表：那张表正是 OQ-36 说的一对多来源，写死了迟早漂移。
   * 一份计划里出现多种动作时（C07 的清单可能是 move/copy 混排）拒绝保存而不是
   * 挑一个——挑错等于把另一部分语义悄悄丢掉。
   */
  /**
   * 采样口径进模板、不进快照（v5，OQ-41 裁决）：当前视图字段清单里有采样键才采集，
   * 值取表单当前值，空则与计划生成同口径落 4096（`FeaturePages` 里 `|| 4096` 的既有缺省，
   * 非方案 §5.2 数值）。判据来自 FEATURE_FIELDS，不另抄一份功能码清单。
   */
  const samplingFromForm = (): RecipeSampling | undefined => {
    const sampling: RecipeSampling = {};
    for (const spec of FEATURE_FIELDS[view] ?? []) {
      if (spec.name !== "sampleWindow" && spec.name !== "sampleBytes" && spec.name !== "sampleBlocks") {
        continue;
      }
      const fallback = spec.name === "sampleBlocks" ? 0 : 4096;
      const raw = Number(form[spec.name] || fallback);
      if (!Number.isFinite(raw) || raw <= 0) {
        continue;
      }
      if (spec.name === "sampleWindow") sampling.sampleWindow = raw;
      if (spec.name === "sampleBlocks") sampling.sampleBlocks = raw;
      if (spec.name === "sampleBytes") sampling.sampleBytes = raw;
    }
    return Object.keys(sampling).length > 0 ? sampling : undefined;
  };

  const onSaveTemplate = async () => {
    if (!plan) return;
    const counts = new Map<string, number>();
    for (const entry of plan.entries) {
      counts.set(entry.action, (counts.get(entry.action) ?? 0) + 1);
    }
    if (counts.size !== 1) {
      setNotice(t("feature.template.mixed"));
      return;
    }
    const action = Array.from(counts.keys())[0] as SaveRecipeInput["kind"];
    setSavingTemplate(true);
    setError(null);
    setNotice(null);
    try {
      const extensions = templateExtensions
        .split(/[,\uFF0C\s]+/)
        .map((value) => value.trim().replace(/^\./, "").toLowerCase())
        .filter(Boolean);
      const saved = await saveRecipe({
        name: templateName.trim(),
        description: templateDescription.trim() || undefined,
        match: extensions.length > 0 ? { extensions } : undefined,
        kind: action,
        // 参数快照就是计划自己带的那份（设计 01 §5）；源路径与输出目录由壳层剔除，
        // 这里不再抄一份路径键表——两份清单必然漂移，判据只留内核一处。
        paramsSnapshot: plan.params,
        // 采样口径进模板不进快照（v5，OQ-41 裁决）：快照保持计划形状，
        // 采样窗口/字节从当前表单采集——套用时模板值优先、没存由壳层按缺省补。
        sampling: samplingFromForm(),
        // 变换正文进模板不进快照（v6，OQ-43 裁决②）：六个功能的请求侧语义
        // 从当前表单采集，套用时注入请求（白名单外拒绝）。
        content: contentFromForm(view, form),
        // 功能码在保存这一刻是**已知**的（就是当前页），不记下来就得事后反推——
        // 而 `split`/`edit-content`/`rename` 这些动作对应多个功能码，反推必然靠猜（OQ-36 裁决①）。
        feature: view
      });
      setTemplateOpen(false);
      setTemplateName("");
      setTemplateDescription("");
      setTemplateExtensions("");
      setNotice(t("feature.template.saved", { name: saved.name }));
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setSavingTemplate(false);
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
    wide = false,
    inputType: "text" | "password" = "text",
    hint?: string
  ) => (
    // key 由调用方在列表里透传（字段网格按 spec 逐项渲染，缺 key 会触发 React 警告）。
    <div className={`field${wide ? " wide" : ""}`} key={name}>
      <label htmlFor={`${view}-${name}`}>{label}</label>
      <input
        id={`${view}-${name}`}
        className="input"
        type={inputType}
        // 密码不交给浏览器/密码管理器记住（ADR-0074 §5.4：一次任务一次输入）。
        autoComplete={inputType === "password" ? "off" : undefined}
        value={form[name] ?? ""}
        onChange={(event) => setField(name, event.target.value)}
        placeholder={placeholder}
        spellCheck={false}
      />
      {hint && <div className="hint">{hint}</div>}
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
                spec.kind === "files" && !isTauri() ? (
                  <div className="field wide" key={spec.name}>
                    <label htmlFor={`${view}-${spec.name}`}>{t(spec.labelKey)}</label>
                    <input
                      id={`${view}-${spec.name}`}
                      className="input"
                      type="file"
                      multiple
                      data-testid="web-file-input"
                      onChange={(event) => onPickFiles(spec.name, event.target.files)}
                    />
                    <div className="hint">
                      {spec.webHintKey
                        ? t(spec.webHintKey)
                        : t("feature.field.files.webHint")}
                    </div>
                  </div>
                ) : spec.kind === "file" && !isTauri() ? (
                  <div className="field wide" key={spec.name}>
                    <label htmlFor={`${view}-${spec.name}`}>{t(spec.labelKey)}</label>
                    <input
                      id={`${view}-${spec.name}`}
                      className="input"
                      type="file"
                      data-testid="web-single-file-input"
                      onChange={(event) => onPickFiles(spec.name, event.target.files)}
                    />
                    <div className="hint">
                      {spec.webHintKey
                        ? t(spec.webHintKey)
                        : t("feature.field.archive.webHint")}
                    </div>
                  </div>
                ) : (spec.kind === "directory" || spec.webDirectory === true) &&
                  !isTauri() ? (
                  <div className="field wide" key={spec.name}>
                    <label htmlFor={`${view}-${spec.name}`}>{t(spec.labelKey)}</label>
                    <input
                      id={`${view}-${spec.name}`}
                      className="input"
                      type="file"
                      multiple
                      data-testid="web-directory-input"
                      // `webkitdirectory` 不在 React 的 `InputHTMLAttributes` 里，
                      // 以属性透传方式设置：TS 不认识它，但浏览器靠它才给
                      // `webkitRelativePath`（相对路径的唯一来源）。
                      {...({ webkitdirectory: "", directory: "" } as Record<string, string>)}
                      onChange={(event) => onPickFiles(spec.name, event.target.files)}
                    />
                    <div className="hint">
                      {spec.webHintKey
                        ? t(spec.webHintKey)
                        : t("feature.field.sourceRoot.webHint")}
                    </div>
                  </div>
                ) : spec.kind === "textarea" || spec.kind === "files" ? (
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
                    spec.wide,
                    spec.kind === "password" ? "password" : "text",
                    !isTauri() && spec.webHintKey
                      ? t(spec.webHintKey)
                      : spec.hintKey
                        ? t(spec.hintKey)
                        : undefined
                  )
                )
              )}
            </div>

            {stale && (
              <div className="banner banner-warn" role="status" data-testid="plan-stale">
                <div>{t("feature.stale")}</div>
              </div>
            )}

            {!isTauri() && plan && planOrigin === "ipc" && (
              // 浏览器里的 mock 计划**必须自报家门**：它算过什么也没写盘，但界面
              // 长得和真计划一样。不标出来，用户会以为「已经按这些条目处理过了」。
              <div className="banner banner-warn" role="status" data-testid="web-plan-demo">
                <div>
                  {t("feature.plan.demo")}{" "}
                  {webPlanSupport(view).level === "native"
                    ? t("feature.plan.demoPick")
                    : t("feature.plan.unsupported", {
                        reason: t(
                          WEB_PLAN_GAP_KEYS[
                            webPlanSupport(view).gap ?? "pending-wiring"
                          ]
                        )
                      })}
                </div>
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

            {sandboxAsk && (
              // §13.10.2：超出本地建议档是**强提示 + 确认后放行**，不是阻断。
              // 确认框只报事实（文件数/总量/单文件最大），不替用户改参数。
              <DangerConfirm
                title={t("feature.sandbox.confirmTitle")}
                body={t("feature.sandbox.confirmBody", {
                  count: sandboxAsk.fileCount,
                  total: formatBytes(sandboxAsk.totalBytes)
                })}
                items={[
                  t("feature.sandbox.itemTotal", {
                    size: formatBytes(sandboxAsk.totalBytes)
                  }),
                  t("feature.sandbox.itemMax", {
                    size: formatBytes(sandboxAsk.maxFileBytes)
                  })
                ]}
                confirmLabel={t("feature.sandbox.confirmAction")}
                onCancel={() => setSandboxAsk(null)}
                onConfirm={() => {
                  setSandboxAsk(null);
                  const client = sharedWasmClient();
                  if (client !== null) void runSandboxApply(client);
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
              {/* 处理模板的保存入口在预览处（方案 §15.2.1：不在独立表单）。
                  存的是这次计划的参数快照与动作；源路径/输出目录由壳层剔除，不落模板。 */}
              <div className="toolbar">
                <span className="hint">{t("feature.template.hint")}</span>
                <button
                  className="button button-secondary"
                  type="button"
                  data-testid="save-template"
                  disabled={plan.entries.length === 0 || stale || savingTemplate || planOrigin === "wasm"}
                  onClick={() => setTemplateOpen((open) => !open)}
                >
                  <Save size={14} /> {t("feature.template.cta")}
                </button>
              </div>
              {templateOpen && (
                <div className="field-grid" data-testid="template-form">
                  <div className="field">
                    <label htmlFor={`${view}-template-name`}>{t("feature.template.name")}</label>
                    <input
                      id={`${view}-template-name`}
                      className="input"
                      value={templateName}
                      onChange={(event) => setTemplateName(event.target.value)}
                      spellCheck={false}
                    />
                  </div>
                  <div className="field">
                    <label htmlFor={`${view}-template-ext`}>
                      {t("feature.template.extensions")}
                    </label>
                    <input
                      id={`${view}-template-ext`}
                      className="input"
                      value={templateExtensions}
                      onChange={(event) => setTemplateExtensions(event.target.value)}
                      placeholder="csv, txt"
                      spellCheck={false}
                    />
                  </div>
                  <div className="field wide">
                    <label htmlFor={`${view}-template-desc`}>
                      {t("feature.template.description")}
                    </label>
                    <input
                      id={`${view}-template-desc`}
                      className="input"
                      value={templateDescription}
                      onChange={(event) => setTemplateDescription(event.target.value)}
                      spellCheck={false}
                    />
                  </div>
                  <div className="button-row">
                    <button
                      className="button button-primary"
                      type="button"
                      data-testid="save-template-submit"
                      disabled={savingTemplate || templateName.trim().length === 0}
                      onClick={() => void onSaveTemplate()}
                    >
                      {savingTemplate ? t("feature.template.saving") : t("feature.template.submit")}
                    </button>
                    <button
                      className="button button-ghost"
                      type="button"
                      onClick={() => setTemplateOpen(false)}
                    >
                      {t("common.cancel")}
                    </button>
                  </div>
                </div>
              )}
              <div
                className="table"
                style={columnVarsStyle(["source", "target"], uiTables.widths(previewTableId))}
              >
                <TableHead
                  tableId={previewTableId}
                  tables={uiTables}
                  columns={[
                    { id: "source", label: t("feature.table.source") },
                    { id: "target", label: t("feature.table.target") }
                  ]}
                />
                {/* 方案 6.2：按目录处理的计划动辄上千条，只挂窗口内的行。
                    排序只重排显示，派发的计划序不动（方案 §7.1）。 */}
                <VirtualList
                  label={t("feature.preview.list")}
                  items={sortRows(
                    plan.entries,
                    uiTables.sortOf(previewTableId),
                    featureSortKey
                  )}
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
                        <strong>{warningKindTitle(t, warning.kind)}</strong>
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
            {planOrigin === "wasm" && !canWebApply && (
              // 真计划 + 假执行比假计划更危险：该功能的执行臂尚未在 Web 装配
              //（改写器/目录遍历等端口缺失），这里如实说明并禁用执行链。
              <div
                className="banner banner-warn"
                role="status"
                data-testid="web-plan-readonly"
              >
                {t("feature.execute.webOnly")}
              </div>
            )}
            {planOrigin === "wasm" && canWebApply && (
              <div className="banner" role="status" data-testid="web-sandbox-note">
                {t("feature.execute.webSandbox")}
              </div>
            )}
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
                  !plan ||
                  busy !== null ||
                  active ||
                  plan.entries.length === 0 ||
                  stale ||
                  // 浏览器执行：只有已接线且尚未产生产物的计划可执行（重跑先撤销/换输入）
                  (planOrigin === "wasm" && (!canWebApply || webApply !== null))
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
                disabled={!plan || stale || planOrigin === "wasm"}
              >
                {t("feature.execute.dryRun")}
              </button>
              <button
                className="button button-secondary"
                type="button"
                onClick={onUndo}
                disabled={
                  !plan ||
                  busy !== null ||
                  active ||
                  stale ||
                  (planOrigin === "wasm" && webApply === null)
                }
              >
                {busy === "undo" ? <Loader2 size={16} className="spin" /> : <Undo2 size={16} />}
                {t("common.undo")}
              </button>
            </div>
            {webApply && (
              <div className="task-panel" data-testid="web-sandbox-result">
                <div className="hint">
                  {t("feature.sandbox.summary", {
                    ...summarizeOutcome(webApply.outcome),
                    state: webApply.state
                  })}
                </div>
                {webApply.products.length === 0 ? (
                  <p className="hint">{t("feature.sandbox.noProducts")}</p>
                ) : (
                  <>
                    <button
                      className="button button-secondary"
                      type="button"
                      data-testid="web-sandbox-download-all"
                      onClick={() => saveProducts(webApply.products)}
                    >
                      {t("feature.sandbox.downloadAll", {
                        count: webApply.products.length
                      })}
                    </button>
                    <ul className="dialog-list" data-testid="web-sandbox-products">
                      {webApply.products.map((product) => (
                        <li className="dialog-list" key={product.path}>
                          <span>{product.path}</span>{" "}
                          <span className="hint">{formatBytes(product.size)}</span>{" "}
                          <button
                            className="button button-secondary"
                            type="button"
                            onClick={() => saveProducts([product])}
                          >
                            {t("feature.sandbox.download")}
                          </button>
                        </li>
                      ))}
                    </ul>
                  </>
                )}
                {persisted !== null && (
                  <p className="hint" data-testid="web-sandbox-persisted">
                    {t("feature.sandbox.persisted", { count: persisted.saved })}{" "}
                    <button
                      className="button button-secondary"
                      type="button"
                      onClick={async () => {
                        const cleared = await clearPersisted();
                        if (cleared) {
                          setPersisted(null);
                          setNotice(t("feature.sandbox.persistedCleared"));
                        }
                      }}
                    >
                      {t("feature.sandbox.clearPersisted")}
                    </button>
                  </p>
                )}
                {/* 产物只存在于本页沙箱；撤销能收回沙箱，收不回已下载的副本。 */}
                <p className="hint">{t("feature.sandbox.scope")}</p>
              </div>
            )}
            {task && (
              <div className="task-panel">
                <div className="task-head">
                  <span className="task-id" title={task.taskId}>{task.taskId}</span>
                  {stateBadge(t, task.state)}
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
                        {entry.entryId} · {dryRunStatusLabel(t, entry.status)}
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
                {/* 条目级明细（设计 06 §3 的 ApplyEntryReport.detail：执行计数类技术行、
                    不含绝对路径、界面在显示）——C10 的逐文件清单走这里。 */}
                {task?.report?.entries
                  .filter((entry) => entry.detail)
                  .map((entry) => (
                    <div key={entry.entryId} className="manifest">
                      {entry.detail}
                    </div>
                  ))}
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
              <div
                className="table"
                style={columnVarsStyle(C05_PREVIEW_COLUMNS, uiTables.widths(C05_PREVIEW_TABLE))}
              >
                <TableHead
                  tableId={C05_PREVIEW_TABLE}
                  tables={uiTables}
                  columns={[
                    { id: "status", label: t("feature.c05.table.status") },
                    { id: "rename", label: t("feature.c05.table.rename") }
                  ]}
                />
                {/* 方案 6.2：改名预览按目录规模动辄上千行，只挂窗口内的行；
                    排序只重排显示，不改名也不改派发序（方案 §7.1）。 */}
                <VirtualList
                  label={t("feature.c05.title")}
                  items={sortRows(
                    preview.rows,
                    uiTables.sortOf(C05_PREVIEW_TABLE),
                    c05SortValue
                  )}
                  getKey={(row) => row.entryId}
                  renderRow={(row) => (
                    <div className="row">
                      <span>
                        {statusBadge(t, row.status)}
                        {row.twoPhase ? t("feature.c05.twoPhase") : ""}
                      </span>
                      <span className="path">
                        {row.oldPath} → {row.newPath}
                        {row.detail ? ` · ${row.detail}` : ""}
                      </span>
                    </div>
                  )}
                />
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
