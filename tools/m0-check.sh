#!/bin/sh
# M0 真机前置验证：一次性收集 linkSlot 集成所需的环境事实。
# 用法（掌机上，或通过 adb shell sh /mnt/sdcard/tools/m0-check.sh）：
#   sh /mnt/sdcard/tools/m0-check.sh > /mnt/sdcard/m0-check.log 2>&1
# 然后把 m0-check.log 取回（adb pull /mnt/sdcard/m0-check.log）。
# 本脚本只读，不修改任何配置。

ROOT="${SLOT_ROOT:-/mnt/sdcard}"
SD="$ROOT"

section() {
	echo
	echo "===== $* ====="
}

run() {
	echo "--- \$ $*"
	"$@" 2>&1
}

section "baseos"
cat /etc/baseos-release 2>&1
run uname -a

section "storage"
cat /proc/mounts 2>&1 | grep -E 'sdcard|mmc|ext4|vfat|exfat'
run df -h "$SD"
run df -h /

section "clock"
run date
run date -u
ls -l /dev/rtc* 2>&1
cat /sys/class/rtc/rtc0/date /sys/class/rtc/rtc0/time 2>&1

section "network interfaces"
run ip addr show
run ip route
run iw dev 2>&1
run iwgetid -r 2>&1
run wpa_cli -i wlan0 status
run udhcpc --help 2>&1 | head -5
cat /run/resolv.conf 2>&1
cat /etc/resolv.conf 2>&1

section "tools"
ls -l /usr/bin/curl /usr/bin/wpa_supplicant /usr/bin/wpa_cli /sbin/udhcpc /bin/ip /usr/bin/rfkill 2>&1
command -v sha256sum 2>&1
command -v wget 2>&1

section "frontend files"
ls -l "$SD/System/slot" "$SD/System/launch_frontend.sh" "$SD/System/linkSlot/shell/linkslot.sh" 2>&1
ls -l /lib/ld-linux-aarch64.so.1 2>&1
echo "--- loader smoke test (--init-root into tmp)"
/lib/ld-linux-aarch64.so.1 "$SD/System/slot" --init-root /tmp/m0-root 2>&1
echo "loader exit=$?"
ls -l /tmp/m0-root 2>&1

section "resource"
run free
cat /proc/cpuinfo 2>&1 | grep -E 'model name|Hardware|Processor' | head -3

section "summary"
echo "把上面输出（或 m0-check.log）交给开发机，用于确认："
echo "  1) 家用 Wi-Fi 由谁负责连接（wpa_cli status 是否 COMPLETED、是否已有 IPv4）"
echo "  2) /mnt/sdcard 文件系统与挂载参数（vfat/exfat、fmask、ro/rw）"
echo "  3) 设备时钟是否可用（date 是否合理）"
echo "  4) System/slot 的启动方式与 loader 是否可用"
echo "  5) curl / sha256sum 等工具是否存在"
