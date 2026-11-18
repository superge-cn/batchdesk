import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  // Web 形态 WASM 产物是否就位（scripts/build-wasm.mjs 置 ENABLED）。
  // 缺失时前端按「能力不可用」降级——**不做任何判定**（设计 05 §0 第 2 条）。
  define: {
    "import.meta.env.VITE_BATCHDESK_WASM": JSON.stringify(
      process.env.VITE_BATCHDESK_WASM ?? "0"
    )
  },
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1"
  },
  clearScreen: false
});
