#!/bin/sh
# 唯一出包入口（P2-04）。
#
#   build/dist.sh [h700|host] [选项]
#
# 选项：
#   --out DIR          产物目录（默认 artifacts/linkslot）
#   --build-dir DIR    Cargo 中间目录（默认 target/dist-build）
#   -j, --jobs N       Cargo 并行度
#   --no-cores         复用已有核心，不重新取/编（产物不能作为发布包）
#   --no-package       只组装，不生成 zip
#   --release          出包时执行发布门槛（verify --release：许可证/对应源码/GLIBC）
#   --dry-run          只打印将执行的命令
#   --in-container     内部使用：已在构建容器中
#   --legacy           转发到旧脚本 build/aarch64-linux.sh
#
# 环境变量：DOCKER_IMAGE / DOCKER_PLATFORM / JOBS / REGISTRY
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
TARGET=h700
OUT="$ROOT/artifacts/linkslot"
BUILD="$ROOT/target/dist-build"
JOBS=${JOBS:-}
NO_CORES=0
NO_PACKAGE=0
RELEASE=0
DRY=0
IN_CONTAINER=0
LEGACY=0

DOCKER_IMAGE=${DOCKER_IMAGE:-rust:1-bullseye}
DOCKER_PLATFORM=${DOCKER_PLATFORM:-linux/arm64}
REGISTRY=${REGISTRY:-ghcr.io/luguiyi/linkslot-build}

SLOT_BUILD_DATE=${SLOT_BUILD_DATE:-$(date -u +%Y-%m-%d)}
export SLOT_BUILD_DATE
if command -v git >/dev/null 2>&1; then
	SLOT_GIT_HASH=${SLOT_GIT_HASH:-$(git -C "$ROOT" rev-parse --short=7 HEAD 2>/dev/null || true)}
	[ -n "${SLOT_GIT_HASH:-}" ] && export SLOT_GIT_HASH
fi

say() { printf '==> %s\n' "$*"; }
warn() { printf '警告：%s\n' "$*" >&2; }
die() { printf '错误：%s\n' "$*" >&2; exit 1; }
run() {
	if [ "$DRY" = 1 ]; then
		printf '  [dry]'; for a in "$@"; do printf ' %s' "$a"; done; printf '\n'
	else
		"$@"
	fi
}

usage() {
	cat <<'EOF'
用法：build/dist.sh [h700|host] [选项]

  h700   生成 SD 卡整包（默认）
  host   只构建桌面版可执行文件，不碰核心、不出卡包

选项见脚本头部注释。
EOF
}

for arg in "$@"; do
	case "$arg" in
	-h | --help) usage; exit 0 ;;
	--dry-run) DRY=1 ;;
	--in-container) IN_CONTAINER=1 ;;
	--legacy) LEGACY=1 ;;
	esac
done

if [ "$LEGACY" = 1 ]; then
	say "--legacy：转发到 build/aarch64-linux.sh（旧入口，保留一个版本）"
	exec sh "$ROOT/build/aarch64-linux.sh"
fi

while [ $# -gt 0 ]; do
	case "$1" in
	h700 | host) TARGET=$1 ;;
	--out) shift; OUT=$1 ;;
	--out=*) OUT=${1#--out=} ;;
	--build-dir) shift; BUILD=$1 ;;
	--build-dir=*) BUILD=${1#--build-dir=} ;;
	-j | --jobs) shift; JOBS=$1 ;;
	--jobs=*) JOBS=${1#--jobs=} ;;
	--no-cores) NO_CORES=1 ;;
	--no-package) NO_PACKAGE=1 ;;
	--release) RELEASE=1 ;;
	--dry-run | --in-container | --legacy | --release) ;;
	-h | --help) usage; exit 0 ;;
	*) die "未知参数：$1" ;;
	esac
	shift
done

