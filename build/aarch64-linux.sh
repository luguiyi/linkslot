#!/bin/sh
# [legacy] 旧的一键构建脚本，保留一个版本用于回退。
#
# 新入口：build/dist.sh（文档与 CI 只用它）；本脚本等价于
#   sh build/dist.sh h700 --legacy
# 计划在 v1.1 删除。
#
# 一键构建 RG34XX / H700 可用的 aarch64 Linux 前端产物。
#
# 用法：在仓库根目录执行
#   bash build/aarch64-linux.sh
# 产物：target/aarch64-linux/（SD 卡根目录结构，直接复制内容到卡根目录）
#
# 构建环境：
#   * Linux aarch64 主机：直接编译；
#   * 其他主机（Linux x86_64 / macOS / Windows WSL）：自动用 Docker
#     rust:1-bullseye (linux/arm64) 构建，需要 Docker + arm64 模拟。
# 详见 build/README.md。

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)

OUT_DEFAULT="$ROOT/target/aarch64-linux"
BUILD_DEFAULT="$ROOT/target/aarch64-build"
OUT="$OUT_DEFAULT"
BUILD="$BUILD_DEFAULT"
DOCKER_IMAGE=${DOCKER_IMAGE:-rust:1-bullseye}
DOCKER_PLATFORM=${DOCKER_PLATFORM:-linux/arm64}
JOBS=${JOBS:-}
SLOT_ORIGINAL=${SLOT_ORIGINAL:-}
SLOT_BUILD_DATE=${SLOT_BUILD_DATE:-$(date -u +%Y-%m-%d)}
export SLOT_BUILD_DATE
if command -v git >/dev/null 2>&1; then
	SLOT_GIT_HASH=${SLOT_GIT_HASH:-$(git -C "$ROOT" rev-parse --short=6 HEAD 2>/dev/null || true)}
	if [ -z "${SLOT_GIT_DIRTY:-}" ]; then
		if [ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no 2>/dev/null || true)" ]; then
			SLOT_GIT_DIRTY=1
		else
			SLOT_GIT_DIRTY=0
		fi
	fi
fi
[ -n "${SLOT_GIT_HASH:-}" ] && export SLOT_GIT_HASH
[ -n "${SLOT_GIT_DIRTY:-}" ] && export SLOT_GIT_DIRTY
TAR=0
NO_CORES=0
DRY=0
IN_CONTAINER=0

usage() {
	cat <<'EOF'
用法：bash build/aarch64-linux.sh [选项]

把 Slot 前端、linkSlot CLI、mGBA/gpSP/fceumm 核心和卡上脚本打包成
SD 卡根目录结构，输出到 target/aarch64-linux/。

选项：
  --out DIR            输出目录（默认 target/aarch64-linux）
  --build-dir DIR      Cargo / 核心的中间产物目录（默认 target/aarch64-build）
  -j, --jobs N         Cargo 并行任务数（默认由 cargo 决定）
  --slot-original FILE 原版 H700 的 System/slot，复制为 System/slot.original
                       供 launch_frontend.sh 启动失败时回退；也可用环境变量
                       SLOT_ORIGINAL 指定，或放到 vendor/slot.original
  --tar                额外生成 <输出目录>.tar.gz
  --no-cores           不编译核心（仅调试 UI 用；需已有缓存核心，不能作为发布包）
  --dry-run            只打印将执行的命令，不实际执行
  -h, --help           显示本帮助

环境变量：
  DOCKER_IMAGE         容器镜像（默认 rust:1-bullseye）
  DOCKER_PLATFORM      容器平台（默认 linux/arm64）
  JOBS                 同 --jobs
  SLOT_ORIGINAL        同 --slot-original

前提：
  * aarch64 Linux 原生构建：已安装 Rust（rust-toolchain.toml 会拉取 1.96）、
    git、curl、cmake、make、gcc 和 binutils；
  * 非 aarch64 主机：Docker，且支持 linux/arm64（x86_64 上需 QEMU binfmt）。
  * 首次构建会下载 mGBA 源码与 gpSP/fceumm 源码包并编译，耗时较长；中间产物
    缓存在 --build-dir，后续增量构建会复用。
EOF
}

