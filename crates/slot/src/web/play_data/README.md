# play_data — browser GBA runtime

Vendored, offline copies of the browser emulator so the device never reaches the
internet at runtime.

| file | source | license |
|---|---|---|
| `loader.js` | EmulatorJS 4.2.3 `data/loader.js` | GPL-3.0 (`licenses/emulatorjs-GPL-3.0.txt`) |
| `emulator.min.js` | EmulatorJS 4.2.3 `data/emulator.min.js` | GPL-3.0 |
| `emulator.min.css` | EmulatorJS 4.2.3 `data/emulator.min.css` | GPL-3.0 |
| `version.json` | EmulatorJS 4.2.3 `data/version.json` | GPL-3.0 |
| `localization/zh-CN.json` | EmulatorJS 4.2.3 `data/localization/zh-CN.json` | GPL-3.0 |
| `compression/extract7z.js` | EmulatorJS 4.2.3 `data/compression/extract7z.js` | GPL-3.0 |
| `compression/extractzip.js` | EmulatorJS 4.2.3 `data/compression/extractzip.js` | GPL-3.0 |
| `cores/mgba-legacy-wasm.data` | EmulatorJS 4.2.3 `data/cores/mgba-legacy-wasm.data` | mGBA MPL-2.0 (`licenses/mgba-MPL-2.0.txt`) |
| `cores/gambatte-legacy-wasm.data` | EmulatorJS 4.2.3 `data/cores/gambatte-legacy-wasm.data` | Gambatte GPL-2.0 (`licenses/gambatte-GPL-2.0.txt`) |
| `cores/fceumm-legacy-wasm.data` | EmulatorJS 4.2.3 `data/cores/fceumm-legacy-wasm.data` | FCEUmm GPL-2.0 (`licenses/fceumm-GPL-2.0.txt`) |

Why the **legacy**, **non-threaded** core:

- EmulatorJS only uses a threaded core when `SharedArrayBuffer` exists, and the
  page is served over plain HTTP on the LAN, where it does not (that needs a
  cross-origin-isolated secure context), so the threaded file is dead weight.
- `legacy` is chosen when the optional core report JSON is absent; it is
  WebGL1-compatible and therefore the most forgiving of the two.

Refresh instructions: download the same files from
`https://cdn.emulatorjs.org/stable/data/` (or a pinned version) and bump the
version above.
