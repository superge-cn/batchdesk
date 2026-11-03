import { describe, expect, it } from "vitest";

import { preferredDirectory, rememberDirectory } from "./uiState";

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
