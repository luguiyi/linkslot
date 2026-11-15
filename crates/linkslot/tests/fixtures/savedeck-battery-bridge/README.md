# SaveDeck 电池存档桥（示例 mod）

这个 mod 自带 Web 管理页面：linkSlot 只托管页面、执行动作和提供受限 API，SaveDeck 的同步模式、WebDAV 选择、本地存档映射都在 `web/` 页面里配置。

这个版本使用 **manual（手动）动作**：启用 mod 后，长按 Y 对当前卡带执行 SaveDeck 同步；冲突时设备弹出选择框。它不再在 `after_sync` 自动扫描整张卡。

**与 GBAStation 的冲突**：两个 mod 都接管长按 Y。linkSlot 只运行优先级最高的那一个（`mod.json` 的 `order`，越小越优先，默认 100），另一个在日志里记为「优先级较低，长按 Y 已跳过」。想让 SaveDeck 生效，就在设备「mod 设置」里把本 mod 用左右键排到 GBAStation 前面；想换回去再排下来。两个都是 100 时按名称排序（GBAStation 在前），所以同时启用时默认是 GBAStation 赢。

映射列表不再写死在 Lua 中：配置保存在 mod 根目录的 `config.json`，页面通过 linkSlot 的受限 bridge 读取和保存它；下一次长按 Y 生效。

## 目录结构

```text
mods/savedeck-battery-bridge/
├── mod.json                 # schema 2，版本 1.5.7，作者 linkSlot，默认停用
├── README.md                # linkSlot mod 详情展示
├── config.json              # 本 mod 的配置（页面可编辑）
├── web/
│   ├── index.html           # linkSlot 打开的管理页面
│   ├── app.js
│   └── style.css
├── sticker.png            # NET 卡槽贴纸（512×145）
└── actions/
    └── sync/
        ├── action.json      # manual，默认启用；长按 Y 执行
        └── main.lua         # SaveDeck 同步实现
```

## 前置条件

1. 设备上的 linkSlot 支持 mod UI（`linkSlot version` 显示 `mods=1`）。
2. SaveDeck 镜像目录通过双向 WebDAV 暴露给 H700。Lua 不能直接读 Windows 路径；如果只是单向导出，不能自动写回 SaveDeck。
3. 在 linkSlot Web「云同步」新增一个不启用的同步方式，例如名称 `SaveDeck`，id 自动生成：
   - URL 指向 WebDAV 服务；
   - 远程根目录直接填 SaveDeck 镜像目录，例如 `/SaveDeck`；
   - 保存 WebDAV 用户名/密码。
4. 不需要再配置兼容存档目录：mod 会直接使用所选同步方式的远程根。
5. 把本 mod 目录复制到卡上的 `mods/savedeck-battery-bridge/`，或通过完整 mod ZIP 安装。
6. mod 默认停用；确认配置后在 mod 列表启用。

## 打开管理页面

1. 在 Web「mods」列表点击本 mod。
2. 点击「打开管理页面」。
3. 页面只列出「云同步」里真实的 WebDAV 同步方式；如果旧 config.json 的 profile 已不存在，会提示重新选择，不会补一个假的 `savedeck` 选项。
4. 页面可以：
   - 选择 WebDAV 同步方式（profile）；
   - 设置 `diagnose` / `two-way` / `pull` / `push`；
   - 设置 `ask` / `newest` / `local` / `remote` 冲突策略；
   - 开关 `romStem` 不匹配时按 `name` 匹配；
   - 新增、删除映射行和平台映射；
   - 保存 `config.json`；
   - 新建/编辑/启停/删除本 mod 的动作和 main.lua。
5. 保存配置后下一次长按 Y 生效，不需要重启。

配置文件示例：

```json
{
  "schema": 1,
  "kind": "linkSlot.savedeck",
  "profile": "p-mvnn8q9b-4va0",
  "mode": "two-way",
  "match_by_name": true,
  "on_conflict": "ask",
  "allow_create": false,
  "batch": 8,
  "platforms": ["GBA", "GB", "GBC"],
  "platform_map": {
    "GBA hack": "GBA"
  },
  "aliases": {
    "GBA/龙珠大冒险": "Dragon Ball - Advanced Adventure"
  }
}
```

匹配优先级：`平台映射` → `aliases` → SaveDeck `romStem` → SaveDeck `name`（`match_by_name=false` 时跳过最后一步）。

### 平台映射

SaveDeck 的 `game.json.platform` 和 linkSlot 的 `Saves/<平台>/` 不一定同名。页面的「平台映射」负责转换：

```text
SaveDeck platform  →  本地 Saves 目录
GBA hack           →  GBA
```

这样 `platform: "GBA hack"` 的 archive 会按 `GBA/<romStem>` 匹配本地 `Saves/GBA/...`，而不会去扫描不存在的 `Saves/GBA hack/`。archive 内部的 `platform` 字段保持不变，推送时也不会改它。

默认已带 `GBA hack → GBA`，也可以在页面增删行。

