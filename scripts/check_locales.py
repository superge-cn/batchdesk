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
KEY_DOMAIN_RE = re.compile(
    r"^(?:app|nav|common|task|c09|feature|tasks|settings|error)\.[A-Za-z0-9_.]+$"
)
# 错误码文案族：映射表在 web/src/errors.ts，key 形如 error.E2004.summary（设计 06 §3）。
ERROR_MAP_FILE = SRC_DIR / "errors.ts"
ERROR_KEY_RE = re.compile(r'"error\.(E\d{4})\.(summary|action)"')
ERROR_BUNDLE_KEY_RE = re.compile(r"^error\.(E\d{4})\.(summary|action|detail)$")
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

    if failures:
        print("国际化门禁未通过：", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    print(
        "国际化门禁通过："
        f"{len(bundles)} 个语言包 × {len(default_keys)} 个 key，key 集合/占位符/语言残留一致；"
        f"源码引用 {len(referenced)} 个 key 全部存在，注释外无硬编码中文；"
        f"错误码文案 {len(mapped)} 个码位两段齐全。"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
