import { useLayoutEffect, useRef, useState, type ReactNode } from "react";

/**
 * 固定行高的窗口化列表（方案 6.2：大列表一律虚拟滚动，禁止整表渲染）。
 *
 * 为什么是"固定行高"：预览表的每一行都是 nowrap 的路径 + 徽标，行高由 U v1.3 的
 * 间距 token 决定且与内容长度无关；固定行高让"总高度 = 条数 × 行高"这条换算成立，
 * 于是滚动条能正确反映真实比例，而不需要引入测量每一行的 ResizeObserver 开销。
 *
 * 行高与可视高度在真实浏览器里靠 DOM 实测（`getBoundingClientRect` / `clientHeight`），
 * 避免在 TS 里复制一份 CSS 数值；jsdom 量不到时用兜底常量（仅首帧/测试环境）。
 *
 * 无障碍：容器 `role="list"`，每行 `role="listitem"`，滚动容器可聚焦（键盘可滚）。
 * 行内容由调用方决定（`.row` 网格、`.result-row`、`.muted` 等），本组件只管窗口。
 */
const FALLBACK_ROW_HEIGHT = 48;
const FALLBACK_VIEWPORT_HEIGHT = 340;

export interface VirtualListProps<T> {
  items: readonly T[];
  renderRow: (item: T, index: number) => ReactNode;
  getKey: (item: T, index: number) => string;
  /** 列表的无障碍名称。 */
  label: string;
  /** 视口上下各多渲染几行，滚动时不会白屏。 */
  overscan?: number;
  /**
   * 仅测试用：显式指定行高与视口高度，绕开 jsdom 的 0 尺寸量取。
   * 生产代码不要传——真实浏览器实测比手写数值可靠。
   */
  rowHeight?: number;
  viewportHeight?: number;
  className?: string;
  testId?: string;
}

export function VirtualList<T>({
  items,
  renderRow,
  getKey,
  label,
  overscan = 3,
  rowHeight: rowHeightProp,
  viewportHeight: viewportHeightProp,
  className = "table-scroll",
  testId
}: VirtualListProps<T>) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [measured, setMeasured] = useState<{ row: number; viewport: number } | null>(null);

  useLayoutEffect(() => {
    const node = scrollRef.current;
    if (!node) return;
    const row = node.querySelector<HTMLElement>("[data-vlist-row]");
    const rowHeight = row?.getBoundingClientRect().height ?? 0;
    const viewportHeight = node.clientHeight;
    if (rowHeight > 0 && viewportHeight > 0) {
      setMeasured((current) =>
        current && current.row === rowHeight && current.viewport === viewportHeight
          ? current
          : { row: rowHeight, viewport: viewportHeight }
      );
    }
  }, [items.length]);

  const rowHeight = rowHeightProp ?? measured?.row ?? FALLBACK_ROW_HEIGHT;
  const viewport = viewportHeightProp ?? measured?.viewport ?? FALLBACK_VIEWPORT_HEIGHT;
  const total = items.length * rowHeight;
  const first = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const count = Math.ceil(viewport / rowHeight) + overscan * 2 + 1;
  const last = Math.min(items.length, first + count);

  return (
    <div
      className={className}
      ref={scrollRef}
      role="list"
      aria-label={label}
      aria-rowcount={items.length}
      tabIndex={0}
      data-testid={testId}
      onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
    >
      <div className="vlist-spacer" style={{ height: `${total}px` }}>
        <div className="vlist-window" style={{ transform: `translateY(${first * rowHeight}px)` }}>
          {items.slice(first, last).map((item, offset) => {
            const index = first + offset;
            return (
              <div
                className="vlist-item"
                data-vlist-row
                role="listitem"
                aria-posinset={index + 1}
                aria-setsize={items.length}
                key={getKey(item, index)}
                style={{ height: `${rowHeight}px` }}
              >
                {renderRow(item, index)}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