## 示例：`龙珠大冒险.sav`

本地存档：

```text
Saves/GBA/龙珠大冒险.sav
```

SaveDeck zip 的 `_savedeck/game.json`：

```json
{
  "platform": "GBA",
  "name": "龙珠大冒险",
  "romStem": "Dragon Ball - Advanced Adventure"
}
```

`romStem` 不匹配时页面会按 `name` 匹配；若 `name` 也不一致，在管理页添加一行：

```text
GBA/龙珠大冒险  →  Dragon Ball - Advanced Adventure
```

建议第一次先在页面选择 `diagnose`，长按 Y 运行一次，查看卡带界面的结果横幅和日志；确认匹配后再改为 `two-way` 或需要的方向。

诊断模式只读，不写本地存档、不写远端 zip、不保存 state。

## 同步行为

- 动作时机是 `manual`：启用 mod 后，长按 Y 不再是内置云同步，而是执行 SaveDeck 同步；停用 mod/动作恢复内置云同步。
- 每次长按只处理**当前卡带游戏**，不扫描/修改其它游戏的本地存档。
- 扫描所选同步方式远程根下的 `*.zip`，读取 `_savedeck/game.json`，按 `platform` + `romStem` 匹配；`match_by_name` 开启时也匹配 `game.name`。
- 优先使用 `state.json` 里记录的 archive；没有缓存时按 `batch` 上限扫描远端 archive。远端 archive 很多时若提示达到 batch 上限，调大 `batch` 后重试。
- 只有 SaveDeck 有、本地没有：写出 `Saves/<平台>/<romStem>.sav`；**之后** linkSlot 云同步才会发现这个本地存档。
- 只有本地存档、SaveDeck 没有对应 archive（典型情况：在 linkSlot 里玩过，但还没在 SaveDeck 里保存过这个游戏）：设备弹窗「SaveDeck 里还没有这个游戏」，提示先去 SaveDeck 打开并保存一次，再回来长按 Y。`diagnose` 模式不弹窗，只在结果横幅里给同样的建议。**弹窗那一次不写任何文件**；确认后自动重跑，以 noop + 同一句建议结束。`allow_create=false` 时不会自动创建 archive。
- 两边内容不同：只有一边变化时按变化方向同步；两边都变化或首次同步两边不同时按 `on_conflict` 处理。
- `on_conflict=ask`（默认）：设备弹窗「用本地覆盖 SaveDeck / 用 SaveDeck 覆盖本地」；A 确认，B 取消。**首次运行只弹窗，不写任何文件**；确认后自动重跑并执行所选方向。
- `on_conflict=newest/local/remote`：不弹窗，直接按策略同步。
- 覆盖前写入 `mods/savedeck-battery-bridge/backup/`。
- 写回只更新已存在的 archive，保留文件名、`game.json` 原有字段和存档条目名。
- MOVE 不被服务器支持或返回 409 时，linkSlot 0.0.30+ 会先备份原 archive 到同目录的 `.linkslot-mod-bak.*`，校验后再直接覆盖；失败会自动恢复并保留备份。

## 限制

- SaveDeck archive 文件名是它生成的不透明 ID，本 mod 不重算文件名，只覆盖已有 archive。
- 只处理 `.srm`/`.sav` 电池存档；`saveFileCount > 1` 会跳过。
- 写回是否被 SaveDeck 当前云同步客户端识别，取决于它如何处理 WebDAV 镜像。请先用无关游戏测试。
- 如果 SaveDeck 目录只是单向导出，写回只会改镜像，不会到 SaveDeck 云。
- 若同时启用 GBAStation 等其它 manual mod，linkSlot 只运行优先级最高的那一个（`mod.json` 的 `order`，越小越优先），被跳过的写进日志；在设备「mod 设置」里用左右键把本 mod 排到前面即可让它接管长按 Y。
- 冲突弹窗和「SaveDeck 里还没有这个游戏」提示都需要 linkSlot 0.0.29+；SaveDeck 的手动模式和 MOVE 安全回退建议使用 0.0.30+。

## 验证

- Web mod 详情应显示 README 和目录树，并能打开管理页面。
- 启用 mod 后长按 Y，确认执行 SaveDeck 而不是内置云同步；停用后恢复。
- 先选 `diagnose`，长按 Y，确认能看到当前游戏和匹配到的 archive；`state.json` 不新增。
- 在管理页添加/修改映射，保存 `config.json`，确认下一次长按 Y 使用新映射。
- 检查日志中的 `SaveDeck：拉取 ...` / `SaveDeck：推送 ...`，并确认 `backup/` 文件生成。
- 出现冲突时确认弹窗；按 B 后本地、远端和 state 均不变。
- 在一个 SaveDeck 里还没有的游戏上长按 Y：应弹「SaveDeck 里还没有这个游戏」，确认后横幅提示先去 SaveDeck 保存一次；本地 `.sav`、远端 zip、`state.json` 都不变。
