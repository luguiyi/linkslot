#!/bin/sh
# BaseOS 入口（第 2 优先级）。
#
# 单前端卡：本文件属于 linkSlot，转交给 System/linkSlot/shell/launch.sh。
# 多前端卡：这个文件应换成"卡级选择器"（读 System/frontend.txt 或按键选择），
#           再 exec 对应前端的 System/<id>/shell/launch.sh。
SD="${SLOT_ROOT:-/mnt/sdcard}"
exec /bin/sh "$SD/System/linkSlot/shell/launch.sh" "$@"
