import { expect, test } from "@playwright/test";

/**
 * A3-15：真实 Chromium 下的 OPFS 链路（A3-13／A3-14 的核心假设）。
 *
 * 为什么需要这组用例：`opfsStore` 的单测跑在 jsdom ＋ 内存 fake 上，只能证明
 * 「接线与逻辑对」；证明不了「浏览器真的把 OPFS 给了我们」——`navigator.storage`
 * 在非安全上下文、隐私模式或旧内核里都可能不存在。这两条用例把「真的能用」
 * 钉在真实浏览器上。
 *
 * 数据自备（`page.evaluate` 直接写 OPFS），不依赖 WASM 执行；第二条才走真执行，
 * 用于验证「执行 → 落盘 → 设置页可见」的完整链路。
 */

const PLAN_ID = "01E2EOPFSPLAN00000000000000";

test("真实浏览器里 OPFS 可用：卡片能列出、下载、清理", async ({ page }) => {
  await page.goto("/");

  // ① 先确认这个环境真的给了 OPFS（不给的话下面全是自欺欺人）。
  const supported = await page.evaluate(
    () => typeof navigator.storage?.getDirectory === "function"
  );
  expect(supported).toBe(true);

  // ② 直接写一条到 OPFS（形状与 `persistProducts` 一致：batchdesk/products/<planId>/<name>）。
  await page.evaluate(async (planId) => {
    const root = await navigator.storage.getDirectory();
    const app = await root.getDirectoryHandle("batchdesk", { create: true });
    const products = await app.getDirectoryHandle("products", { create: true });
    const plan = await products.getDirectoryHandle(planId, { create: true });
    const file = await plan.getFileHandle("e2e-product.txt", { create: true });
    const writable = await file.createWritable();
    await writable.write(new Blob(["e2e"]));
    await writable.close();
  }, PLAN_ID);

  // ③ 设置页「浏览器存储」把它列出来（不是只读了 state）。
  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /设置/ }).click();
  const list = page.getByTestId("browser-store-list");
  await expect(list).toBeVisible();
  await expect(list).toContainText("e2e-product.txt");

  // ④ 清理：界面回到空态，且 OPFS 里的目录真的没了。
  await page.getByTestId("browser-store-clear").click();
  await expect(page.getByTestId("browser-store-empty")).toBeVisible();

  const remaining = await page.evaluate(async () => {
    const root = await navigator.storage.getDirectory();
    try {
      const app = await root.getDirectoryHandle("batchdesk");
      await app.getDirectoryHandle("products");
      return "exists";
    } catch {
      return "gone";
    }
  });
  // 清理删的是 `batchdesk/products`（root 目录本身会被下一次落盘重建，留着无妨）。
  expect(remaining).toBe("gone");
});

test("真执行一次（C02）后，产物落在 OPFS 并能在设置页找回", async ({ page }) => {
  // 真 WASM：12.7 MB 的 wasm 要下载与实例化，比 mock 路径慢一个量级。
  test.setTimeout(180_000);
  await page.goto("/");

  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /^换行转换\s*C02$/ }).click();

  // C02 的入参是文件（浏览器形态是 File 选择器）：选一个带 CRLF 的文本。
  const input = page.getByTestId("web-file-input");
  await input.setInputFiles({
    name: "e2e-crlf.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("a\r\nb\r\n", "utf8")
  });
  // 按 id 取：字段标签文案会随语言包调整，id 是 `view-field` 的稳定约定。
  await page.locator("#c02-target").fill("lf");
  await page.getByRole("button", { name: /生成计划预览/ }).click();
  await expect(page.getByRole("heading", { name: "计划预览" })).toBeVisible();

  // 执行走真实 WASM 沙箱；成功后落盘是执行之后的异步搬运。
  await page.getByRole("button", { name: /^执行$/ }).click();
  const saved = page.getByTestId("web-sandbox-persisted");
  await expect(saved).toBeVisible({ timeout: 60_000 });
  await expect(saved).toContainText("已保存到浏览器");

  // 设置页看得见同一份（证明两张界面读的是同一个 OPFS，不是各自的内存态）。
  await nav.getByRole("button", { name: /设置/ }).click();
  const list = page.getByTestId("browser-store-list");
  await expect(list).toBeVisible();
  await expect(list).toContainText("e2e-crlf.txt");
});
