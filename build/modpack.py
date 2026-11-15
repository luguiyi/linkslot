#!/usr/bin/env python3
"""把 `mods/<id>/` 打成可分发的 mod ZIP（P8）。

用法：
    python3 build/modpack.py [--out DIR] [--check]

约定（与 Web「导出」一致）：
- ZIP 内层固定为 `mods/<id>/...`（导入器按这层识别，与卡内 `linkSlot/mods/` 无关）；
- 跳过 `config.json`（可能含 API Key / 服务器地址，用户应在设备上填写）、
  `.trash/`、`*.log`；
- 输出 `linkslot-mod-<id>-v<version>.zip`，并写 `SHA256SUMS`；
- 同样的输入产生同样的 ZIP（固定时间戳 + 排序），便于发布与校验。
"""

import argparse
import hashlib
import io
import json
import os
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MODS = ROOT / "mods"

SKIP_NAMES = {"config.json"}
SKIP_DIRS = {".trash"}
SKIP_SUFFIXES = {".log"}
EPOCH = int(os.environ.get("SOURCE_DATE_EPOCH", "0") or "0")


def zip_time() -> tuple:
    """固定但合法的 ZIP 时间戳（1980 起），优先用 SOURCE_DATE_EPOCH。"""
    if EPOCH <= 0:
        return (1980, 1, 1, 0, 0, 0)
    import datetime

    moment = datetime.datetime.fromtimestamp(EPOCH, datetime.timezone.utc)
    year = max(moment.year, 1980)
    return (year, moment.month, moment.day, moment.hour, moment.minute, moment.second)


def wanted(path: Path, mod_dir: Path) -> bool:
    rel = path.relative_to(mod_dir)
    if any(part in SKIP_DIRS for part in rel.parts[:-1]):
        return False
    if rel.name in SKIP_NAMES or rel.suffix.lower() in SKIP_SUFFIXES:
        return False
    return True


def load_manifest(mod_dir: Path) -> dict:
    manifest = json.loads((mod_dir / "mod.json").read_text(encoding="utf-8"))
    for key in ("schema", "id", "name", "version"):
        if key not in manifest:
            raise SystemExit(f"{mod_dir}/mod.json 缺少 {key}")
    if manifest["schema"] != 2:
        raise SystemExit(f"{mod_dir}/mod.json schema 不是 2")
    return manifest


def build(mod_dir: Path, out_dir: Path) -> Path:
    manifest = load_manifest(mod_dir)
    mod_id = manifest["id"]
    version = manifest["version"]
    files = sorted(
        path for path in mod_dir.rglob("*") if path.is_file() and wanted(path, mod_dir)
    )
    target = out_dir / f"linkslot-mod-{mod_id}-v{version}.zip"
    stamp = zip_time()
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as archive:
        for path in files:
            arcname = f"mods/{mod_id}/{path.relative_to(mod_dir).as_posix()}"
            info = zipfile.ZipInfo(arcname, date_time=stamp)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            archive.writestr(info, path.read_bytes())
    return target


def workspace_version() -> str:
    import re

    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'(?m)^\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', text, re.S)
    if not match:
        raise SystemExit("无法从 Cargo.toml 读取 workspace 版本")
    return match.group(1)


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(description="Build distributable mod ZIPs")
    parser.add_argument("--out", default=str(ROOT / "dist" / "mods"))
    parser.add_argument(
        "--check",
        action="store_true",
        help="只校验已有 mod 清单，不写 ZIP",
    )
    parser.add_argument(
        "--bundle",
        action="store_true",
        help="额外把所有 mod 包打成一个发布附件 bundle",
    )
    args = parser.parse_args(argv)

    out_dir = Path(args.out)
    if not MODS.is_dir():
        raise SystemExit(f"缺少 {MODS}")
    mod_dirs = sorted(path for path in MODS.iterdir() if (path / "mod.json").is_file())
    if not mod_dirs:
        raise SystemExit("mods/ 下没有带 mod.json 的 mod")
    out_dir.mkdir(parents=True, exist_ok=True)
    if not args.check:
        # 清掉上一次构建留下的包，避免旧版本混进发布附件。
        for stale in out_dir.glob("linkslot-mod-*.zip"):
            stale.unlink()
        for stale in out_dir.glob("linkslot-mods-bundle-*.zip"):
            stale.unlink()

    lines = []
    for mod_dir in mod_dirs:
        manifest = load_manifest(mod_dir)
        if args.check:
            print(f"ok {manifest['id']} v{manifest['version']}")
            continue
        target = build(mod_dir, out_dir)
        digest = hashlib.sha256(target.read_bytes()).hexdigest()
        size = target.stat().st_size
        print(f"{target.name}  {size} B  {digest}")
        lines.append(f"{digest}  {target.name}")
    if not args.check:
        (out_dir / "SHA256SUMS").write_text("\n".join(lines) + "\n", encoding="utf-8")
        print(f"wrote {len(lines)} package(s) to {out_dir}")
        if args.bundle:
            version = workspace_version()
            bundle = out_dir / f"linkslot-mods-bundle-v{version}.zip"
            stamp = zip_time()
            with zipfile.ZipFile(bundle, "w", zipfile.ZIP_DEFLATED) as archive:
                for line in lines:
                    name = line.split("  ", 1)[1]
                    info = zipfile.ZipInfo(name, date_time=stamp)
                    info.compress_type = zipfile.ZIP_DEFLATED
                    archive.writestr(info, (out_dir / name).read_bytes())
                info = zipfile.ZipInfo("SHA256SUMS", date_time=stamp)
                info.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(info, (out_dir / "SHA256SUMS").read_bytes())
            digest = hashlib.sha256(bundle.read_bytes()).hexdigest()
            print(f"{bundle.name}  {bundle.stat().st_size} B  {digest}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
