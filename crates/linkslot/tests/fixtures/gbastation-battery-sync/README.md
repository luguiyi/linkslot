# GBAStation 电池存档同步（示例 mod）

把本机的 GBA / GB / GBC 电池存档（`.sav` / `.srm`）与局域网里 GBAStation 的 Web 服务做手动双向同步。

启用这个 mod 后，**长按 Y 会执行本 mod 的手动同步，不再触发原来的云同步**；停用 mod（mods 列表里的总开关）即可恢复长按 Y 的云同步行为。

**与其它存档桥的冲突**：如果同时启用了另一个也接管长按 Y 的 mod（例如 SaveDeck），linkSlot 只运行优先级最高的那一个（`mod.json` 的 `order`，越小越优先，默认 100），被跳过的一方会写进日志。在设备「mod 设置」里用左右键调整顺序：排在最上面的那个生效。两者都是默认 100 时按名称排序，GBAStation 在前。

- 版本：1.3.7
- 作者：linkSlot
- 时机：`manual`（长按 Y）
- 平台：`GBA` / `GB` / `GBC`
- 冲突选择：`first_sync=ask` / `on_conflict=ask` 时在设备上弹窗，需 linkSlot 0.0.29+
- 兼容：管理页同时接受动作 id 为 `sync` 或 Web 导入生成的 `a-...` id
- 游戏映射：逐行填写「linkSlot 游戏名 → GBAStation id / 标题 / ROM 文件名」，不需要手写 JSON
- 游玩时间：成功同步后，把本机 `playtime.json` 与 GBAStation `playTime` 合并；首次为两边相加，之后按新增时长累加，写入 `Logs/linkSlot.log`
- 只有电池存档，不处理即时存档、金手指、封面

## 目录结构

```text
mods/gbastation-battery-sync/
├── mod.json                       # schema 2，默认停用
├── README.md
├── config.json                    # 管理页读写的配置
├── actions/
│   └── sync/
│       ├── action.json            # manual，order 100，默认启用
│       └── main.lua               # 同步实现
├── sticker.png                    # NET 卡槽贴纸（512×145）
└── web/
    ├── index.html                 # linkSlot 内打开的管理页
    ├── app.js
    └── style.css
```

## 前置条件

1. 设备上的 linkSlot 构建支持 **manual 执行时机** 和 mod 的 **http API**（`linkSlot version` 可看到 mods 支持信息）。
2. GBAStation 正在同一局域网内运行，并已打开它的 **Web 服务**。GBAStation 的 Web 管理页会显示访问地址，通常是：

   ```text
   http://192.168.1.23:8080
   ```

   以实际显示的 IP 和端口为准；管理页只保留协议、主机和端口。
3. 要同步的游戏在 **linkSlot 游戏库**和 **GBAStation 游戏库**里都存在。两边的 ROM 文件名/标题需要能对应上，必要时在管理页的「游戏映射」中按行配置。
4. 设备与 GBAStation 处于同一个**可信局域网**。GBAStation 的 Web 服务没有登录口令，不要把它暴露到公网。

## 安装

1. 把构建产出的独立 mod ZIP（`linkslot-mod-*.zip`）解压到存储卡根目录，得到：

   ```text
   存储卡根/mods/gbastation-battery-sync/
   ```

   也可以手工把本目录复制到 `mods/gbastation-battery-sync/`。
2. 确认 `mod.json`、`config.json`、`actions/sync/action.json`、`main.lua` 和 `web/index.html` 都在对应位置。
3. mod 和动作默认状态：mod **停用**，动作 `sync` **启用**。两个开关都启用后才会执行。

## 打开管理页并配置

1. 在 linkSlot Web 的 **mods** 列表点击「GBAStation 电池存档同步」。
2. 点击「打开管理页面」。
3. 填写：

   | 字段 | 说明 |
   | --- | --- |
   | GBAStation Web 地址 | 例如 `http://192.168.1.23:8080`。只填协议 + IP + 端口；保存时管理页会只保留来源（origin），不会保留路径。 |
   | mode | `two-way` 双向；`pull` 只下载（GBAStation → 本机）；`push` 只上传（本机 → GBAStation）。 |
   | first_sync | 第一次同步时两边都有不同存档的处理：`remote` 用 GBAStation 覆盖本地；`local` 用本地覆盖 GBAStation；`ask` 在设备上弹窗让你选择。 |
   | on_conflict | 两边都改过时的处理：`ask` 在设备上弹窗；`remote` GBAStation 覆盖本地；`local` 本地覆盖 GBAStation。 |
   | backup | 覆盖前是否写备份，默认开。 |
   | verify | 上传完成后是否重新下载并比较 SHA-256，默认开。 |
   | 游戏映射 | 可选，按行填写本机 linkSlot 游戏名和 GBAStation 的游戏 id / 标题 / ROM 文件名。 |

4. 点击「保存配置」。配置文件是 `mods/gbastation-battery-sync/config.json`，下一次长按 Y 生效，不需要重启。
5. 回到 mods 列表，把本 mod 的总开关设为**启用**；确认动作 `sync` 也是启用状态。
   打开或保存管理页不会自动启用 mod，必须手动打开总开关。

