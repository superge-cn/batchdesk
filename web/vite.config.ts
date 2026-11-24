import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import fs from "node:fs";

export default defineConfig({
  plugins: [react()],
  // Web 形态 WASM 产物是否就位。
  //
  // **单一来源＝`web/wasm/ENABLED` 文件本身**（scripts/build-wasm.mjs 生成）。
  // 此前读的是环境变量 `VITE_BATCHDESK_WASM`，而全仓没有任何地方在构建前设置它，
  // 于是「产物已就位」与「前端认为可用」是脱节的——本地跑 `pnpm dev` 永远降级。
  // 现在构建脚本写完产物后顺手导出该 env（见 build-wasm.mjs），开发时则由
  // `web/wasm/ENABLED` 的存在性决定，两条路都指向同一个事实。
  define: {
    "import.meta.env.VITE_BATCHDESK_WASM": JSON.stringify(
      process.env.VITE_BATCHDESK_WASM ??
        (fs.existsSync(`${__dirname}/wasm/ENABLED`) ? "1" : "0")
    )
  },
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1"
  },
  clearScreen: false
});
