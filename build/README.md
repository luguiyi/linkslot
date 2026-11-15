# 构建（唯一的构建说明）

本目录是 linkSlot 的**唯一构建入口**。旧的 `build/aarch64-linux.sh` 保留一个版本，
`build/dist.sh --legacy` 会转发过去；文档与 CI 只使用 `build/dist.sh`。

```
build/
├─ dist.sh           # 唯一出包实现：h700 整包 / host 桌面构建
├─ dist.ps1          # Windows 入口（探测 sh 或 WSL 后转发）
├─ verify.sh         # 产物校验（结构 / 架构 / GLIBC / 版本 / 许可证）
├─ package.py        # 可复现 zip 打包
├─ fetch-cores.sh    # 按 manifest/cores.toml 取/编 6 个核心
├─ clean-build.sh    # 用 git archive 导出干净树后完整出包（等价 fresh clone）
├─ modpack.py        # 把 mods/<id>/ 打成可分发的 mod ZIP（顶层 mods/<id>/）
├─ manifest.py       # 读取 manifest/cores.toml（无第三方依赖）
├─ skeleton.txt      # 卡目录骨架（由 gen-skeleton.py 从 root.rs DIRS 生成）
├─ container/        # 构建镜像 Dockerfile
└─ README.md
```

## 前置条件

| 构建主机 | 要求 |
|---|---|
| aarch64 Linux（推荐） | Rust 1.96（`rust-toolchain.toml` 自动拉取）、git、curl、cmake、make、g++、python3、file、zip、binutils；**glibc ≤ 2.31**（高于 2.31 会自动改走容器） |
| Windows / macOS / x86_64 Linux | Docker Desktop 或 Docker Engine，支持 `linux/arm64`（x86_64 需 QEMU binfmt） |
| 磁盘 | ≥ 40 GB 空闲（构建缓存 + 核心源码） |

Windows 推荐流程：

```powershell
# 1. 确认 Docker 可用且能跑 arm64 容器
docker run --rm --platform linux/arm64 alpine uname -m     # 应输出 aarch64

# 2. 出包（自动在容器里构建）
powershell -ExecutionPolicy Bypass -File build\dist.ps1

# 3. 校验（发布门槛）
powershell -ExecutionPolicy Bypass -File build\dist.ps1 --help
```

## 用法

```sh
# 生成 SD 卡整包 → artifacts/linkslot/
sh build/dist.sh h700

# 发布用：出包时直接跑发布门槛（许可证 / 对应源码 / GLIBC）
sh build/dist.sh h700 --release

# 只组装、不打包
sh build/dist.sh h700 --no-package

# 复用已构建的核心（调试 UI 用，不能作为发布包）
sh build/dist.sh h700 --no-cores

# 只出桌面版（不碰核心、不出卡包）
sh build/dist.sh host

# 干净树验证：导出 HEAD → 构建 → 校验（不依赖工作区未跟踪文件）
sh build/clean-build.sh

# 打包全部 mod（输出 dist/mods/linkslot-mod-<id>-v<version>.zip + SHA256SUMS）
python3 build/modpack.py --bundle
# 发布时可固定时间戳，保证同样输入得到同样的 ZIP：
SOURCE_DATE_EPOCH=$(git log -1 --format=%ct) python3 build/modpack.py --bundle

# 校验产物（--release 为发布门槛）
sh build/verify.sh artifacts/linkslot --release

# 打 zip（可复现：同一输入两次 SHA256 一致）
python3 build/package.py --dir artifacts/linkslot --out artifacts/linkslot-v1.0.0-h700.zip
```

## 产物

```
artifacts/linkslot/
├─ Games/ Saves/ States/ Labels/ Guides/ Info/ Cheats/ Bootlogos/ Overlay/
│  Wallpapers/ Music/ BIOS/            # 公共区（跨前端共享）
├─ linkSlot/                           # 本前端私有
│  ├─ Config/（前端设置与状态；见其中的 README.txt）
│  ├─ Fronts/  Metadata/  mods/
│  ├─ Logs/
│  └─ version.json       # 版本、commit、upstream、逐文件 sha256
├─ System/
│  ├─ launch_frontend.sh    # BaseOS 入口（唯一留在 System 根的文件）
│  └─ linkSlot/
│     ├─ bin/
│     │  ├─ linkslot        # 前端主程序（aarch64）
│     │  ├─ linkslot_sync   # 云同步 CLI（TLS + Lua）
│     │  └─ fsck.exfat
│     ├─ shell/             # 全部 .sh（launch.sh、linkslot.sh、slot-wifi.sh…）
│     ├─ cores/<核心>/      # 每个核心一个目录（mgba、gpsp、snes9x…）
│     ├─ assets/            # bezels、carts、bootlogo.bmp
│     └─ licenses/          # 许可证、补丁、GPL 核心对应源码
└─ README-ZH-CN.md
```

## 核心与合规

- 核心清单一律以 `manifest/cores.toml` 为准；
- `kind = "build"`：用 `cores/<id>/build.sh` 现场编译（源码/补丁随仓库）；
- `kind = "vendor"`：从 `vendor/<dylib>` 取预编译产物并校验 sha256；缺失时回退
  `card/System/<dylib>`（仅开发机，不入库）；
- **发布门槛**：`verify.sh --release` 会拒绝缺许可证、缺对应源码、GLIBC > 2.31 的核心。
  `supafaust` 标记为 `optional = true`：**不随发布包分发**（许可证未知、要求 GLIBC 2.34），
  因此不会阻断发布；需要 SFC 的用户自行把核心放进 `System/linkSlot/cores/supafaust/`。

## 常见问题

| 现象 | 处理 |
|---|---|
| `exec format error` | `docker run --privileged --rm tonistiigi/binfmt --install arm64` |
| Docker Hub 不可达 | `DOCKER_IMAGE=<镜像站>/library/rust:1.96-bullseye sh build/dist.sh h700` |
| 宿主 glibc 太新 | 脚本自动改走容器，无需手动处理 |
| 核心缺失 | 把 `.so` 放进 `vendor/`，或修正 `manifest/cores.toml` |
| 产物结构不对 | `sh build/verify.sh <目录>` 会逐项列出缺什么 |