say() {
	printf '==> %s\n' "$*"
}

warn() {
	printf '警告：%s\n' "$*" >&2
}

die() {
	printf '错误：%s\n' "$*" >&2
	exit 1
}

run() {
	if [ "$DRY" = 1 ]; then
		printf '  [dry]'
		for arg in "$@"; do
			printf ' %s' "$arg"
		done
		printf '\n'
	else
		"$@"
	fi
}

abs_path() {
	case "$1" in
	/*) printf '%s\n' "$1" ;;
	*) printf '%s/%s\n' "$ROOT" "$1" ;;
	esac
}

read_taskfile_var() {
	# taskfile.yml 里两空格缩进的变量：MGBA_COMMIT / GPSP_COMMIT / FCEUMM_COMMIT
	sed -n "s/^  $1: *//p" "$ROOT/taskfile.yml" | tr -d '\r' | head -n 1
}

# 先做一次轻量扫描：--help / --dry-run / --in-container 在解析参数前就要生效。
for arg in "$@"; do
	case "$arg" in
	-h | --help)
		usage
		exit 0
		;;
	--dry-run) DRY=1 ;;
	--in-container) IN_CONTAINER=1 ;;
	esac
done

HOST_OS=$(uname -s)
HOST_ARCH=$(uname -m)
HOST_GLIBC=""
HOST_GLIBC_OK=0
NATIVE=0
if [ "$HOST_OS" = "Linux" ]; then
	case "$HOST_ARCH" in
	aarch64 | arm64) NATIVE=1 ;;
	esac
fi

# 设备是 bullseye（glibc 2.31）。aarch64 Linux 主机若 glibc 更新，编出来的
# 二进制/核心会要求新符号而无法在设备上启动；这种情况同样改走 Docker
# 的 rust:1-bullseye (arm64) 容器。
if [ "$NATIVE" = 1 ]; then
	if command -v ldd >/dev/null 2>&1; then
		line=$(ldd --version 2>&1 | head -n 1)
		case "$line" in
		*GLIBC*) HOST_GLIBC=${line##* } ;;
		esac
	fi
	if [ -n "$HOST_GLIBC" ] && newest=$(printf '%s\n2.31\n' "$HOST_GLIBC" | sort -V 2>/dev/null | tail -n 1); then
		[ "$newest" = "2.31" ] && HOST_GLIBC_OK=1
	fi
	if [ "$HOST_GLIBC_OK" = 0 ] && [ "$IN_CONTAINER" = 0 ]; then
		say "本机 glibc ${HOST_GLIBC:-未知} 高于设备要求（≤2.31），改用容器构建以保证兼容"
		NATIVE=0
	fi
fi

# 非原生 aarch64 Linux：用 Docker 的 arm64 容器重新执行本脚本。
if [ "$NATIVE" = 0 ] && [ "$IN_CONTAINER" = 0 ]; then
	container_cmd="docker run --rm --platform $DOCKER_PLATFORM -v $ROOT:/src -w /src \
-v slot-aarch64-cargo-registry:/usr/local/cargo/registry \
-v slot-aarch64-cargo-git:/usr/local/cargo/git \
-v slot-aarch64-rustup:/usr/local/rustup \
-e CARGO_HOME=/usr/local/cargo $DOCKER_IMAGE \
bash build/aarch64-linux.sh --in-container"
	if [ "$DRY" = 1 ]; then
		say "当前主机 $HOST_OS/$HOST_ARCH 不是 aarch64 Linux，将在容器中构建："
		printf '  %s\n' "$container_cmd"
		printf '  （dry-run：未实际执行）\n'
		exit 0
	fi
	command -v docker >/dev/null 2>&1 ||
		die "当前主机不是可用的 aarch64 构建环境（设备要求 glibc ≤2.31），且找不到 docker。请安装 Docker（需支持 linux/arm64），或在 glibc ≤2.31 的 aarch64 Linux（如 Debian 11 bullseye）上直接运行本脚本。"
	say "在 $DOCKER_IMAGE ($DOCKER_PLATFORM) 容器中构建..."
	if ! docker run --rm \
		--platform "$DOCKER_PLATFORM" \
		-v "$ROOT:/src" \
		-w /src \
		-v slot-aarch64-cargo-registry:/usr/local/cargo/registry \
		-v slot-aarch64-cargo-git:/usr/local/cargo/git \
		-v slot-aarch64-rustup:/usr/local/rustup \
		-e CARGO_HOME=/usr/local/cargo \
		"$DOCKER_IMAGE" \
		bash build/aarch64-linux.sh --in-container "$@"; then
		die "容器构建失败。若日志提示 exec format error，请先安装 arm64 binfmt：docker run --privileged --rm tonistiigi/binfmt --install arm64；或改在 aarch64 Linux 上构建。"
	fi
	exit 0
fi

# 原生 / 容器内部：解析完整参数。
while [ $# -gt 0 ]; do
	case "$1" in
	--out)
		shift
		[ $# -gt 0 ] || die "--out 需要一个目录"
		OUT=$1
		;;
	--out=*) OUT=${1#--out=} ;;
	--build-dir)
		shift
		[ $# -gt 0 ] || die "--build-dir 需要一个目录"
		BUILD=$1
		;;
	--build-dir=*) BUILD=${1#--build-dir=} ;;
	--slot-original)
		shift
		[ $# -gt 0 ] || die "--slot-original 需要一个文件"
		SLOT_ORIGINAL=$1
		;;
	--slot-original=*) SLOT_ORIGINAL=${1#--slot-original=} ;;
	-j | --jobs)
		shift
		[ $# -gt 0 ] || die "-j/--jobs 需要一个数字"
		JOBS=$1
		;;
	--jobs=*) JOBS=${1#--jobs=} ;;
	--tar) TAR=1 ;;
	--no-cores) NO_CORES=1 ;;
	--dry-run) DRY=1 ;;
	--in-container) IN_CONTAINER=1 ;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		die "未知参数：$1（用 --help 查看用法）"
		;;
	esac
	shift
