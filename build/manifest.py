#!/usr/bin/env python3
"""Read manifest/cores.toml without third-party dependencies.

The build container ships Python 3.9 (no `tomllib`), and we do not want to add a
dependency only to read a file we control.  This parser understands the exact
subset used by `manifest/cores.toml`: table headers, `key = value` pairs,
quoted strings, integers, booleans and one-line string arrays.  Anything else
raises, so a typo fails the build instead of silently disappearing.

Usage:
    python3 build/manifest.py list
    python3 build/manifest.py get <core> <field>
    python3 build/manifest.py check
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "manifest" / "cores.toml"

REQUIRED = {
    "build": ["kind", "builder", "commit", "dylib", "machines", "license", "license_file"],
    "vendor": ["kind", "dylib", "machines", "sha256", "license"],
}


def parse_value(raw: str):
    raw = raw.strip()
    if raw.startswith('"') and raw.endswith('"'):
        return raw[1:-1]
    if raw.startswith("[") and raw.endswith("]"):
        body = raw[1:-1].strip()
        if not body:
            return []
        return [parse_value(part) for part in body.split(",")]
    if raw in ("true", "false"):
        return raw == "true"
    try:
        return int(raw)
    except ValueError as exc:  # pragma: no cover - defensive
        raise SystemExit(f"manifest: unsupported value {raw!r}") from exc


def parse(path: Path) -> dict:
    cores: dict[str, dict] = {}
    current = None
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        text = line.split("#", 1)[0].strip()
        if not text:
            continue
        if text.startswith("[") and text.endswith("]"):
            table = text[1:-1].strip()
            if not table.startswith("core."):
                current = None
                continue
            current = cores.setdefault(table[len("core.") :], {})
            continue
        if current is None:
            continue
        if "=" not in text:
            raise SystemExit(f"manifest: {path}:{number}: expected key = value")
        key, value = text.split("=", 1)
        current[key.strip()] = parse_value(value)
    return cores


def check(cores: dict) -> None:
    problems = []
    for name, spec in cores.items():
        kind = spec.get("kind")
        if kind not in REQUIRED:
            problems.append(f"{name}: unknown kind {kind!r}")
            continue
        for field in REQUIRED[kind]:
            if field not in spec:
                problems.append(f"{name}: missing {field}")
    if not cores:
        problems.append("no cores parsed")
    if problems:
        raise SystemExit("manifest check failed:\n  " + "\n  ".join(problems))


def check_release(cores: dict) -> None:
    """额外检查发布门槛：许可证、GLIBC、sha256。"""
    check(cores)
    problems = []
    for name, spec in cores.items():
        # optional 核心不随包分发：许可证与 GLIBC 义务不落在发布包上。
        if spec.get("optional") is True:
            continue
        if spec.get("license") in (None, "", "unknown"):
            problems.append(f"{name}: licence unknown (release blocker)")
        if not spec.get("license_file") and spec.get("license") not in (None, "", "unknown"):
            problems.append(f"{name}: licence_file missing")
        glibc = spec.get("glibc_max")
        if glibc is not None and tuple(int(p) for p in str(glibc).split(".")) > (2, 31):
            problems.append(f"{name}: needs GLIBC {glibc} > device 2.31")
        if spec.get("kind") == "vendor" and not spec.get("sha256"):
            problems.append(f"{name}: vendor core without sha256")
    if problems:
        raise SystemExit("release check failed:\n  " + "\n  ".join(problems))


def main(argv: list[str]) -> int:
    # Windows 文本模式会把 \n 变成 \r\n，shell 的 $(...) 会带上 \r，导致
    # `manifest.py get <core> field` 永远查不到。统一输出 LF。
    try:
        sys.stdout.reconfigure(newline="\n")
    except AttributeError:  # pragma: no cover - very old Python
        pass
    cores = parse(MANIFEST)
    if len(argv) < 2:
        print(__doc__)
        return 2
    command = argv[1]
    if command == "list":
        for name in cores:
            print(name)
        return 0
    if command == "check":
        check(cores)
        print(f"manifest ok: {len(cores)} cores")
        return 0
    if command == "check-release":
        check_release(cores)
        print(f"manifest release-ok: {len(cores)} cores")
        return 0
    if command == "get" and len(argv) == 4:
        name, field = argv[2], argv[3]
        spec = cores.get(name)
        if spec is None or field not in spec:
            return 1
        value = spec[field]
        if isinstance(value, bool):
            print("true" if value else "false")
        elif isinstance(value, list):
            print(",".join(value))
        else:
            print(value)
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
