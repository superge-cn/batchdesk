#!/usr/bin/env python3
"""国际化门禁（设计 09 §2／§4）：语言包完整性与界面文案资源化。

检查项：
1. 两个语言文件存在且是 JSON 对象；
2. key 集合完全一致（缺失即构建失败，设计 09 §2.3）；
3. 值非空；
4. 同一 key 的占位符集合一致（`{count}` 之类，避免英文丢参数）；
5. 非默认语言（en-US）里不残留中文字符；
6. 源码引用的 key 都存在于语言包（拼错 key 不会静默显示 raw key）；
7. 界面源码（web/src 下的 .ts/.tsx，除语言包与测试）不在注释外硬编码中文
   （设计 09 §2.1：禁止硬编码用户可见字符串）。
8. 错误码文案族（设计 06 §2／§3）：`web/src/errors.ts` 的每个码位必须有
   `summary`/`action` 两段两语言文案，且与设计 06 §2 的码表**逐码对齐**；
9. 界面源码不得渲染 `IpcError.message`／`detail` 原文（设计 06 §3：技术细节只进
   诊断日志，文案不得含路径/文件名/堆栈）——取原文的位置只允许 errors.ts 的兜底
   与 IPC 归一化层。
10. 界面源码不得把线协议枚举（`{x.status}`／`{x.state}`）当文案渲染（设计 09 §2），
   枚举→key 的唯一出口是 `web/src/labels.tsx`。
11. 预警类别三方逐类对齐（设计 06 §2A ↔ `web/src/warnings.ts` ↔ 两语言包）：表里有类别而
   映射没接、映射里有而表没有、语言包缺文案或多出映射表外的 `warning.*` key，都直接红；
   表解析为空即报「门禁自校失败」。
12. 预检类别三方逐类对齐（设计 01 的 `PreflightItem.kind` 枚举 ↔ `web/src/preflightKinds.ts`
   ↔ 两语言包）：同第 ⑪ 项手法；内核新增类别而界面没接文案即红（OQ-38 口径：预检项的
   技术明细不进界面，只显示级别＋本地化类别＋错误码）。

只使用标准库；CI 的 web job 与本地门禁必跑。

用法：
    python scripts/check_locales.py
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCALES_DIR = ROOT / "web" / "src" / "locales"
SRC_DIR = ROOT / "web" / "src"
DEFAULT_LANG = "zh-CN"
REQUIRED_LANGS = ["zh-CN", "en-US"]

# 资源 key 的域前缀：与 web/src 的 t("...")／注册表字面量共用同一套命名。
# `templates` 是 2026-11-05 新增的顶层视图（处理模板，C37）的域。
KEY_DOMAIN_RE = re.compile(
    r"^(?:app|ai|nav|common|task|c09|templates|feature|tasks|settings|error|warning)\.[A-Za-z0-9_.]+$"
)
# 错误码文案族：映射表在 web/src/errors.ts，key 形如 error.E2004.summary（设计 06 §3）。
ERROR_MAP_FILE = SRC_DIR / "errors.ts"
ERROR_KEY_RE = re.compile(r'"error\.(E\d{4})\.(summary|action)"')
ERROR_BUNDLE_KEY_RE = re.compile(r"^error\.(E\d{4})\.(summary|action|detail)$")
# 设计 06 §2 的码表：映射表必须与契约表**逐码对齐**——将来码表新增一个码位而
# errors.ts／语言包没接文案，这里直接红（此前是"人记得去补"）。
DESIGN06_FILE = ROOT / "docs" / "design" / "06-错误码与用户文案契约.md"
DESIGN06_CODE_RE = re.compile(r"^\|\s*(E\d{4})\s*\|", re.M)
# 设计 01 的 PreflightItem.kind 枚举（追加式枚举，冻结后只增不改）：预检类别的对照源。
DESIGN01_FILE = ROOT / "docs" / "design" / "01-内核接口与IPC契约.md"
PREFLIGHT_INTERFACE_RE = re.compile(r"interface PreflightItem\s*\{(?P<body>[^{}]*)\}", re.S)
PREFLIGHT_ENUM_RE = re.compile(r'kind:\s*((?:"[a-z0-9\-]+"\s*\|\s*)*"[a-z0-9\-]+")')
PREFLIGHT_MAP_FILE = SRC_DIR / "preflightKinds.ts"
PREFLIGHT_MAP_ENTRY_RE = re.compile(r'"([a-z0-9\-]+)":\s*"c09\.preflight\.kind\.([A-Za-z0-9]+)"')
PREFLIGHT_BUNDLE_KEY_RE = re.compile(r"^c09\.preflight\.kind\.([A-Za-z0-9]+)$")
PLACEHOLDER_RE = re.compile(r"\{(\w+)\}")
CJK_RE = re.compile(r"[\u3000-\u303f\u4e00-\u9fff\uff00-\uffef]")

# 只有中文文件允许出现中文；英文文件里出现中文即为漏译。
CJK_ALLOWED_IN = {DEFAULT_LANG}

# 硬编码检查的例外：
# - 语言包本身；
# - 浏览器预览夹具（web/src/lib/ipc/mock.ts）——它不是产品文案，而是"假数据"，
#   真实桌面运行不会加载；把假数据翻成两种语言反而会掩盖漏译；
# - 测试文件（断言的期望文案）。
HARDCODED_SKIP = {
    Path("web/src/locales"),
}
HARDCODED_SKIP_FILES = {Path("web/src/lib/ipc/mock.ts")}
HARDCODED_SKIP_SUFFIX = (".test.tsx", ".test.ts")
# 行内豁免：解析用户输入（如"表头 有|无"里的 `"无"`）不是界面文案。
EXEMPT_MARKER = "i18n-exempt"

# 设计 06 §3 的强制力落点：`IpcError.message`／`detail` 是后端技术串（本仓库里常含
# 绝对路径与源码位置），只能进诊断日志，界面渲染必须走 `describeError`／`ErrorBanner`。
# 这一条此前只写在 ADR 里、靠人记得，本批同一族的第二处泄漏（执行结果表）就是这么漏的。
RAW_ERROR_TEXT_RE = re.compile(r"\berror\??\.(message|detail)\b")
# 设计 06 §2A 的预警类别表：类别 ↔ 界面 key ↔ 两语言文案必须逐类对齐（同第 ⑧ 项的手法，
# 对照源从"码位表"换成"类别表"）。映射表在 web/src/warnings.ts。
WARN_MAP_FILE = SRC_DIR / "warnings.ts"
WARN_TABLE_ROW_RE = re.compile(r"^\| `([a-z0-9-]+)` \| `warning\.([A-Za-z0-9]+)\.title` \|", re.M)
WARN_MAP_ENTRY_RE = re.compile(r'"([a-z0-9-]+)":\s*"warning\.([A-Za-z0-9]+)\.title"')
WARN_BUNDLE_KEY_RE = re.compile(r"^warning\.([A-Za-z0-9]+)\.title$")

# 第 ⑩ 项：界面不得把线协议枚举当文案渲染。`{entry.status}`／`{task.state}` 这类插值
# 印出来的是 `ok`／`partial-failed` 这种协议字符串——同一个 bug 在 2026-11-04 连出现两次
# （任务状态徽标、条目级状态），第三次只能靠机器挡。枚举→key 的唯一出口是 `labels.tsx`。
# 只认"整段插值就是一条成员访问"这一种写法（`{row.status}`／`{ detail.summary.state }`）：
# 要求花括号里不含逗号/冒号/展开，否则会把 `{ ...current, state: event.state }` 这类
# 对象字面量误判成渲染点。函数调用形态（`{label(task.state)}`）不在射程内，如实写明。
RAW_PROTOCOL_ENUM_RE = re.compile(
    r"\{\s*[A-Za-z_$][\w$]*(?:\??\.[\w$]+)*\.(status|state)\s*\??\s*\}"
)

# 允许读原文的两处：`errors.ts` 的"码表外兜底"与 IPC 归一化层（它们就是唯一的出口）。
RAW_ERROR_TEXT_ALLOWED = {
    Path("web/src/errors.ts"),
    Path("web/src/lib/ipc/index.ts"),
}


def load_bundle(lang: str) -> dict[str, str]:
    path = LOCALES_DIR / f"{lang}.json"
    if not path.exists():
        raise SystemExit(f"[locale] 缺少语言包：{path.relative_to(ROOT)}")
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise SystemExit(f"[locale] {path.name} 不是合法 JSON：{error}") from error
    if not isinstance(data, dict):
        raise SystemExit(f"[locale] {path.name} 顶层必须是对象")
    return data


def strip_comments(text: str) -> str:
    """去掉行注释、块注释与 JSX 注释，只留代码与字符串。"""
    out: list[str] = []
    index = 0
    length = len(text)
    in_block = False
    in_line = False
    in_string: str | None = None
    while index < length:
        char = text[index]
        pair = text[index : index + 2]
        if in_line:
            if char == "\n":
                in_line = False
                out.append(char)
            index += 1
            continue
        if in_block:
            if pair == "*/":
                in_block = False
                index += 2
                continue
            if char == "\n":
                out.append(char)
            index += 1
            continue
        if in_string:
            out.append(char)
            if char == "\\":
                index += 2
                continue
            if char == in_string:
                in_string = None
            index += 1
            continue
        if pair == "//":
            in_line = True
            index += 2
            continue
        if pair == "/*":
            in_block = True
            index += 2
            continue
        if char in "\"'`":
            in_string = char
            out.append(char)
            index += 1
            continue
        out.append(char)
        index += 1
    return "".join(out)


def source_files() -> list[Path]:
    files: list[Path] = []
    for path in SRC_DIR.rglob("*"):
        if not path.is_file():
            continue
        if path.suffix not in {".ts", ".tsx"}:
            continue
        if path.name.endswith(HARDCODED_SKIP_SUFFIX):
            continue
        files.append(path)
    return sorted(files)


def main() -> int:
    failures: list[str] = []

    bundles = {lang: load_bundle(lang) for lang in REQUIRED_LANGS}
    default_keys = set(bundles[DEFAULT_LANG])

    # 2. key 集合一致
    for lang, bundle in bundles.items():
        missing = sorted(default_keys - set(bundle))
        extra = sorted(set(bundle) - default_keys)
        if missing:
            failures.append(f"{lang} 缺少 {len(missing)} 个 key：{', '.join(missing[:5])}")
        if extra:
            failures.append(f"{lang} 多出 {len(extra)} 个 key：{', '.join(extra[:5])}")

    # 3/4/5. 值、占位符、语言残留
    for lang, bundle in bundles.items():
        for key, value in bundle.items():
            where = f"{lang}.{key}"
            if not isinstance(value, str):
                failures.append(f"{where} 不是字符串")
                continue
            if not value.strip():
                failures.append(f"{where} 为空")
            if lang not in CJK_ALLOWED_IN and CJK_RE.search(value):
                failures.append(f"{where} 在 {lang} 中残留中文：{value}")
            if key in bundles[DEFAULT_LANG]:
                expected = set(PLACEHOLDER_RE.findall(bundles[DEFAULT_LANG][key]))
                actual = set(PLACEHOLDER_RE.findall(value))
                if expected != actual:
                    failures.append(
                        f"{where} 占位符不一致：期望 {sorted(expected)}，实际 {sorted(actual)}"
                    )

    # 6. 源码引用的 key 都存在
    referenced: dict[str, list[str]] = {}
    for path in source_files():
        text = strip_comments(path.read_text(encoding="utf-8"))
        for match in re.finditer(r'"([^"\n]+)"', text):
            key = match.group(1)
            if KEY_DOMAIN_RE.match(key):
                referenced.setdefault(key, []).append(str(path.relative_to(ROOT)))
    for key, files in sorted(referenced.items()):
        if key not in default_keys:
            failures.append(f"源码引用了不存在的 key：{key}（{files[0]}）")

    # 7. 注释外不硬编码中文
    for path in source_files():
        relative = path.relative_to(ROOT)
        if relative in HARDCODED_SKIP_FILES:
            continue
        if any(str(relative).startswith(str(skip)) for skip in HARDCODED_SKIP):
            continue
        original = path.read_text(encoding="utf-8").splitlines()
        for number, line in enumerate(strip_comments("\n".join(original)).splitlines(), start=1):
            if not CJK_RE.search(line):
                continue
            raw = original[number - 1] if number - 1 < len(original) else ""
            if EXEMPT_MARKER in raw:
                continue
            failures.append(f"{relative}:{number} 硬编码中文：{line.strip()[:60]}")

    # 8. 错误码文案族（设计 06 §3／设计 09 §5）：映射表里的码位必须有 summary 与
    # action 两段中文/英文文案；语言包里也不得出现映射表之外的 error.* 码位。
    if not ERROR_MAP_FILE.exists():
        failures.append("找不到 web/src/errors.ts：错误码文案族没有映射表")
    else:
        map_text = strip_comments(ERROR_MAP_FILE.read_text(encoding="utf-8"))
        mapped: dict[str, set[str]] = {}
        for code, part in ERROR_KEY_RE.findall(map_text):
            mapped.setdefault(code, set()).add(part)
        if not mapped:
            failures.append("web/src/errors.ts 里没有任何 error.<code>.summary/action 映射")
        for code, parts in sorted(mapped.items()):
            for part in ("summary", "action"):
                if part not in parts:
                    failures.append(f"错误码 {code} 在 errors.ts 里缺 {part} 映射")
                key = f"error.{code}.{part}"
                for lang in REQUIRED_LANGS:
                    if key not in bundles[lang]:
                        failures.append(f"{lang} 缺错误码文案：{key}")
        for lang, bundle in bundles.items():
            for key in bundle:
                match = ERROR_BUNDLE_KEY_RE.match(key)
                if match and match.group(1) not in mapped:
                    failures.append(
                        f"{lang}.{key} 的码位没有进 errors.ts 映射表"
                        "（要么补映射，要么删文案）"
                    )

        if DESIGN06_FILE.exists():
            table = set(DESIGN06_CODE_RE.findall(DESIGN06_FILE.read_text(encoding="utf-8")))
            if not table:
                failures.append("设计 06 §2 的码表解析为空：门禁自校失败（探测器失效＝没有正例）")
            unbound = sorted(table - set(mapped))
            strays = sorted(set(mapped) - table)
            if unbound:
                failures.append(
                    f"设计 06 §2 有 {len(unbound)} 个码位没进 errors.ts 映射表：{', '.join(unbound)}"
                )
            if strays:
                failures.append(
                    f"errors.ts 有 {len(strays)} 个码位不在设计 06 §2 码表里：{', '.join(strays)}"
                )

    # 9. 界面源码不得渲染后端错误原文（设计 06 §3：detail 仅进诊断日志，
    #    文案不得暴露路径/文件名/堆栈）。出口只有 describeError／ErrorBanner。
    leaks: list[str] = []
    scanned_ui: list[Path] = []
    for path in source_files():
        relative = path.relative_to(ROOT)
        if relative in RAW_ERROR_TEXT_ALLOWED:
            continue
        if any(str(relative).startswith(str(skip)) for skip in HARDCODED_SKIP):
            continue
        scanned_ui.append(relative)
        lines = strip_comments(path.read_text(encoding="utf-8")).splitlines()
        for number, line in enumerate(lines, start=1):
            if RAW_ERROR_TEXT_RE.search(line):
                leaks.append(f"{relative}:{number} 直接取用后端错误原文：{line.strip()[:70]}")
    if leaks:
        failures.extend(leaks)
    if len(leaks) == 0 and not (SRC_DIR / "ErrorBanner.tsx").exists():
        failures.append("找不到 web/src/ErrorBanner.tsx：界面错误文案没有唯一出口（探测器自校失败）")

    # 10. 界面不得渲染线协议枚举（设计 09 §2；同上，靠机器而非记性）。
    enum_leaks: list[str] = []
    for path in source_files():
        relative = path.relative_to(ROOT)
        if path.suffix != ".tsx":
            continue
        if any(str(relative).startswith(str(skip)) for skip in HARDCODED_SKIP):
            continue
        lines = strip_comments(path.read_text(encoding="utf-8")).splitlines()
        for number, line in enumerate(lines, start=1):
            if RAW_PROTOCOL_ENUM_RE.search(line):
                enum_leaks.append(
                    f"{relative}:{number} 把线协议枚举直接渲染成文案（应走 labels.tsx 的枚举→key）："
                    f"{line.strip()[:70]}"
                )
    if enum_leaks:
        failures.extend(enum_leaks)
    if not (SRC_DIR / "labels.tsx").exists():
        failures.append("找不到 web/src/labels.tsx：枚举→key 文案没有单一来源（探测器自校失败）")

    # 11. 预警类别三方逐类对齐（设计 06 §2A ↔ web/src/warnings.ts ↔ 两语言包）。
    design06_text = DESIGN06_FILE.read_text(encoding="utf-8") if DESIGN06_FILE.exists() else ""
    table = dict(WARN_TABLE_ROW_RE.findall(design06_text[design06_text.index("## 2A."):]
                 if "## 2A." in design06_text else ""))
    if not table:
        failures.append("设计 06 §2A 的预警类别表解析为空：门禁自校失败（探测器失效＝没有正例）")
    if not WARN_MAP_FILE.exists():
        failures.append("找不到 web/src/warnings.ts：预警类别没有界面映射表")
        mapped_warnings: dict[str, str] = {}
    else:
        mapped_warnings = dict(
            WARN_MAP_ENTRY_RE.findall(strip_comments(WARN_MAP_FILE.read_text(encoding="utf-8")))
        )
        if not mapped_warnings:
            failures.append("web/src/warnings.ts 里没有任何 warning.<kind>.title 映射")
    if table and mapped_warnings:
        unbound = sorted(set(table) - set(mapped_warnings))
        strays = sorted(set(mapped_warnings) - set(table))
        if unbound:
            failures.append(
                f"设计 06 §2A 有 {len(unbound)} 个类别没进 warnings.ts 映射：{', '.join(unbound)}"
            )
        if strays:
            failures.append(
                f"warnings.ts 有 {len(strays)} 个类别不在设计 06 §2A 表里：{', '.join(strays)}"
            )
    for kind, key in mapped_warnings.items():
        bundle_key = f"warning.{key}.title"
        if table.get(kind) and table[kind] != key:
            failures.append(
                f"{kind}：warnings.ts 用 {key}，设计 06 §2A 表定的是 {table[kind]}（key 必须同源）"
            )
        for lang in REQUIRED_LANGS:
            if bundle_key not in bundles[lang]:
                failures.append(f"{lang} 缺预警类别文案：{bundle_key}")
    for lang, bundle in bundles.items():
        for key in bundle:
            match = WARN_BUNDLE_KEY_RE.match(key)
            if match and match.group(1) not in set(mapped_warnings.values()):
                failures.append(
                    f"{lang}.{key} 的类别没有进 warnings.ts 映射表（要么补映射，要么删文案）"
                )

    # 12. 预检类别三方逐类对齐（设计 01 的 PreflightItem.kind 枚举 ↔ preflightKinds.ts ↔ 两语言包）。
    design01_text = DESIGN01_FILE.read_text(encoding="utf-8") if DESIGN01_FILE.exists() else ""
    interface_match = PREFLIGHT_INTERFACE_RE.search(design01_text)
    enum_match = (
        PREFLIGHT_ENUM_RE.search(interface_match.group("body")) if interface_match else None
    )
    design_kinds = set(re.findall(r'"([a-z0-9\-]+)"', enum_match.group(1))) if enum_match else set()
    if not design_kinds:
        failures.append("设计 01 的 PreflightItem.kind 枚举解析为空：门禁自校失败（探测器失效＝没有正例）")
    mapped_preflight: dict[str, str] = {}
    if not PREFLIGHT_MAP_FILE.exists():
        failures.append("找不到 web/src/preflightKinds.ts：预检类别没有界面映射表")
    else:
        mapped_preflight = dict(
            PREFLIGHT_MAP_ENTRY_RE.findall(strip_comments(PREFLIGHT_MAP_FILE.read_text(encoding="utf-8")))
        )
        if not mapped_preflight:
            failures.append("web/src/preflightKinds.ts 里没有任何 c09.preflight.kind.* 映射")
    if design_kinds and mapped_preflight:
        unbound = sorted(design_kinds - set(mapped_preflight))
        strays = sorted(set(mapped_preflight) - design_kinds)
        if unbound:
            failures.append(
                f"设计 01 的 kind 枚举有 {len(unbound)} 个类别没进 preflightKinds.ts 映射：{', '.join(unbound)}"
            )
        if strays:
            failures.append(
                f"preflightKinds.ts 有 {len(strays)} 个类别不在设计 01 枚举里：{', '.join(strays)}"
            )
    mapped_preflight_keys = {f"c09.preflight.kind.{suffix}" for suffix in mapped_preflight.values()}
    for kind, suffix in mapped_preflight.items():
        bundle_key = f"c09.preflight.kind.{suffix}"
        for lang in REQUIRED_LANGS:
            if bundle_key not in bundles[lang]:
                failures.append(f"{lang} 缺预检类别文案：{bundle_key}")
    for lang, bundle in bundles.items():
        for key in bundle:
            match = PREFLIGHT_BUNDLE_KEY_RE.match(key)
            if match and key not in mapped_preflight_keys:
                failures.append(
                    f"{lang}.{key} 的类别没有进 preflightKinds.ts 映射表（要么补映射，要么删文案）"
                )

    if failures:
        print("国际化门禁未通过：", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    print(
        "国际化门禁通过："
        f"{len(bundles)} 个语言包 × {len(default_keys)} 个 key，key 集合/占位符/语言残留一致；"
        f"源码引用 {len(referenced)} 个 key 全部存在，注释外无硬编码中文；"
        f"错误码文案 {len(mapped)} 个码位两段齐全，且与设计 06 §2 码表逐码对齐；"
        f"界面 {len(scanned_ui)} 个源码文件零后端错误原文、零线协议枚举裸露；"
        f"预警类别 {len(mapped_warnings)} 类与设计 06 §2A 逐类对齐；"
        f"预检类别 {len(mapped_preflight)} 类与设计 01 的 PreflightItem.kind 枚举逐类对齐。"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
