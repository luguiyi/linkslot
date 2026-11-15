#!/bin/sh
# snes9x（libretro）—— 设备构建配方。
#
# 与 fceumm 同一套约定：stamp 输出可比较的构建指纹，build 编译并写 .meta。
# snes9x 是 SFC 的**官方随包核心**；supafaust 只作为可选核心（用户自备）。
set -eu

here="$(cd "$(dirname "$0")" && pwd)"

# 与设备一致的基线：Cortex-A53，不启用 LTO（snes9x 默认已关闭）。
device_cflags="-mcpu=cortex-a53 -O2"

usage() {
	echo "usage: $0 stamp COMMIT | build COMMIT TARBALL WORKDIR OUT" >&2
	exit 2
}

sha256() {
	if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi |
		cut -d' ' -f1
}

stamp() {
	echo "commit=$1"
	echo "source=https://github.com/libretro/snes9x/archive/$1.tar.gz"
	echo "recipe=make -C libretro platform=unix"
	echo "device_cflags=$device_cflags"
	for p in "$here"/*.patch; do
		[ -e "$p" ] || continue
		echo "patch=$(basename "$p") sha256:$(sha256 "$p")"
	done
}

build() {
	commit="$1" tarball="$2" work="$3" out="$4"
	src="$work/snes9x-$commit"

	rm -rf "$work"
	mkdir -p "$work"
	tar -xzf "$tarball" -C "$work"
	if [ ! -f "$src/libretro/Makefile" ]; then
		echo "$tarball does not hold snes9x-$commit/libretro/Makefile" >&2
		exit 1
	fi

	for p in "$here"/*.patch; do
		[ -e "$p" ] || continue
		git -C "$src" apply -p1 "$p"
	done

	CFLAGS="$device_cflags" CXXFLAGS="$device_cflags" \
		make -C "$src/libretro" platform=unix \
		-j"$(nproc)"

	mkdir -p "$(dirname "$out")"
	cp "$src/libretro/snes9x_libretro.so" "$out"
	stamp "$commit" >"$out.meta"
}

case "${1:-}" in
stamp)
	[ $# -eq 2 ] && [ -n "$2" ] || usage
	stamp "$2"
	;;
build)
	[ $# -eq 5 ] && [ -n "$2" ] && [ -n "$3" ] && [ -n "$4" ] && [ -n "$5" ] || usage
	build "$2" "$3" "$4" "$5"
	;;
*)
	usage
	;;
esac
