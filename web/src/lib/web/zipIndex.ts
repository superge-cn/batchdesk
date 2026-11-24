/**
 * ZIP 中央目录读取（ADR-0103）：**只读元数据，不解压任何条目**。
 *
 * 为什么自己写：C10 的计划输入只需要中央目录里的十个字段（名/类型/大小/加密/
 * 压缩方法…），而 JS 解压库的入口都是「解出内容」（`unzipSync` 之类会把整包读进
 * 内存并解码全部条目）。计划阶段读几 MB 的中央目录就够了——为这点需求引一个
 * 体积是本模块十倍、且要过许可证评审的依赖不划算（AGENTS 不变量 9）。
 *
 * 与桌面 `platform::archive::ZipArchiveService::inspect` **逐字段同口径**（对着
 * `zip` crate 9.1.3 的实现核过，并用四个真实语料对拍）：
 * - `encrypted`：通用标志位 **bit 0**（`ZipFlags::Encrypted`，不是「有没有密码」的推断）；
 * - `supported`：**有效**压缩方法为 0(stored) / 8(deflate)。注意 AES 条目在中央目录
 *   写的是方法 **99**，真实方法在 AES 扩展字段（0x9901）第 6–7 字节——`zip` crate
 *   会把它还原（实测 AES 夹具的 `compression()` 是 `Stored`），本模块同样还原；
 * - `kind`：原始名**末字节**为 `/` 或 `\`（`zip::format::functions::is_dir`）；
 * - 名称：bit 11 置位 → 严格 UTF-8；否则全 ASCII 直读；否则 **GB18030 回退**；
 * - `size` / `compressedSize`：中央目录原值（含 Zip64 扩展字段 0x0001）。
 *
 * 唯一已知偏差：Rust 侧 GB18030 用 `Abort` 策略（解码失败 → `None`），浏览器
 * `TextDecoder` 是**有损**的（坏字节 → U+FFFD 且不报错）。本模块把「解出 U+FFFD」
 * 判为解码失败返回 `null`，与桌面同向（`None` 会被内核记入 `unsafe_names` 拦下，
 * 是保守方向）。见 ADR-0103 §偏差。
 */

/** 中央目录条目事实（对齐 WASM 线协议 `WebArchiveEntry`，camelCase）。 */
export interface ZipEntryFact {
  /** 中央目录顺序号（C10 的 `index` 字段，与桌面一致）。 */
  index: number;
  /** GBK/UTF-8 解码后的相对名；无法解码时为 `null`（内核会拦下并告警）。 */
  name: string | null;
  kind: "file" | "dir";
  /** 未压缩字节数。 */
  size: number;
  /** 压缩后字节数（加密条目含加密头）。 */
  compressedSize: number;
  /** 是否走了 GB18030 回退（未设 bit 11 且含非 ASCII 字节）。 */
  gbkFallback: boolean;
  encrypted: boolean;
  /** 压缩方法是否为 stored/deflate（其余方法桌面也不支持）。 */
  supported: boolean;
}

const SIG_EOCD = 0x06054b50;
const SIG_EOCD64 = 0x06064b50;
const SIG_EOCD64_LOCATOR = 0x07064b50;
const SIG_CDH = 0x02014b50;
/** 结束记录固定部分长度（不含最长 65535 字节的注释）。 */
const EOCD_SIZE = 22;
const EOCD64_LOCATOR_SIZE = 20;
/** 中央目录文件头固定部分长度（不含名/扩展/注释）。 */
const CDH_SIZE = 46;
const ZIP64_EXTRA_ID = 0x0001;
/** AES 扩展：真实压缩方法在其数据区第 6–7 字节（`zip` crate 的 `AexEncryption`）。 */
const AES_EXTRA_ID = 0x9901;
const METHOD_STORED = 0;
const METHOD_DEFLATED = 8;
const METHOD_AES = 99;
const U16_SENTINEL = 0xffff;
const U32_SENTINEL = 0xffffffff;

/**
 * ZIP 读取失败。错误码与桌面同族：`E3003`「archive cannot be read」
 * （`platform::archive::zip_error` 的非密码分支）。
 */
export class ZipIndexError extends Error {
  readonly code = "E3003";

  constructor(detail: string) {
    super(`E3003: archive cannot be read (${detail})`);
    this.name = "ZipIndexError";
  }
}

/**
 * 从 `Blob`/`File` 读中央目录。
 *
 * **只做两次随机读**：末尾窗口（定位 EOCD，≤ 64 KiB）＋中央目录本体。
 * 不读条目数据，因此 4 GB 的包在浏览器里也只是几 MB 的读取量。
 */