done

OUT=$(abs_path "$OUT")
BUILD=$(abs_path "$BUILD")

MGBA_COMMIT=$(read_taskfile_var MGBA_COMMIT)
GPSP_COMMIT=$(read_taskfile_var GPSP_COMMIT)
FCEUMM_COMMIT=$(read_taskfile_var FCEUMM_COMMIT)
[ -n "$MGBA_COMMIT" ] || die "无法从 taskfile.yml 读取 MGBA_COMMIT"
[ -n "$GPSP_COMMIT" ] || die "无法从 taskfile.yml 读取 GPSP_COMMIT"
[ -n "$FCEUMM_COMMIT" ] || die "无法从 taskfile.yml 读取 FCEUMM_COMMIT"

MGBA_SO="$BUILD/cores/mgba_libretro.so"
GPSP_SO="$BUILD/cores/gpsp_libretro.so"
GPSP_TARBALL="$BUILD/cores/src/gpsp-$GPSP_COMMIT.tar.gz"
FCEUMM_SO="$BUILD/cores/fceumm_libretro.so"
FCEUMM_TARBALL="$BUILD/cores/src/fceumm-$FCEUMM_COMMIT.tar.gz"

# 重新构建前保留上一次产出的 slot.original（若用户已放进产物目录）。
if [ -f "$OUT/System/linkSlot/bin/linkslot.original" ] && [ -z "$SLOT_ORIGINAL" ]; then
	SLOT_ORIGINAL="$BUILD/keep-slot.original"
	run mkdir -p "$BUILD"
	run cp "$OUT/System/linkSlot/bin/linkslot.original" "$SLOT_ORIGINAL"
fi
if [ -z "$SLOT_ORIGINAL" ] && [ -f "$ROOT/vendor/slot.original" ]; then
	SLOT_ORIGINAL="$ROOT/vendor/slot.original"
