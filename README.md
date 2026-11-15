# linkSlot

[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-4d4d57)](LICENSE)
[![Based on slot v1.6.0](https://img.shields.io/badge/based%20on-slot%20v1.6.0-4d4d57)](https://github.com/slot-cfw/slot)

**linkSlot** 是面向 Anbernic H700 系列掌机的 Game Boy 系前端，基于上游
[slot](https://github.com/slot-cfw/slot) v1.6.0 定制，增加了中文界面、
FC / SFC / MD 机种、Web 管理端与云存档同步（`linkSlot` CLI）。

> 本项目是独立维护的分支，与上游无隶属关系；上游版权与许可见
> [NOTICE](NOTICE) 与 [LICENSE](LICENSE)。

## 支持

| 项 | 内容 |
|---|---|
| 设备 | Anbernic H700 系（aarch64 Linux，glibc ≤ 2.31） |
| 机种 | GBA / GB / GBC / FC(NES) / SFC(SNES) / MD(Genesis) |
| 核心 | mGBA、gpSP、fceumm、**snes9x**、genesis_plus_gx（SFC 默认 snes9x；可选 supafaust 需自备） |
| 语言 | 简体中文、English |

> **SFC 说明**：SFC 默认使用随包分发的 **snes9x**（GPL-2.0，随包附对应源码）。
> 可选的 `supafaust_libretro.so` 因许可证与 GLIBC 兼容性原因不随包分发；把它放入
> `System/linkSlot/cores/supafaust/` 后会自动出现在核心选择器里。

## 安装

1. 下载 Releases 中的 `linkslot-v1.0.0-h700.zip`；
2. 解压后把 `<包名>/` 下的内容复制到 SD 卡（SD2）根目录；
3. 插入设备开机即可；ROM 放到 `Games/<机种>/`。

完整用户手册见 [README-ZH-CN.md](README-ZH-CN.md)。

## 构建

见 [build/README.md](build/README.md)。要求：Rust 1.96、Docker（或 aarch64 Linux 主机）。

## 目录

| 路径 | 内容 |
|---|---|
| `crates/` | Rust workspace |
| `card/` | 卡内脚本与默认资源 |
| `cores/` | 模拟器核心补丁与构建脚本 |
| `manifest/` | 核心与打包清单 |
| `doc/` | 设计文档与 mod API |
| `mods/` | 示例 mod |

## 许可与归属

GPL-3.0-or-later。本仓库包含基于 slot v1.6.0 的修改，上游作者与许可声明见
[NOTICE](NOTICE)。上游 README 曾披露其 Rust 前端由 AI 辅助完成并经过人工审阅。

## 免责声明

本项目与 Anbernic 无关；ROM 与 BIOS 需自备并确保合法持有。
