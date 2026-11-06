import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent
} from "react";
import { getUiState, setUiState } from "./lib/ipc";
import type { UiSortDirection, UiTableState } from "./lib/ipc/types";
import { clampPixels, readPixelVar, rememberColumnWidth, rememberSort } from "./lib/uiState";
import { useI18n } from "./i18n";

/**
 * 表格把手：列宽拖拽与列头排序（设计 03 §11.2 的 `tables`、§11.3 的恢复与重置）。
 *
 * 为什么把手必须存在：`tables` 段的持久化早已完成，但界面上没有任何入口能改它——
 * 「持久化没人能改的值是假绿」（TRACEABILITY 03 行）。
 *
 * 两条边界：
 * - 排序只改**显示顺序**，派发顺序与跳过判定仍由内核的计划序决定（方案 §7.1），
 *   所以排序值不进「参数变更→旧计划失效」的表单快照（方案 §7.3）；
 * - 宽度数值不在 TS 里复制：区间上下界读 CSS token、当前宽度取 DOM 实测
 *   （同 `VirtualList` 量行高的理由），TS 只搬数字、不定义视觉。
 */

/** 设计 03 §11.1 的"防抖"：一次连续拖动或连按方向键只落一次盘。 */
const PERSIST_DEBOUNCE_MS = 300;
/** token 读不到时的回退下限（正常路径由 `--table-column-min` 决定）。 */
const FALLBACK_MIN_WIDTH = 88;
const FALLBACK_RESIZE_STEP = 16;

export interface TableColumn {
  /** 进 `tables[tableId].columnWidths` 与 `sort.column` 的稳定标识，不随文案变。 */
  id: string;
  /** 已翻译的列名。 */
  label: string;
  sortable?: boolean;
  resizable?: boolean;
}

/** 表头单元格渲染时补上的当前排序态与回调（`sortDirection` 为 `null`＝该列不是当前排序列）。 */
type ResolvedColumn = TableColumn & {
  tableId: string;
  sortDirection: UiSortDirection | null;
  onSort: (tableId: string, column: string) => void;
  onResize: (column: string, px: number | null) => void;
  onResizeEnd: (column: string, px: number) => void;
};

export interface UiTableHandles {
  widths: (tableId: string) => Record<string, number> | undefined;
  sortOf: (tableId: string) => UiTableState["sort"];
  previewWidth: (tableId: string, column: string, px: number | null) => void;
  commitWidth: (tableId: string, column: string, px: number) => void;
  toggleSort: (tableId: string, column: string) => void;
  forget: () => void;
}

/**
 * 一张表（或多张表）的视图态：列宽、排序，以及"先并再写＋防抖落盘"。
 *
 * `setUiState` 的 patch 是顶层浅合并、`tables` 整节替换，所以每次写入都基于最新全节
 * 合并——否则改了 C09 就把功能页的列宽擦掉了。
 */
