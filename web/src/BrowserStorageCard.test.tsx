import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BrowserStorageCard } from "./BrowserStorageCard";
import { I18nProvider } from "./i18n";
import { installFakeOpfs } from "./test/fakeOpfs";

/**
 * 设置页「浏览器存储」卡片（A3-14）。
 *
 * A3-13 只做了写入与清除；这里补的是**读回闭环**：关掉页面之后还能不能
 * 把产物找回来。用例覆盖支持／不支持两种形态、空列表、列出、单件下载、整体清理。
 */

function renderCard() {
  return render(
    <I18nProvider initialLang="zh-CN" initialTheme="light">
      <BrowserStorageCard />
    </I18nProvider>
  );
}

/** jsdom 没有 `URL.createObjectURL`（真实浏览器有）；下载断言只看它收到什么。 */
let createdUrls: Blob[];

beforeEach(() => {
  createdUrls = [];
  Object.defineProperty(URL, "createObjectURL", {
    configurable: true,
    value: vi.fn((blob: Blob) => {
      createdUrls.push(blob);
      return "blob:fake";
    })
  });
  Object.defineProperty(URL, "revokeObjectURL", {
    configurable: true,
    value: vi.fn()
  });
  // jsdom 不支持真实导航：点 `<a download>` 会打印 "Not implemented: navigation"。
  // 下载行为由 `createdUrls` 断言，这里只把导航动作压掉，保持输出干净。
  vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => undefined);
});

describe("BrowserStorageCard", () => {
  it("浏览器不支持本地存储时如实说明，不显示空列表冒充「已经查过了」", async () => {
    // 不装 fake：jsdom 里 `navigator.storage.getDirectory` 不存在。
    renderCard();
    expect(await screen.findByTestId("browser-store-unsupported")).toBeVisible();
    expect(screen.queryByTestId("browser-store-list")).toBeNull();
  });

  it("支持但没有产物时显示空态", async () => {
    const { restore } = installFakeOpfs();
    try {
      renderCard();
      expect(await screen.findByTestId("browser-store-empty")).toBeVisible();
    } finally {
      restore();
    }
  });

  it("列出已保存产物：名字 + 大小，并可按单件下载（字节来自 OPFS）", async () => {
    const { root, restore } = installFakeOpfs();
    try {
      // 直接往 fake OPFS 里放一条：绕过执行流程，只测这张卡片。
      const app = await root.getDirectoryHandle("batchdesk", { create: true });
      const products = await app.getDirectoryHandle("products", { create: true });
      const plan = await products.getDirectoryHandle("01ABCDEFGHJKMNPQRSTVWXYZ0", { create: true });
      const handle = await plan.getFileHandle("result.txt", { create: true });
      const writable = await handle.createWritable();
      await writable.write(new Blob(["hello"]));
      await writable.close();

      renderCard();
      const list = await screen.findByTestId("browser-store-list");
      expect(list.textContent).toContain("result.txt");
      expect(list.textContent).toContain("5 B");

      const user = userEvent.setup();
      await user.click(screen.getByTestId("browser-store-download"));

      // 下载走的是与执行面板同一个保存器：拿到的是 OPFS 里那份字节。
      await waitFor(() => expect(createdUrls).toHaveLength(1));
      expect(createdUrls[0].size).toBe(5);
    } finally {
      restore();
    }
  });

  it("整体清理：列表清空，OPFS 里也真的没了", async () => {
    const { root, restore } = installFakeOpfs();
    try {
      const app = await root.getDirectoryHandle("batchdesk", { create: true });
      const products = await app.getDirectoryHandle("products", { create: true });
      const plan = await products.getDirectoryHandle("01ABCDEFGHJKMNPQRSTVWXYZ0", { create: true });
      const handle = await plan.getFileHandle("a.txt", { create: true });
      const writable = await handle.createWritable();
      await writable.write(new Blob(["x"]));
      await writable.close();

      renderCard();
      await screen.findByTestId("browser-store-list");

      const user = userEvent.setup();
      await user.click(screen.getByTestId("browser-store-clear"));

      expect(await screen.findByTestId("browser-store-empty")).toBeVisible();
      // 不是只改了界面状态：fake OPFS 里的 products 目录也没了。
      expect(app.dirs.has("products")).toBe(false);
    } finally {
      restore();
    }
  });
});