export async function readZipEntriesFromFile(file: Blob): Promise<ZipEntryFact[]> {
  const size = file.size;
  if (size < EOCD_SIZE) {
    throw new ZipIndexError(`file is ${size} bytes, shorter than an end record`);
  }
  const tailLength = Math.min(size, EOCD_SIZE + 0xffff);
  const tailStart = size - tailLength;
  const tail = await readRange(file, tailStart, size);
  const eocdInTail = findEndOfCentralDirectory(tail);
  if (eocdInTail < 0) {
    throw new ZipIndexError("end of central directory not found");
  }
  const eocd = new DataView(tail.buffer, tail.byteOffset, tail.byteLength);
  let entryCount = eocd.getUint16(eocdInTail + 10, true);
  let directorySize = eocd.getUint32(eocdInTail + 12, true);
  let directoryOffset = eocd.getUint32(eocdInTail + 16, true);

  // Zip64：三个字段任一取哨兵值，就必须去 Zip64 结束记录取真值。
  if (
    entryCount === U16_SENTINEL ||
    directorySize === U32_SENTINEL ||
    directoryOffset === U32_SENTINEL
  ) {
    const locatorAt = tailStart + eocdInTail - EOCD64_LOCATOR_SIZE;
    if (locatorAt < 0) {
      throw new ZipIndexError("zip64 locator is missing");
    }
    const locator = new DataView((await readRange(file, locatorAt, locatorAt + EOCD64_LOCATOR_SIZE)).buffer);
    if (locator.getUint32(0, true) !== SIG_EOCD64_LOCATOR) {
      throw new ZipIndexError("zip64 locator signature mismatch");
    }
    const recordAt = toSafeNumber(locator.getBigUint64(8, true), "zip64 record offset");
    const record = await readRange(file, recordAt, recordAt + 56);
    const view = new DataView(record.buffer, record.byteOffset, record.byteLength);
    if (view.getUint32(0, true) !== SIG_EOCD64) {
      throw new ZipIndexError("zip64 end record signature mismatch");
    }
    entryCount = toSafeNumber(view.getBigUint64(32, true), "zip64 entry count");
    directorySize = toSafeNumber(view.getBigUint64(40, true), "zip64 directory size");
    directoryOffset = toSafeNumber(view.getBigUint64(48, true), "zip64 directory offset");
  }

  if (directorySize === 0) {
    return [];
  }
  const directory = await readRange(file, directoryOffset, directoryOffset + directorySize);
  return parseCentralDirectory(directory, entryCount);
}

/**
 * 解析中央目录字节（纯函数，供测试与 Worker 内复用）。
 *
 * `entryCount` 只用于日志式的上限保护：实际以**中央目录签名**为准逐条推进，
 * 遇到签名不符即停（损坏包不静默产出半截清单——条目数与解析数不符时抛错）。
 */
export function parseCentralDirectory(
  directory: Uint8Array,
  entryCount: number
): ZipEntryFact[] {
  const view = new DataView(directory.buffer, directory.byteOffset, directory.byteLength);
  const entries: ZipEntryFact[] = [];
  let offset = 0;
  while (offset + CDH_SIZE <= directory.byteLength) {
    if (view.getUint32(offset, true) !== SIG_CDH) {
      break;
    }
    const flags = view.getUint16(offset + 8, true);
    const method = view.getUint16(offset + 10, true);
    let compressedSize = view.getUint32(offset + 20, true);
    let size = view.getUint32(offset + 24, true);
    const nameLength = view.getUint16(offset + 28, true);
    const extraLength = view.getUint16(offset + 30, true);
    const commentLength = view.getUint16(offset + 32, true);
    const nameStart = offset + CDH_SIZE;
    const extraStart = nameStart + nameLength;
    const next = extraStart + extraLength + commentLength;
    if (next > directory.byteLength) {
      throw new ZipIndexError("central directory entry runs past the directory");
    }
    const rawName = directory.subarray(nameStart, extraStart);
    const extras = parseExtraFields(directory.subarray(extraStart, extraStart + extraLength));

    // Zip64 扩展字段按「未压缩 → 压缩 → 本地头偏移 → 起始盘号」的顺序**只补哨兵字段**。
    if (size === U32_SENTINEL || compressedSize === U32_SENTINEL) {
      const zip64 = extras.zip64;
      if (zip64) {
        let cursor = 0;
        if (size === U32_SENTINEL && cursor + 8 <= zip64.byteLength) {
          size = toSafeNumber(readU64(zip64, cursor), "entry size");
          cursor += 8;
        }
        if (compressedSize === U32_SENTINEL && cursor + 8 <= zip64.byteLength) {
          compressedSize = toSafeNumber(readU64(zip64, cursor), "entry compressed size");
        }
      }
    }

    // AES 条目：中央目录写 99，真实方法在 0x9901 扩展里（`zip` crate 同法还原）。
    const effectiveMethod = method === METHOD_AES ? (extras.aesMethod ?? method) : method;
    const decoded = decodeEntryName(rawName, (flags & 0x0800) !== 0);
    entries.push({
      index: entries.length,
      name: decoded.name,
      kind: isDirectoryName(rawName) ? "dir" : "file",
      size,
      compressedSize,
      gbkFallback: decoded.gbkFallback,
      encrypted: (flags & 0x0001) !== 0,
      supported: effectiveMethod === METHOD_STORED || effectiveMethod === METHOD_DEFLATED
    });
    offset = next;
  }
  if (entryCount !== 0xffff && entries.length !== entryCount) {
    throw new ZipIndexError(
      `central directory holds ${entries.length} entries, end record says ${entryCount}`
    );
  }
  return entries;
}

