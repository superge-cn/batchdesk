// 免打包转译：把 src/**/*.{ts,tsx} 逐文件转成 ESM（esbuild 只做 loader，不 bundle），
// 输出到 mirror/，用 Node 原生模块解析运行——react/jsx-runtime 由 Node 直接 require
// （这条路在当前宿主上是好的；bundle 会被作用域问题打坏，见 git 历史）。
// `./lib/ipc` 的替换：转译完成后用 fake-ipc 的产物覆盖 mirror 里的 index.js。
import { build } from "../node_modules/.pnpm/esbuild@0.28.2/node_modules/esbuild/lib/main.js";
import { fileURLToPath } from "node:url";
import path from "node:path";
import fs from "node:fs";

const here = path.dirname(fileURLToPath(import.meta.url));
const webRoot = path.dirname(here);
const srcRoot = path.join(webRoot, "src");
const mirror = path.join(here, "mirror");
const fakeIpc = path.join(here, "fake-ipc.ts");

fs.rmSync(mirror, { recursive: true, force: true });

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

// 非代码资产原样镜像（i18n 的语言包 json 由运行期 import 断言加载）
function copyAssets(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) copyAssets(full);
    else if (entry.name.endsWith(".json")) {
      const rel = path.relative(srcRoot, full);
      const dest = path.join(mirror, "src", rel);
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      fs.copyFileSync(full, dest);
    }
  }
}
copyAssets(srcRoot);

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
  "entry-controls.tsx"
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
        if (spec.endsWith(".json")) return prefix + '"' + spec + '" with { type: "json" }';
        if (/\.(js|css)$/.test(spec)) return match;
        return prefix + '"' + spec + '.js"';
      }
    );
    if (fixed !== code) fs.writeFileSync(full, fixed);
  }
}
fixExtensions(mirror);

// 入口转译到 mirror 根，比原 .probe 深一层：`../src` 需要变成 `../../src`
for (const entry of entries) {
  const out = path.join(mirror, entry.replace(/\.tsx$/, ".js"));
  if (!fs.existsSync(out)) continue;
  const code = fs.readFileSync(out, "utf8");
  fs.writeFileSync(out, code.split('"../src/').join('"../../src/'));
}

fs.writeSync(1, "mirror ready: " + mirror + "\n");
