#!/bin/sh
# 校验组装好的卡目录（P2-05）。
#
#   build/verify.sh <目录> [--release]
#
# 默认做结构检查；--release 额外要求许可证齐全、GLIBC ≤2.31（缺一项即失败）。
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
DIR=${1:-}
[ -n "$DIR" ] || { echo "用法：build/verify.sh <目录> [--release]" >&2; exit 2; }
shift || true
RELEASE=0
for a in "$@"; do [ "$a" = "--release" ] && RELEASE=1; done

fail=0
ok() { printf '  ok   %s\n' "$*"; }
bad() { printf '  FAIL %s\n' "$*"; fail=$((fail + 1)); }
warn() { printf '  warn %s\n' "$*"; }

[ -d "$DIR" ] || { echo "错误：目录不存在：$DIR" >&2; exit 2; }

echo "==> 必需文件"
for f in \
	"System/linkSlot/bin/linkslot" "System/linkSlot/bin/linkslot_sync" \
	"System/launch_frontend.sh" "System/linkSlot/shell/launch.sh" "System/linkSlot/shell/linkslot.sh" "System/linkSlot/shell/linkslot-net.sh" \
	"System/linkSlot/licenses/linkslot-deps.txt" \
	"README-ZH-CN.md" "linkSlot/version.json" "linkSlot/mods/README.txt"; do
	[ -e "$DIR/$f" ] && ok "$f" || bad "缺少 $f"
done

echo "==> 核心（5 个）"
for core in $(python3 "$ROOT/build/manifest.py" list); do
	dylib=$(python3 "$ROOT/build/manifest.py" get "$core" dylib)
	optional=$(python3 "$ROOT/build/manifest.py" get "$core" optional 2>/dev/null || true)
	if [ -f "$DIR/System/linkSlot/cores/$core/$dylib" ]; then
		ok "$core/$dylib"
	elif [ "$optional" = "true" ]; then
		warn "可选核心 $core 未随包提供（用户可自行放入 System/linkSlot/cores/$core/）"
	else
		bad "缺少核心 $core/$dylib"
	fi
done

echo "==> 架构与 GLIBC"
check_binary() {
	bin=$1
	[ -f "$bin" ] || return 0
	if command -v file >/dev/null 2>&1; then
		case "$(file -b "$bin")" in
		*aarch64* | *ARM*aarch64*) ok "$(basename "$bin") aarch64" ;;
		*) bad "$(basename "$bin") 不是 aarch64：$(file -b "$bin")" ;;
		esac
	fi
	if command -v readelf >/dev/null 2>&1; then
		max=$(readelf -V "$bin" 2>/dev/null |
			sed -n 's/.*Name: GLIBC_\([0-9][0-9.]*\).*/\1/p' |
			sort -V | tail -n 1)
		if [ -n "$max" ]; then
			newest=$(printf '%s\n2.31\n' "$max" | sort -V | tail -n 1)
			if [ "$newest" = "2.31" ]; then
				ok "$(basename "$bin") GLIBC ≤ 2.31"
			else
				bad "$(basename "$bin") 需要 GLIBC_$max（设备只有 2.31）"
			fi
		fi
	fi
}
for bin in "$DIR/System/linkSlot/bin/linkslot" "$DIR/System/linkSlot/bin/linkslot_sync" "$DIR"/System/linkSlot/cores/*/*_libretro.so; do
	[ -e "$bin" ] && check_binary "$bin"
done

echo "==> version.json 与版本一致性"
if [ -f "$DIR/linkSlot/version.json" ]; then
	version=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$DIR/linkSlot/version.json" 2>/dev/null || true)
	source_version=$(awk -F'"' '/^version = /{print $2; exit}' "$ROOT/Cargo.toml")
	if [ -n "$version" ] && [ "$version" = "$source_version" ]; then
		ok "version.json = $version"
	else
		bad "version.json($version) 与源码版本($source_version) 不一致"
	fi
	python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$DIR/linkSlot/version.json" 2>/dev/null &&
		ok "version.json 可解析" || bad "version.json 损坏"
else
	bad "缺少 linkSlot/version.json"
fi

echo "==> 清单"
python3 "$ROOT/build/manifest.py" check && ok "cores.toml 结构" || bad "cores.toml 结构"

if [ "$RELEASE" = 1 ]; then
	echo "==> 发布门槛（--release）"
	if python3 "$ROOT/build/manifest.py" check-release; then
		ok "核心许可证 / GLIBC / sha256"
	else
		bad "发布门槛未过（见上）"
	fi
	for p in "$DIR"/System/linkSlot/licenses/*.patch; do
		[ -e "$p" ] && ok "补丁 $(basename "$p")"
	done
	gpsp_commit=$(python3 "$ROOT/build/manifest.py" get gpsp commit)
	[ -f "$DIR/System/linkSlot/licenses/gpsp-$gpsp_commit.tar.gz" ] &&
		ok "gpSP 对应源码" || bad "缺少 gpSP 对应源码"
	fceumm_commit=$(python3 "$ROOT/build/manifest.py" get fceumm commit)
	[ -f "$DIR/System/linkSlot/licenses/fceumm-$fceumm_commit.tar.gz" ] &&
		ok "fceumm 对应源码" || bad "缺少 fceumm 对应源码"
	snes9x_commit=$(python3 "$ROOT/build/manifest.py" get snes9x commit)
	[ -f "$DIR/System/linkSlot/licenses/snes9x-$snes9x_commit.tar.gz" ] &&
		ok "snes9x 对应源码" || bad "缺少 snes9x 对应源码"
fi

if [ "$fail" -gt 0 ]; then
	echo "校验失败：$fail 项" >&2
	exit 1
fi
echo "校验通过：$DIR"
