#!/usr/bin/env python3
"""生成 `linkSlot/version.json`（P3-03）。

    python3 build/version_json.py --dir artifacts/linkslot \
        --version 1.0.0 --commit abc1234 --upstream slot-1.6.0 \
        --build-date 2026-11-12

字段：schema / product / version / commit / upstream / build / files。
`files` 是"卡内相对路径 → sha256:…"，按路径排序；同一目录两次生成结果逐字节一致。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

SCHEMA = 1
PRODUCT = "linkslot"


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def build(root: Path, version: str, commit: str, upstream: str, build_date: str) -> dict:
    files = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        if rel == "linkSlot/version.json":
            continue
        files[rel] = f"sha256:{sha256_of(path)}"
    return {
        "schema": SCHEMA,
        "product": PRODUCT,
        "version": version,
        "commit": commit,
        "upstream": upstream,
        "build": build_date,
        "files": files,
    }


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dir", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--commit", default="unknown")
    parser.add_argument("--upstream", default="slot-1.6.0")
    parser.add_argument("--build-date", required=True)
    args = parser.parse_args(argv[1:])

    root = Path(args.dir)
    if not root.is_dir():
        raise SystemExit(f"error: not a directory: {root}")

    payload = build(root, args.version, args.commit, args.upstream, args.build_date)
    out = root / "linkSlot" / "version.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=False) + "\n",
        encoding="utf-8",
    )
    print(f"wrote {out} ({len(payload['files'])} files)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
