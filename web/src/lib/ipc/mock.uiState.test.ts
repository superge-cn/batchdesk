import { beforeEach, describe, expect, it } from "vitest";

import { mockGetUiState, mockResetUiState, mockSetUiState } from "./mock";

// 浏览器 mock 必须与壳层 `UiState::apply_patch` 同口径，否则 Playwright 与探针跑的是
// 一条壳层不存在的链路：写进去的分节读回来必须还在（本批补的 layout/tables 透传）。

beforeEach(() => {
  mockResetUiState();
});

describe("mock 的 UiState 透传（设计 03 §11）", () => {
  it("layout 与 tables 写入后能原样读回", () => {
    mockSetUiState({
      layout: { sidebarWidth: 280 },
      tables: { "table.c09.plan": { columnWidths: { source: 300 }, sort: { column: "source", direction: "asc" } } }
    });

    const state = mockGetUiState();
    expect(state.layout).toEqual({ sidebarWidth: 280 });
    expect(state.tables?.["table.c09.plan"]).toEqual({
      columnWidths: { source: 300 },
      sort: { column: "source", direction: "asc" }
    });
  });

  it("读改写之间不丢兄弟分节：写 tables 不会擦掉 navigation", () => {
    mockSetUiState({ navigation: { lastSection: "c09", recentDirectories: ["/data"] } });
    mockSetUiState({ tables: { "table.feature.f12": { columnWidths: { source: 240 } } } });

    const state = mockGetUiState();
    expect(state.navigation?.recentDirectories).toEqual(["/data"]);
    expect(state.tables?.["table.feature.f12"]?.columnWidths).toEqual({ source: 240 });
  });

  it("返回的是副本：调用方改返回值不会污染壳层状态", () => {
    mockSetUiState({ layout: { sidebarWidth: 280 } });
    const leaked = mockGetUiState();
    delete (leaked as { layout?: unknown }).layout;

    expect(mockGetUiState().layout).toEqual({ sidebarWidth: 280 });
  });

  it("uiStateVersion 与 updatedAt 由壳层管，写入即 E1002", () => {
    expect(() => mockSetUiState({ uiStateVersion: 9 })).toThrow();
    expect(() => mockSetUiState({ updatedAt: "x" })).toThrow();
  });

  it("重置只清界面状态，回到没有 layout/tables 的默认", () => {
    mockSetUiState({
      layout: { sidebarWidth: 280 },
      tables: { "table.c09.plan": { columnWidths: { source: 300 } } }
    });

    const reset = mockResetUiState();
    expect(reset.layout).toBeUndefined();
    expect(reset.tables).toBeUndefined();
    expect(mockGetUiState().tables).toBeUndefined();
  });
});
