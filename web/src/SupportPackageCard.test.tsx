import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

// 支持包（设计 10 §5；方案 §9.7／§12.5）：先预览后导出，写回的就是刚预览的那份；
// 内容不含文件内容/完整路径，界面不渲染后端错误原文。

const buildSupportPackage = vi.fn();
const writeSupportPackage = vi.fn();

vi.mock("./lib/ipc", () => ({
  buildSupportPackage: (...args: unknown[]) => buildSupportPackage(...args),
  writeSupportPackage: (...args: unknown[]) => writeSupportPackage(...args),
  toIpcError: (value: unknown) => value
}));

import type { SupportPackage } from "./lib/ipc/types";
import { SupportPackageCard } from "./SupportPackageCard";

const pkg: SupportPackage = {
  version: "0.1.0",
  build: "dev",
  os: "windows",
  arch: "x86_64",
  generatedAt: "2026-11-10T12:00:00Z",
  diagnosticLog: [
    {
      ts: "2026-11-10T12:00:04Z",
      level: "warn",
      target: "task-queue",
      code: "E2002",
      taskId: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8A",
      message: "task finished with partial failures"
    }
  ],
  errorCodeStats: [{ code: "E2002", count: 1 }],
  reproSteps: ""
};

describe("SupportPackageCard", () => {
  beforeEach(() => {
    buildSupportPackage.mockReset();
    writeSupportPackage.mockReset();
    buildSupportPackage.mockResolvedValue(pkg);
    writeSupportPackage.mockResolvedValue("C:/app-data/exports/batchdesk-support-1.json");
  });

  it("预览按当前勾选组装并渲染环境、统计与日志", async () => {
    render(<SupportPackageCard />);
    // 导出按钮在预览之前不可用（不给"没内容可导"的死按钮）。
    expect(screen.getByRole("button", { name: "导出" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "预览" }));
    await waitFor(() => expect(buildSupportPackage).toHaveBeenCalledTimes(1));
    expect(buildSupportPackage).toHaveBeenCalledWith({
      reproSteps: "",
      includeDiagnosticLog: true,
      includeErrorCodeStats: true
    });
    expect(screen.getByText(/0\.1\.0/)).toBeInTheDocument();
    // 统计行与日志行都带 E2002：断言统计行的完整文本，避免只按码位匹配到多处。
    expect(screen.getByText("E2002 × 1")).toBeInTheDocument();
    expect(screen.getByText("task finished with partial failures")).toBeInTheDocument();
  });

  it("导出写回刚预览的那份并显示落盘路径", async () => {
    render(<SupportPackageCard />);
    await userEvent.click(screen.getByRole("button", { name: "预览" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "导出" })).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: "导出" }));
    await waitFor(() => expect(writeSupportPackage).toHaveBeenCalledWith(pkg));
    expect(screen.getByText(/batchdesk-support-1\.json/)).toBeInTheDocument();
  });

  it("关掉段落后按新选择重新预览", async () => {
    render(<SupportPackageCard />);
    await userEvent.click(screen.getByLabelText("包含最近诊断日志"));
    await userEvent.click(screen.getByRole("button", { name: "预览" }));
    await waitFor(() => expect(buildSupportPackage).toHaveBeenCalledTimes(1));
    expect(buildSupportPackage).toHaveBeenCalledWith({
      reproSteps: "",
      includeDiagnosticLog: false,
      includeErrorCodeStats: true
    });
  });

  it("失败后清空预览并只显示本地化提示", async () => {
    buildSupportPackage.mockRejectedValue({ code: "E9001", message: "C:/secret/path failed" });
    render(<SupportPackageCard />);
    await userEvent.click(screen.getByRole("button", { name: "预览" }));
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    // 后端原文（含路径）不得进界面（设计 06 §3／ADR-0061）。
    expect(screen.queryByText(/C:\/secret/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "导出" })).toBeDisabled();
  });
});