fi

if [ "$DRY" = 0 ]; then
	command -v cargo >/dev/null 2>&1 || die "找不到 cargo，请先安装 Rust 工具链"
	command -v git >/dev/null 2>&1 || die "找不到 git（构建 mGBA 源码需要）"
	command -v curl >/dev/null 2>&1 || die "找不到 curl（下载 gpSP/fceumm 源码包需要）"
fi

cargo_build() {
	if [ -n "$JOBS" ]; then
		run cargo build "$@" -j "$JOBS"
	else
		run cargo build "$@"
	fi
}

say "构建 Slot 前端（release profile：不用 fat LTO）"
# 不要用 [profile.device] 的 fat LTO：它生成的 aarch64 二进制会丢掉部分
# 启动/菜单代码（v0.8.0 首次出包因此卡在开机画面）。v0.7.x 的正常包也是
# 不带 fat LTO 的构建；这里的 release profile 与它一致。
cargo_build --release --target-dir "$BUILD" -p slot --no-default-features --features device
if [ "$DRY" = 0 ] && [ ! -f "$BUILD/release/slot" ]; then
	die "构建后缺少 $BUILD/release/slot"
fi

say "构建 linkSlot CLI（linkslot profile + tls-rustls,mods）"
cargo_build --profile linkslot --target-dir "$BUILD" -p linkslot --features tls-rustls,mods
if [ "$DRY" = 0 ] && [ ! -f "$BUILD/linkslot/linkSlot" ]; then
	die "构建后缺少 $BUILD/linkslot/linkSlot"
fi

core_current() {
	# core_current <so> <cores/xxx/build.sh> <commit>
	[ -f "$1" ] || return 1
	[ -f "$1.meta" ] || return 1
	[ "$(sh "$2" stamp "$3" 2>/dev/null | tr -d '\r')" = "$(tr -d '\r' <"$1.meta")" ]
}

say "构建 / 复用模拟器核心"
if [ "$NO_CORES" = 1 ]; then
	warn "已指定 --no-cores：不编译核心，产物不能作为发布包"
	if [ ! -f "$MGBA_SO" ] && [ -f "$ROOT/vendor/mgba_libretro.so" ]; then
		MGBA_SO="$ROOT/vendor/mgba_libretro.so"
	fi
	if [ ! -f "$GPSP_SO" ] && [ -f "$ROOT/vendor/gpsp_libretro.so" ]; then
		GPSP_SO="$ROOT/vendor/gpsp_libretro.so"
	fi
	if [ ! -f "$GPSP_TARBALL" ] && [ -f "$ROOT/vendor/gpsp-src.tar.gz" ]; then
		GPSP_TARBALL="$ROOT/vendor/gpsp-src.tar.gz"
	fi
	if [ ! -f "$FCEUMM_SO" ] && [ -f "$ROOT/vendor/fceumm_libretro.so" ]; then
		FCEUMM_SO="$ROOT/vendor/fceumm_libretro.so"
	fi
	if [ ! -f "$FCEUMM_TARBALL" ] && [ -f "$ROOT/vendor/fceumm-src.tar.gz" ]; then
		FCEUMM_TARBALL="$ROOT/vendor/fceumm-src.tar.gz"
	fi
	if [ "$DRY" = 0 ]; then
		[ -f "$MGBA_SO" ] || die "--no-cores 但没有可复用的 mGBA 核心（先完整构建一次）"
		[ -f "$GPSP_SO" ] || die "--no-cores 但没有可复用的 gpSP 核心（先完整构建一次）"
		[ -f "$FCEUMM_SO" ] || die "--no-cores 但没有可复用的 fceumm 核心（先完整构建一次）"
	fi