export function useUiTableState(): UiTableHandles {
  const [stored, setStored] = useState<Record<string, UiTableState>>({});
  const [live, setLive] = useState<Record<string, Record<string, number>>>({});
  const storedRef = useRef(stored);
  const timer = useRef<number | null>(null);

  const write = useCallback((next: Record<string, UiTableState>) => {
    storedRef.current = next;
    setStored(next);
  }, []);

  const flush = useCallback(() => {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
    // UiState 是可丢的本机状态，写失败静默（同 navigation 分节的既有口径）。
    setUiState({ tables: storedRef.current }).catch(() => undefined);
  }, []);

  const schedule = useCallback(() => {
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(flush, PERSIST_DEBOUNCE_MS);
  }, [flush]);

  useEffect(() => {
    let disposed = false;
    getUiState()
      .then((state) => {
        if (!disposed && state.tables) write(state.tables);
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      if (timer.current !== null) flush();
    };
  }, [flush, write]);

  const widths = useCallback(
    (tableId: string) => {
      const base = stored[tableId]?.columnWidths;
      const preview = live[tableId];
      if (!base && !preview) return undefined;
      return { ...base, ...preview };
    },
    [live, stored]
  );

  const sortOf = useCallback((tableId: string) => stored[tableId]?.sort, [stored]);

  const previewWidth = useCallback((tableId: string, column: string, px: number | null) => {
    setLive((current) => {
      const forTable = { ...current[tableId] };
      if (px === null) delete forTable[column];
      else forTable[column] = px;
      return { ...current, [tableId]: forTable };
    });
  }, []);

  const commitWidth = useCallback(
    (tableId: string, column: string, px: number) => {
      write(rememberColumnWidth(storedRef.current, tableId, column, px));
      previewWidth(tableId, column, null);
      schedule();
    },
    [previewWidth, schedule, write]
  );

  const toggleSort = useCallback(
    (tableId: string, column: string) => {
      write(rememberSort(storedRef.current, tableId, column));
      schedule();
    },
    [schedule, write]
  );

  const forget = useCallback(() => {
    write({});
    setLive({});
  }, [write]);

  return { widths, sortOf, previewWidth, commitWidth, toggleSort, forget };
}

/** 把列宽写成 `--col-N` 自定义属性：没拖过的列继续用 CSS 里的默认轨道。 */
export function columnVarsStyle(
  columnIds: readonly string[],
  widths?: Record<string, number>
): CSSProperties {
  const vars: Record<string, string> = {};
  columnIds.forEach((id, index) => {
    const px = widths?.[id];
    if (px !== undefined) vars[`--col-${index + 1}`] = `${px}px`;
  });
  return vars as CSSProperties;
}

function HeadCell({ column }: { column: ResolvedColumn }) {
  const { t } = useI18n();
  const handleRef = useRef<HTMLSpanElement>(null);
  const drag = useRef<{ startX: number; startWidth: number; last: number } | null>(null);
  const sortable = column.sortable !== false;
  const resizable = column.resizable !== false;
  const active = column.sortDirection !== null;

  // 区间上下界与步进都读 `:root` 的 token（tokens.css 里定义它们的地方）：
  // 真实浏览器里 token 沿继承链可见，但 jsdom 只在 documentElement 上能读回——
  // 读同一处让生产路径与测试路径走的是同一份数值来源，不在 TS 里另立视觉值。
  const bounds = () => {
    const styles = getComputedStyle(document.documentElement);
    const min = readPixelVar(styles, "--table-column-min");
    const max = readPixelVar(styles, "--table-column-max");
    return { min: min ?? FALLBACK_MIN_WIDTH, max: max ?? Number.POSITIVE_INFINITY };
  };

  /** 单元格实测宽度；量不到（未挂载/隐藏）时从下限起步，键盘与鼠标两条路不断链。 */
  const widthOfCell = () => {
    const cell = handleRef.current?.parentElement;
    const measured = cell ? cell.getBoundingClientRect().width : 0;
    if (measured > 0) return measured;
    const { min } = bounds();
    return min;
  };

  const onPointerDown = (event: ReactPointerEvent<HTMLSpanElement>) => {
    const startWidth = widthOfCell();
    const { min } = bounds();
    drag.current = { startX: event.clientX, startWidth, last: Math.max(startWidth, min) };
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLSpanElement>) => {
    const state = drag.current;
    if (!state) return;
    const { min, max } = bounds();
    state.last = clampPixels(state.startWidth + (event.clientX - state.startX), min, max);
    if (resizable) column.onResize?.(column.id, state.last);
  };

  const onPointerUp = (event: ReactPointerEvent<HTMLSpanElement>) => {
    const state = drag.current;
    event.currentTarget.releasePointerCapture(event.pointerId);
    drag.current = null;
    if (state) column.onResizeEnd?.(column.id, state.last);
  };

  const onPointerCancel = () => {
    drag.current = null;
    column.onResize?.(column.id, null);
  };

  // 键盘等价：`role="separator"` 不带按键就无法用键盘调宽。
  const onKeyDown = (event: ReactKeyboardEvent<HTMLSpanElement>) => {
    const delta = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    if (delta === 0) return;
    event.preventDefault();
    const step =
      readPixelVar(getComputedStyle(document.documentElement), "--resize-step") ??
      FALLBACK_RESIZE_STEP;
    const { min, max } = bounds();
    column.onResizeEnd(column.id, clampPixels(widthOfCell() + delta * step, min, max));
  };

  const arrow = column.sortDirection === "asc" ? "↑" : column.sortDirection === "desc" ? "↓" : "";

  return (
    <span className="col-head">
      {sortable ? (
        <button
          className="col-sort"
          type="button"
          onClick={() => column.onSort(column.tableId, column.id)}
          aria-pressed={active}
          aria-label={t("table.sort.aria", {
            column: column.label,
            direction:
              column.sortDirection === null
                ? t("table.sort.none")
                : column.sortDirection === "desc"
                  ? t("table.sort.desc")
                  : t("table.sort.asc")
          })}
        >
          {column.label}
          {arrow && <span className="col-arrow" aria-hidden="true">{arrow}</span>}
        </button>
      ) : (
        <span>{column.label}</span>
      )}
      {resizable && (
        <span
          className="col-resize"
          ref={handleRef}
          role="separator"
          aria-orientation="vertical"
          tabIndex={0}
          aria-label={t("table.resize.aria", { column: column.label })}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerCancel}
          onKeyDown={onKeyDown}
        />
      )}
    </span>
  );
}

export interface TableHeadProps {
  tableId: string;
  columns: readonly TableColumn[];
  tables: UiTableHandles;
}

/**
 * 表头行：DOM 仍是 `.row.row-head` 的单元格序列。
 *
 * 列宽的 `--col-N` 要同时作用于表头与虚拟滚动的行，所以由调用方落在表容器上
 * （`columnVarsStyle(columns.map(c => c.id), tables.widths(tableId))`）。
 */
export function TableHead({ tableId, columns, tables }: TableHeadProps) {
  const sort = tables.sortOf(tableId);
  return (
    <div className="row row-head">
      {columns.map((column) => {
        const resolved: ResolvedColumn = {
          ...column,
          tableId,
          sortDirection: sort && sort.column === column.id ? sort.direction : null,
          onSort: tables.toggleSort,
          onResize: (id, px) => tables.previewWidth(tableId, id, px),
          onResizeEnd: (id, px) => tables.commitWidth(tableId, id, px)
        };
        return <HeadCell key={column.id} column={resolved} />;
      })}
    </div>
  );
}
