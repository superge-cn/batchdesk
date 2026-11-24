import { expect, test } from "@playwright/test";
import { crc32 } from "node:zlib";

/**
 * A3-16：真实浏览器里逐族冒烟真 WASM。
 *
 * 背景：A3-15 用一条真执行 e2e 抓到 `SystemTime` 在 wasm32 上 panic（28 个 plan 入口
 * 全线受影响）。这说明「native 绿 ＋ vitest 绿」证明不了 wasm32 运行时可用。
 * 本文件把真 WASM 铺到**成本最高的几个族**：归档编解码（zip＋aes）、目录端口、
 * 改名、清单解析——它们各自依赖不同的 wasm 路径，任何一条崩都说明该族不可用。
 *
 * 与 `browser-storage.spec.ts` 的分工：那边验「执行后落盘」这一条纵向链路；
 * 这里横向铺开，只断言「计划出得来、执行有产物」。
 */

/** 最小的 stored（不压缩）ZIP：够 zip 库按中央目录读条目即可。 */
function storedZip(files: { name: string; text: string }[]): Buffer {
  const locals: Buffer[] = [];
  const central: Buffer[] = [];
  let offset = 0;
  for (const file of files) {
    const name = Buffer.from(file.name, "utf8");
    const data = Buffer.from(file.text, "utf8");
    const checksum = crc32(data);

    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0x0800, 6); // UTF-8 名
    local.writeUInt16LE(0, 8); // stored
    local.writeUInt16LE(0, 10);
    local.writeUInt16LE(0, 12);
    local.writeUInt32LE(checksum, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(name.length, 26);
    local.writeUInt16LE(0, 28);
    locals.push(local, name, data);

    const entry = Buffer.alloc(46);
    entry.writeUInt32LE(0x02014b50, 0);
    entry.writeUInt16LE(20, 4);
    entry.writeUInt16LE(20, 6);
    entry.writeUInt16LE(0x0800, 8);
    entry.writeUInt16LE(0, 10);
    entry.writeUInt16LE(0, 12);
    entry.writeUInt16LE(0, 14);
    entry.writeUInt32LE(checksum, 16);
    entry.writeUInt32LE(data.length, 20);
    entry.writeUInt32LE(data.length, 24);
    entry.writeUInt16LE(name.length, 28);
    entry.writeUInt16LE(0, 30);
    entry.writeUInt16LE(0, 32);
    entry.writeUInt16LE(0, 34);
    entry.writeUInt16LE(0, 36);
    entry.writeUInt32LE(0, 38);
    entry.writeUInt32LE(offset, 42);
    central.push(entry, name);

    offset += local.length + name.length + data.length;
  }

  const directory = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(0, 4);
  end.writeUInt16LE(0, 6);
  end.writeUInt16LE(files.length, 8);
  end.writeUInt16LE(files.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  end.writeUInt16LE(0, 20);
  return Buffer.concat([...locals, directory, end]);
}

/**
 * 目录选择：`webkitdirectory` 才给的 `webkitRelativePath`，Playwright 的
 * `setInputFiles` 造不出来（它只给平铺文件名）。用 DataTransfer 手工构造再派发
 * change —— 走的是与真实选择器完全相同的 React 路径。
 */
async function pickDirectory(
  page: import("@playwright/test").Page,
  fieldId: string,
  files: { rel: string; text: string }[]
) {
  await page.evaluate(
    ({ fieldId, files }) => {
      const input = document.getElementById(fieldId) as HTMLInputElement | null;
      if (input === null) throw new Error(`no input #${fieldId}`);
      const transfer = new DataTransfer();
      for (const file of files) {
        const parts = file.rel.split("/");
        const created = new File([file.text], parts[parts.length - 1], {
          type: "text/plain"
        });
        Object.defineProperty(created, "webkitRelativePath", { value: file.rel });
        transfer.items.add(created);
      }
      input.files = transfer.files;
      input.dispatchEvent(new Event("change", { bubbles: true }));
    },
    { fieldId, files }
  );
}

test("C10 解压：ZIP 中央目录 → WASM 解析 → 沙箱执行 → 产物可见", async ({ page }) => {
  test.setTimeout(180_000);
  await page.goto("/");

  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /^ZIP 解压\/压缩\s*C10$/ }).click();

  await page.locator("#c10-archive").setInputFiles({
    name: "e2e.zip",
    mimeType: "application/zip",
    buffer: storedZip([
      { name: "docs/a.txt", text: "hello" },
      { name: "docs/sub/b.txt", text: "world" }
    ])
  });
  await page.locator("#c10-mode").fill("extract");
  await page.locator("#c10-outputDir").fill("/out");
  // 这两个是**必填**（placeholder 只是示例值，不是默认值）：不填走 E1001。
  await page.locator("#c10-maxTotalBytes").fill("104857600");
  await page.locator("#c10-maxRatio").fill("100");
  await page.getByRole("button", { name: /生成计划预览/ }).click();
  await expect(page.getByRole("heading", { name: "计划预览" })).toBeVisible();

  // 执行：`MemoryArchiveService` 在 wasm 里按条目解压——这条路径此前零覆盖。
  await page.getByRole("button", { name: /^执行$/ }).click();
  const panel = page.getByTestId("web-sandbox-result");
  await expect(panel).toBeVisible({ timeout: 60_000 });
  await expect(panel).toContainText("docs/a.txt");
  await expect(panel).toContainText("docs/sub/b.txt");
});

