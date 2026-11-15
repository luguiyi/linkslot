#!/usr/bin/env python3
"""把组装好的卡目录打成可复现的 zip（P2-06）。

    python3 build/package.py --dir artifacts/linkslot --out artifacts/linkslot-v1.0.0-h700.zip

规则：
- 目录内所有文件按相对路径写入，`System/` 下可执行文件 0755，其余 0644；
- 保留空目录（zip 需要显式 entry）；
- 时间戳统一取 SOURCE_DATE_EPOCH（默认当前时间），同一输入两次打包 SHA256 一致。
"""

from __future__ import annotations

import argparse
import os
import sys
import time
import zipfile
from pathlib import Path

EXEC_PREFIXES = ("System/",)


def mode_for(rel: str, is_dir: bool) -> int:
    if is_dir:
        return 0o755
    if rel.startswith(EXEC_PREFIXES) and not rel.endswith((".txt", ".json", ".md", ".meta", ".so", ".patch")):
        return 0o755
    if rel.startswith("System/") and rel.endswith(".sh"):
        return 0o755
    return 0o644


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dir", required=True, help="组装好的卡目录")
    parser.add_argument("--out", required=True, help="输出 zip 路径")
    parser.add_argument("--prefix", default="", help="zip 内的顶层目录名")
    args = parser.parse_args(argv[1:])

    root = Path(args.dir)
    if not root.is_dir():
        raise SystemExit(f"error: not a directory: {root}")

    epoch = os.environ.get("SOURCE_DATE_EPOCH")
    stamp = int(epoch) if epoch else int(time.time())
    date_time = time.gmtime(stamp)[:6]

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)

    entries = 0
    with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
        for path in sorted(root.rglob("*")):
            rel = path.relative_to(root).as_posix()
            if args.prefix:
                rel = f"{args.prefix}/{rel}"
            if path.is_dir():
                info = zipfile.ZipInfo(rel + "/", date_time=date_time)
                info.external_attr = (mode_for(rel, True) | 0o040000) << 16
                zf.writestr(info, b"")
                continue
            info = zipfile.ZipInfo(rel, date_time=date_time)
            info.external_attr = (mode_for(rel, False) | 0o100000) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            zf.writestr(info, path.read_bytes())
            entries += 1

    total = out.stat().st_size
    print(f"packaged {entries} files -> {out} ({total} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
