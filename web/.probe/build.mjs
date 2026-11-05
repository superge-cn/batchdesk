// esbuild 在 pnpm 布局里没有顶层链接，直接指向实际包（原生二进制，不依赖 WASM）。
import { build } from "../node_modules/.pnpm/esbuild@0.28.2/node_modules/esbuild/lib/main.js";
import { fileURLToPath } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const webRoot = path.dirname(here);
const fakeIpc = path.join(here, "fake-ipc.ts");

// 把 `./lib/ipc` 的导入重定向到测试替身：只影响探针打包，不改仓库源码。
const redirectIpc = {
  name: "redirect-ipc",
  setup(b) {
    b.onResolve({ filter: /(^|\/)lib\/ipc$/ }, () => ({ path: fakeIpc }));
  }
};

// 输出 CJS + 全部裸包外置：让 Node 自己解析 react/lucide（CJS 与 ESM 都能混用），
// 避免把 CJS 包塞进 ESM bundle 后出现 "Dynamic require is not supported"。
await build({
  entryPoints: [path.join(here, "entry-settings.tsx"), path.join(here, "entry-app.tsx"), path.join(here, "entry-cancel.tsx"), path.join(here, "entry-errors.tsx")],
  outdir: here,
  outExtension: { ".js": ".cjs" },
  bundle: true,
  platform: "node",
  format: "cjs",
  jsx: "automatic",
  target: "node22",
  packages: "external",
  absWorkingDir: webRoot,
  plugins: [redirectIpc],
  logLevel: "info"
});