test("F10 目录清单：目录事实 → WASM 端口 → 沙箱执行 → CSV 产物", async ({ page }) => {
  test.setTimeout(180_000);
  await page.goto("/");

  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /^目录清单\s*F10$/ }).click();

  await pickDirectory(page, "f10-roots", [
    { rel: "素材/a.txt", text: "aaa" },
    { rel: "素材/sub/b.txt", text: "hello" }
  ]);
  await page.locator("#f10-outputDir").fill("/out");
  await page.getByRole("button", { name: /生成计划预览/ }).click();
  await expect(page.getByRole("heading", { name: "计划预览" })).toBeVisible();

  // 执行走 `stage_dirs` + `stage_listing` 两步登记，端口在 wasm 里写 BOM CSV。
  await page.getByRole("button", { name: /^执行$/ }).click();
  const panel = page.getByTestId("web-sandbox-result");
  await expect(panel).toBeVisible({ timeout: 60_000 });
  await expect(panel).toContainText("素材.csv");
});

test("F12 批量改名：目录第一层 → WASM rename → 产物是新名字", async ({ page }) => {
  test.setTimeout(180_000);
  await page.goto("/");

  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /^批量改名\s*F12$/ }).click();

  await pickDirectory(page, "f12-dir", [
    { rel: "素材/old.txt", text: "a" },
    { rel: "素材/sub/c.txt", text: "b" }
  ]);
  await page.locator("#f12-prefix").fill("new_");
  await page.locator("#f12-scope").fill("full");
  await page.getByRole("button", { name: /生成计划预览/ }).click();
  await expect(page.getByRole("heading", { name: "计划预览" })).toBeVisible();

  await page.getByRole("button", { name: /^执行$/ }).click();
  const panel = page.getByTestId("web-sandbox-result");
  await expect(panel).toBeVisible({ timeout: 60_000 });
  // 只改第一层：根层文件改名，子目录名也改名（目录条目的 rename 会搬走整棵子树）。
  await expect(panel).toContainText("new_old.txt");
});

test("F13 正则改名：目录第一层 → WASM plan_f13（regex）→ 产物是新名字", async ({ page }) => {
  test.setTimeout(180_000);
  await page.goto("/");

  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /^正则改名\s*F13$/ }).click();

  await pickDirectory(page, "f13-dir", [
    { rel: "素材/2026-a.txt", text: "a" },
    { rel: "素材/2026-b.txt", text: "b" }
  ]);
  // Playwright 的 `fill()` 直写 value，不经过按键解析——正则可以原样写。
  await page.locator("#f13-pattern").fill("^(\\d{4})-");
  // `$1_` 会被内核按「组名 1_」解析（E1002），后接下划线/字母时必须用 `${1}`。
  await page.locator("#f13-replace").fill("${1}_");
  await page.locator("#f13-scope").fill("stem");
  await page.getByRole("button", { name: /生成计划预览/ }).click();
  await expect(page.getByRole("heading", { name: "计划预览" })).toBeVisible();

  await page.getByRole("button", { name: /^执行$/ }).click();
  const panel = page.getByTestId("web-sandbox-result");
  await expect(panel).toBeVisible({ timeout: 60_000 });
  // 捕获组替换的结果：`2026-a.txt` → `2026_a.txt`（`scope=stem` 只动主名）。
  await expect(panel).toContainText("2026_a.txt");
});

test("C07 清单移动：字节清单 → WASM 解析 → 沙箱 move → 产物可见", async ({ page }) => {
  test.setTimeout(180_000);
  await page.goto("/");

  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /^清单移动\/复制\s*C07$/ }).click();

  // 清单是**字节型**入口（`parse_c07_manifest`，原始字节不过 `Blob.text()`）。
  await page.locator("#c07-manifest").setInputFiles({
    name: "manifest.csv",
    mimeType: "text/csv",
    // 用 String.fromCharCode(10) 拼行尾：写在这里的字面 `\n` 在工具链里容易被
    // 当成真实换行（本文件已经踩过一次），显式拼字符最稳。
    buffer: Buffer.from("a.jpg,photos" + String.fromCharCode(10), "utf8")
  });
  await pickDirectory(page, "c07-sourceRoot", [{ rel: "素材/a.jpg", text: "jpg" }]);
  await page.locator("#c07-targetRoot").fill("/dst");
  await page.locator("#c07-operation").fill("move");
  await page.getByRole("button", { name: /生成计划预览/ }).click();
  await expect(page.getByRole("heading", { name: "计划预览" })).toBeVisible();

  await page.getByRole("button", { name: /^执行$/ }).click();
  const panel = page.getByTestId("web-sandbox-result");
  await expect(panel).toBeVisible({ timeout: 60_000 });
  await expect(panel).toContainText("/dst/photos/a.jpg");
});

test("C11 CSV 去重：按表头名去重 → WASM csv-dedup → 产物可见", async ({ page }) => {
  test.setTimeout(180_000);
  await page.goto("/");

  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /^CSV 去重\s*C11$/ }).click();

  const newline = String.fromCharCode(10);
  await page.getByTestId("web-file-input").setInputFiles({
    name: "rows.csv",
    mimeType: "text/csv",
    buffer: Buffer.from(
      ["id,name", "1,a", "1,b", "2,c"].join(newline) + newline,
      "utf8"
    )
  });
  await page.locator("#c11-keyColumns").fill("id");
  await page.locator("#c11-outputDir").fill("/out");
  await page.locator("#c11-target").fill("utf-8");
  await page.getByRole("button", { name: /生成计划预览/ }).click();
  await expect(page.getByRole("heading", { name: "计划预览" })).toBeVisible();

  await page.getByRole("button", { name: /^执行$/ }).click();
  const panel = page.getByTestId("web-sandbox-result");
  await expect(panel).toBeVisible({ timeout: 60_000 });
  await expect(panel).toContainText("rows.csv");
});
