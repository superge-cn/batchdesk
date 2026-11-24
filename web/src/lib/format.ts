/**
 * 展示层的通用格式化（不是业务判定）。
 *
 * 为什么单独成文件：同一个「字节数→人话」的换算在任务面板（条目进度）与
 * Web 沙箱产物面板都要用；各写一份必然漂移（单位、小数位）。
 */

/** 字节数的人类可读形式（条目内进度/产物大小用，ADR-0100）。单位符号按 IEC 口径。 */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = "B";
  for (const candidate of units) {
    value /= 1024;
    unit = candidate;
    if (value < 1024) {
      break;
    }
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${unit}`;
}
