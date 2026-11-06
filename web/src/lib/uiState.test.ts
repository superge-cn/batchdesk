import { describe, expect, it } from "vitest";

import {
  FEATURE_SECTION,
  clampPixels,
  preferredDirectory,
  readPixelVar,
  rememberColumnWidth,
  rememberDirectory,
  rememberSidebarWidth,
  rememberSort,
  rememberView,
  restoreView,
  sortDirectionOf,
  sortRows
} from "./uiState";

// 设计 03 §11.2/§11.3：UiState 的 navigation 段只用于"上次目录 + 最近目录"。
// 这里钉住三条：去重置顶、兄弟字段不被擦掉（patch 是顶层浅合并）、空输入不写脏值。

describe("最近目录（设计 03 §11.2）", () => {
  it("新目录置顶，重复目录被提到最前而不是出现两次", () => {
    let navigation = rememberDirectory(undefined, "/data/a");
    navigation = rememberDirectory(navigation, "/data/b");
    navigation = rememberDirectory(navigation, "/data/a");

    expect(navigation.lastDirectory).toBe("/data/a");
    expect(navigation.recentDirectories).toEqual(["/data/a", "/data/b"]);
  });

  it("只动 lastDirectory 与 recentDirectories，兄弟字段原样保留", () => {
    const navigation = rememberDirectory(
      {
        lastSection: "f12",
        lastFeature: "f12",
        lastDirectory: "/old",
        recentDirectories: ["/old"]
      },
      "/new"
    );

    expect(navigation.lastSection).toBe("f12");
    expect(navigation.lastFeature).toBe("f12");
    expect(navigation.lastDirectory).toBe("/new");
    expect(navigation.recentDirectories).toEqual(["/new", "/old"]);
  });

  it("空目录不写入，也不把已有记录弄坏", () => {
    const before = { lastDirectory: "/keep", recentDirectories: ["/keep"] };
    const after = rememberDirectory(before, "   ");
    expect(after.lastDirectory).toBe("/keep");
    expect(after.recentDirectories).toEqual(["/keep"]);
  });

  it("预填只看 lastDirectory：没有记录就返回 null（不阻塞启动）", () => {
    expect(preferredDirectory(undefined)).toBeNull();
    expect(preferredDirectory({})).toBeNull();
    expect(preferredDirectory({ lastDirectory: "  " })).toBeNull();
    expect(preferredDirectory({ lastDirectory: "/data/a" })).toBe("/data/a");
  });
});

describe("上次页面（设计 03 §11.2 的 lastSection/lastFeature）", () => {
  it("功能页记成 section=feature ＋ feature=<id>；离开时不清 feature", () => {
    let navigation = rememberView(undefined, FEATURE_SECTION, "f12");
    expect(navigation.lastSection).toBe("feature");
    expect(navigation.lastFeature).toBe("f12");

    navigation = rememberView(navigation, "c09");
    expect(navigation.lastSection).toBe("c09");
    // 离开功能页不擦 lastFeature：恢复规则只在 section=feature 时才看它。
    expect(navigation.lastFeature).toBe("f12");
  });

  it("恢复：section=feature 且功能仍在导航表里才回功能页", () => {
    expect(
      restoreView({ lastSection: "feature", lastFeature: "f12" }, ["f12"], ["c09", "tasks", "settings"])
    ).toBe("f12");
  });

  it("恢复：功能已下线就回默认，不把用户带到空白页", () => {
    expect(
      restoreView({ lastSection: "feature", lastFeature: "f99" }, ["f12"], ["c09", "tasks", "settings"])
    ).toBeNull();
    expect(
      restoreView({ lastSection: "feature" }, ["f12"], ["c09", "tasks", "settings"])
    ).toBeNull();
  });

  it("恢复：顶层视图直接认 lastSection；无记录或未知值返回 null", () => {
    expect(restoreView({ lastSection: "settings" }, [], ["c09", "tasks", "settings"])).toBe(
      "settings"
    );
    expect(restoreView(undefined, ["f12"], ["c09"])).toBeNull();
    expect(restoreView({ lastSection: "nope" }, ["f12"], ["c09"])).toBeNull();
  });
});

// 设计 03 §11.2 的 layout/tables 与 §11.3 的 clamp：这两段的"能改"全靠界面上的把手，
// 把手的行为先钉在这里（纯函数层），组件层见 App.uiHandles.test.tsx。

