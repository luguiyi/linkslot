#!/bin/sh
# 在"干净导出树"上跑一遍完整出包（P6-02 的本机版）。
#
#   sh build/clean-build.sh [h700|host]
#
# 做法：用 `git archive HEAD` 导出一棵只含已跟踪文件的树（等价于 fresh clone，
# 但不需要网络），在临时目录里执行 dist.sh → verify.sh，最后打印产物位置。
#
# 用途：验证"仓库自己就能出包"——不依赖工作区里的未跟踪文件（vendor/、
# worklogs、旧 target 缓存等）。CI 与换机迁移都以这个结果为基准。
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
TARGET=${1:-h700}
DEST=${DEST:-$(mktemp -d "${TMPDIR:-/tmp}/linkslot-clean.XXXXXX")}

say() { printf '==> %s\n' "$*"; }
die() { printf '错误：%s\n' "$*" >&2; exit 1; }

command -v git >/dev/null 2>&1 || die "需要 git"
[ -d "$ROOT/.git" ] || die "$ROOT 不是 git 仓库"

say "导出干净树：$DEST"
mkdir -p "$DEST"
git -C "$ROOT" archive HEAD | tar -x -C "$DEST"

say "已跟踪文件数：$(find "$DEST" -type f | wc -l)"
[ -f "$DEST/Cargo.toml" ] || die "导出树缺少 Cargo.toml"
[ -f "$DEST/build/dist.sh" ] || die "导出树缺少 build/dist.sh"

say "构建 $TARGET"
cd "$DEST"
sh build/dist.sh "$TARGET" --out "$DEST/artifacts/linkslot"

if [ "$TARGET" = h700 ]; then
	say "校验"
	sh build/verify.sh "$DEST/artifacts/linkslot"
	say "产物：$DEST/artifacts/linkslot"
else
	say "桌面产物：$DEST/target/release/"
fi

say "完成（干净树保留在 $DEST，便于排查）"
