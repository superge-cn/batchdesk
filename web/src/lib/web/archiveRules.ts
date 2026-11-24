/**
 * C08 归档规则文本 → 结构化条件（浏览器侧解析）。
 *
 * 为什么要在前端解析：桌面壳与 CLI 收的是**原始行文本**（`rules: Vec<String>`），
 * 解析在 Rust 侧（`plan_manifest.rs::parse_archive_condition`，
 * 与 CLI `parse_archive_condition` 同语义）。而 WASM 入口 `plan_c08` 收的是
 * **结构化条件**（`WebArchiveRule`），所以浏览器必须先做这一步。
 *
 * 与 Rust 侧同语义、同错误文案（`E1002`）：四种类型 `ext:`／`mtime:`／`size:`／`name:`，
 * 行格式 `<子目录>|<类型>:<参数>`。差异只在错误码前缀之后的措辞由本模块给，
 * 界面文案仍走码位映射（设计 06 §3），不把英文原样印给用户。
 *
 * 校验放在这里而不是丢给内核：`plan_c08` 的条件是结构化枚举，参数不合法时
 * serde 只会报「unknown variant」，用户看不出是哪一行、哪个字段错了。
 */

/** 线协议 `WebArchiveCondition`（`#[serde(tag = "kind", content = "value")]`）。 */
export type WebArchiveCondition =
  | { kind: "extensions"; value: string[] }
  | { kind: "modifiedDateRange"; value: { from?: string; to?: string } }
  | { kind: "sizeRange"; value: { min?: number; max?: number } }
  | { kind: "nameKeyword"; value: string };

export interface WebArchiveRule {
  subDir: string;
  condition: WebArchiveCondition;
}

/**
 * 解析失败。直接带内核错误码 `E1002`（参数不合法）：调用方原样上抛即可，
 * 界面按码位出文案（设计 06 §3），不把英文原样印给用户。
 *
 * 行号是**结构化字段**而不是拼进字符串：重抛时补行号，避免前缀叠两次
 * （`line 2: line 2: ...`）。
 */
export class ArchiveRuleError extends Error {
  readonly code = "E1002";
  /** 不带行号的原始原因。 */
  readonly reason: string;

  constructor(reason: string, line?: number) {
    super(line === undefined ? reason : `line ${line}: ${reason}`);
    this.name = "ArchiveRuleError";
    this.reason = reason;
  }
}

/**
 * 解析一行规则（`<子目录>|<类型>:<参数>`）。
 *
 * 只按**第一个** `|` 切分：子目录名里不该出现 `|`，而参数里出现 `|`（如
 * `name:a|b`）应当被当成参数内容——与 Rust 的 `split_once('|')` 一致。
 */
export function parseArchiveRule(line: string): WebArchiveRule {
  const text = line.trim();
  const split = text.indexOf("|");
  if (split < 0) {
    throw new ArchiveRuleError(
      `c08: rule must be <subdir>|<type>:<params>: ${text}`
    );
  }
  const subDir = text.slice(0, split).trim();
  const params = text.slice(split + 1).trim();
  return { subDir, condition: parseArchiveCondition(params) };
}

/**
 * 解析条件参数（`ext:jpg,png` 等）。
 *
 * 逐条对齐 `plan_manifest.rs::parse_archive_condition`：
 * - `ext:` 至少一个扩展名（去空白、丢空项），**不**补点号、不改大小写；
 * - `mtime:` 必须 `from..to`，两端可空（空＝不限），日期原样透传给内核比较；
 * - `size:` 必须 `min..max`，两端可空，非空必须是字节数（非负整数）；
 * - `name:` 关键字非空。
 */
export function parseArchiveCondition(params: string): WebArchiveCondition {
  if (params.startsWith("ext:")) {
    const extensions = params
      .slice("ext:".length)
      .split(",")
      .map((extension) => extension.trim())
      .filter((extension) => extension.length > 0);
    if (extensions.length === 0) {
      throw new ArchiveRuleError("ext rule requires at least one extension");
    }
    return { kind: "extensions", value: extensions };
  }
  if (params.startsWith("mtime:")) {
    const range = params.slice("mtime:".length);
    const bounds = splitRange(range, "mtime");
    return {
      kind: "modifiedDateRange",
      value: {
        ...(bounds.from !== undefined ? { from: bounds.from } : {}),
        ...(bounds.to !== undefined ? { to: bounds.to } : {})
      }
    };
  }
  if (params.startsWith("size:")) {
    const range = params.slice("size:".length);
    const bounds = splitRange(range, "size");
    return {
      kind: "sizeRange",
      value: {
        ...(bounds.from !== undefined ? { min: parseBytes(bounds.from) } : {}),
        ...(bounds.to !== undefined ? { max: parseBytes(bounds.to) } : {})
      }
    };
  }
  if (params.startsWith("name:")) {
    const keyword = params.slice("name:".length);
    if (keyword.length === 0) {
      throw new ArchiveRuleError("name rule requires a keyword");
    }
    return { kind: "nameKeyword", value: keyword };
  }
  throw new ArchiveRuleError(
    `unknown rule type (expected ext:/mtime:/size:/name/): ${params}`
  );
}

/** `from..to`；两端空串视为「不限」（返回 `undefined`，线协议里就不出现该字段）。 */
function splitRange(
  range: string,
  kind: "mtime" | "size"
): { from?: string; to?: string } {
  const at = range.indexOf("..");
  if (at < 0) {
    throw new ArchiveRuleError(`${kind} rule requires from..to: ${range}`);
  }
  const from = range.slice(0, at).trim();
  const to = range.slice(at + 2).trim();
  return {
    ...(from.length > 0 ? { from } : {}),
    ...(to.length > 0 ? { to } : {})
  };
}

function parseBytes(text: string): number {
  // 只接受纯十进制：`Number("1e6")` 会放过科学计数法，与 Rust 的 `parse::<u64>()` 不同口径。
  if (!/^\d+$/.test(text)) {
    throw new ArchiveRuleError(`size bound must be a byte count: ${text}`);
  }
  const value = Number(text);
  if (!Number.isSafeInteger(value)) {
    throw new ArchiveRuleError(`size bound is out of range: ${text}`);
  }
  return value;
}

/**
 * 解析整段规则文本（每行一条）。空行跳过——文本域末尾的换行不该报错。
 *
 * 行号写进错误信息：规则多的时候「哪一行错了」比「什么错了」更重要。
 */
export function parseArchiveRules(text: string): WebArchiveRule[] {
  const rules: WebArchiveRule[] = [];
  const lines = text.split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.trim().length === 0) continue;
    try {
      rules.push(parseArchiveRule(line));
    } catch (cause) {
      if (cause instanceof ArchiveRuleError) {
        throw new ArchiveRuleError(cause.reason, index + 1);
      }
      throw cause;
    }
  }
  return rules;
}
