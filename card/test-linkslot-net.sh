#!/bin/sh
# Off-device tests for card/System/linkSlot/shell/linkslot-net.sh using fake commands.
# Run with: sh card/test-linkslot-net.sh
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
SCRIPT="$HERE/System/linkSlot/shell/linkslot-net.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
FAILS=0
RC=0

fail() {
	echo "FAIL: $*"
	FAILS=$((FAILS + 1))
}

pass() {
	echo "ok: $*"
}

make_fakes() {
	mkdir -p "$TMP/bin"
	cat >"$TMP/bin/ip" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_IP_LOG"
case "$1" in
addr) cat "$FAKE_IP_OUT" 2>/dev/null ;;
route)
	case "$*" in
	"route show default")
		[ -s "$FAKE_ROUTE_FILE" ] && cat "$FAKE_ROUTE_FILE"
		;;
	"route add default via "*)
		printf '%s\n' "${*#route add }" >"$FAKE_ROUTE_FILE"
		;;
	esac
	;;
esac
exit 0
EOF
	cat >"$TMP/bin/wpa_cli" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_WPA_CLI_LOG"
case "$*" in
*status)
	echo "wpa_state=$(cat "$FAKE_WPA_STATE" 2>/dev/null)"
	echo "ssid=$(cat "$FAKE_SSID" 2>/dev/null)"
	;;
*terminate)
	: >"$FAKE_WPA_STATE"
	;;
esac
exit 0
EOF
	cat >"$TMP/bin/wpa_supplicant" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_SUP_LOG"
[ "${FAKE_SUP_FAIL:-0}" = 1 ] && exit 1
if [ "${FAKE_SUP_SET_STATE:-1}" = 1 ]; then
	printf '%s' "${FAKE_SSID_VALUE:-home}" >"$FAKE_SSID"
	echo COMPLETED >"$FAKE_WPA_STATE"
fi
exit 0
EOF
	cat >"$TMP/bin/udhcpc" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_UDHCPC_LOG"
[ "${FAKE_UDHCPC_FAIL:-0}" = 1 ] && exit 1
if [ -n "${FAKE_UDHCPC_IP:-}" ]; then
	echo "2: wlan0    inet $FAKE_UDHCPC_IP/24 brd 192.168.1.255 scope global wlan0" >"$FAKE_IP_OUT"
	: >"$FAKE_RESOLV_CONF"
fi
exit 0
EOF
	cat >"$TMP/bin/rfkill" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_RFKILL_LOG"
exit 0
EOF
	cat >"$TMP/bin/chmod" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_CHMOD_LOG"
exit 0
EOF
	chmod 755 "$TMP/bin/ip" "$TMP/bin/wpa_cli" "$TMP/bin/wpa_supplicant" \
		"$TMP/bin/udhcpc" "$TMP/bin/rfkill" "$TMP/bin/chmod"
}

reset() {
	# Per-test overrides must not leak into the next case.
	unset SSID PSK ASSOC_WAIT DHCP_WAIT UDHCPC_IP SUP_SET_STATE SUP_FAIL WPA_CLI SUPPLICANT 2>/dev/null || true
	rm -rf "$TMP/net" "$TMP/run" "$TMP/ctrl" "$TMP/bin"
	mkdir -p "$TMP/net/wlan0" "$TMP/run" "$TMP/ctrl" "$TMP/bin"
	echo up >"$TMP/net/wlan0/operstate"
	: >"$TMP/ip.out"
	: >"$TMP/wpa.state"
	: >"$TMP/ssid"
	: >"$TMP/ip.log"
	: >"$TMP/wpa-cli.log"
	: >"$TMP/sup.log"
	: >"$TMP/udhcpc.log"
	: >"$TMP/rfkill.log"
	: >"$TMP/chmod.log"
	printf '%s\n' "default via 192.168.1.1 dev wlan0" >"$TMP/route"
	printf '%s\n' "nameserver 192.168.1.1" >"$TMP/run/resolv.conf"
	make_fakes
}

