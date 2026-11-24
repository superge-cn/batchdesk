/**
 * 表单值解析助手（Web 计划通道与功能页共用）。
 *
 * 存在理由：`FeaturePages` 把表单存成 `Record<string, string>`，而「表单串 →
 * 线协议值」的语法转换（切分、去空白、是/否、正整数）在**两条路径**上都要用：
 * 桌面 IPC 的 `planOperationFor` 与浏览器 WASM 的 `planWithFiles`。
 * 两处各写一份必然漂移（字段名已经漂过一次），因此抽到这里。
 *
 * 边界：只做语法转换与缺省形态判断，**不做业务默认值**——「缺省用 4096」这类
 * 语义留在各自的调用点，免得改一个默认值要动两层。
 */

/** 原样取字符串；空串视为未填（空串在表单里等价于「没输入」）。 */
export function filled(
  form: Record<string, string>,
  name: string
): string | undefined {
  const value = form[name];
  return value === undefined || value === "" ? undefined : value;
}

/** 去首尾空白后取字符串；空串视为未填。 */
export function trimmed(
  form: Record<string, string>,
  name: string
): string | undefined {
  const value = form[name]?.trim();
  return value === undefined || value === "" ? undefined : value;
}

/**
 * 取正整数。非数字、`0`、负数一律视为**未填**——这些值交给内核报 E1002，
 * 而不是在这里悄悄替换成 1（静默改参会让用户看到一份「按别的参数算的」计划）。
 */
export function positiveInt(
  form: Record<string, string>,
  name: string
): number | undefined {
  const raw = trimmed(form, name);
  if (raw === undefined) return undefined;
  const value = Number(raw);
  return Number.isFinite(value) && value > 0 ? value : undefined;
}

/** 取正数（允许小数，用于「解压膨胀比」这类比例值）。非数字、`0`、负数视为未填。 */
export function positiveNumber(
  form: Record<string, string>,
  name: string
): number | undefined {
  const raw = trimmed(form, name);
  if (raw === undefined) return undefined;
  const value = Number(raw);
  return Number.isFinite(value) && value > 0 ? value : undefined;
}

/** 取整数（允许 0，用于「第 0 行」这类边界值）。非数字视为未填。 */
export function nonNegativeInt(
  form: Record<string, string>,
  name: string
): number | undefined {
  const raw = trimmed(form, name);
  if (raw === undefined) return undefined;
  const value = Number(raw);
  return Number.isFinite(value) && value >= 0 ? value : undefined;
}

/** 按分隔符切成去空白的非空列表。 */
export function splitList(
  value: string | undefined,
  separator = ","
): string[] {
  return (value ?? "")
    .split(separator)
    .map((item) => item.trim())
    .filter(Boolean);
}

/** 切分并转成正整数列表（非法项丢弃——与列选择「只认正序号」同口径）。 */
export function splitNumbers(value: string | undefined): number[] {
  return splitList(value)
    .map((item) => Number(item))
    .filter((item) => Number.isFinite(item) && item > 0);
}

/** 按换行切成非空列表（路径清单、多行文本域）。 */
export function splitLines(value: string | undefined): string[] {
  return (value ?? "")
    .split(/\r?\n/)
    .map((item) => item.trim())
    .filter(Boolean);
}

/**
 * 「表头 有|无」输入框的取值判定——这是**输入约定**，不是界面文案
 * （界面文案在语言包里，用户可见的提示不走这里）。
 */
export function isNoHeader(value: string | undefined): boolean {
  const normalized = (value ?? "").trim().toLowerCase();
  return normalized === "无" || normalized === "no" || normalized === "false"; // i18n-exempt
}

/** 表单里的「是/开」取值：yes/true/1/是。 */
export function isAffirmative(value: string | undefined): boolean {
  const normalized = (value ?? "").trim().toLowerCase();
  return ["yes", "true", "1", "是"].includes(normalized); // i18n-exempt
}
