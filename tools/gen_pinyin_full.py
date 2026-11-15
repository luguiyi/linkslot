"""Generate slot-store's compact full-pinyin table from pinyin-data.

Usage:
    python tools/gen_pinyin_full.py pinyin.txt crates/slot-store/src/pinyin_full.bin

Input is mozillazg/pinyin-data's pinyin.txt (MIT).  The output is:
    u32 count
    count * u32 blob offsets
    NUL-terminated, comma-separated ASCII readings (tone marks stripped)
The table covers U+3400..U+9FFF.
"""

from __future__ import annotations

import re
import struct
import sys
import unicodedata

START = 0x3400
END = 0xA000
COUNT = END - START

_UMLAUT = {"ü": "v", "ǖ": "v", "ǘ": "v", "ǚ": "v", "ǜ": "v"}


def ascii_reading(raw: str) -> str:
    raw = raw.strip().lower()
    for src, dst in _UMLAUT.items():
        raw = raw.replace(src, dst)
    decomposed = unicodedata.normalize("NFD", raw)
    stripped = "".join(c for c in decomposed if not unicodedata.combining(c))
    return "".join(c for c in stripped if "a" <= c <= "z")


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    source, target = sys.argv[1], sys.argv[2]
    entries: dict[int, list[str]] = {}
    line_re = re.compile(r"^U\+([0-9A-Fa-f]+):\s*([^#]*)")
    with open(source, encoding="utf-8") as fh:
        for line in fh:
            match = line_re.match(line)
            if not match:
                continue
            cp = int(match.group(1), 16)
            if not START <= cp < END:
                continue
            readings: list[str] = []
            for raw in match.group(2).split(","):
                reading = ascii_reading(raw)
                if reading and reading not in readings:
                    readings.append(reading)
            if readings:
                entries[cp] = readings

    blob = bytearray()
    offsets: list[int] = []
    for cp in range(START, END):
        offsets.append(len(blob))
        text = ",".join(entries.get(cp, []))
        blob.extend(text.encode("ascii"))
        blob.append(0)

    with open(target, "wb") as fh:
        fh.write(struct.pack("<I", COUNT))
        fh.write(struct.pack("<%dI" % COUNT, *offsets))
        fh.write(blob)
    print(f"wrote {target}: {len(offsets)} entries, {len(blob)} bytes blob")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