else
	if core_current "$MGBA_SO" "$ROOT/cores/mgba/build.sh" "$MGBA_COMMIT"; then
		say "复用已构建的 mGBA 核心：$MGBA_SO"
	else
		say "构建 mGBA 核心 @ $MGBA_COMMIT（首次会 clone 源码并装 cmake）"
		run sh "$ROOT/cores/mgba/build.sh" build "$MGBA_COMMIT" "$BUILD/mgba" "$MGBA_SO"
	fi

	if [ ! -f "$GPSP_TARBALL" ]; then
		run mkdir -p "$(dirname "$GPSP_TARBALL")"
		say "下载 gpSP 源码包 @ $GPSP_COMMIT"
		run curl -fsSL -o "$GPSP_TARBALL.part" \
			"https://github.com/libretro/gpsp/archive/$GPSP_COMMIT.tar.gz"
		run mv "$GPSP_TARBALL.part" "$GPSP_TARBALL"
	fi
	if core_current "$GPSP_SO" "$ROOT/cores/gpsp/build.sh" "$GPSP_COMMIT"; then
		say "复用已构建的 gpSP 核心：$GPSP_SO"
	else
		say "构建 gpSP 核心 @ $GPSP_COMMIT"
		run sh "$ROOT/cores/gpsp/build.sh" build "$GPSP_COMMIT" "$GPSP_TARBALL" \
			"$BUILD/gpsp-work" "$GPSP_SO"
	fi

	if [ ! -f "$FCEUMM_TARBALL" ]; then
		run mkdir -p "$(dirname "$FCEUMM_TARBALL")"
		say "下载 fceumm 源码包 @ $FCEUMM_COMMIT"
		run curl -fsSL -o "$FCEUMM_TARBALL.part" \
			"https://github.com/libretro/libretro-fceumm/archive/$FCEUMM_COMMIT.tar.gz"
		run mv "$FCEUMM_TARBALL.part" "$FCEUMM_TARBALL"
	fi
	if core_current "$FCEUMM_SO" "$ROOT/cores/fceumm/build.sh" "$FCEUMM_COMMIT"; then
		say "复用已构建的 fceumm 核心：$FCEUMM_SO"
	else
		say "构建 fceumm 核心 @ $FCEUMM_COMMIT"
		run sh "$ROOT/cores/fceumm/build.sh" build "$FCEUMM_COMMIT" "$FCEUMM_TARBALL" \
			"$BUILD/fceumm-work" "$FCEUMM_SO"
	fi
fi

say "组装 SD 卡根目录：$OUT"
run rm -rf "$OUT"
run mkdir -p "$OUT"
run "$BUILD/release/slot" --init-root "$OUT"

run cp "$BUILD/release/slot" "$OUT/System/linkSlot/bin/linkslot"
run cp "$BUILD/linkslot/linkSlot" "$OUT/System/linkSlot/bin/linkslot_sync"
run cp -R "$ROOT/card/System/." "$OUT/System/"
run mkdir -p "$OUT/System/linkSlot/assets/carts"
run mkdir -p "$OUT/linkSlot/Config" "$OUT/linkSlot/mods"
if [ -d "$ROOT/card/linkSlot" ]; then
	run cp -R "$ROOT/card/linkSlot/." "$OUT/linkSlot/"
fi
# Screen overlays (FC mask): the art lives on the card, the package only ships
# the folder and its README so `Overlay/fc.png` has a place to land.
run mkdir -p "$OUT/Overlay"
if [ -d "$ROOT/card/Overlay" ]; then
	run cp -R "$ROOT/card/Overlay/." "$OUT/Overlay/"
fi
README_SRC="$ROOT/README-ZH-CN.md"
[ -f "$README_SRC" ] || README_SRC="$ROOT/workdoc/README-ZH-CN.md"
run cp "$README_SRC" "$OUT/README-ZH-CN.md"
# 与参考发布包一致的 Cheats 目录骨架（不放任何用户金手指数据）。
run mkdir -p "$OUT/Cheats/GBA/mgba" "$OUT/Cheats/GBA/gpsp"

if [ -f "$MGBA_SO" ] || [ "$DRY" = 1 ]; then
	run cp "$MGBA_SO" "$OUT/System/linkSlot/cores/mgba/mgba_libretro.so"
else
	warn "缺少 mGBA 核心，产物不完整"
