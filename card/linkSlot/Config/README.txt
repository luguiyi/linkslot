# linkSlot/Config —— 本前端的设置与状态

整个目录由 linkSlot 读写；删掉它会恢复默认设置（游戏、存档不受影响）。

| 文件 | 作用 |
|---|---|
| `slot.state` | 前端状态：音量、亮度、遮罩选择、每个机种的读档/存档开关等 |
| `theme.txt` | 主题调色板（也可由 mod 主题包提供） |
| `language.txt` | 界面语言（`zh-CN` / `en-US`） |
| `selected_core.txt` | 每个游戏手动选定的核心（`<游戏名> = <核心>`） |
| `carousel.txt` | 货架轮播（最近/收藏）设置 |
| `controls.txt` | 按键映射 |
| `shortcuts.txt` | 快捷键 |
| `cart_color.txt` | 卡带颜色覆盖 |
| `cart_shell.txt` | 卡带外壳样式覆盖 |
| `video_mode.txt` | 视频模式 |
| `wallpaper.deck` | 壁纸卡组 |
| `bootlogo.bmp` | 自定义开机图（可选） |
| `config.toml` / `storages.toml` / `log_level.txt` | linkSlot 同步 CLI 的配置与日志级别 |
| `web.toml` | 内置 Web 管理页设置 |
| `cache/` / `work/` / `upgrade/` | 同步缓存、临时工作区、升级暂存 |

日志在 `linkSlot/Logs/` 下，不放这里。
