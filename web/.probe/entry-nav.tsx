// 导航可达（ADR-0040…0044 的回归）行为探针：对应 web/src/App.navigation.test.tsx 的 6 条断言。
// 历史缺陷是"App import 了 FeaturePages 却从未渲染它"——导航只换标题、主区空白，
// tsc 与 cargo 都发现不了，所以每条断言都盯"点开之后主区真的有内容"。
import { act } from "react";
import { createRoot } from "react-dom/client";
import App from "../src/App";
import { resetState } from "./fake-ipc";

const results: Array<[string, boolean, string]> = [];
function check(name: string, ok: boolean, detail = "") {
  results.push([name, ok, detail]);
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function settle<T>(label: string, fn: () => T | undefined, timeoutMs = 5000): Promise<T | undefined> {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const value = fn();
    if (value) return value;
    await act(async () => {
      await sleep(20);
    });
  }
  console.warn("settle 超时：" + label);
  return false;
}

const VIEWS: Array<[string, string]> = [
  ["CSV 分割", "CSV 分割（F05）"],
  ["CSV 合并", "CSV 合并（F06）"],
  ["CSV 抽取列", "CSV 抽取指定列（F07）"],
  ["CSV 前后 N 行", "CSV 抽取前/后 N 行（F08）"],
  ["CSV 行区间", "CSV 抽取位置行区间（F09）"],
  ["目录清单", "生成目录清单（F10）"],
  ["清单建档", "清单建档（F11）"],
  ["批量改名", "批量改名（F12）"],
  ["TXT 按字节分割", "TXT 按字节分割（F01）"],
  ["TXT 按行数分割", "TXT 按行数分割（F02）"],
  ["TXT 按份数分割", "TXT 按份数分割（F03）"],
  ["TXT 合并", "TXT 合并（F04）"],
  ["编码转换", "编码转换（C01）"],
  ["换行转换", "换行符转换（C02）"],
  ["查找替换", "查找替换（C03）"],
  ["名称查找替换", "名称查找替换（C04）"],
  ["清单移动/复制", "清单驱动移动/复制（C07）"],
  ["按规则归档", "按规则归档（C08）"],
  ["ZIP 解压/压缩", "ZIP 解压/压缩（C10）"],
  ["CSV 去重", "CSV 去重（C11）"],
  ["CSV 过滤", "CSV 过滤（C12）"],
  ["CSV 排序", "CSV 排序（C13）"],
  ["CSV 列拆分/合并", "CSV 列拆分/合并（C14）"]
];

let mainEl: HTMLElement | null = null;
let navEl: HTMLElement | null = null;

function queryAll(root: ParentNode, selector: string): HTMLElement[] {
  return Array.from(root.querySelectorAll(selector));
}

function norm(text: string) {
  return text.replace(/\s+/g, " ").trim();
}

function navButton(label: string, code: string): HTMLElement | undefined {
  const matches = queryAll(navEl ?? document, "button").filter((button) => {
    const text = norm(button.textContent ?? "");
    return text.startsWith(label) && text.endsWith(code);
  });
  return matches.length === 1 ? matches[0] : undefined;
}

function mainButtons(predicate: (text: string) => boolean): HTMLElement[] {
  return queryAll(mainEl ?? document, "button").filter((b) => predicate(norm(b.textContent ?? "")));
}

function hasHeading(name: string): boolean {
  return queryAll(mainEl ?? document, "h1,h2,h3,h4").some(
    (h) => norm(h.textContent ?? "") === name
  );
}

async function clickNav(label: string, code: string) {
  const button = navButton(label, code);
  if (!button) {
    check(`导航项「${label} ${code}」存在且唯一`, false, "找不到或重复");
    return false;
  }
  await act(async () => {
    button.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  });
  return true;
}

