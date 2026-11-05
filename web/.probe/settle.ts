// 探针共享的轮询等待：必须返回谓词值本身（而不是 true），
// 调用点才能拿到元素/对象——entry-nav 初版在这里栽过跟头（拿到 boolean）。
export const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export async function settle<T>(label: string, fn: () => T | undefined, timeoutMs = 5000): Promise<T | undefined> {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const value = fn();
    if (value) return value;
    await sleep(20);
  }
  console.warn("settle 超时：" + label);
  return undefined;
}
