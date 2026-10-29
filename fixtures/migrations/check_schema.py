#!/usr/bin/env python3
"""配置迁移样本 schema 校验。

权威是 docs/design/03-配置schema与迁移契约.md §4.1 的字段清单；本脚本只做
“字段名/类型/枚举值/configVersion”级别的机械校验，不重复定义默认值或阈值。

用法：
    python fixtures/migrations/check_schema.py
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
VERSIONED = re.compile(r"^v(\d+)/config-v\1\.json$")

SCOPE_KEYS = {
    "general",
    "processing",
    "performance",
    "security",
    "logging",
}

# 字段名 -> (允许的 Python 类型, 枚举/说明)
# 列表类型用 list 表示，元素类型在下方单独校验。
SCHEMA = {
    "general": {
        "language": (str, None),
        "theme": (str, {"system", "light", "dark"}),
        "defaultOutputDir": (str, None),
        "launchBehavior": (str, {"normal", "minimized"}),
    },
    "processing": {
        "onConflict": (str, {"skip", "sequence", "overwrite", "ask"}),
        "encodingPolicy": (str, {"auto", "strict-utf8"}),
        "newlinePolicy": (str, {"keep", "crlf", "lf"}),
        "deletionMode": (str, {"move-to-quarantine", "permanent"}),
        "keepBackup": (bool, None),
    },
    "performance": {
        "concurrency": ((int, str), {"auto"}),
        "memoryBudget": ((int, str), {"auto"}),
        "largeFileThreshold": ((int, str), {"auto"}),
    },
    "security": {
        "protectedDirs": (list, None),
        "quarantineDir": (str, None),
        "quarantineRetentionDays": (int, None),
        "placeholderPolicy": (str, {"skip", "download-then-process"}),
        "diskSpaceMarginFloorBytes": (int, None),
        "diskSpaceMarginRatio": (float, None),
    },
    "logging": {
        "retentionDays": (int, None),
        "logFullPaths": (bool, None),
    },
}


def fail(errors: list[str], path: Path, msg: str) -> None:
    errors.append(f"{path.relative_to(ROOT)}: {msg}")


def check_value(errors: list[str], path: Path, scope: str, key: str, value: object) -> None:
    expected_type, allowed = SCHEMA[scope][key]
    if isinstance(value, bool) and expected_type is not bool:
        fail(errors, path, f"{scope}.{key} 类型错误：期望 {expected_type}，得到 bool")
        return
    if not isinstance(value, expected_type):
        fail(errors, path, f"{scope}.{key} 类型错误：期望 {expected_type}，得到 {type(value).__name__}")
        return
    if allowed is not None and value not in allowed:
        fail(errors, path, f"{scope}.{key} 枚举值非法：{value!r}（允许 {sorted(allowed)}）")
    if key == "protectedDirs":
        if not all(isinstance(item, str) for item in value):
            fail(errors, path, "security.protectedDirs 必须是字符串数组")
    if key in {"concurrency", "memoryBudget", "largeFileThreshold"} and isinstance(value, str):
        if value != "auto":
            fail(errors, path, f"performance.{key} 只允许整数或 \"auto\"")


def check_document(path: Path) -> list[str]:
    errors: list[str] = []
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        fail(errors, path, f"JSON 解析失败：{exc}")
        return errors

    if not isinstance(doc, dict):
        fail(errors, path, "根节点必须是对象")
        return errors

    allowed_top = {"configVersion", "updatedAt", "scopes"}
    unknown_top = set(doc) - allowed_top
    if unknown_top:
        fail(errors, path, f"未知顶层字段：{sorted(unknown_top)}")
    if not isinstance(doc.get("configVersion"), int) or isinstance(doc.get("configVersion"), bool):
        fail(errors, path, "configVersion 必须是整数")
    if not isinstance(doc.get("updatedAt"), str):
        fail(errors, path, "updatedAt 必须是 RFC 3339 字符串")
    scopes = doc.get("scopes")
    if not isinstance(scopes, dict):
        fail(errors, path, "scopes 必须是对象")
        return errors

    unknown_scopes = set(scopes) - SCOPE_KEYS
    if unknown_scopes:
        fail(errors, path, f"未知 scope：{sorted(unknown_scopes)}")
    for scope, fields in scopes.items():
        if scope not in SCHEMA:
            continue
        if not isinstance(fields, dict):
            fail(errors, path, f"{scope} 必须是对象")
            continue
        unknown = set(fields) - set(SCHEMA[scope])
        if unknown:
            fail(errors, path, f"{scope} 含未知字段：{sorted(unknown)}")
        for key, value in fields.items():
            if key in SCHEMA[scope]:
                check_value(errors, path, scope, key, value)
    return errors


def main() -> int:
    errors: list[str] = []
    samples = sorted(
        p for p in HERE.glob("v*/config-v*.json") if p.is_file()
    )
    if not samples:
        print("未找到迁移样本：fixtures/migrations/v*/config-v*.json")
        return 1

    for path in samples:
        rel = path.relative_to(HERE).as_posix()
        match = VERSIONED.match(rel)
        if not match:
            fail(errors, path, "文件名必须为 v{N}/config-v{N}.json")
            continue
        expected_version = int(match.group(1))
        doc = json.loads(path.read_text(encoding="utf-8"))
        if doc.get("configVersion") != expected_version:
            fail(
                errors,
                path,
                f"configVersion={doc.get('configVersion')!r} 与目录版本 v{expected_version} 不一致",
            )
        errors.extend(check_document(path))

    if errors:
        print("迁移样本 schema 校验失败：")
        for err in errors:
            print(f"- {err}")
        return 1

    print(f"迁移样本 schema 校验通过：{len(samples)} 个样本。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