run() {
	PATH="$TMP/bin:$PATH" \
		FAKE_IP_OUT="$TMP/ip.out" FAKE_IP_LOG="$TMP/ip.log" \
		FAKE_ROUTE_FILE="$TMP/route" \
		FAKE_WPA_STATE="$TMP/wpa.state" FAKE_SSID="$TMP/ssid" \
		FAKE_WPA_CLI_LOG="$TMP/wpa-cli.log" FAKE_SUP_LOG="$TMP/sup.log" \
		FAKE_UDHCPC_LOG="$TMP/udhcpc.log" FAKE_RFKILL_LOG="$TMP/rfkill.log" \
		FAKE_CHMOD_LOG="$TMP/chmod.log" \
		FAKE_RESOLV_CONF="$TMP/run/resolv.conf" \
		FAKE_UDHCPC_IP="${UDHCPC_IP-192.168.1.50}" \
		FAKE_SUP_SET_STATE="${SUP_SET_STATE-1}" FAKE_SUP_FAIL="${SUP_FAIL-0}" \
		AGS_NET_SYS="$TMP/net" AGS_RUN="$TMP/run" AGS_CTRL_DIR="$TMP/ctrl" \
		AGS_RESOLV_CONF="$TMP/run/resolv.conf" \
		AGS_LINKSLOT_SSID="${SSID-home}" AGS_LINKSLOT_PSK="${PSK-secret}" \
		AGS_LINKSLOT_ASSOC_WAIT_S="${ASSOC_WAIT-1}" \
		AGS_LINKSLOT_DHCP_WAIT_S="${DHCP_WAIT-1}" \
		AGS_WPA_SUPPLICANT="${SUPPLICANT-$TMP/bin/wpa_supplicant}" \
		AGS_WPA_CLI="${WPA_CLI-$TMP/bin/wpa_cli}" \
		AGS_IP="$TMP/bin/ip" AGS_UDHCPC="$TMP/bin/udhcpc" \
		AGS_RFKILL="$TMP/bin/rfkill" \
		sh "$SCRIPT" "$@"
}

run_capture() {
	if run "$@" >"$TMP/out" 2>"$TMP/err"; then
		RC=0
	else
		RC=$?
	fi
}

check_ok() {
	desc="$1"
	shift
	run_capture "$@"
	if [ "$RC" -eq 0 ]; then
		pass "$desc"
	else
		fail "$desc: exit $RC"
		sed 's/^/  err: /' "$TMP/err" >&2
	fi
}

check_fail() {
	desc="$1"
	shift
	run_capture "$@"
	if [ "$RC" -ne 0 ]; then
		pass "$desc"
	else
		fail "$desc: expected failure but exit 0"
	fi
}

expect_contains() {
	desc="$1"
	pattern="$2"
	file="$3"
	if grep -qF -e "$pattern" "$file" 2>/dev/null; then
		pass "$desc"
	else
		fail "$desc (missing '$pattern' in $file)"
		sed 's/^/  content: /' "$file" 2>/dev/null | head -n 20 >&2
	fi
}

expect_empty() {
	desc="$1"
	file="$2"
	if [ -s "$file" ]; then
		fail "$desc (expected empty file)"
		sed 's/^/  content: /' "$file" | head -n 20 >&2
	else
		pass "$desc"
	fi
}

echo "# linkslot-net.sh off-device tests"

# --- status -------------------------------------------------------------
reset
echo "2: wlan0    inet 192.168.1.10/24 brd 192.168.1.255 scope global wlan0" >"$TMP/ip.out"
echo COMPLETED >"$TMP/wpa.state"
check_ok "status: connected" status
expect_contains "status: reports address" "已连接" "$TMP/err"

reset
rm -rf "$TMP/net/wlan0"
check_fail "status: wlan0 missing" status
expect_contains "status: explains missing device" "找不到无线网卡" "$TMP/err"

reset
echo down >"$TMP/net/wlan0/operstate"
check_fail "status: link down" status
expect_contains "status: explains link down" "未启用" "$TMP/err"

reset
echo COMPLETED >"$TMP/wpa.state"
check_fail "status: no ipv4" status
expect_contains "status: explains missing ipv4" "IPv4" "$TMP/err"

reset
echo COMPLETED >"$TMP/wpa.state"
echo "2: wlan0    inet 169.254.10.10/16 brd 169.254.255.255 scope link wlan0" >"$TMP/ip.out"
check_fail "status: link-local address is not usable" status

reset
WPA_CLI=/nonexistent/wpa_cli
check_fail "status: missing wpa_cli" status
expect_contains "status: explains missing wpa_cli" "缺少" "$TMP/err"

reset
echo SCANNING >"$TMP/wpa.state"
check_fail "status: not associated" status
expect_contains "status: explains association state" "未关联" "$TMP/err"

# --- up: already connected ---------------------------------------------
reset
echo COMPLETED >"$TMP/wpa.state"
echo home >"$TMP/ssid"
echo "2: wlan0    inet 192.168.1.10/24 brd 192.168.1.255 scope global wlan0" >"$TMP/ip.out"
check_ok "up: already connected" up
expect_empty "up: no supplicant restart when connected" "$TMP/sup.log"
expect_empty "up: no dhcp when address exists" "$TMP/udhcpc.log"
if [ -e "$TMP/run/linkslot-wpa.conf" ]; then
	fail "up: should not rewrite wpa config when already connected"