async function main() {
  resetState();
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<App />);
  });

  navEl = (await settle("导航区渲染", () => {
    const nav = document.querySelector('nav[aria-label="功能导航"]');
    return nav ?? undefined;
  }) as unknown) as HTMLElement | null;
  mainEl = document.querySelector('main');

  // ① 默认视图是 C09 工作台
  check("默认视图是 C09 工作台", (mainEl?.textContent ?? "").includes("重复文件查找"));

  // ② 23 个功能视图逐个可达：标题 + 生成计划预览 + 撤销
  let titlesOk = 0;
  let planBtnOk = 0;
  let undoBtnOk = 0;
  const failures: string[] = [];
  for (const [label, title] of VIEWS) {
    const code = title.match(/[（(]([^)）]+)[)）]/)?.[1] ?? "";
    if (!(await clickNav(label, code))) {
      failures.push(label + "（导航项缺失）");
      continue;
    }
    await settle("标题渲染: " + title, () => (mainEl?.textContent ?? "").includes(title) ? true : undefined);
    const hasTitle = (mainEl?.textContent ?? "").includes(title);
    const hasPlan = mainButtons((text) => text.includes("生成计划预览")).length > 0;
    const hasUndo = mainButtons((text) => text === "撤销").length > 0;
    if (hasTitle) titlesOk += 1;
    if (hasPlan) planBtnOk += 1;
    if (hasUndo) undoBtnOk += 1;
    if (!hasTitle || !hasPlan || !hasUndo) failures.push(`${title}(标题${hasTitle}/计划${hasPlan}/撤销${hasUndo})`);
    require("node:fs").writeSync(
      1,
      `  [${titlesOk + 1}/${VIEWS.length}] ${title} 标题${hasTitle}/计划${hasPlan}/撤销${hasUndo}\n`
    );
    if (global.gc) global.gc();
  }
  check("23 个功能视图标题都渲染", titlesOk === VIEWS.length, `${titlesOk}/${VIEWS.length} ${failures.join("；")}`.slice(0, 200));
  check("每个功能视图都有「生成计划预览」", planBtnOk === VIEWS.length, `${planBtnOk}/${VIEWS.length}`);
  check("每个功能视图都有「撤销」", undoBtnOk === VIEWS.length, `${undoBtnOk}/${VIEWS.length}`);

  // ③ 能回到 C09
  {
    if (await clickNav("CSV 合并", "F06")) {
      await settle("F06 渲染", () => (mainEl?.textContent ?? "").includes("CSV 合并（F06）") ? true : undefined);
    }
    const back = mainButtons((text) => text.includes("返回 C09"))[0];
    if (back) await act(async () => back.dispatchEvent(new window.MouseEvent("click", { bubbles: true })));
    const backOk = await settle("回到 C09", () => (mainEl?.textContent ?? "").includes("重复文件查找") ? true : undefined);
    check("功能页可经「返回 C09」回到工作台", !!backOk);
  }

  // ④ 任务与撤销：真视图而非占位
  {
    const entry = queryAll(navEl ?? document, "button").find((b) => norm(b.textContent ?? "").includes("任务与撤销"));
    check("「任务与撤销」侧栏项可用", !!entry && !(entry as HTMLButtonElement).disabled);
    if (entry) await act(async () => entry.dispatchEvent(new window.MouseEvent("click", { bubbles: true })));
    const heading = await settle("任务页标题", () => (hasHeading("任务与撤销") ? true : undefined));
    const emptyHint = await settle("空队列指引", () => (mainEl?.textContent ?? "").includes("还没有任务记录") ? true : undefined);
    check("任务页有标题", !!heading);
    check("空队列给出下一步指引", !!emptyHint);
    const back = mainButtons((text) => text.includes("返回 C09"))[0];
    if (back) await act(async () => back.dispatchEvent(new window.MouseEvent("click", { bubbles: true })));
    await settle("回到 C09", () => (mainEl?.textContent ?? "").includes("重复文件查找") ? true : undefined);
  }

  // ⑤ 设置：五组配置与语言选择
  {
    const entry = queryAll(navEl ?? document, "button").find((b) => norm(b.textContent ?? "").includes("设置"));
    check("「设置」侧栏项可用", !!entry && !(entry as HTMLButtonElement).disabled);
    if (entry) await act(async () => entry.dispatchEvent(new window.MouseEvent("click", { bubbles: true })));
    await settle("设置页渲染", () => (hasHeading("通用") ? true : undefined));
    let allHeadings = true;
    for (const title of ["通用", "处理", "性能", "安全", "日志"]) {
      if (!hasHeading(title)) allHeadings = false;
    }
    check("五个配置分组都在", allHeadings);
    const language = document.querySelector("select#general-language") as HTMLSelectElement | null;
    check("界面语言选择存在且为 zh-CN", !!language && language.value === "zh-CN", language ? language.value : "未找到 select#general-language");
    const back = mainButtons((text) => text.includes("返回 C09"))[0];
    if (back) await act(async () => back.dispatchEvent(new window.MouseEvent("click", { bubbles: true })));
    await settle("回到 C09", () => (mainEl?.textContent ?? "").includes("重复文件查找") ? true : undefined);
  }

  // ⑥ 处理模板：真渲染 + 列表非空 + 不给假「应用」按钮
  {
    const entry = queryAll(navEl ?? document, "button").find((b) => norm(b.textContent ?? "").includes("处理模板"));
    check("「处理模板」侧栏项可用", !!entry && !(entry as HTMLButtonElement).disabled);
    if (entry) await act(async () => entry.dispatchEvent(new window.MouseEvent("click", { bubbles: true })));
    const heading = await settle("模板页标题", () => (hasHeading("处理模板") ? true : undefined));
    check("模板页有标题（h2）", !!heading);
    const hasFixture = await settle("模板夹具出列表", () => (mainEl?.textContent ?? "").includes("CSV 分片（每片 5 万行）") ? true : undefined);
    check("模板列表非空（夹具那条在）", !!hasFixture);
    check("页面写明 OQ-36 未裁决", (mainEl?.textContent ?? "").includes("OQ-36"));
    const fakeApply = mainButtons((text) => text.startsWith("应用"));
    check("不提供点了没反应的「应用」按钮", fakeApply.length === 0, String(fakeApply.length));
    const back = mainButtons((text) => text.includes("返回 C09"))[0];
    if (back) await act(async () => back.dispatchEvent(new window.MouseEvent("click", { bubbles: true })));
    await settle("回到 C09", () => (mainEl?.textContent ?? "").includes("重复文件查找") ? true : undefined);
  }

  await act(async () => root.unmount());
  host.remove();

  let failed = 0;
  for (const [name, ok, detail] of results) {
    if (!ok) failed += 1;
    const suffix = !ok && detail ? "  (" + detail + ")" : "";
    console.log((ok ? "PASS  " : "FAIL  ") + name + suffix);
  }
  console.log("\n" + (results.length - failed) + "/" + results.length + " passed");
  process.exit(failed === 0 ? 0 : 1);
}

void main();