fi
if [ -f "$GPSP_SO" ] || [ "$DRY" = 1 ]; then
	run cp "$GPSP_SO" "$OUT/System/linkSlot/cores/gpsp/gpsp_libretro.so"
else
	warn "缺少 gpSP 核心，产物不完整"
fi
if [ -f "$FCEUMM_SO" ] || [ "$DRY" = 1 ]; then
	run cp "$FCEUMM_SO" "$OUT/System/linkSlot/cores/fceumm/fceumm_libretro.so"
else
	warn "缺少 fceumm 核心，产物不完整"
fi

# 许可证与对应源码（发布合规要求）。
run mkdir -p "$OUT/System/licenses"
if [ -d "$ROOT/licenses" ] || [ "$DRY" = 1 ]; then
	run cp -R "$ROOT/licenses/." "$OUT/System/linkSlot/licenses/"
fi
if [ -f "$ROOT/LICENSE" ]; then
	run cp "$ROOT/LICENSE" "$OUT/System/linkSlot/licenses/slot-GPL-3.0.txt"
fi
if [ -f "$GPSP_TARBALL" ] || [ "$DRY" = 1 ]; then
	run cp "$GPSP_TARBALL" "$OUT/System/linkSlot/licenses/gpsp-$GPSP_COMMIT.tar.gz"
fi
if [ -f "$GPSP_SO.meta" ] || [ "$DRY" = 1 ]; then
	run cp "$GPSP_SO.meta" "$OUT/System/linkSlot/licenses/gpsp-$GPSP_COMMIT.meta"
fi
for p in "$ROOT"/cores/gpsp/*.patch; do
	if [ -e "$p" ]; then
		run cp "$p" "$OUT/System/linkSlot/licenses/gpsp-$(basename "$p")"
	fi
done
if [ -f "$FCEUMM_TARBALL" ] || [ "$DRY" = 1 ]; then
	run cp "$FCEUMM_TARBALL" "$OUT/System/linkSlot/licenses/fceumm-$FCEUMM_COMMIT.tar.gz"
fi
if [ -f "$FCEUMM_SO.meta" ] || [ "$DRY" = 1 ]; then
	run cp "$FCEUMM_SO.meta" "$OUT/System/linkSlot/licenses/fceumm-$FCEUMM_COMMIT.meta"
fi

MGBA_META_COMMIT=$MGBA_COMMIT
if [ -f "$MGBA_SO.meta" ]; then
	meta_commit=$(sed -n 's/^commit=//p' "$MGBA_SO.meta" | tr -d '\r' | head -n 1)
	[ -n "$meta_commit" ] && MGBA_META_COMMIT=$meta_commit
fi
if [ -f "$MGBA_SO.meta" ] || [ "$DRY" = 1 ]; then
	run cp "$MGBA_SO.meta" "$OUT/System/linkSlot/licenses/mgba-$MGBA_META_COMMIT.meta"
fi
for p in "$ROOT"/cores/mgba/*.patch; do
	if [ -e "$p" ]; then
		run cp "$p" "$OUT/System/linkSlot/licenses/mgba-$(basename "$p")"
	fi
done

if [ -n "$SLOT_ORIGINAL" ] && [ -f "$SLOT_ORIGINAL" ]; then
	run cp "$SLOT_ORIGINAL" "$OUT/System/linkSlot/bin/linkslot.original"
else
	warn "未提供原版 H700 的 System/slot.original：启动回退功能不可用。"
	warn "可把它放到 vendor/slot.original，或用 --slot-original 指定。"
fi

verify_arch() {
	# verify_arch <binary>
	[ -f "$1" ] || die "缺少 $1"
	if command -v file >/dev/null 2>&1; then
		case "$(file -b "$1")" in
		*aarch64* | *ARM*aarch64*) ;;
		*) die "$1 不是 aarch64 可执行文件：$(file -b "$1")" ;;
		esac
	fi
	if command -v readelf >/dev/null 2>&1; then
		max_glibc=$(readelf -V "$1" 2>/dev/null |
			sed -n 's/.*Name: GLIBC_\([0-9][0-9.]*\).*/\1/p' |
			sort -V | tail -n 1)
		if [ -n "$max_glibc" ]; then
			newest=$(printf '%s\n2.31\n' "$max_glibc" | sort -V | tail -n 1)
			[ "$newest" = "2.31" ] ||
				die "$1 需要 GLIBC_$max_glibc（设备只有 2.31）"
		fi
	fi
}

