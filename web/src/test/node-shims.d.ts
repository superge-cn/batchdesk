/**
 * 测试专用的最小 Node 类型声明。
 *
 * web 侧没有引入 `@types/node`（产品代码不该碰 Node API），而读语料文件的测试
 * 需要磁盘访问——这里只声明实际用到的三个入口，而不是把整套 Node 类型放进来：
 * 一旦 `src/` 里出现别的 `node:` 导入，会立刻报"模块不存在"，评审就能看见。
 */
declare module "node:fs" {
  export function readFileSync(path: string): Uint8Array;
  export function existsSync(path: string): boolean;
}

declare module "node:path" {
  export function join(...parts: string[]): string;
  export function resolve(...parts: string[]): string;
}
