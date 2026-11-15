# 构建本修改版

本版已整合上游 Slot 1.9.0；主要改动为中文界面、Noto CJK 字库及四级 GLSL 渲染选项。

建议在 AArch64 Linux（glibc 2.31 或相近兼容环境）上构建。原版发布流程使用 ARM64 的 Rust bullseye 容器。

```sh
cargo +1.99.0 build --locked --profile device -p slot --no-default-features --features device
```

将 `target/device/slot` 复制到原 H700 运行包的 `System/slot`，添加 `card/System/shaders` 和新增许可证。已有模拟器核心可继续使用，无需重编译。字体与着色器都由 include_bytes/include_str 编译进程序。

本次交付实际使用 Windows Rust 1.99.0、AArch64 Rust 标准库、LLVM LLD 和 Debian bullseye ARM64 glibc 2.31 / GCC 10 sysroot 进行交叉编译，未改变运行包的动态库依赖要求。正式测试仍应在设备上完成。

新增着色器实现见 `crates/slot-gfx/src/four_pass.rs`。预设值固定在 PARAMETERS 常量中；默认缩放为中间级输入尺寸、末级显示区域尺寸，采样沿用 Slot 默认最近邻。`four-pass` 是持久化配置值。

界面文案以 `translations/` 目录下的结构化翻译表为准（编译期嵌入）。用户提供的包不含 BaseOS，因此不涉及底层系统页面和游戏 ROM 汉化。

`tools/qa` 是本次定向验证程序：直接使用 Slot 界面与渲染模块生成中文检查图，验证选项持久化、GLES2 四级渲染、图像方向及关闭后的状态恢复。Windows 上需要可用的 GNU Rust 链接工具链及 ANGLE 的 libEGL.dll / libGLESv2.dll，通过 SLOT_TEST_ANGLE 指向 DLL 目录，然后在 tools/qa 中执行 cargo run --release。ANGLE DLL 不随包分发。

原项目完整测试未在此 Windows 环境执行；部分旧测试以英文文案或原字体尺寸为期望，需要按中文界面维护。不要将本次定向验证表述为完整测试通过。

## R2

启动保护、字库裁剪和着色器按需初始化见 README-ZH-CN.md。构建后必须同时复制 card/System/launch_frontend.sh，并从用户原版 H700 包提取 System/slot 为 System/slot.original。嵌入字体为 assets/SlotSimplifiedChinese.otf，原始完整字体不再参与构建。tools/qa 增加可选着色器故障注入验证，故障注入只在 test-support 功能下编译，不包含在掌机发行构建中。

## 一键构建 aarch64 Linux 产物（build/）

Linux 上一键生成 SD 卡根目录产物（Slot + linkSlot + mGBA/gpSP 核心 + 卡上脚本 + 许可证）：

```sh
bash build/aarch64-linux.sh
# 产物：target/aarch64-linux/，把其内容复制到 SD 卡根目录即可
```

- 在 aarch64 Linux 上直接编译。
- 在 x86_64 Linux / macOS / Windows 上自动用 Docker（`rust:1-bullseye`, `linux/arm64`）构建，需要 Docker 与 arm64 模拟。
- Windows 也可用 `powershell -File build\aarch64-linux.ps1`（优先 WSL，其次 Docker Desktop）。
- 首次构建会下载并编译 mGBA / gpSP 核心，耗时较长；后续复用 `target/aarch64-build/` 缓存。
- 可选用 `--slot-original <原版 System/slot>` 让启动回退可用、`--tar` 生成压缩包、`--no-cores` 只迭代 UI。完整选项见 `build/README.md`。

该脚本等价于 `task dist:device OUT=target/aarch64-linux`，但不需要安装 `task`，并固定输出到 `target/aarch64-linux/`。

## 云同步 CLI（linkSlot）

`crates/linkslot` 是独立 CLI（不编入 `slot` 主二进制），构建与主程序分开：

- 主机快速检查（HTTP；不含 TLS，Windows 无 C 工具链也能编/测）：
  ```sh
  cargo test -p linkslot
  cargo build -p linkslot
  ```
- 设备发行构建（HTTPS 必须启用 `tls-rustls`，需要 ring 的 C 工具链）：
  ```sh
  # arm64 Linux 或 rust:1-bullseye arm64 容器里原生构建
  cargo build --profile linkslot --target-dir target-device -p linkslot --features tls-rustls

  # 或在 x86_64 Ubuntu VM 上交叉编译
  rustup target add aarch64-unknown-linux-gnu
  sudo apt install gcc-aarch64-linux-gnu
  CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
  AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
  CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --profile linkslot --target aarch64-unknown-linux-gnu -p linkslot --features tls-rustls
  ```
- 产物检查：
  ```sh
  file target-device/linkslot/linkSlot        # 必须是 aarch64
  readelf -V target-device/linkslot/linkSlot | grep GLIBC | sort -u   # 需要 ≤ 2.31（bullseye）
  ls -lh target-device/linkslot/linkSlot      # strip 后目标 < 5MB
  ```
- 部署文件：`System/linkSlot/bin/linkslot`（动态链接 AArch64 二进制）+ `System/linkSlot/shell/linkslot.sh`（`/bin/sh` 包装，经 `/lib/ld-linux-aarch64.so.1` 启动，FAT32 无可执行位也适用）+ `System/linkSlot/shell/linkslot-net.sh`（按需连接家用 Wi-Fi）。`task dist:device` / `task deploy:device` 已包含这三个文件。
- 配置：`linkSlot/Config/config.toml`，可用 `System/linkSlot/shell/linkslot.sh config init` 生成模板；密码建议用环境变量 `LINKSLOT_WEBDAV_PASSWORD`。
- 手持端行为（v0.6）：主界面选中卡带后长按 **Y 约 1.5 秒**触发 `linkSlot sync`；冲突时 UI 直接做“覆盖云端/覆盖本地”方向选择并自动备份被覆盖方。`push`/`pull`/`status` 保留给 PC/调试使用。
- 注意：本仓库原有的 Windows 构建链（Rust + LLD + bullseye sysroot）**没有交叉 C 编译器**，直接编 `--features tls-rustls` 会卡在 ring；需要先按上面任一方式准备工具链（M0 阶段确认）。主机默认构建不支持 HTTPS，`url` 为 https 时会给出配置错误。
