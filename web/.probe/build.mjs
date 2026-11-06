// 免打包转译：把 src/**/*.{ts,tsx} 逐文件转成 ESM（esbuild 只做 loader，不 bundle），
// 输出到 mirror/，用 Node 原生模块解析运行——react/jsx-runtime 由 Node 直接 require
// （这条路在当前宿主上是好的；bundle 会被作用域问题打坏，见 git 历史）。
// `./lib/ipc` 的替换：转译完成后用 fake-ipc 的产物覆盖 mirror 里的 index.js。
import { build } from "../node_modules/.pnpm/esbuild@0.28.2/node_modules/esbuild/lib/main.js";
import { fileURLToPath } from "node:url";
import path from "node:path";
import fs from "node:fs";
import { createRequire } from "node:module";

const here = path.dirname(fileURLToPath(import.meta.url));
const webRoot = path.dirname(here);
const srcRoot = path.join(webRoot, "src");
const mirror = path.join(here, "mirror");
const fakeIpc = path.join(here, "fake-ipc.ts");

// react 系具名导出垫片：Node 原生 ESM 对 react 的 CJS 具名导出探测会失败
// （"Named export 'createContext' not found"），改为经 default 取全量再逐名转出。
const INTEROP = new Map([
  ["react", "react-interop.mjs"],
  ["react-dom", "react-dom-interop.mjs"],
  ["react-dom/client", "react-dom-client-interop.mjs"],
  ["react/jsx-dev-runtime", "react-jsx-dev-runtime-interop.mjs"],
  ["react/jsx-runtime", "react-jsx-runtime-interop.mjs"]
]);
const requireFromWeb = createRequire(path.join(webRoot, "package.json"));
fs.mkdirSync(mirror, { recursive: true });
for (const [spec, file] of INTEROP) {
  const mod = requireFromWeb(spec);
  const lines = ['import mod from "' + spec + '";', "export default mod;"];
  for (const key of Object.keys(mod)) {
    if (/^[A-Za-z_$][\w$]*$/.test(key)) lines.push("export const " + key + " = mod[" + JSON.stringify(key) + "];");
  }
  fs.writeFileSync(path.join(mirror, file), lines.join("\n") + "\n");
}

// 不做递归删除：转译是纯覆盖写，陈旧镜像文件无害（删除类操作会被沙箱拦截）。
// 若 mirror 里出现已从 src 移除的陈旧模块，删除它即可。

function walk(dir) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full));
    else if (/\.(ts|tsx)$/.test(entry.name) && !/\.test\.(ts|tsx)$/.test(entry.name)) out.push(full);
  }
  return out;
}

const files = walk(srcRoot);

// 语言包不镜像拷贝（沙箱拒绝新建文件）：转译后把 i18n 的 locale 路径改指真实
// src/locales（mirror/src → ../../../src），由 Node 以 json 断言加载。

await build({
  entryPoints: files,
  outdir: path.join(mirror, "src"),
  outbase: srcRoot,
  bundle: false,
  platform: "node",
  format: "esm",
  jsx: "automatic",
  target: "node22",
  absWorkingDir: webRoot,
  logLevel: "silent"
});

await build({
  entryPoints: [fakeIpc],
  outfile: path.join(mirror, "src", "lib", "ipc", "index.js"),
  bundle: false,
  platform: "node",
  format: "esm",
  target: "node22",
  absWorkingDir: webRoot,
  logLevel: "silent"
});

const entries = [
  "entry-settings.tsx",
  "entry-app.tsx",
  "entry-cancel.tsx",
  "entry-errors.tsx",
  "entry-nav.tsx",
  "entry-warnings.tsx",
  "entry-i18n.tsx",
  "entry-tasks.tsx",
  "entry-controls.tsx",
    "fake-ipc.ts"
];
for (const entry of entries) {
  const src = path.join(here, entry);
  if (!fs.existsSync(src)) continue;
  await build({
    entryPoints: [src],
    outfile: path.join(mirror, "probe", entry.replace(/\.tsx$/, ".js")),
    bundle: false,
    platform: "node",
    format: "esm",
    jsx: "automatic",
    target: "node22",
    absWorkingDir: webRoot,
    logLevel: "silent"
  });
}

// ESM 解析需要显式扩展名：给相对导入补 .js（转译后的 import 语句仍是无扩展名）。
function fixExtensions(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      fixExtensions(full);
      continue;
    }
    if (!entry.name.endsWith(".js")) continue;
    const code = fs.readFileSync(full, "utf8");
    const fixed = code.replace(
      /(from\s+|import\(\s*)"(\.\.?\/[^"]+?)"/g,
      (match, prefix, spec) => {
        // react 系裸说明符 → 具名导出垫片（相对 importer 计算路径）
        const interop = INTEROP.get(spec);
        if (interop) {
          let rel = path.relative(path.dirname(full), path.join(mirror, interop)).split(path.sep).join("/");
          if (!rel.startsWith(".")) rel = "./" + rel;
          return prefix + '"' + rel + '"';
        }
        // 语言包指回真实 src/locales（mirror 不拷 json；mirror/src 深三层到 web）
        if (spec.endsWith(".json") && spec.startsWith("./locales/")) {
          return (
            prefix +
            '"../../../src/locales/' +
            spec.slice("./locales/".length) +
            '" with { type: "json" }'
          );
        }
        if (spec.endsWith(".json")) return prefix + '"' + spec + '" with { type: "json" }';
        if (/\.(js|css)$/.test(spec)) return match;
        // ESM 不做目录索引解析：./lib/ipc 优先解析为 ./lib/ipc/index.js（若存在）
        const base = path.resolve(path.dirname(full), spec);
        if (fs.existsSync(base + ".js")) return prefix + '"' + spec + '.js"';
        if (fs.existsSync(path.join(base, "index.js"))) return prefix + '"' + spec + '/index.js"';
        return prefix + '"' + spec + '.js"';
      }
    );
    if (fixed !== code) fs.writeFileSync(full, fixed);
  }
}
fixExtensions(mirror);

// 入口转译到 mirror/probe/：`../src` 对 mirror/src 的相对深度与原布局一致，无需改写
for (const entry of entries) {
  const out = path.join(mirror, "probe", entry.replace(/\.tsx$/, ".js"));
  if (!fs.existsSync(out)) continue;
}

fs.writeSync(1, "mirror ready: " + mirror + "\n");
