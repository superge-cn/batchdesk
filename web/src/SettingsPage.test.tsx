import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

// 设置页的行为断言（设计 03 §7／§4.1）：只写 ConfigDocument、
// 逐项显示来源、恢复默认走 resetScope、语言切换即时生效。

vi.mock("./lib/ipc", () => ({
  getConfig: vi.fn(),
  setConfig: vi.fn(),
  resetScope: vi.fn(),
  toIpcError: (value: unknown) => value
}));

import { getConfig, resetScope, setConfig } from "./lib/ipc";
import { I18nProvider } from "./i18n";
import { SettingsPage } from "./SettingsPage";
import type { ScopeView, SettingValue } from "./lib/ipc/types";

function setting(value: unknown, user = false): SettingValue {
  return {
    value,
    effectiveSource: user ? "user" : "default",
    ...(user ? { userValue: value } : {}),
    defaultValue: value
  };
}

function views(): Record<string, ScopeView> {
  return {
    general: {
      scope: "general",
      values: {
        language: setting("zh-CN"),
        theme: setting("light"),
        defaultOutputDir: setting(null),
        launchBehavior: setting("normal")
      }
    },
    processing: {
      scope: "processing",
      values: {
        onConflict: setting("skip"),
        encodingPolicy: setting("auto"),
        newlinePolicy: setting("keep"),
        deletionMode: setting("move-to-quarantine"),
        keepBackup: setting(true, true)
      }
    },
    performance: {
      scope: "performance",
      values: {
        concurrency: setting("auto"),
        memoryBudget: setting("auto"),
        largeFileThreshold: setting("auto")
      }
    },
    security: {
      scope: "security",
      values: {
        protectedDirs: setting([]),
        quarantineDir: setting(null),
        quarantineRetentionDays: setting(null),
        placeholderPolicy: setting("skip"),
        diskSpaceMarginFloorBytes: setting(null),
        diskSpaceMarginRatio: setting(null)
      }
    },
    logging: {
      scope: "logging",
      values: { retentionDays: setting(null), logFullPaths: setting(false) }
    }
  };
}

let store: Record<string, ScopeView> = views();

function renderPage(
  options: {
    onResetUiState?: () => Promise<void>;
    onExecutionSettingsChanged?: () => void;
  } = {}
) {
  return render(
    <I18nProvider initialLang="zh-CN" initialTheme="light">
      <SettingsPage
        onBack={() => undefined}
        onResetUiState={options.onResetUiState}
        onExecutionSettingsChanged={options.onExecutionSettingsChanged}
      />
    </I18nProvider>
  );
}

beforeEach(() => {
  store = views();
  vi.clearAllMocks();
  vi.mocked(getConfig).mockImplementation(async (scope) => store[scope ?? "general"]);
  vi.mocked(setConfig).mockImplementation(async (scope, patch) => {
    const target = store[scope as string];
    for (const [key, value] of Object.entries(patch)) {
      target.values[key] = setting(value, true);
    }
    return target;
  });
  vi.mocked(resetScope).mockImplementation(async (scope) => {
    store[scope as string] = views()[scope as string];
    return store[scope as string];
  });
});

describe("设置页", () => {
  it("按设计 03 的五组渲染，并逐项标出生效值来源", async () => {
    renderPage();
    for (const title of ["通用", "处理", "性能", "安全", "日志"]) {
      expect(await screen.findByRole("heading", { name: title })).toBeVisible();
    }
    // keepBackup 在 store 里是 user 档，其余是默认档
    const backupField = screen.getByLabelText("保留备份（查找替换类）").closest(".field");
    expect(backupField?.textContent).toContain("已自定义");
    const conflictField = screen.getByLabelText("冲突策略").closest(".field");
    expect(conflictField?.textContent).toContain("默认值");
  });

  it("改动字段即写 ConfigDocument 的对应 scope", async () => {
    renderPage();
    const check = await screen.findByLabelText("保留备份（查找替换类）");
    await userEvent.click(check);
    await waitFor(() =>
      expect(setConfig).toHaveBeenCalledWith("processing", { keepBackup: false })
    );
    expect(setConfig).toHaveBeenCalledTimes(1);
  });

  it("语言切换即时生效并写 general.language", async () => {
    renderPage();
    const select = await screen.findByLabelText("界面语言");
    await userEvent.selectOptions(select, "en-US");
    await waitFor(() => expect(document.documentElement.lang).toBe("en-US"));
    // 界面文案随语言切换（I18N-02）：分组标题与字段标签都变英文，不是只改了 lang 属性。
    expect(await screen.findByRole("heading", { name: "General" })).toBeVisible();
    expect(screen.getByRole("heading", { name: "Logging" })).toBeVisible();
    expect(setConfig).toHaveBeenCalledWith("general", { language: "en-US" });
  });

  it("「恢复本组默认」调 resetScope，不逐项写空值", async () => {
    renderPage();
    const loggingCard = (await screen.findByRole("heading", { name: "日志" })).closest("section");
    const resetButton = loggingCard?.querySelector("button");
    expect(resetButton).toBeTruthy();
    await userEvent.click(resetButton as HTMLElement);
    await waitFor(() => expect(resetScope).toHaveBeenCalledWith("logging"));
    expect(setConfig).not.toHaveBeenCalled();
  });

  it("只读档（E1007）给出可读提示，而不是静默失败", async () => {
    vi.mocked(setConfig).mockRejectedValueOnce({
      code: "E1007",
      message: "config was written by a newer version",
      retryable: false
    });
    renderPage();
    const check = await screen.findByLabelText("保留备份（查找替换类）");
    await userEvent.click(check);
    expect(
      await screen.findByText(/配置文件由更高版本写入，当前为只读/)
    ).toBeVisible();
  });
});

