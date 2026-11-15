#!/bin/sh
# 取/编模拟器核心（P2-03）。
#
# 用法：build/fetch-cores.sh <device|host> [输出目录，默认 vendor]
#
# - kind = "build"  的核心调用 cores/<id>/build.sh 现场编译；
# - kind = "vendor" 的核心从 vendor/ 或本地 card/System/ 取预编译产物，并校验 sha256；
# - 已存在且 sha256 匹配的产物直接复用。
#
# 依赖：python3（读 manifest）、sha256sum（或 shasum）。
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
CHANNEL=${1:-device}
OUT=${2:-$ROOT/vendor}

py() { python3 "$ROOT/build/manifest.py" "$@"; }

sha256_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d' ' -f1
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | cut -d' ' -f1
	else
		echo "错误：找不到 sha256sum/shasum" >&2
		exit 1
	fi
}

say() { printf '==> %s\n' "$*"; }
die() { printf '错误：%s\n' "$*" >&2; exit 1; }

command -v python3 >/dev/null 2>&1 || die "需要 python3 读取 manifest/cores.toml"
mkdir -p "$OUT"

fetch_build() {
	name=$1
	commit=$(py get "$name" commit)
	builder=$(py get "$name" builder)
	dylib=$(py get "$name" dylib)
	out="$OUT/$dylib"

	if [ -f "$out" ] && [ -f "$out.meta" ] && [ "$(tr -d '\r' <"$out.meta")" = "$(sh "$ROOT/$builder" stamp "$commit" 2>/dev/null | tr -d '\r')" ]; then
		say "复用 $dylib（commit $commit）"
		return 0
	fi

	say "构建 $name @ $commit"
	case "$name" in
	mgba)
		sh "$ROOT/$builder" build "$commit" "$ROOT/target/cores/mgba" "$out"
		;;
	gpsp | fceumm | snes9x)
		src="$ROOT/vendor/$name-src.tar.gz"
		if [ ! -f "$src" ]; then
			case "$name" in
			gpsp) url="https://github.com/libretro/gpsp/archive/$commit.tar.gz" ;;
			fceumm) url="https://github.com/libretro/libretro-fceumm/archive/$commit.tar.gz" ;;
			snes9x) url="https://github.com/libretro/snes9x/archive/$commit.tar.gz" ;;
			esac
			mkdir -p "$ROOT/vendor"
			say "下载 $name 源码 $url"
			curl -fsSL -o "$src.part" "$url"
			mv "$src.part" "$src"
		fi
		sh "$ROOT/$builder" build "$commit" "$src" "$ROOT/target/cores/$name-work" "$out"
		;;
	*)
		die "未定义 $name 的构建命令"
		;;
	esac
}

fetch_vendor() {
	name=$1
	dylib=$(py get "$name" dylib)
	want=$(py get "$name" sha256)
	out="$OUT/$dylib"

	if [ -f "$out" ] && [ "$(sha256_of "$out")" = "$want" ]; then
		say "复用 $dylib"
		return 0
	fi

	for src in "$ROOT/vendor/$dylib" "$ROOT/card/System/$dylib"; do
		if [ -f "$src" ] && [ "$(sha256_of "$src")" = "$want" ]; then
			cp "$src" "$out"
			say "取用 $dylib（来自 ${src#"$ROOT"/}）"
			return 0
		fi
	done

	optional=$(py get "$name" optional 2>/dev/null || true)
	if [ "$optional" = "true" ]; then
		warn "可选核心 $dylib 未提供，跳过（不随发布包分发；用户可自行放入 System/）"
		return 0
	fi

	die "缺少 $dylib：vendor/ 与 card/System/ 都没有匹配 sha256=$want 的产物。
     请在 vendor/ 放入该核心，或修正 manifest/cores.toml。"
}

say "核心通道：$CHANNEL，输出：$OUT"
for name in $(py list); do
	kind=$(py get "$name" kind)
	if [ "$CHANNEL" = host ] && [ "$kind" = build ]; then
		say "host 通道跳过 $name 的交叉编译（仅用于设备包）"
		continue
	fi
	case "$kind" in
	build) fetch_build "$name" ;;
	vendor) fetch_vendor "$name" ;;
	*) die "$name: 未知 kind=$kind" ;;
	esac
done

say "核心就绪：$(ls -1 "$OUT"/*.so 2>/dev/null | wc -l) 个"