/** 从末尾倒扫 EOCD 签名；注释最长 65535 字节，故最多看 22 + 65535 字节。 */
function findEndOfCentralDirectory(tail: Uint8Array): number {
  const view = new DataView(tail.buffer, tail.byteOffset, tail.byteLength);
  for (let at = tail.byteLength - EOCD_SIZE; at >= 0; at -= 1) {
    if (view.getUint32(at, true) !== SIG_EOCD) {
      continue;
    }
    // 注释长度必须与「记录到文件尾」的距离吻合，否则是同字节序列的假阳性。
    const commentLength = view.getUint16(at + 20, true);
    if (at + EOCD_SIZE + commentLength === tail.byteLength) {
      return at;
    }
  }
  return -1;
}

/** 目录判定：与 `zip` crate 一致，只看原始名末字节（`/` 或 `\`）。 */
function isDirectoryName(rawName: Uint8Array): boolean {
  const last = rawName[rawName.length - 1];
  return last === 0x2f || last === 0x5c;
}

/**
 * 名称解码（对齐 `platform::archive::decode_name`）：
 * bit 11 → 严格 UTF-8（失败 `null`）；全 ASCII → 直读；否则 GB18030 回退。
 */
function decodeEntryName(
  rawName: Uint8Array,
  utf8Flag: boolean
): { name: string | null; gbkFallback: boolean } {
  if (utf8Flag) {
    try {
      return { name: strictUtf8.decode(rawName), gbkFallback: false };
    } catch {
      return { name: null, gbkFallback: false };
    }
  }
  if (isAscii(rawName)) {
    let text = "";
    for (const byte of rawName) {
      text += String.fromCharCode(byte);
    }
    return { name: text, gbkFallback: false };
  }
  const text = decodeGb18030(rawName);
  if (text === null) {
    return { name: null, gbkFallback: true };
  }
  // 有损解码出的 U+FFFD 在桌面侧是「解码失败」（Abort 策略）——同向处理。
  return { name: text.includes("\ufffd") ? null : text, gbkFallback: true };
}

function isAscii(bytes: Uint8Array): boolean {
  for (const byte of bytes) {
    if (byte >= 0x80) {
      return false;
    }
  }
  return true;
}

const strictUtf8 = new TextDecoder("utf-8", { fatal: true });
let gb18030Decoder: TextDecoder | null | undefined;

/** GB18030 解码（浏览器 Encoding Standard 的必选编码；不可用时返回 `null`）。 */
function decodeGb18030(bytes: Uint8Array): string | null {
  if (gb18030Decoder === undefined) {
    try {
      gb18030Decoder = new TextDecoder("gb18030");
    } catch {
      gb18030Decoder = null;
    }
  }
  return gb18030Decoder ? gb18030Decoder.decode(bytes) : null;
}

/** 扩展字段解析结果（只取本模块需要的两段）。 */
interface ExtraFields {
  /** Zip64（0x0001）数据区。 */
  zip64: Uint8Array | null;
  /** AES（0x9901）里记录的真实压缩方法；无该扩展时为 `null`。 */
  aesMethod: number | null;
}

function parseExtraFields(extra: Uint8Array): ExtraFields {
  const found: ExtraFields = { zip64: null, aesMethod: null };
  let at = 0;
  while (at + 4 <= extra.byteLength) {
    const id = extra[at] | (extra[at + 1] << 8);
    const length = extra[at + 2] | (extra[at + 3] << 8);
    const bodyStart = at + 4;
    if (bodyStart + length > extra.byteLength) {
      break;
    }
    if (id === ZIP64_EXTRA_ID && found.zip64 === null) {
      found.zip64 = extra.subarray(bodyStart, bodyStart + length);
    }
    if (id === AES_EXTRA_ID && length >= 7) {
      found.aesMethod = extra[bodyStart + 5] | (extra[bodyStart + 6] << 8);
    }
    at = bodyStart + length;
  }
  return found;
}

function readU64(bytes: Uint8Array, at: number): bigint {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return view.getBigUint64(at, true);
}

/** BigInt → number，超出 `Number.MAX_SAFE_INTEGER` 说明包大小不现实，直接拒绝。 */
function toSafeNumber(value: bigint, what: string): number {
  if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
    throw new ZipIndexError(`${what} exceeds the safe integer range`);
  }
  return Number(value);
}

/** 按字节区间读 `Blob`（`File.slice` 不复制数据，读多少拷多少）。 */
async function readRange(blob: Blob, start: number, end: number): Promise<Uint8Array> {
  const slice = blob.slice(start, end);
  if (typeof slice.arrayBuffer === "function") {
    return new Uint8Array(await slice.arrayBuffer());
  }
  // jsdom 的 Blob 缺 arrayBuffer；测试环境走 FileReader（同一份代码，不改产品路径）。
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(new Uint8Array(reader.result as ArrayBuffer));
    reader.onerror = () => reject(reader.error);
    reader.readAsArrayBuffer(slice);
  });
}
