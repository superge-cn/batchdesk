# ADR-0082 基准夹具的 Windows 采样与工作目录

日期：2026-11-07 ｜ 状态：已采纳

## 1. 背景

Windows 实机日（见 `docs/testing/Windows实机日检查清单-v0.2.md` §9·B4）要在 Windows 上跑
C-1／C-2 正式测量。`fixtures/bench/measure.py` 此前有三处 Linux 假设，Windows 上直接失效：

1. `BIN = target/release/batchdesk` 缺 `.exe` 后缀，子进程起不来；
2. RSS 采样读 `/proc/<pid>/status` 的 `VmRSS`，Windows 无 `/proc`——采样线程立刻空转退出，
   峰值恒为 0，`--rss-limit-mib` 保险丝静默失效（不崩，但内存结论全丢）；
3. 工作目录硬编码 `/tmp/batchdesk-bench`，Windows 上 `/tmp` 解析为**当前盘根**：
   若从 C 盘目录起跑，GiB 级工作副本会落进小容量系统盘。

## 2. 决策

- **BIN**：`os.name == "nt"` 时取 `batchdesk.exe`。
- **RSS 采样**：Windows 分支用 ctypes 调 `GetProcessMemoryInfo`（`K32GetProcessMemoryInfo`，
  回退 `psapi.dll`）取 `WorkingSetSize`，采样周期 50 ms、越限 `TerminateProcess`（`os.kill(pid, 9)`）
  的既有语义不变；非 Windows 分支保持 `/proc` 原样。**口径注记**：`WorkingSetSize` 与 `VmRSS`
  同为「驻留物理内存」口径但不完全等价，Windows 结果行的 RSS 列按此口径记录，不做跨平台直接比较。
- **工作目录**：`workdir()` 改为 `os.environ.get("BATCHDESK_BENCH_DIR", "/tmp")` 之下——
  Linux 缺省行为逐字节不变（CI bench job 不受影响）；Windows 实机通过环境变量指到大盘。

不选的替代：外置采样器（PowerShell 旁路采样再手工并表）会破坏「夹具自带计时与采样」的
证据自洽性；引入 `psutil` 依赖则为一个采样点新增第三方依赖，ctypes 标准库即可。

## 3. 后果

- Windows 实机日的内存保险丝与第③子句（内存占用恒定）判定恢复可用；
- 结果行的 RSS 口径须写明平台（手册 §3 已同步一句）；
- Linux 行为不变：`BATCHDESK_BENCH_DIR` 未设时路径与既有完全一致。
