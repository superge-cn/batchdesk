/**
 * 执行产物的浏览器持久化（OPFS，ADR-0115）。
 *
 * 为什么是「执行后搬运」而不是「执行时直写」：内核 `FsAdapter` 是**同步**契约
 * （设计 04，已冻结），OPFS 全是**异步** API。把异步后移到执行之后，与 staging
 * 把异步前移（读 `File` → 登记字节）是对称的——两者都不改内核一行。
 *
 * 零强制力（设计 05）：OPFS 不是安全边界，用户清站点数据即失效。因此：
 * - 任何失败都**静默降级**（返回 0／空），绝不阻断执行、不改计划、不冒充授权；
 * - 「能不能落盘」不是配额或权限判定，配额由浏览器自己管。
 *
 * 目录形状：`batchdesk/products/<planId>/<名字>`。按 planId 分目录，撤销或重跑时
 * 能整目录清掉，界面也能列出「哪次执行在浏览器里留了什么」。
 */

const ROOT_DIR = "batchdesk";
const PRODUCTS_DIR = "products";

type DirHandle = FileSystemDirectoryHandle;
type FileHandle = FileSystemFileHandle;

/** OPFS 里的一条已落盘产物。 */
export interface PersistedProduct {
  planId: string;
  name: string;
  size: number;
}

/** 一次落盘的结果（失败计数只用于界面提示，不阻断任何流程）。 */
export interface PersistOutcome {
  saved: number;
  failed: number;
}

/**
 * OPFS 入口是否可用。
 *
 * 不支持时调用方退回「只下载」——不是错误分支，是形态差异（Safari 旧版、
 * 非安全上下文都没有 `storage.getDirectory`）。
 */
export function opfsSupported(): boolean {
  return (
    typeof navigator !== "undefined" &&
    typeof navigator.storage === "object" &&
    navigator.storage !== null &&
    typeof navigator.storage.getDirectory === "function"
  );
}

async function appRoot(root?: DirHandle): Promise<DirHandle | null> {
  try {
    const base = root ?? (await navigator.storage.getDirectory());
    return await base.getDirectoryHandle(ROOT_DIR, { create: true });
  } catch {
    return null;
  }
}

async function productsRoot(root?: DirHandle): Promise<DirHandle | null> {
  const app = await appRoot(root);
  if (app === null) return null;
  try {
    return await app.getDirectoryHandle(PRODUCTS_DIR, { create: true });
  } catch {
    return null;
  }
}

/**
 * 目录的异步迭代入口。
 *
 * TS 的 DOM lib 还没声明 `FileSystemDirectoryHandle.entries()`（Chrome／WebView2
 * 早已实现），这里按需窄化；调用处都在 try/catch 里，老浏览器上取不到就按空处理。
 */
type DirectoryEntries = AsyncIterable<[string, FileSystemHandle]>;

function entriesOf(dir: DirHandle): DirectoryEntries {
  return (dir as unknown as { entries(): DirectoryEntries }).entries();
}

async function writeFile(dir: DirHandle, name: string, blob: Blob): Promise<boolean> {
  try {
    const handle = await dir.getFileHandle(name, { create: true });
    const writable = await handle.createWritable();
    await writable.write(blob);
    await writable.close();
    return true;
  } catch {
    return false;
  }
}

async function removeFiles(dir: DirHandle): Promise<void> {
  try {
    for await (const [name, child] of entriesOf(dir)) {
      if (child.kind !== "file") continue;
      // 单个删不掉不影响其余文件；落盘是尽力而为。
      await dir.removeEntry(name).catch(() => undefined);
    }
  } catch {
    // 列不出来就当作没有残留。
  }
}

/**
 * 把一次执行的产物写进 OPFS。
 *
 * 同一 `planId` 重跑先清目录：否则新旧产物混在一起，界面无法说明
 * 「这次留下了什么」。文件名由调用方用 `productFileNames` 算好——
 * 与下载走同一套去重算法，两处不会漂移。
 */
export async function persistProducts(
  planId: string,
  files: { name: string; blob: Blob }[],
  root?: DirHandle
): Promise<PersistOutcome> {
  if (files.length === 0) return { saved: 0, failed: 0 };
  if (root === undefined && !opfsSupported()) {
    return { saved: 0, failed: files.length };
  }
  const dir = await productsRoot(root);
  if (dir === null) return { saved: 0, failed: files.length };

  let planDir: DirHandle;
  try {
    planDir = await dir.getDirectoryHandle(planId, { create: true });
  } catch {
    return { saved: 0, failed: files.length };
  }
  await removeFiles(planDir);

  let saved = 0;
  for (const file of files) {
    if (await writeFile(planDir, file.name, file.blob)) saved += 1;
  }
  return { saved, failed: files.length - saved };
}

/** 列出浏览器里已落盘的产物（按 planId 分组留给调用方）。 */
export async function listPersisted(root?: DirHandle): Promise<PersistedProduct[]> {
  const dir = await productsRoot(root);
  if (dir === null) return [];
  const result: PersistedProduct[] = [];
  try {
    for await (const [planId, handle] of entriesOf(dir)) {
      if (handle.kind !== "directory") continue;
      const planDir = handle as DirHandle;
      try {
        for await (const [name, child] of entriesOf(planDir)) {
          if (child.kind !== "file") continue;
          try {
            const file = await (child as FileHandle).getFile();
            result.push({ planId, name, size: file.size });
          } catch {
            // 读不到就跳过：这里只是展示用，不追求精确。
          }
        }
      } catch {
        continue;
      }
    }
  } catch {
    return result;
  }
  return result;
}

/** 读回一条已落盘产物（界面「重新下载」用）。 */
export async function readPersisted(
  planId: string,
  name: string,
  root?: DirHandle
): Promise<Blob | null> {
  const dir = await productsRoot(root);
  if (dir === null) return null;
  try {
    const planDir = await dir.getDirectoryHandle(planId);
    const handle = await planDir.getFileHandle(name);
    return await handle.getFile();
  } catch {
    return null;
  }
}

/** 清空浏览器里的全部产物（用户主动清理；不碰沙箱与会话状态）。 */
export async function clearPersisted(root?: DirHandle): Promise<boolean> {
  const app = await appRoot(root);
  if (app === null) return false;
  try {
    await app.removeEntry(PRODUCTS_DIR, { recursive: true });
    return true;
  } catch {
    // 目录本来就不存在：对「清理」而言已经是目标状态。
    return true;
  }
}