### 游戏映射示例

本机 linkSlot 里的游戏名是 `龙珠大冒险`，GBAStation 里可能是英文标题。若自动匹配失败，在管理页「游戏映射」点「+ 添加映射」，按行填写：

| linkSlot 游戏名 | GBAStation 的 id / 标题 / ROM 文件名 |
| --- | --- |
| `龙珠大冒险` | `Dragon Ball - Advanced Adventure` |

不需要手写 JSON；页面会按行保存到 `config.json` 的 `games` 对象。留空行自动忽略，重复的游戏名会提示合并。值也可以直接填 GBAStation 页面里显示的游戏 id（`/api/games` 返回的 `id` 字段）。游戏 id 可能包含斜杠或完整路径，本 mod 会把它作为单个路径段交给 http API，不会拼坏 URL。

匹配顺序：

1. `games` 中与本机游戏名完全一致的映射（其次是不区分 ASCII 大小写的同名映射）；
2. 同平台 + GBAStation 的 ROM 路径文件名 / 标题**完全一致**；
3. 同平台 + 忽略 ASCII 大小写 / 规范化标点后的匹配。

同一平台上匹配到 0 个或多个游戏都会报错，并提示你用 `games` 映射指定 id。

## 使用：长按 Y

1. 在 linkSlot 的卡带/存档列表选中要同步的游戏。
2. **长按 Y**。linkSlot 执行 `manual` 动作，本 mod 开始同步。
3. 结果会通过 `mods.report` 显示在卡带界面的结果横幅，并写入 linkSlot 日志；管理页也会显示动作状态。
4. 需要先切回云同步时，到 mods 列表**停用本 mod**即可。

在管理页保存的配置只影响后续的手动同步，不会自动触发同步。

## 游玩时间

成功同步存档后，本 mod 还会合并游玩时间：

- 本机来源：`States/<平台>/<核心>/<游戏名>/playtime.json` 的 `total_seconds`；
- GBAStation 来源：`/api/games` 返回的 `playTime`（秒）；
- 首次同步把两边已有时间相加；
- 之后在 `mods/gbastation-battery-sync/state/playtime/<平台>/<key>.json` 保存基线，只累加两边新增时长，避免重复同步造成翻倍；
- 合并后的值写回本机 `playtime.json`，并通过 `PUT /api/game/<id>` 写回 GBAStation；
- 无基线且两边数值相同（例如 mod 状态文件丢失）时按已同步处理，避免重复相加；
- 游玩时间同步失败只记录警告，不改变存档同步结果。

## 同步规则

本机存档读取顺序：优先 `.sav`，没有有效 `.sav` 时回退到 `.srm`。如果 `.sav` 是 0 字节，会继续尝试 `.srm`。

### 首次同步（没有本地状态文件时）

- 只有本机有存档：上传到 GBAStation（`mode=pull` 时跳过并提示）。
- 只有 GBAStation 有存档：下载到本机（`mode=push` 时跳过并提示）。
- 两边都有且 SHA-256 相同：只记录状态，不传输。
- 两边都有但不同：按 `first_sync` 处理；`ask` 会在设备上弹窗让你选 GBAStation 还是本地。

### 后续同步（有状态文件时）

游玩时间的合并独立于存档方向，成功后执行一次；状态保存在 `mods/gbastation-battery-sync/state/playtime/`。

- 只有本机变化：上传（`mode=pull` 时跳过）。
- 只有 GBAStation 变化：下载（`mode=push` 时跳过）。
- 两边都变化：属于冲突，按 `on_conflict` 处理；`ask` 会在设备上弹窗。
- 两边都没有变化或内容一致：`noop`。

`ask` 的弹窗有两个选项：「用 GBAStation 覆盖本地」和「用本地覆盖 GBAStation」。↑/↓ 选择，A 确认，B 取消；取消不会写任何文件。需要 linkSlot 0.0.29+；旧版会回退为提示你到管理页改 `first_sync`/`on_conflict`。

### 状态文件

同步状态写在：

```text
mods/gbastation-battery-sync/state/<平台>/<key>.json
```

`key` 是 `hash.sha256(平台 .. "\n" .. 游戏名)` 的前 16 个十六进制字符。状态里记录上一次的本地/远端 SHA-256、远端存档路径和本地路径。不要手工编辑；删除它只会让下一次同步按「首次同步」重新判断。

## 备份

覆盖前会把被覆盖的一侧写到固定文件名：

```text
mods/gbastation-battery-sync/backups/<平台>/<key>-local.sav    # 下载覆盖本机前，备份原本地存档
mods/gbastation-battery-sync/backups/<平台>/<key>-remote.sav   # 上传覆盖远端前，备份原远端存档
```

注意：

- 文件由本 mod 直接写入，**没有删除 API**；每次覆盖会替换同名备份文件，不看时间戳。
- 如果备份写入失败，mod 会停止本次传输，不覆盖目标。
- 备份只是方便手工恢复；它们不会自动上传或回滚。需要保留历史版本时，请把 `backups/` 复制到电脑。
- `backup=false` 时完全不写备份，风险自负。