describe("执行期设置（方案 §7.10 的「新设置将在下一批生效」）", () => {
  it("改并发：当场提示下一批生效，并通知调用方让旧预览失效", async () => {
    const onExecutionSettingsChanged = vi.fn();
    renderPage({ onExecutionSettingsChanged });
    const input = await screen.findByLabelText("并发数");
    // 受控输入：一次 change 完成整值替换（auto → 4），不逐键触发中间值保存。
    fireEvent.change(input, { target: { value: "4" } });
    await waitFor(() =>
      expect(setConfig).toHaveBeenCalledWith("performance", { concurrency: 4 })
    );
    expect(await screen.findByText(/新设置将在下一批生效/)).toBeVisible();
    expect(onExecutionSettingsChanged).toHaveBeenCalledTimes(1);
    // 字段提示必须写清手动档范围与「尚未派发的条目」口径，而不是只靠保存后的横幅。
    const field = input.closest(".field");
    expect(field?.textContent).toContain("1–16");
    expect(field?.textContent).toContain("尚未派发的条目");
  });

  it("改其它字段不触发执行期设置失效", async () => {
    const onExecutionSettingsChanged = vi.fn();
    renderPage({ onExecutionSettingsChanged });
    const check = await screen.findByLabelText("保留备份（查找替换类）");
    await userEvent.click(check);
    await waitFor(() =>
      expect(setConfig).toHaveBeenCalledWith("processing", { keepBackup: false })
    );
    expect(onExecutionSettingsChanged).not.toHaveBeenCalled();
  });

  it("「恢复本组默认」把并发改回自动档时同样让旧预览失效", async () => {
    const onExecutionSettingsChanged = vi.fn();
    renderPage({ onExecutionSettingsChanged });
    const performanceCard = (await screen.findByRole("heading", { name: "性能" })).closest(
      "section"
    );
    const resetButton = performanceCard?.querySelector("button");
    expect(resetButton).toBeTruthy();
    await userEvent.click(resetButton as HTMLElement);
    await waitFor(() => expect(resetScope).toHaveBeenCalledWith("performance"));
    expect(onExecutionSettingsChanged).toHaveBeenCalledTimes(1);
  });
});

describe("本机界面状态（设计 03 §11.3）", () => {
  it("重置界面状态调用回调并给出提示，且不碰任何 ConfigDocument 命令", async () => {
    const onResetUiState = vi.fn(async () => undefined);
    const user = userEvent.setup();
    renderPage({ onResetUiState });

    await user.click(await screen.findByRole("button", { name: /重置界面状态/ }));

    expect(onResetUiState).toHaveBeenCalledTimes(1);
    expect(await screen.findByText(/最近位置已清空/)).toBeInTheDocument();
    // 边界：界面状态与用户设置是两个存储，重置界面状态不得触发 setConfig/resetScope。
    expect(setConfig).not.toHaveBeenCalled();
    expect(resetScope).not.toHaveBeenCalled();
  });

  it("不传回调时按钮保持禁用（独立渲染不假装能重置）", async () => {
    renderPage();
    const button = await screen.findByRole("button", { name: /重置界面状态/ });
    expect(button).toBeDisabled();
  });
});
