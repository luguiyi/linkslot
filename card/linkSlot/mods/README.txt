linkSlot mods 目录
=============================

每个 mod 是一个目录（容器），必须有 README 和 Web 管理页面：

  linkSlot/mods/<mod-id>/mod.json
      mod 信息（schema=2、name、version、author 可选、enabled、order 可选）
  linkSlot/mods/<mod-id>/README.md
      在 linkSlot mod 详情里展示的简介
  linkSlot/mods/<mod-id>/web/index.html
      这个 mod 自己的管理页面（必需）
  linkSlot/mods/<mod-id>/config.json
      可选：mod 自己的配置
  linkSlot/mods/<mod-id>/actions/<action-id>/action.json
      动作信息（schema=1、name、timing、enabled、order）
  linkSlot/mods/<mod-id>/actions/<action-id>/main.lua
      该动作的 Lua 5.4 脚本（UTF-8，最大 256 KiB）
  linkSlot/mods/<mod-id>/actions/<action-id>/sticker.png
      可选：该手动动作在长按 Y 卡片上的贴纸（透明底 PNG，建议 ≤1024px、≤4 MiB）
  linkSlot/mods/<mod-id>/sticker.png
      可选：整个 mod 共用的贴纸；动作级贴纸优先，都没有时用内置文字贴纸

<mod-id> 和 <action-id> 只能用小写字母、数字、_ 和 -，并以小写字母或数字开头。
mod 的 version 默认 1.0.0，只能使用字母、数字和 . _ + -（最长 32 字符）；
独立 mod ZIP 文件名会带上该版本，例如 linkslot-mod-<mod-id>-v1.2.0.zip。
每个动作有自己的执行时机和启用状态；mod 的 enabled 是总开关，停用 mod 后其下所有动作都不执行。

启用方式：
  1. 推荐：设备 Web 管理 →「mods」页点击 mod，打开该 mod 自带的管理页面，配置动作和 mod 选项；
  2. 或手工修改 mod.json / action.json 的 "enabled"。
  改动在下一次云同步或长按 Y 时生效。发布包不带任何已启用 mod。

安装方式：
  - 推荐通过 Web「mods → 导入」安装完整 mod ZIP；
  - 也可把 mod 目录复制到 linkSlot/mods/<mod-id>/。
  - 没有 web/index.html 的 mod 缺少管理页面，无法在 Web 中管理。

执行时机（action.json 的 timing）：
  before_upload / after_upload / before_download / after_download / after_sync / manual

  after_sync 在整次 sync 完成后运行一次，即使本次没有传输或全部 noop。
  manual 不在云同步中运行：它登记到卡带界面的长按 Y 菜单。存在启用的 manual
  动作时，长按 Y 会先弹出卡片（内置「同步云存档」+ 每个启用的手动动作一张），
  左右选择、A 开始、B 关闭；app 用 mods run --id <mod> --action <action>
  只运行选中的那一个。没有启用的 manual 动作时，长按 Y 仍是直接云同步。
  manual 可用 mods.choose{...} 在设备上弹出 2–4 个选项让用户选择（A 确认 /
  B 取消）；首次运行会返回 choice 请求，设备用 --choice 重新运行。
  同一时机的动作按 mod 的 order、action 的 order、名称排序。

优先级（mod.json 的 order，可选，默认 100，数字越小越优先）：
  只影响长按 Y 卡片列表与 mod 设置页的顺序，不再决定谁「抢到」长按 Y：
  同时启用 GBAStation 与 SaveDeck 时，两张卡片都会出现，由用户选择。
  设备「mod 设置」页按优先级从高到低列出 mod，选中后按左右键调整顺序；
  调整结果会写回各 mod 的 mod.json。

旧版 schema=1 的单脚本 mod（mod.json 带 timing、根目录 main.lua）不再支持，
请重新安装新版动作结构 mod，或在 Web「mods」页卸载旧结构。

安全警告：
  mod 可以读写 States/、Saves/ 和它自己的 linkSlot/mods/<mod-id>/，并按 mod
  管理页选择的 WebDAV 同步方式远程根访问 WebDAV；还可通过 zip API 解析/生成
  内存中的 zip。manual 动作还可通过受控 http API 访问该 mod config.json 中
  base_url 指向的一个服务（例如局域网里的 GBAStation）。只启用你信任的 mod，
  不要运行来源不明的 main.lua；启用前请备份存档。

完整 API、限制和示例见项目文档 `doc/mod-api.md`，以及
Web「mods」页的 API 速查。

说明：
  - 只识别目录；linkSlot/mods/ 下平铺的 .lua 不会执行。
  - Web 删除动作会移到 linkSlot/mods/<mod-id>/.trash/actions/；卸载 mod 会永久删除整个 mod 目录。
  - 本目录及 mod 不参与云同步，也不会被发布包覆盖。
  - linkSlot 提供的是通用 Lua 适配框架，不承诺 SaveDeck 原生兼容。