else
	pass "up: no wpa config rewrite when connected"
fi

# --- up: needs association ---------------------------------------------
reset
check_ok "up: associates and gets address" up
expect_contains "up: config has ssid" 'ssid="home"' "$TMP/run/linkslot-wpa.conf"
expect_contains "up: config has psk" 'psk="secret"' "$TMP/run/linkslot-wpa.conf"
expect_contains "up: config has ctrl_interface" "ctrl_interface=$TMP/ctrl" "$TMP/run/linkslot-wpa.conf"
if [ -s "$TMP/sup.log" ]; then
	pass "up: starts wpa_supplicant"
else
	fail "up: wpa_supplicant not started"
fi
expect_contains "up: calls udhcpc -b" "-i wlan0 -b" "$TMP/udhcpc.log"
expect_contains "up: reports connected" "网络已连接" "$TMP/err"
# chmod 600 is invoked explicitly; the msys test host cannot represent POSIX
# modes, so assert the command rather than stat -c %a.
expect_contains "up: sets wpa config mode 600" "600 $TMP/run/linkslot-wpa.conf" "$TMP/chmod.log"

# --- up: special characters are escaped --------------------------------
reset
PSK='se"cr\et'
check_ok "up: psk with quotes/backslash" up
expect_contains "up: escapes psk quote" 'se\"cr\\et' "$TMP/run/linkslot-wpa.conf"

# --- up: reuse existing supplicant for the same SSID -------------------
reset
echo COMPLETED >"$TMP/wpa.state"
echo home >"$TMP/ssid"
check_ok "up: reuses associated supplicant" up
expect_empty "up: does not restart matching supplicant" "$TMP/sup.log"
if [ -s "$TMP/udhcpc.log" ]; then
	pass "up: still obtains dhcp address on reused link"
else
	fail "up: udhcpc not called on reused link"
fi

# --- up: restarts supplicant for a different SSID ----------------------
reset
echo COMPLETED >"$TMP/wpa.state"
echo other-network >"$TMP/ssid"
check_ok "up: switches to configured SSID" up
expect_contains "up: terminates old supplicant" "terminate" "$TMP/wpa-cli.log"
if [ -s "$TMP/sup.log" ]; then
	pass "up: starts new supplicant after switch"
else
	fail "up: supplicant not restarted after SSID switch"
fi

# --- up: missing supplicant binary -------------------------------------
reset
SUPPLICANT=/nonexistent/wpa_supplicant
check_fail "up: missing wpa_supplicant" up
expect_contains "up: explains missing supplicant" "缺少" "$TMP/err"

# --- up: supplicant start failure --------------------------------------
reset
SUP_FAIL=1
check_fail "up: supplicant start failure" up
expect_contains "up: reports start failure" "启动 wpa_supplicant 失败" "$TMP/err"

# --- up: association timeout -------------------------------------------
reset
SUP_SET_STATE=0
ASSOC_WAIT=0
check_fail "up: association timeout" up
expect_contains "up: reports association timeout" "关联 Wi-Fi 超时" "$TMP/err"

# --- up: dhcp timeout ---------------------------------------------------
reset
UDHCPC_IP=""
DHCP_WAIT=0
check_fail "up: dhcp timeout" up
expect_contains "up: reports dhcp timeout" "获取 IPv4 地址超时" "$TMP/err"

# --- up: no wlan0 --------------------------------------------------------
reset
rm -rf "$TMP/net/wlan0"
check_fail "up: no wireless device" up
expect_contains "up: explains missing device" "找不到无线网卡" "$TMP/err"

# --- up: missing SSID ----------------------------------------------------
reset
SSID=""
check_fail "up: empty ssid" up
expect_contains "up: explains missing ssid" "未配置" "$TMP/err"

# --- up: rfkill / ip link are attempted --------------------------------
reset
check_ok "up: brings link up" up
expect_contains "up: rfkill unblock wifi" "unblock wifi" "$TMP/rfkill.log"
expect_contains "up: ip link set wlan0 up" "link set wlan0 up" "$TMP/ip.log"

# --- usage ---------------------------------------------------------------
reset
check_fail "usage: unknown subcommand fails" bogus

echo
if [ "$FAILS" -gt 0 ]; then
	echo "FAILED: $FAILS test(s)"
	exit 1
fi
echo "PASSED: all linkslot-net.sh tests"
exit 0
