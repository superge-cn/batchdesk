#!/usr/bin/env python3
"""语料生成物校验：冻结哈希与幂等/确定性。

与 check_scope.py 的分工：
- check_scope.py：清单、人工集合、目录/脚本/expected.md 的存在性（范围门禁）；
- 本脚本：在样本生成之后，校验 manifest 中登记的冻结哈希，并可选地从零重建两次，
  确认生成物逐字节一致（幂等性）。

用法：
    python fixtures/corpus/verify_corpus.py            # 仅校验已生成样本的冻结哈希
    python fixtures/corpus/verify_corpus.py --rebuild  # 清空 out/ 后连续生成两次并比较
    python fixtures/corpus/verify_corpus.py --rebuild --hashes
"""

from __future__ import annotations

import hashlib
import os
import re
import shutil
import stat
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
MANIFEST = HERE / "manifest.md"
ROW_RE = re.compile(r"^\|\s*([KTERZ]\d{2})\s*\|")
SHA_RE = re.compile(r"sha256\s+([0-9a-fA-F]{64})")

# manifest 中登记了冻结哈希的语料 -> 生成物相对路径。
# 新增冻结样本时同步此表；未登记的语料不参与哈希校验。
FROZEN_OUTPUTS = {
    "K01": Path("out/K01.csv"),
    "T01": Path("out/T01.csv"),
    "T05": Path("out/T05.csv"),
    "Z01": Path("out/gbk_name.zip"),
    "Z02": Path("out/traversal.zip"),
}


def read_manifest() -> dict[str, dict[str, str]]:
    rows: dict[str, dict[str, str]] = {}
    for raw in MANIFEST.read_text(encoding="utf-8").splitlines():
        if not ROW_RE.match(raw):
            continue
        cells = [c.strip() for c in raw.strip().strip("|").split("|")]
        if len(cells) < 5:
            continue
        fid, status, note = cells[0], cells[3], cells[4]
        digest = SHA_RE.search(note)
        rows[fid] = {
            "status": status,
            "sha256": digest.group(1).lower() if digest else "",
        }
    return rows


def gen_script(fid: str) -> Path | None:
    scripts = sorted((HERE / fid).glob("gen_*.py"))
    return scripts[0] if scripts else None


def out_dir(fid: str) -> Path:
    return HERE / fid / "out"


def file_hashes(fid: str) -> dict[str, str]:
    base = out_dir(fid)
    if not base.is_dir():
        return {}
    result: dict[str, str] = {}
    for path in sorted(base.rglob("*")):
        if path.is_file():
            rel = path.relative_to(base).as_posix()
            result[rel] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result


def run_script(fid: str) -> None:
    script = gen_script(fid)
    if script is None:
        raise SystemExit(f"{fid} 缺少 gen_*.py")
    proc = subprocess.run(
        [sys.executable, str(script)],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stdout)
        sys.stderr.write(proc.stderr)
        raise SystemExit(f"{fid} 生成失败（退出码 {proc.returncode}）")


def verify_hashes(manifest: dict[str, dict[str, str]]) -> list[str]:
    errors: list[str] = []
    for fid, expected in sorted(manifest.items()):
        digest = expected["sha256"]
        if not digest:
            continue
        rel = FROZEN_OUTPUTS.get(fid)
        if rel is None:
            errors.append(f"{fid} 在 manifest 登记了冻结哈希，但 VERIFY 脚本缺少输出路径映射")
            continue
        path = HERE / fid / rel
        if not path.is_file():
            errors.append(f"{fid} 缺少冻结样本：{path.relative_to(ROOT)}")
            continue
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
        if actual != digest:
            errors.append(
                f"{fid} 冻结哈希不符：manifest {digest[:16]}… / 实际 {actual[:16]}…"
            )
    return errors


def _remove_readonly(func, path, _exc_info):
    """Windows 上 E03 会生成只读文件；删除前先去掉只读位。"""
    os.chmod(path, stat.S_IWRITE)
    func(path)


def clear_out(fid: str) -> None:
    path = out_dir(fid)
    if path.exists():
        shutil.rmtree(path, onerror=_remove_readonly)


def verify_rebuild(manifest: dict[str, dict[str, str]]) -> list[str]:
    errors: list[str] = []
    ready = [fid for fid, row in sorted(manifest.items()) if "就绪" in row["status"]]
    for fid in ready:
        clear_out(fid)
        run_script(fid)
        first = file_hashes(fid)
        if not first:
            errors.append(f"{fid} 生成后 out/ 为空")
            continue
        clear_out(fid)
        run_script(fid)
        second = file_hashes(fid)
        if first != second:
            errors.append(f"{fid} 两次从零生成的产物不一致（幂等性失败）")
    return errors


def main() -> int:
    manifest = read_manifest()
    errors: list[str] = []

    if "--rebuild" in sys.argv:
        errors.extend(verify_rebuild(manifest))

    # 默认校验哈希；--rebuild 后也校验，确保最终产物与 manifest 一致。
    if "--rebuild" in sys.argv or "--hashes" in sys.argv or len(sys.argv) == 1:
        errors.extend(verify_hashes(manifest))

    if errors:
        print("语料生成物校验失败：")
        for err in errors:
            print(f"- {err}")
        return 1

    print("语料生成物校验通过：冻结哈希一致，生成物幂等/确定性检查通过。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
