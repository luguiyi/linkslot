# -*- coding: utf-8 -*-
"""Generate build/skeleton.txt from crates/slot/src/root.rs DIRS.

The runtime `root::DIRS` list is the single source of truth; this script only
projects it into the offline skeleton the packaging step uses.
"""
import io, re, sys

ROOT = r"D:\workspace\linkslot"
SRC = ROOT + r"\crates\slot\src\root.rs"
OUT = ROOT + r"\build\skeleton.txt"

text = io.open(SRC, encoding="utf-8").read()
start = text.index("pub const DIRS")
end = text.index("];", start)
block = text[start:end]
dirs = re.findall(r'"([^"]+)"', block)

header = (
    "# 卡目录骨架（由 build/gen-skeleton.py 生成，勿手改）\n"
    "#\n"
    "# 唯一来源：crates/slot/src/root.rs 的 DIRS。\n"
    "# 运行 crates/slot/tests/skeleton.rs 会在它和 DIRS 不一致时失败。\n"
)
io.open(OUT, "w", encoding="utf-8", newline="\n").write(header + "\n".join(dirs) + "\n")
print("wrote", len(dirs), "dirs to", OUT)
