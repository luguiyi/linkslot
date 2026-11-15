#!/usr/bin/env python3
"""架构依赖护栏（P3-14）。

读 `cargo metadata --no-deps`，按下面的规则表检查项目内依赖方向。

    python3 build/check-deps.py            # 报告模式：列出问题，始终 exit 0
    python3 build/check-deps.py --enforce  # 严格模式：有任何问题即 exit 1

规则分两级：
- enforced：当前必须成立，CI 用 --enforce 跑；
- advisory：目标状态，当前仍违反，修好后把 enforce 改成 true 即可。
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys

# (被检查的 crate, 禁止依赖的 crate, 是否强制, 说明)
RULES = [
    ("slot-model", "*", True, "契约层必须零依赖"),
    ("slot-model", "slot-store", True, "领域层不得反向依赖持久化层"),
    ("slot-model", "slot-ui", True, "领域层不得依赖表现层"),
    ("slot-store", "slot-ui", True, "持久化层不得依赖表现层"),
    ("slot-store", "slot-gfx", True, "持久化层不得依赖图形层"),
    ("slot-i18n", "*", True, "翻译表必须零依赖"),
    ("linkslot", "slot-store", True, "CLI 只依赖契约层，不牵入持久化实现"),
    ("slot-ui", "slot-store", False, "表现层应只依赖 slot-model（v1.1 目标）"),
    ("slot-ui", "slot-power", True, "表现层不应依赖硬件电源层"),
]


def load_deps() -> dict[str, list[str]]:
    raw = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    meta = json.loads(raw)
    workspace = {pkg["name"] for pkg in meta["packages"]}
    deps: dict[str, list[str]] = {}
    for pkg in meta["packages"]:
        internal = [
            dep["name"]
            for dep in pkg["dependencies"]
            if dep["name"] in workspace
        ]
        deps[pkg["name"]] = internal
    return deps


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--enforce", action="store_true")
    args = parser.parse_args(argv[1:])

    deps = load_deps()
    problems: list[str] = []
    advisories: list[str] = []

    for crate, forbidden, enforced, why in RULES:
        if crate not in deps:
            problems.append(f"规则指向不存在的 crate：{crate}")
            continue
        actual = deps[crate]
        if forbidden == "*":
            hits = actual
        else:
            hits = [d for d in actual if d == forbidden]
        for dep in hits:
            message = f"{crate} -> {dep}（{why}）"
            if enforced:
                problems.append(message)
            else:
                advisories.append(message)

    for item in advisories:
        print(f"advisory: {item}")
    for item in problems:
        print(f"VIOLATION: {item}")

    if problems:
        print(f"\n{len(problems)} 条强制规则被违反")
        return 1 if args.enforce else 0
    print(f"依赖方向检查通过：{len(deps)} 个 crate，{len(RULES)} 条规则")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