abs() { case "$1" in /*) printf '%s\n' "$1" ;; *) printf '%s/%s\n' "$ROOT" "$1" ;; esac; }
OUT=$(abs "$OUT")
BUILD=$(abs "$BUILD")

# ---------- host：桌面构建 ----------
if [ "$TARGET" = host ]; then
	command -v cargo >/dev/null 2>&1 || die "找不到 cargo"
	say "构建桌面版 slot"
	run cargo build --release -p slot
	say "构建桌面版 linkSlot（默认特性，无 TLS/Lua）"
	run cargo build -p linkslot
	say "完成：target/release/slot.exe（或 slot）"
	exit 0
fi

# ---------- h700：设备整包 ----------
HOST_OS=$(uname -s)
HOST_ARCH=$(uname -m)
NATIVE=0
if [ "$HOST_OS" = "Linux" ]; then
	case "$HOST_ARCH" in aarch64 | arm64) NATIVE=1 ;; esac
fi

# 设备是 bullseye（glibc 2.31）；更新的 glibc 会要求新符号，同样走容器。
if [ "$NATIVE" = 1 ] && command -v ldd >/dev/null 2>&1; then
	version=$(ldd --version 2>&1 | head -n 1)
	glibc=${version##* }
	if [ -n "$glibc" ]; then
		newest=$(printf '%s\n2.31\n' "$glibc" | sort -V | tail -n 1)
		[ "$newest" = "2.31" ] || { say "本机 glibc $glibc > 2.31，改用容器"; NATIVE=0; }
	fi
fi

if [ "$NATIVE" = 0 ] && [ "$IN_CONTAINER" = 0 ]; then
	container="docker run --rm --platform $DOCKER_PLATFORM \
 -v $ROOT:/src -w /src \
 -v linkslot-cargo-registry:/usr/local/cargo/registry \
 -v linkslot-cargo-git:/usr/local/cargo/git \
 -v linkslot-rustup:/usr/local/rustup \
 -e CARGO_HOME=/usr/local/cargo \
 $DOCKER_IMAGE \
 sh build/dist.sh h700 --in-container --out /src/artifacts/linkslot"
	if [ "$DRY" = 1 ]; then
		say "非 aarch64 Linux 主机，将在容器中构建："
		printf '  %s\n' "$container"
		exit 0
	fi
	command -v docker >/dev/null 2>&1 ||
		die "当前主机不能原生构建 aarch64，且找不到 docker。请安装 Docker（支持 linux/arm64）或在 glibc ≤2.31 的 aarch64 Linux 上运行。"
	say "在 $DOCKER_IMAGE ($DOCKER_PLATFORM) 容器中构建"
	docker run --rm --platform "$DOCKER_PLATFORM" \
		-v "$ROOT:/src" -w /src \
		-v linkslot-cargo-registry:/usr/local/cargo/registry \
		-v linkslot-cargo-git:/usr/local/cargo/git \
		-v linkslot-rustup:/usr/local/rustup \
		-e CARGO_HOME=/usr/local/cargo \
		"$DOCKER_IMAGE" \
		sh build/dist.sh h700 --in-container --out /src/artifacts/linkslot ||
		die "容器构建失败（exec format error 时先装 binfmt：docker run --privileged --rm tonistiigi/binfmt --install arm64）"
	exit 0
fi

command -v cargo >/dev/null 2>&1 || die "找不到 cargo"
command -v python3 >/dev/null 2>&1 || die "找不到 python3（读取 manifest / 打包需要）"

cargo_build() {
	if [ -n "$JOBS" ]; then run cargo build "$@" -j "$JOBS"; else run cargo build "$@"; fi
}

say "1/6 构建前端（release，不用 fat LTO）"
cargo_build --release --target-dir "$BUILD" -p slot --no-default-features --features device
[ "$DRY" = 1 ] || [ -f "$BUILD/release/slot" ] || die "缺少 $BUILD/release/slot"

say "2/6 构建 linkSlot CLI（tls-rustls + mods）"
cargo_build --profile linkslot --target-dir "$BUILD" -p linkslot --features tls-rustls,mods
[ "$DRY" = 1 ] || [ -f "$BUILD/linkslot/linkSlot" ] || die "缺少 $BUILD/linkslot/linkSlot"

say "3/6 取/编模拟器核心"
if [ "$NO_CORES" = 1 ]; then
	warn "--no-cores：复用 vendor/ 已有核心，产物不能作为发布包"
else
	run sh "$ROOT/build/fetch-cores.sh" device "$ROOT/vendor"
fi

say "4/6 组装卡目录：$OUT"
run rm -rf "$OUT"
run mkdir -p "$OUT"
if [ "$DRY" = 0 ]; then
	# `tr -d '\r'` 让 Windows 上编辑过的 skeleton.txt（CRLF）也能用。
	tr -d '\r' <"$ROOT/build/skeleton.txt" |
		while IFS= read -r dir; do
			case "$dir" in '' | '#'*) continue ;; esac
			mkdir -p "$OUT/$dir"
		done
fi
run cp "$BUILD/release/slot" "$OUT/System/linkSlot/bin/linkslot"
run cp "$BUILD/linkslot/linkSlot" "$OUT/System/linkSlot/bin/linkslot_sync"
run cp -R "$ROOT/card/System/." "$OUT/System/"
[ -d "$ROOT/card/linkSlot" ] && run cp -R "$ROOT/card/linkSlot/." "$OUT/linkSlot/"
[ -d "$ROOT/card/Overlay" ] && run cp -R "$ROOT/card/Overlay/." "$OUT/Overlay/"
run cp "$ROOT/card/Labels/README.txt" "$OUT/Labels/README.txt"
run cp "$ROOT/card/Wallpapers/README.txt" "$OUT/Wallpapers/README.txt"
run cp "$ROOT/card/linkSlot/mods/README.txt" "$OUT/linkSlot/mods/README.txt"
run cp "$ROOT/README-ZH-CN.md" "$OUT/README-ZH-CN.md"
run mkdir -p "$OUT/System/linkSlot/licenses"
run cp -R "$ROOT/licenses/." "$OUT/System/linkSlot/licenses/"
run cp "$ROOT/LICENSE" "$OUT/System/linkSlot/licenses/slot-GPL-3.0.txt"

for core in $(python3 "$ROOT/build/manifest.py" list); do
	dylib=$(python3 "$ROOT/build/manifest.py" get "$core" dylib)
	optional=$(python3 "$ROOT/build/manifest.py" get "$core" optional 2>/dev/null || true)
	if [ "$optional" = "true" ]; then
		# optional 核心不随发布包分发（即使用户机器上的 vendor/ 里有）。
		warn "可选核心 $core 不随包（用户自备时放入 $OUT/System/linkSlot/cores/$core/）"
	elif [ -f "$ROOT/vendor/$dylib" ]; then
		run mkdir -p "$OUT/System/linkSlot/cores/$core"
		run cp "$ROOT/vendor/$dylib" "$OUT/System/linkSlot/cores/$core/$dylib"
	elif [ "$optional" = "true" ]; then
		warn "可选核心 $dylib 未随包（SFC 需用户自备核心，放入 $OUT/System/linkSlot/cores/<核心>/）"
	else
		warn "缺少核心 $dylib，产物不完整"
	fi
done

# 合规：GPL 核心的对应源码与补丁
if [ -f "$ROOT/vendor/gpsp-src.tar.gz" ]; then
	commit=$(python3 "$ROOT/build/manifest.py" get gpsp commit)
	run cp "$ROOT/vendor/gpsp-src.tar.gz" "$OUT/System/linkSlot/licenses/gpsp-$commit.tar.gz"
	[ -f "$ROOT/vendor/gpsp_libretro.so.meta" ] && run cp "$ROOT/vendor/gpsp_libretro.so.meta" "$OUT/System/linkSlot/licenses/gpsp-$commit.meta"
	for p in "$ROOT"/cores/gpsp/*.patch; do [ -e "$p" ] && run cp "$p" "$OUT/System/linkSlot/licenses/gpsp-$(basename "$p")"; done
else
	warn "缺少 gpSP 对应源码，发布包不合规"
fi
if [ -f "$ROOT/vendor/fceumm-src.tar.gz" ]; then
	commit=$(python3 "$ROOT/build/manifest.py" get fceumm commit)
	run cp "$ROOT/vendor/fceumm-src.tar.gz" "$OUT/System/linkSlot/licenses/fceumm-$commit.tar.gz"
	[ -f "$ROOT/vendor/fceumm_libretro.so.meta" ] && run cp "$ROOT/vendor/fceumm_libretro.so.meta" "$OUT/System/linkSlot/licenses/fceumm-$commit.meta"
	for p in "$ROOT"/cores/fceumm/*.patch; do [ -e "$p" ] && run cp "$p" "$OUT/System/linkSlot/licenses/fceumm-$(basename "$p")"; done
else
	warn "缺少 fceumm 对应源码，发布包不合规"
fi
if [ -f "$ROOT/vendor/snes9x-src.tar.gz" ]; then
	commit=$(python3 "$ROOT/build/manifest.py" get snes9x commit)
	run cp "$ROOT/vendor/snes9x-src.tar.gz" "$OUT/System/linkSlot/licenses/snes9x-$commit.tar.gz"
	[ -f "$ROOT/vendor/snes9x_libretro.so.meta" ] && run cp "$ROOT/vendor/snes9x_libretro.so.meta" "$OUT/System/linkSlot/licenses/snes9x-$commit.meta"
	for p in "$ROOT"/cores/snes9x/*.patch; do [ -e "$p" ] && run cp "$p" "$OUT/System/linkSlot/licenses/snes9x-$(basename "$p")"; done
else
	warn "缺少 snes9x 对应源码，发布包不合规"
fi
for p in "$ROOT"/cores/mgba/*.patch; do [ -e "$p" ] && run cp "$p" "$OUT/System/linkSlot/licenses/mgba-$(basename "$p")"; done
[ -f "$ROOT/vendor/mgba_libretro.so.meta" ] && run cp "$ROOT/vendor/mgba_libretro.so.meta" "$OUT/System/linkSlot/licenses/mgba-$(grep '^commit=' "$ROOT/vendor/mgba_libretro.so.meta" | cut -d= -f2).meta"

say "5/6 生成 version.json"
if [ "$DRY" = 0 ]; then
	VERSION=${VERSION:-$(awk -F'"' '/^version = /{print $2; exit}' "$ROOT/Cargo.toml")}
	[ -n "$VERSION" ] || die "无法从 Cargo.toml 读取 workspace 版本"
	run python3 "$ROOT/build/version_json.py" \
		--dir "$OUT" \
		--version "$VERSION" \
		--commit "${SLOT_GIT_HASH:-unknown}" \
		--upstream "slot-1.6.0" \
		--build-date "$SLOT_BUILD_DATE"
fi

say "6/6 校验与打包"
if [ "$RELEASE" = 1 ]; then
	run sh "$ROOT/build/verify.sh" "$OUT" --release
else
	run sh "$ROOT/build/verify.sh" "$OUT"
fi
if [ "$NO_PACKAGE" = 0 ]; then
	VERSION=${VERSION:-$(awk -F'"' '/^version = /{print $2; exit}' "$ROOT/Cargo.toml")}
	run python3 "$ROOT/build/package.py" --dir "$OUT" --out "$ROOT/artifacts/linkslot-v$VERSION-h700.zip"
fi

say "完成：$OUT"
