# 0100. Web 形态 WASM 构建链与前端装载

- 日期：2026-11-18
- 状态：已落地（Wave 5 ④）
- 相关：设计 05《Web 强制力边界契约》§0/§3；前置 [ADR-0098](./0098-WASM导出面骨架与零强制力feature.md)、[ADR-0099](./0099-Web形态的平台规则承载.md)

## 背景

ADR-0098/0099 把 WASM 导出面与内核承载都备好了，但**产物没有生成链**，`web/` 也无装载入口。
本决策补齐「构建 → 装载 → 降级」三段。

## 决策

### 1. 构建链：`scripts/build-wasm.mjs`

```
cargo build -p batchdesk-wasm --target wasm32-unknown-unknown --features web-zero-force
  → wasm-bindgen --target web --out-dir web/wasm --out-name batchdesk_wasm
  → 置 web/wasm/ENABLED 标记
```

- 工具链：`wasm-bindgen-cli` **版本必须与 crate 依赖一致**（本机 0.2.130，用 `--version` 锁）；
- 产物落 `web/wasm/`，**不入库**（`.gitignore` 排除）——它们是派生物，且含平台相关字节；
- **路径坑**：`new URL(import.meta.url).pathname` 在 Windows 得到 `/E:/...`（无效 cwd，
  表现为 `spawnSync ENOENT`）——改用 `fileURLToPath`。

### 2. 前端装载：`web/src/lib/wasm/{contract.ts,load.ts}`

- `contract.ts`：`BatchDeskWasm` 接口（与 Rust 导出一一对应）、`KernelError`（`E<码位>` 解析）、
  `parsePlanResult`；
- `load.ts`：**懒加载**（动态 `import("../../../wasm/batchdesk_wasm.js")`）＋**缓存**；
  `VITE_BATCHDESK_WASM`（vite `define`，由构建脚本环境变量驱动）判断产物是否就位。

### 3. 降级语义（零强制力的正面表达）

产物缺失/加载失败时返回 `null`，**调用方按「能力不可用」处理**——
前端**不做任何判定**（设计 05 §0 第 2 条）。
配额/授权**不在前端契约里出现**：由服务端下发（设计 07），前端只渲染。

> 换言之：**Web 端"没有 WASM"不构成阻断**——这与「零强制力」一致；
> 它只是**能力降级**（设计 05 §1 的能力降级分支）。

## 验证

- `scripts/build-wasm.mjs` 产出 `web/wasm/{ENABLED,batchdesk_wasm.js,batchdesk_wasm_bg.wasm,*.d.ts}`；
- `web/src/lib/wasm/contract.test.ts` **5 项全过**（E 码位解析、非 E 形式、JSON 结果、
  能力事实形状恰为 `form/localMode/exportVersion`）；
- `pnpm test` 全绿；`pnpm build` 通过（5.51s）。

## 未做（下一批）

- **Worker 化**：当前是主线程懒加载；移到 Worker 需处理 `postMessage` 与
  `wasm-bindgen` 的 `--target web` 在 Worker 内的初始化（`init` 传 `self`），属独立一批；
- **服务端下发配额**：设计 07 的云端服务端尚不存在；前端只预留「渲染服务端结果」的位置；
- **其余功能接入**：f05/f07/c07… 按 ADR-0099 的承载方式逐个评估。