## 上传与校验

上传流程：`save/start` → `upload/chunk`（一次写入全部字节）→ `upload/finish`。

- 上传前若远端已有存档，先写 `-remote.sav` 备份。
- `verify=true` 时，上传完成后会再下载一次远端存档，与本地 SHA-256 比较；不一致会报告 `verify` 错误且不更新状态文件。下载由 linkSlot 的原子写入完成，不再额外回读。
- 如果 chunk 失败，会尽力调用 `upload/cancel`；如果 finish 失败，则不会调用 cancel——因为 GBAStation 的 cancel 会删除目标存档，而此时数据其实可能已经写完整。请根据 `-remote.sav` 备份人工检查/恢复。
- **finish 失败后不要立刻再次长按 Y**：下一次同步可能把远端那份不完整的副本当成「远端变化」下载到本机。先确认 GBAStation 里的存档是否完整；需要时用 `-remote.sav` 恢复，再重新同步。
- **上传中断风险**：GBAStation 的电池存档上传会直接写 `<ROM名>.sav`，不是事务性的。Wi-Fi 不稳定、设备断电、GBAStation 退出都可能导致远端存档被截断。不要在重要存档上使用不稳定的网络；同步前把 `backups/` 复制到电脑。详见「限制与风险」。

## 限制与风险

- **只同步电池存档**：`.sav` / `.srm`。不处理即时存档（`.ssN`）、截图、金手指、ROM 或封面。
- **不新建游戏**：GBAStation 里必须已经有对应游戏；本 mod 不需要、也不会向 GBAStation 导入 ROM。
- **不删除存档**：两边都没有存档时只报告 `noop`；不会调用任何删除接口。
- **只支持 GBA / GB / GBC**。其它平台长按 Y 会直接跳过。
- **单个电池存档**：GBAStation 返回多个电池存档条目时会报错，不自动选择。
- **无时间戳冲突判断**：是否变化基于 SHA-256，不比较文件修改时间。
- **GBAStation API 属于其内部 Web API**：GBAStation 升级后接口可能变化；若接口不兼容，本 mod 会报告 HTTP/JSON 错误，需要更新 mod。
- **仅可信局域网**：GBAStation Web 服务没有登录口令，本 mod 也不发送任何凭据。不要把它转发到公网，也不要在不可信 Wi-Fi 上使用。
- **mod 脚本可以覆盖存档**：只装入/启用你信任的 mod。
- **长按 Y 被接管**：启用本 mod 后，长按 Y 执行手动 mod；停用后恢复原来的云同步。

## 排障

| 现象 | 处理 |
| --- | --- |
| 提示「尚未配置 GBAStation Web 地址」 | 打开管理页填写 `base_url`，保存，再确认 mod 总开关已启用。 |
| 请求失败 / `HTTP 404` | 确认 GBAStation 正在运行且 Web 服务已开启；在同网段电脑浏览器打开 `http://IP:8080/api/games` 验证。 |
| 提示找不到游戏 | 检查两边游戏名/平台；在管理页「游戏映射」里添加一行，把 linkSlot 游戏名映射到 GBAStation id 或标题。 |
| 提示「首次同步」或「两边都改过」 | 设备会弹出选择框；若没有弹窗，确认版本是 linkSlot 0.0.29+，或把 `first_sync` / `on_conflict` 改成 `remote` / `local` 后重试。 |
| `mode=pull` / `mode=push` 跳过 | 改成 `two-way`，或按需要改成允许该方向的值。 |
| 上传后校验失败 | Wi-Fi 可能不稳定；检查 `backups/<平台>/<key>-remote.sav`，必要时在 GBAStation 里手工恢复，然后重试。 |
| 想恢复原来的长按 Y 云同步 | 在 mods 列表停用本 mod；停用后不会执行本 mod。 |
| 配置保存冲突 | 管理页点「重新读取」后再保存；页面使用 revision 乐观并发。 |

## 配置示例

```json
{
  "schema": 1,
  "kind": "linkSlot.gbastation",
  "base_url": "http://192.168.1.23:8080",
  "mode": "two-way",
  "first_sync": "remote",
  "on_conflict": "ask",
  "backup": true,
  "verify": true,
  "platforms": ["GBA", "GB", "GBC"],
  "games": {
    "龙珠大冒险": "Dragon Ball - Advanced Adventure"
  }
}
```

## 验证清单

- mods 列表能看到本 mod，详情能显示 README 和目录树，并能打开管理页。
- 管理页能读取/保存 `config.json`；保存后下一次长按 Y 使用新配置。
- 先选 `first_sync=ask` 或 `on_conflict=ask` 验证提示，再切到 `remote` / `local` 验证实际传输。
- 成功上传后检查 GBAStation 的存档列表；成功下载后检查本机 `.sav`/`.srm` 和 `backups/` 文件。
- 关闭 `verify` 与打开 `verify` 各跑一次，确认请求数和结果符合预期。
- 停用 mod 后长按 Y 应恢复云同步，不再出现本 mod 的日志。
