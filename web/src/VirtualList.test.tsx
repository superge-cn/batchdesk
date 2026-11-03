import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { VirtualList } from "./VirtualList";

// 方案 6.2：大列表一律虚拟滚动，禁止整表渲染。
// 这组断言钉住"DOM 里只挂窗口内的行"，而不是"滚动条看起来能滚"。
function items(count: number): string[] {
  return Array.from({ length: count }, (_, index) => `第 ${index} 条`);
}

function list(source: string[]) {
  return (
    <VirtualList
      label="条目"
      items={source}
      getKey={(item) => item}
      rowHeight={20}
      viewportHeight={200}
      overscan={3}
      renderRow={(item) => <div className="row">{item}</div>}
    />
  );
}

describe("VirtualList 的窗口化", () => {
  it("2000 条只挂窗口内的行，未渲染的条目不在 DOM 里", () => {
    render(list(items(2000)));
    const root = screen.getByRole("list", { name: "条目" });
    // 视口 200 / 行高 20 = 10 行 + 上下 overscan 3 + 1 行余量
    const rows = root.querySelectorAll('[role="listitem"]');
    expect(rows.length).toBeLessThanOrEqual(17);
    expect(rows.length).toBeGreaterThan(0);
    expect(screen.getByText("第 0 条")).toBeInTheDocument();
    expect(screen.queryByText("第 1999 条")).toBeNull();
    expect(screen.queryByText("第 500 条")).toBeNull();
  });

  it("占位高度等于 条数 × 行高，滚动条比例因此真实", () => {
    render(list(items(2000)));
    const spacer = screen
      .getByRole("list", { name: "条目" })
      .querySelector(".vlist-spacer") as HTMLElement;
    expect(spacer.style.height).toBe("40000px");
  });

  it("滚动后窗口跟着位移：新窗口渲染、旧窗口从 DOM 摘除", () => {
    render(list(items(2000)));
    const root = screen.getByRole("list", { name: "条目" });
    fireEvent.scroll(root, { target: { scrollTop: 1000 } });
    // 1000 / 20 = 50，减去 overscan 3 → 从第 47 条起
    expect(screen.getByText("第 47 条")).toBeInTheDocument();
    expect(screen.queryByText("第 0 条")).toBeNull();
    expect(screen.queryByText("第 1999 条")).toBeNull();
  });

  it("滚到底部能看到最后一条（大列表末页不被裁掉）", () => {
    render(list(items(2000)));
    const root = screen.getByRole("list", { name: "条目" });
    fireEvent.scroll(root, { target: { scrollTop: 2000 * 20 } });
    expect(screen.getByText("第 1999 条")).toBeInTheDocument();
    expect(screen.queryByText("第 0 条")).toBeNull();
  });

  it("空列表不渲染任何行，占位高度为 0", () => {
    render(list([]));
    const root = screen.getByRole("list", { name: "条目" });
    expect(root.querySelectorAll('[role="listitem"]').length).toBe(0);
    expect((root.querySelector(".vlist-spacer") as HTMLElement).style.height).toBe("0px");
  });

  it("行序号与集合大小对辅助技术可见（aria-posinset/aria-setsize）", () => {
    render(list(items(2000)));
    const first = screen.getAllByRole("listitem")[0];
    expect(first).toHaveAttribute("aria-posinset", "1");
    expect(first).toHaveAttribute("aria-setsize", "2000");
    expect(screen.getByRole("list", { name: "条目" })).toHaveAttribute("aria-rowcount", "2000");
  });
});