if [ "$DRY" = 0 ]; then
	say "生成版本清单"
	PROJECT_VERSION=$(awk -F'"' '/^version = /{print $2; exit}' "$ROOT/Cargo.toml")
	[ -n "$PROJECT_VERSION" ] || die "无法从 Cargo.toml 读取 workspace 版本"
	command -v sha256sum >/dev/null 2>&1 || die "生成 version.json 需要 sha256sum"
	{
		printf '{\n  "schema": 1,\n  "product": "linkslot",\n  "version": "%s",\n  "build": "%s",\n  "files": {\n' "$PROJECT_VERSION" "$SLOT_BUILD_DATE"
		first_file=1
		find "$OUT/System" -type f | LC_ALL=C sort | while IFS= read -r file; do
			rel=${file#"$OUT/"}
			digest=$(sha256sum "$file" | cut -d ' ' -f 1)
			if [ "$first_file" = 1 ]; then
				first_file=0
			else
				printf ',\n'
			fi
			printf '    "%s": "sha256:%s"' "$rel" "$digest"
		done
		printf '\n  }\n}\n'
	} > "$OUT/linkSlot/version.json"
	[ -s "$OUT/linkSlot/version.json" ] || die "linkSlot/version.json 生成失败"

	say "校验产物"
	for f in \
		"$OUT/System/linkSlot/bin/linkslot" \
		"$OUT/System/linkSlot/bin/linkslot_sync" \
		"$OUT/System/launch_frontend.sh" \
		"$OUT/System/linkSlot/shell/linkslot.sh" \
		"$OUT/System/linkSlot/shell/linkslot-net.sh" \
		"$OUT/System/linkSlot/shell/slot-wifi.sh" \
		"$OUT/System/linkSlot/shell/linkslot-link.sh" \
		"$OUT/System/linkSlot/cores/fceumm/fceumm_libretro.so" \
		"$OUT/System/linkSlot/licenses/linkslot-deps.txt" \
		"$OUT/README-ZH-CN.md" \
		"$OUT/linkSlot/version.json" \
		"$OUT/mods/README.txt"; do
		[ -f "$f" ] || die "缺少 $f"
	done
	verify_arch "$OUT/System/linkSlot/bin/linkslot"
	verify_arch "$OUT/System/linkSlot/bin/linkslot_sync"
	version_output=$("$OUT/System/linkSlot/bin/linkslot_sync" version 2>&1 || true)
	case "$version_output" in
		*mods=1*) ;;
		*) die "System/linkSlot 未启用 mods：$version_output" ;;
	esac
	if [ "$NO_CORES" = 0 ]; then
		verify_arch "$OUT/System/linkSlot/cores/mgba/mgba_libretro.so"
		verify_arch "$OUT/System/linkSlot/cores/gpsp/gpsp_libretro.so"
		verify_arch "$OUT/System/linkSlot/cores/fceumm/fceumm_libretro.so"
	fi
fi

if [ "$TAR" = 1 ]; then
	say "打包 $OUT.tar.gz"
	run tar -czf "$OUT.tar.gz" -C "$(dirname "$OUT")" "$(basename "$OUT")"
fi

if [ "$DRY" = 1 ]; then
	say "dry-run 完成（未执行任何命令）"
	exit 0
fi

say "完成：$OUT"
du -sh "$OUT" 2>/dev/null || true
printf '把 %s 的内容复制到 SD 卡根目录即可。\n' "$OUT"
if [ -z "$SLOT_ORIGINAL" ]; then
	printf '提示：如需启动回退，请把原版 H700 的 System/slot 放到 %s/System/slot.original。\n' "$OUT"
fi
