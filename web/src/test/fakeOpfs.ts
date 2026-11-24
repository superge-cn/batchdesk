/**
 * 内存版 OPFS 目录句柄（测试专用）。
 *
 * jsdom 没有 OPFS：`navigator.storage.getDirectory` 不存在。这里按**被测代码实际
 * 用到的 API 面**实现一个形状相同的假句柄——多实现反而会掩盖「代码依赖了未声明的
 * API」这类问题。
 *
 * 两处复用：`opfsStore.test.ts`（存储层单测，显式传 `root`）与
 * `FeaturePages.webPlan.test.tsx`（接线测试，用 `installFakeOpfs()` 装到
 * `navigator.storage`，走真实默认参数路径）。
 */

export class FakeDir {
  readonly kind = "directory" as const;
  readonly dirs = new Map<string, FakeDir>();
  readonly files = new Map<string, Blob>();

  constructor(readonly name = "root") {}

  /** 真实 OPFS 拒绝含分隔符的名字；fake 也照做，免得实现偷偷依赖宽松行为。 */
  private assertName(name: string): void {
    if (name.includes("/") || name.includes("\\") || name === "." || name === "..") {
      throw Object.assign(new Error("TypeMismatchError"), { name: "TypeMismatchError" });
    }
  }

  async getDirectoryHandle(name: string, options?: { create?: boolean }): Promise<FakeDir> {
    this.assertName(name);
    const existing = this.dirs.get(name);
    if (existing !== undefined) return existing;
    if (options?.create !== true) {
      throw Object.assign(new Error("NotFoundError"), { name: "NotFoundError" });
    }
    const created = new FakeDir(name);
    this.dirs.set(name, created);
    return created;
  }

  async getFileHandle(name: string, options?: { create?: boolean }): Promise<FakeFile> {
    this.assertName(name);
    const existing = this.files.get(name);
    if (existing !== undefined) return new FakeFile(name, this, existing);
    if (options?.create !== true) {
      throw Object.assign(new Error("NotFoundError"), { name: "NotFoundError" });
    }
    return new FakeFile(name, this, new Blob([]));
  }

  async removeEntry(name: string, options?: { recursive?: boolean }): Promise<void> {
    if (this.files.delete(name)) return;
    if (options?.recursive === true) {
      if (this.dirs.delete(name)) return;
      throw Object.assign(new Error("NotFoundError"), { name: "NotFoundError" });
    }
    const child = this.dirs.get(name);
    if (child !== undefined && child.dirs.size === 0 && child.files.size === 0) {
      this.dirs.delete(name);
      return;
    }
    throw Object.assign(new Error("NotFoundError"), { name: "NotFoundError" });
  }

  async *entries(): AsyncIterableIterator<[string, FakeDir | FakeFile]> {
    for (const [name, dir] of this.dirs) yield [name, dir];
    for (const [name, blob] of this.files) yield [name, new FakeFile(name, this, blob)];
  }
}

export class FakeFile {
  readonly kind = "file" as const;

  constructor(
    readonly name: string,
    private readonly dir: FakeDir,
    private store: Blob
  ) {}

  async createWritable(): Promise<{
    write: (data: Blob) => Promise<void>;
    close: () => Promise<void>;
  }> {
    return {
      write: async (data: Blob) => {
        this.store = data;
      },
      close: async () => {
        this.dir.files.set(this.name, this.store);
      }
    };
  }

  async getFile(): Promise<File> {
    return new File([this.store], this.name);
  }
}

/** 把假句柄当 `FileSystemDirectoryHandle` 用（形状一致，只少未被调用的成员）。 */
export function asHandle(dir: FakeDir): FileSystemDirectoryHandle {
  return dir as unknown as FileSystemDirectoryHandle;
}

/**
 * 把假 OPFS 装到 `navigator.storage`，让 `opfsSupported()` 与默认参数路径都走它。
 *
 * 返回卸载函数：用例结束后必须还原，否则后面「不支持 OPFS」的断言会误判。
 */
export function installFakeOpfs(): { root: FakeDir; restore: () => void } {
  const root = new FakeDir();
  const original = Object.getOwnPropertyDescriptor(navigator, "storage");
  Object.defineProperty(navigator, "storage", {
    configurable: true,
    value: { getDirectory: async () => asHandle(root) }
  });
  return {
    root,
    restore: () => {
      if (original !== undefined) {
        Object.defineProperty(navigator, "storage", original);
      } else {
        delete (navigator as { storage?: unknown }).storage;
      }
    }
  };
}