describe("侧栏宽度与 clamp（设计 03 §11.2/§11.3）", () => {
  it("clampPixels 越界贴边，非数值退回下限", () => {
    expect(clampPixels(240, 176, 384)).toBe(240);
    expect(clampPixels(80, 176, 384)).toBe(176);
    expect(clampPixels(4000, 176, 384)).toBe(384);
    expect(clampPixels(Number.NaN, 176, 384)).toBe(176);
  });

  it("readPixelVar 只认 px 形态的值，其它一律当作没定义", () => {
    const styles = (value: string) =>
      ({ getPropertyValue: () => value }) as unknown as CSSStyleDeclaration;
    expect(readPixelVar(styles(" 88px "), "--table-column-min")).toBe(88);
    expect(readPixelVar(styles("2rem"), "--table-column-min")).toBeNull();
    expect(readPixelVar(styles(""), "--table-column-min")).toBeNull();
  });

  it("rememberSidebarWidth 只动 sidebarWidth，兄弟字段原样保留", () => {
    const layout = rememberSidebarWidth({ sidebarCollapsed: true }, 280);
    expect(layout).toEqual({ sidebarCollapsed: true, sidebarWidth: 280 });
  });
});

describe("列宽与排序（设计 03 §11.2 的 tables）", () => {
  it("列宽按表按列合并，别的表与别的列不被擦掉", () => {
    const first = rememberColumnWidth(undefined, "table.c09.plan", "source", 300);
    const second = rememberColumnWidth(first, "table.c09.plan", "status", 96);
    const third = rememberColumnWidth(second, "table.feature.f12", "source", 220);

    expect(third["table.c09.plan"]?.columnWidths).toEqual({ source: 300, status: 96 });
    expect(third["table.feature.f12"]?.columnWidths).toEqual({ source: 220 });
  });

  it("同列翻转方向、换列回升序，且只留最后一次排序", () => {
    let tables = rememberSort(undefined, "table.c09.plan", "source");
    expect(tables["table.c09.plan"]?.sort).toEqual({ column: "source", direction: "asc" });
    tables = rememberSort(tables, "table.c09.plan", "source");
    expect(tables["table.c09.plan"]?.sort).toEqual({ column: "source", direction: "desc" });
    tables = rememberSort(tables, "table.c09.plan", "reason");
    expect(tables["table.c09.plan"]?.sort).toEqual({ column: "reason", direction: "asc" });
    expect(sortDirectionOf(tables, "table.c09.plan", "source")).toBeNull();
    expect(sortDirectionOf(tables, "table.c09.plan", "reason")).toBe("asc");
  });

  it("换表写入排序，前一张表的列宽仍在", () => {
    const widths = rememberColumnWidth(undefined, "table.c09.plan", "source", 300);
    const sorted = rememberSort(widths, "table.c09.result", "status");
    expect(sorted["table.c09.plan"]?.columnWidths).toEqual({ source: 300 });
    expect(sorted["table.c09.result"]?.sort).toEqual({ column: "status", direction: "asc" });
  });

  it("没有排序记录时原样返回：显示序就是计划序", () => {
    const rows = [{ name: "b" }, { name: "a" }];
    expect(sortRows(rows, undefined, (row) => row.name)).toEqual(rows);
  });

  it("升序/降序互为反序，同值保持计划序（sort 是稳定的）", () => {
    const rows = [
      { entryId: "e1", reason: "duplicate" },
      { entryId: "e2", reason: "older" },
      { entryId: "e3", reason: "duplicate" }
    ];
    const key = (row: { entryId: string; reason: string }, column: string) =>
      column === "reason" ? row.reason : row.entryId;

    expect(sortRows(rows, { column: "reason", direction: "asc" }, key).map((r) => r.entryId)).toEqual(
      ["e1", "e3", "e2"]
    );
    expect(sortRows(rows, { column: "reason", direction: "desc" }, key).map((r) => r.entryId)).toEqual(
      ["e2", "e1", "e3"]
    );
  });

  it("取不到值的行排在最后，翻转方向也不把它推到前面", () => {
    const rows = [{ entryId: "e1" }, { entryId: "e2", target: undefined }, { entryId: "e3", target: "z" }];
    const key = (row: { entryId: string; target?: string }, column: string) =>
      column === "target" ? row.target : row.entryId;

    expect(
      sortRows(rows, { column: "target", direction: "asc" }, key).map((row) => row.entryId)
    ).toEqual(["e3", "e1", "e2"]);
    expect(
      sortRows(rows, { column: "target", direction: "desc" }, key).map((row) => row.entryId)
    ).toEqual(["e3", "e1", "e2"]);
  });
});
