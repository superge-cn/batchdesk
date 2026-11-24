import { describe, expect, it } from "vitest";
import { asHandle, FakeDir } from "../../test/fakeOpfs";
import {
  clearPersisted,
  listPersisted,
  opfsSupported,
  persistProducts,
  readPersisted
} from "./opfsStore";

const blob = (text: string) => new Blob([text], { type: "text/plain" });

/** jsdom 的 Blob 没有 `text()`（真实浏览器有）；测试里用 FileReader 读。 */
function readText(target: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsText(target);
  });
}

describe("opfsStore", () => {
  it("落盘按 planId 分目录，文件名由调用方给（与下载同一套去重）", async () => {
    const root = new FakeDir();
    const outcome = await persistProducts(
      "01ABCDEFGHJKMNPQRSTVWXYZ0",
      [
        { name: "a.txt", blob: blob("aaa") },
        { name: "2-a.txt", blob: blob("bbb") }
      ],
      asHandle(root)
    );
    expect(outcome).toEqual({ saved: 2, failed: 0 });

    const app = root.dirs.get("batchdesk")!;
    const products = app.dirs.get("products")!;
    const planDir = products.dirs.get("01ABCDEFGHJKMNPQRSTVWXYZ0")!;
    expect([...planDir.files.keys()].sort()).toEqual(["2-a.txt", "a.txt"]);
    expect(await readText(await (await planDir.getFileHandle("a.txt")).getFile())).toBe("aaa");
  });

  it("同一 planId 重跑先清残留：不然界面说不清「这次留下了什么」", async () => {
    const root = new FakeDir();
    await persistProducts("01ABCDEFGHJKMNPQRSTVWXYZ0", [{ name: "old.txt", blob: blob("x") }], asHandle(root));
    await persistProducts("01ABCDEFGHJKMNPQRSTVWXYZ0", [{ name: "new.txt", blob: blob("y") }], asHandle(root));

    const planDir = root.dirs.get("batchdesk")!.dirs.get("products")!.dirs.get("01ABCDEFGHJKMNPQRSTVWXYZ0")!;
    expect([...planDir.files.keys()]).toEqual(["new.txt"]);
  });

  it("列出已落盘产物（planId／名字／大小）", async () => {
    const root = new FakeDir();
    await persistProducts("01ABCDEFGHJKMNPQRSTVWXYZ0", [{ name: "a.txt", blob: blob("12345") }], asHandle(root));
    await persistProducts("01ABCDEFGHJKMNPQRSTVWXYZ1", [{ name: "b.txt", blob: blob("123") }], asHandle(root));

    const listed = await listPersisted(asHandle(root));
    expect(listed).toHaveLength(2);
    expect(listed.find((item) => item.planId.endsWith("0"))).toEqual({
      planId: "01ABCDEFGHJKMNPQRSTVWXYZ0",
      name: "a.txt",
      size: 5
    });
  });

  it("读回：存在的给 Blob，不存在的给 null（不抛）", async () => {
    const root = new FakeDir();
    await persistProducts("01ABCDEFGHJKMNPQRSTVWXYZ0", [{ name: "a.txt", blob: blob("hello") }], asHandle(root));

    const found = await readPersisted("01ABCDEFGHJKMNPQRSTVWXYZ0", "a.txt", asHandle(root));
    expect(await readText(found!)).toBe("hello");
    expect(await readPersisted("01ABCDEFGHJKMNPQRSTVWXYZ0", "missing.txt", asHandle(root))).toBeNull();
    expect(await readPersisted("01ZZZZZZZZZZZZZZZZZZZZZZZZZ", "a.txt", asHandle(root))).toBeNull();
  });

  it("清理：整棵 products 目录移除；本来就没有也算成功", async () => {
    const root = new FakeDir();
    await persistProducts("01ABCDEFGHJKMNPQRSTVWXYZ0", [{ name: "a.txt", blob: blob("x") }], asHandle(root));
    expect(await clearPersisted(asHandle(root))).toBe(true);
    expect(await listPersisted(asHandle(root))).toEqual([]);
    // 第二次：目录已不在，语义上已是目标状态，仍然算成功。
    expect(await clearPersisted(asHandle(root))).toBe(true);
  });

  it("不支持 OPFS 时不抛：返回全失败，调用方退回「只下载」", async () => {
    // jsdom 里 `navigator.storage.getDirectory` 不存在——正是要覆盖的形态差异。
    expect(opfsSupported()).toBe(false);
    expect(
      await persistProducts("01ABCDEFGHJKMNPQRSTVWXYZ0", [{ name: "a.txt", blob: blob("x") }])
    ).toEqual({ saved: 0, failed: 1 });
  });

  it("没有产物时直接返回零，不去建目录", async () => {
    const root = new FakeDir();
    expect(await persistProducts("01ABCDEFGHJKMNPQRSTVWXYZ0", [], asHandle(root))).toEqual({
      saved: 0,
      failed: 0
    });
    expect(root.dirs.size).toBe(0);
  });

  it("单个文件写失败不影响其余文件（落盘是尽力而为）", async () => {
    const root = new FakeDir();
    const handle = asHandle(root);
    // 让第一个文件名拿到一个会抛的句柄：直接改 FakeDir 的原型方法代价太大，
    // 这里用非法名（含路径分隔符）触发真实实现的失败分支。
    const outcome = await persistProducts(
      "01ABCDEFGHJKMNPQRSTVWXYZ0",
      [
        { name: "sub/a.txt", blob: blob("bad") },
        { name: "b.txt", blob: blob("ok") }
      ],
      handle
    );
    expect(outcome).toEqual({ saved: 1, failed: 1 });
    const planDir = root.dirs.get("batchdesk")!.dirs.get("products")!.dirs.get("01ABCDEFGHJKMNPQRSTVWXYZ0")!;
    expect([...planDir.files.keys()]).toEqual(["b.txt"]);
  });
});
