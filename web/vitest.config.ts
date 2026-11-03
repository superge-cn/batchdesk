import { defineConfig } from "vitest/config";

// 测试配置**故意不加载** `@vitejs/plugin-react`：
// 该插件在测试路径上唯一的作用是 React Fast Refresh，而 jsdom 里没有 HMR；
// 它的开发期机制会拉起 Node 内置 HTTP 客户端（undici），在地址空间受限的
// 容器里实例化 WASM 会失败并让整个 run 崩在启动阶段（`RangeError:
// WebAssembly.instantiate(): Out of memory`），与用例本身无关。
// JSX 交给 Vite 内置的 esbuild 自动运行时处理，产物与插件路径一致。
export default defineConfig({
  esbuild: { jsx: "automatic" },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    // 23 个导航项逐个点击断言，单条用例耗时超过默认 5 s
    testTimeout: 90000
  }
});
