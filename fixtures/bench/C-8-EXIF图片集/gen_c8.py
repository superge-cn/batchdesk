#!/usr/bin/env python3
"""C-8 基准数据集生成脚本：EXIF 图片集（有/无拍摄时间两类）。

判定权威：T v1.12 附录 C 的 C-8 行与本目录 README（本脚本只负责产出数据集，不承载阈值）。
对应对象：C28（P1）的日期回退行为——EXIF 拍摄时间优先，缺失时回退文件修改时间。

两类固定构造（数量与尺寸为构造参数，非产品阈值）
- ① 含 EXIF `DateTimeOriginal`（36867）：20 张 JPEG，尺寸 64/96/128/160 各 5 张；
- ② 无 EXIF：同尺寸、同数量、同格式；
- 两类所有文件的 mtime 统一固定为 2020-01-01T00:00:00Z，使"回退到文件时间"的断言可重复；
- 像素内容由固定种子决定（逐像素确定性填充），JPEG 编码参数固定 → 产物逐字节可复现。

设计要点
- 自校验：① 逐张读回 EXIF 且 36867 存在；② 逐张确认无 EXIF；两类 mtime 均为固定值；
- 依赖 Pillow（仅测试工具链使用，不进产品依赖）；
- 合成图片：不使用任何真实照片；产物落 `out/`，不入库。
"""

from __future__ import annotations

import argparse
import shutil
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"

SEED = 2026
SIZES = (64, 96, 128, 160)
PER_SIZE = 5
FIXED_MTIME = 1577836800  # 2020-01-01T00:00:00Z
EXIF_DATETIME_ORIGINAL = 36867
SHOOT_TIME = "2019:05:04 10:30:00"


def build_image(path: Path, size: int, with_exif: bool) -> None:
    img = Image.new("RGB", (size, size))
    # 确定性像素：固定公式，不依赖随机状态推进顺序
    img.putdata([
        ((x * 7 + y * 13) % 256, (x * 3) % 256, (y * 5) % 256)
        for y in range(size) for x in range(size)
    ])
    kwargs: dict = {"quality": 85, "optimize": False}
    if with_exif:
        exif = Image.Exif()
        exif[EXIF_DATETIME_ORIGINAL] = SHOOT_TIME
        kwargs["exif"] = exif.tobytes()
    img.save(path, format="JPEG", **kwargs)


def wipe_out_dir() -> None:
    """只清理本脚本自己的 out/ 目录（幂等重建的前提）。"""
    if OUT_DIR.exists():
        shutil.rmtree(OUT_DIR)
    OUT_DIR.mkdir(parents=True)


def verify() -> None:
    with_exif = OUT_DIR / "with_exif"
    without = OUT_DIR / "without_exif"
    n_expected = len(SIZES) * PER_SIZE
    for d, expect in ((with_exif, True), (without, False)):
        files = sorted(d.glob("*.jpg"))
        if len(files) != n_expected:
            raise SystemExit(f"C-8 数据集失效：{d.name} 张数 {len(files)} ≠ 预期 {n_expected}")
        for p in files:
            with Image.open(p) as im:
                has = EXIF_DATETIME_ORIGINAL in (im.getexif() or {})
            if has != expect:
                raise SystemExit(f"C-8 数据集失效：{p.name} 的 EXIF 存在性与预期不符")
            if int(p.stat().st_mtime) != FIXED_MTIME:
                raise SystemExit(f"C-8 数据集失效：{p.name} 的 mtime 未固定")


def main() -> None:
    global PER_SIZE
    parser = argparse.ArgumentParser(description="生成 C-8 EXIF 图片集基准数据集")
    parser.add_argument("--per-size", type=int, default=PER_SIZE, help=f"每尺寸张数（默认 {PER_SIZE}）")
    args = parser.parse_args()

    PER_SIZE = args.per_size

    wipe_out_dir()
    for sub in ("with_exif", "without_exif"):
        (OUT_DIR / sub).mkdir()

    for size in SIZES:
        for i in range(PER_SIZE):
            build_image(OUT_DIR / "with_exif" / f"{size}_{i:02d}.jpg", size, True)
            build_image(OUT_DIR / "without_exif" / f"{size}_{i:02d}.jpg", size, False)

    for p in OUT_DIR.rglob("*.jpg"):
        import os
        os.utime(p, (FIXED_MTIME, FIXED_MTIME))

    verify()

    log = OUT_DIR / "gen.log"
    log.write_text(
        f"sizes={SIZES} per_size={PER_SIZE} seed={SEED} quality=85 "
        f"exif_tag={EXIF_DATETIME_ORIGINAL} shoot_time='{SHOOT_TIME}' mtime={FIXED_MTIME}\n",
        encoding="utf-8",
    )
    print(f"C-8 已生成：{OUT_DIR}（日志 {log}）")


if __name__ == "__main__":
    main()
