#!/bin/sh
# Off-device tests for card/System/linkSlot/shell/slot-wifi.sh using fake commands.
# No real Wi-Fi is touched: wpa_cli / wpa_supplicant / ip / rfkill and the
# delegated linkslot-net.sh are faked under a temp directory.
# Run with: sh card/test-slot-wifi.sh
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
SCRIPT="$HERE/System/linkSlot/shell/slot-wifi.sh"
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
	cat >"$TMP/bin/wpa_cli" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$FAKE_WPA_CLI_LOG"
cmd=
for arg in "$@"; do
	cmd=$arg
done
case "$cmd" in
status)
	echo "wpa_state=$(cat "$FAKE_WPA_STATE" 2>/dev/null)"
	echo "ssid=$(cat "$FAKE_SSID" 2>/dev/null)"
	if [ -n "${FAKE_WPA_IP:-}" ]; then
		echo "ip_address=$FAKE_WPA_IP"
	fi
	exit 0
	;;
scan)
	[ "${FAKE_SCAN_FAIL:-0}" = 1 ] && exit 1
	exit 0
	;;
scan_results)
	[ "${FAKE_SCAN_FAIL:-0}" = 1 ] && exit 1
	if [ -f "${FAKE_SCAN_RESULTS:-}" ]; then
		cat "$FAKE_SCAN_RESULTS"
	fi
	exit 0
	;;
terminate)
	exit 0
	;;
esac
exit 0
EOF
	cat >"$TMP/bin/wpa_supplicant" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$FAKE_SUP_LOG"
prev=
conf=
for arg in "$@"; do
	if [ "$prev" = "-c" ]; then
		conf=$arg
	fi
	prev=$arg
done
if [ -n "$conf" ] && [ -n "${FAKE_SUP_CONF_OUT:-}" ]; then
	cp "$conf" "$FAKE_SUP_CONF_OUT" 2>/dev/null || true
fi
if [ "${FAKE_SUP_FAIL:-0}" = 1 ]; then
	exit 1
fi
if [ "${FAKE_SUP_FAIL_D:-0}" = 1 ]; then
	case "$*" in
	*-Dnl80211*) exit 1 ;;
	esac
fi
if [ -n "${FAKE_SUP_SOCKET:-}" ]; then
	mkdir -p "$(dirname "$FAKE_SUP_SOCKET")" 2>/dev/null || true
	: >"$FAKE_SUP_SOCKET"
fi
exit 0
EOF
	cat >"$TMP/bin/ip" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$FAKE_IP_LOG"
case "$1" in
addr) cat "$FAKE_IP_OUT" 2>/dev/null ;;
link)
	case "$2" in
	show)
		if [ "${FAKE_LINK_MISSING:-0}" = 1 ]; then
			exit 1
		fi
		if [ "$(cat "$FAKE_LINK_UP" 2>/dev/null)" = 1 ]; then
			echo "2: wlan0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc noop state UP group default qlen 1000"
		else
			echo "2: wlan0: <BROADCAST,MULTICAST> mtu 1500 qdisc noop state DOWN group default qlen 1000"
		fi
		;;
	set)
		case "$4" in
		up) echo 1 >"$FAKE_LINK_UP" ;;
		down) echo 0 >"$FAKE_LINK_UP" ;;
		esac
		;;
	esac
	;;
esac
exit 0
EOF
	cat >"$TMP/bin/rfkill" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$FAKE_RFKILL_LOG"
case "$1" in
list)
	echo "0: phy0: Wireless LAN"
	soft=no
	hard=no
	[ "$(cat "$FAKE_RFKILL_BLOCKED" 2>/dev/null)" = 1 ] && soft=yes
	[ "${FAKE_RFKILL_HARD:-0}" = 1 ] && hard=yes
	echo "	Soft blocked: $soft"
	echo "	Hard blocked: $hard"
	;;
unblock)
	[ "${FAKE_RFKILL_STUCK:-0}" = 1 ] || echo 0 >"$FAKE_RFKILL_BLOCKED"
	;;
block)
	[ "${FAKE_RFKILL_STUCK:-0}" = 1 ] || echo 1 >"$FAKE_RFKILL_BLOCKED"
	;;
esac
exit 0
EOF
	cat >"$TMP/fake-net.sh" <<'EOF'
#!/bin/sh
{
	printf 'argv=%s\n' "$*"
	printf 'ssid=%s\n' "${LINKSLOT_WIFI_SSID:-}"
	printf 'psk=%s\n' "${LINKSLOT_WIFI_PSK:-}"
	printf 'timeout=%s\n' "${LINKSLOT_WIFI_TIMEOUT:-}"
} >"$FAKE_NET_LOG"
exit "${FAKE_NET_RC:-0}"
EOF
	chmod 755 "$TMP/bin/wpa_cli" "$TMP/bin/wpa_supplicant" \
		"$TMP/bin/ip" "$TMP/bin/rfkill" "$TMP/fake-net.sh"
}

reset() {
	# Per-test overrides must not leak into the next case.
	unset WLAN SCAN_WAIT WPA_IP SCAN_RESULTS_FILE SCAN_FAIL SUP_FAIL SUP_FAIL_D \
		NET_RC FAKE_WPA_CLI FAKE_WPA_SUPPLICANT SUP_SOCKET WIFI_TIMEOUT \
		FAKE_RFKILL RFKILL_HARD RFKILL_STUCK LINK_MISSING 2>/dev/null || true
	rm -rf "$TMP/ctrl" "$TMP/scan-ctrl" "$TMP/bin" "$TMP/fake-net.sh"
	rm -f "$TMP"/*.log "$TMP"/*.out "$TMP"/sup-conf.copy \
		"$TMP"/rfkill-blocked "$TMP"/link-up 2>/dev/null || true
	mkdir -p "$TMP/ctrl" "$TMP/scan-ctrl" "$TMP/bin"
	: >"$TMP/ip.out"
	: >"$TMP/wpa.state"
	: >"$TMP/ssid"
	: >"$TMP/scan-results"
	: >"$TMP/wpa-cli.log"
	: >"$TMP/sup.log"
	: >"$TMP/ip.log"
	: >"$TMP/net.log"
	: >"$TMP/rfkill.log"
	echo 0 >"$TMP/rfkill-blocked"
	echo 1 >"$TMP/link-up"
	make_fakes
}

run() {
	PATH="$TMP/bin:$PATH" \
		FAKE_WPA_CLI_LOG="$TMP/wpa-cli.log" \
		FAKE_SUP_LOG="$TMP/sup.log" \
		FAKE_SUP_CONF_OUT="$TMP/sup-conf.copy" \
		FAKE_IP_LOG="$TMP/ip.log" \
		FAKE_IP_OUT="$TMP/ip.out" \
		FAKE_LINK_UP="$TMP/link-up" \
		FAKE_LINK_MISSING="${LINK_MISSING-0}" \
		FAKE_RFKILL_LOG="$TMP/rfkill.log" \
		FAKE_RFKILL_BLOCKED="$TMP/rfkill-blocked" \
		FAKE_RFKILL_HARD="${RFKILL_HARD-0}" \
		FAKE_RFKILL_STUCK="${RFKILL_STUCK-0}" \
		FAKE_WPA_STATE="$TMP/wpa.state" \
		FAKE_SSID="$TMP/ssid" \
		FAKE_WPA_IP="${WPA_IP-}" \
		FAKE_SCAN_RESULTS="${SCAN_RESULTS_FILE-$TMP/scan-results}" \
		FAKE_SCAN_FAIL="${SCAN_FAIL-0}" \
		FAKE_SUP_FAIL="${SUP_FAIL-0}" \
		FAKE_SUP_FAIL_D="${SUP_FAIL_D-0}" \
		FAKE_SUP_SOCKET="${SUP_SOCKET-$TMP/scan-ctrl/wlan0}" \
		FAKE_NET_LOG="$TMP/net.log" \
		FAKE_NET_RC="${NET_RC-0}" \
		LINKSLOT_LOG_LEVEL="${LOG_LEVEL-info}" \
		LINKSLOT_WIFI_TIMEOUT="${WIFI_TIMEOUT-30}" \
		AGS_LINKSLOT_WLAN="${WLAN-wlan0}" \
		AGS_WPA_CLI="${FAKE_WPA_CLI-$TMP/bin/wpa_cli}" \
		AGS_WPA_SUPPLICANT="${FAKE_WPA_SUPPLICANT-$TMP/bin/wpa_supplicant}" \
		AGS_IP="$TMP/bin/ip" \
		AGS_RFKILL="${FAKE_RFKILL-$TMP/bin/rfkill}" \
		AGS_CTRL_DIR="$TMP/ctrl" \
		AGS_SLOT_WIFI_CTRL="$TMP/scan-ctrl" \
		AGS_SLOT_WIFI_CONF="$TMP/scan-ctrl/wpa.conf" \
		AGS_SLOT_WIFI_SCAN_WAIT_S="${SCAN_WAIT-1}" \
		AGS_SLOT_WIFI_NET_SCRIPT="$TMP/fake-net.sh" \
		sh "$SCRIPT" "$@"
}

save_scan_results() {
	{
		printf 'bssid / frequency / signal level / flags / ssid\n'
		printf '%s\t%s\t%s\t%s\t%s\n' \
			00:11:22:33:44:55 2412 -42 '[WPA2-PSK-CCMP][ESS]' 'Cafe Wi-Fi'
		printf '%s\t%s\t%s\t%s\t%s\n' \
			00:11:22:33:44:66 2412 -70 '[ESS]' 'Open'
		printf '%s\t%s\t%s\t%s\t%s\n' \
			00:11:22:33:44:77 2412 -55 '[WPA2-PSK-CCMP][ESS]' 'Cafe Wi-Fi'
		printf '%s\t%s\t%s\t%s\t%s\n' \
			00:11:22:33:44:88 2412 -80 '[ESS]' ''
		printf '%s\t%s\t%s\t%s\t%s\n' \
			00:11:22:33:44:99 2412 -30 '[WPA2-PSK-CCMP][ESS]' 'Stronger'
	} >"$TMP/scan-results"
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
		sed 's/^/  out: /' "$TMP/out" >&2
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

check_exit() {
	desc="$1"
	want="$2"
	shift 2
	run_capture "$@"
	if [ "$RC" -eq "$want" ]; then
		pass "$desc"
	else
		fail "$desc: expected exit $want, got $RC"
		sed 's/^/  err: /' "$TMP/err" >&2
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

expect_line() {
	desc="$1"
	line="$2"
	file="$3"
	if grep -qxF -e "$line" "$file" 2>/dev/null; then
		pass "$desc"
	else
		fail "$desc (missing line '$line' in $file)"
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

expect_missing() {
	desc="$1"
	path="$2"
	if [ -e "$path" ]; then
		fail "$desc ($path still exists)"
	else
		pass "$desc"
	fi
}

expect_lines() {
	desc="$1"
	file="$2"
	pattern="$3"
	expected=$(printf '%b' "$pattern")
	actual=$(cat "$file" 2>/dev/null || true)
	if [ "$actual" = "$expected" ]; then
		pass "$desc"
	else
		fail "$desc (content mismatch)"
		printf '  expected: [%s]\n  actual:   [%s]\n' "$expected" "$actual" >&2
	fi
}

expect_count() {
	desc="$1"
	file="$2"
	want="$3"
	got=$(wc -l <"$file" | tr -d ' ')
	if [ "$got" = "$want" ]; then
		pass "$desc"
	else
		fail "$desc (expected $want lines, got $got)"
	fi
}

echo "# slot-wifi.sh off-device tests"

# --- status ---------------------------------------------------------------
reset
echo COMPLETED >"$TMP/wpa.state"
echo home >"$TMP/ssid"
echo "2: wlan0    inet 192.168.1.10/24 brd 192.168.1.255 scope global wlan0" >"$TMP/ip.out"
check_ok "status: connected" status
expect_lines "status: machine-readable contract" "$TMP/out" \
	'state=connected\nssid=home\nip=192.168.1.10'
expect_contains "status: logs connection" "已连接" "$TMP/err"

reset
echo DISCONNECTED >"$TMP/wpa.state"
check_ok "status: disconnected still exits 0" status
expect_lines "status: disconnected contract" "$TMP/out" \
	'state=disconnected\nssid=\nip='

reset
echo COMPLETED >"$TMP/wpa.state"
echo home >"$TMP/ssid"
WPA_IP=10.0.0.7
check_ok "status: falls back to wpa_cli ip_address" status
expect_contains "status: ip from wpa_cli" "ip=10.0.0.7" "$TMP/out"

reset
FAKE_WPA_CLI=/nonexistent/wpa_cli
check_ok "status: exit 0 without wpa_cli" status
expect_lines "status: no wpa_cli reads as disconnected" "$TMP/out" \
	'state=disconnected\nssid=\nip='

reset
echo COMPLETED >"$TMP/wpa.state"
echo home >"$TMP/ssid"
echo "2: wlan0    inet 169.254.10.10/16 brd 169.254.255.255 scope link wlan0" >"$TMP/ip.out"
check_ok "status: link-local address is not usable" status
expect_contains "status: no usable ip" "state=disconnected" "$TMP/out"

# --- radio ----------------------------------------------------------------
reset
check_ok "radio-status: radio on" radio-status
expect_lines "radio-status: on contract" "$TMP/out" 'radio=on'

reset
echo 1 >"$TMP/rfkill-blocked"
check_ok "radio-status: soft blocked is off" radio-status
expect_lines "radio-status: off contract" "$TMP/out" 'radio=off'
expect_contains "radio-status: consults rfkill" "list wifi" "$TMP/rfkill.log"

reset
RFKILL_HARD=1
check_ok "radio-status: hard blocked is off" radio-status
expect_lines "radio-status: hard blocked contract" "$TMP/out" 'radio=off'

reset
FAKE_RFKILL=/nonexistent/rfkill
check_ok "radio-status: no rfkill, link up means on" radio-status
expect_lines "radio-status: no rfkill on contract" "$TMP/out" 'radio=on'
expect_contains "radio-status: falls back to ip link" "link show wlan0" "$TMP/ip.log"

reset
FAKE_RFKILL=/nonexistent/rfkill
echo 0 >"$TMP/link-up"
check_ok "radio-status: no rfkill, link down means off" radio-status
expect_lines "radio-status: no rfkill off contract" "$TMP/out" 'radio=off'

reset
FAKE_RFKILL=/nonexistent/rfkill
LINK_MISSING=1
check_ok "radio-status: no wlan0 means off" radio-status
expect_lines "radio-status: missing device contract" "$TMP/out" 'radio=off'

reset
echo 1 >"$TMP/rfkill-blocked"
echo 0 >"$TMP/link-up"
check_ok "radio on: unblocks and brings the link up" radio on
expect_lines "radio on: on contract" "$TMP/out" 'radio=on'
expect_contains "radio on: unblocks rfkill" "unblock wifi" "$TMP/rfkill.log"
expect_contains "radio on: brings wlan0 up" "link set wlan0 up" "$TMP/ip.log"

reset
check_ok "radio off: blocks and brings the link down" radio off
expect_lines "radio off: off contract" "$TMP/out" 'radio=off'
expect_contains "radio off: blocks rfkill" "block wifi" "$TMP/rfkill.log"
expect_contains "radio off: brings wlan0 down" "link set wlan0 down" "$TMP/ip.log"
expect_contains "radio off: terminates supplicant" "terminate" "$TMP/wpa-cli.log"
check_ok "radio off: repeated off succeeds" radio off
expect_lines "radio off: repeated contract" "$TMP/out" 'radio=off'

reset
echo 1 >"$TMP/rfkill-blocked"
RFKILL_STUCK=1
check_fail "radio on: stuck rfkill fails" radio on
expect_contains "radio on: explains failure" "开启 WiFi 失败" "$TMP/err"

reset
RFKILL_STUCK=1
check_fail "radio off: stuck rfkill fails" radio off
expect_contains "radio off: explains failure" "关闭 WiFi 失败" "$TMP/err"

# --- scan: gated by the radio --------------------------------------------
reset
echo 1 >"$TMP/rfkill-blocked"
check_fail "scan: fails when the radio is off" scan
expect_contains "scan: explains radio off" "WiFi 已关闭" "$TMP/err"
expect_empty "scan: no supplicant when the radio is off" "$TMP/sup.log"
expect_empty "scan: no wpa_cli scan when the radio is off" "$TMP/wpa-cli.log"

# --- scan: reuse a running supplicant -------------------------------------
reset
: >"$TMP/ctrl/wlan0"
save_scan_results
check_ok "scan: reuses running supplicant" scan
expect_lines "scan: sorted, deduped, filtered, tab-separated" "$TMP/out" \
	'Stronger\t-30\t1\nCafe Wi-Fi\t-42\t1\nOpen\t-70\t0'
expect_contains "scan: asks wpa_cli to scan" "-p $TMP/ctrl -i wlan0 scan" "$TMP/wpa-cli.log"
expect_empty "scan: does not start a second supplicant" "$TMP/sup.log"

# --- scan: temporary supplicant when none is running ----------------------
reset
save_scan_results
check_ok "scan: starts a temporary supplicant" scan
expect_contains "scan: temp supplicant argv" "-B -i wlan0 -c $TMP/scan-ctrl/wpa.conf" "$TMP/sup.log"
expect_contains "scan: temp config sets ctrl_interface" \
	"ctrl_interface=$TMP/scan-ctrl" "$TMP/sup-conf.copy"
expect_contains "scan: terminates temp supplicant" "terminate" "$TMP/wpa-cli.log"
expect_missing "scan: temp control dir removed" "$TMP/scan-ctrl"

# SSID 去重按字面比较（正则元字符不能当模式）。
reset
{
	printf 'bssid / frequency / signal level / flags / ssid\n'
	printf '%s\t%s\t%s\t%s\t%s\n' 11:22:33:44:55:66 2437 -50 '[ESS]' 'a.b'
	printf '%s\t%s\t%s\t%s\t%s\n' 11:22:33:44:55:77 2437 -60 '[ESS]' 'axb'
	printf '%s\t%s\t%s\t%s\t%s\n' 11:22:33:44:55:88 2437 -70 '[ESS]' 'a.b'
} >"$TMP/scan-results"
check_ok "scan: deduplicates SSIDs literally" scan
expect_lines "scan: literal SSID dedupe" "$TMP/out" 'a.b\t-50\t0\naxb\t-60\t0'

reset
save_scan_results
SUP_FAIL_D=1
check_ok "scan: retries without -Dnl80211" scan
expect_count "scan: supplicant retried once" "$TMP/sup.log" 2

# --- scan: no networks ----------------------------------------------------
reset
printf 'bssid / frequency / signal level / flags / ssid\n' >"$TMP/scan-results"
check_ok "scan: no visible networks" scan
expect_empty "scan: empty output when nothing is visible" "$TMP/out"

# --- scan: failures -------------------------------------------------------
reset
SCAN_FAIL=1
check_fail "scan: fails when scan_results is unavailable" scan
expect_contains "scan: explains scan failure" "扫描" "$TMP/err"

reset
SUP_FAIL=1
check_fail "scan: temporary supplicant start failure" scan
expect_contains "scan: explains supplicant failure" "启动 wpa_supplicant" "$TMP/err"

reset
FAKE_WPA_SUPPLICANT=/nonexistent/wpa_supplicant
check_fail "scan: missing wpa_supplicant" scan
expect_contains "scan: explains missing supplicant" "缺少" "$TMP/err"

# --- connect: delegates to linkslot-net.sh --------------------------------
reset
check_ok "connect: delegates to linkslot-net.sh" connect home secret123
expect_line "connect: runs 'up'" "argv=up" "$TMP/net.log"
expect_line "connect: exports ssid" "ssid=home" "$TMP/net.log"
expect_line "connect: exports psk" "psk=secret123" "$TMP/net.log"
expect_line "connect: exports timeout" "timeout=30" "$TMP/net.log"

reset
check_ok "connect: open network" connect open-wifi ""
expect_line "connect: runs 'up'" "argv=up" "$TMP/net.log"
expect_line "connect: empty psk is passed through" "psk=" "$TMP/net.log"

# connect 默认按脚本自身位置推导 ROOT（../System/linkSlot/shell/linkslot-net.sh）。
reset
mkdir -p "$TMP/card/System"
cp "$SCRIPT" "$TMP/card/System/linkSlot/shell/slot-wifi.sh"
cat >"$TMP/card/System/linkSlot/shell/linkslot-net.sh" <<'EOF'
#!/bin/sh
{
	printf 'argv=%s\n' "$*"
	printf 'ssid=%s\n' "${LINKSLOT_WIFI_SSID:-}"
	printf 'psk=%s\n' "${LINKSLOT_WIFI_PSK:-}"
} >"$TMP_NET_LOG"
exit 0
EOF
chmod 755 "$TMP/card/System/linkSlot/shell/linkslot-net.sh"
if TMP_NET_LOG="$TMP/net.log" AGS_LINKSLOT_WLAN=wlan0 AGS_CTRL_DIR="$TMP/ctrl" \
	sh "$TMP/card/System/linkSlot/shell/slot-wifi.sh" connect home secret123 >"$TMP/out" 2>"$TMP/err"; then
	pass "connect: uses linkslot-net.sh next to the script"
else
	fail "connect: default linkslot-net.sh resolution failed"
	sed 's/^/  err: /' "$TMP/err" >&2
fi
expect_line "connect: default net script runs 'up'" "argv=up" "$TMP/net.log"
expect_line "connect: default net script gets ssid" "ssid=home" "$TMP/net.log"

reset
NET_RC=1
check_exit "connect: delegated failure is propagated" 1 connect home secret123

reset
check_exit "connect: empty ssid exits 2" 2 connect "" secret123
expect_contains "connect: usage for empty ssid" "用法" "$TMP/err"

# --- usage ---------------------------------------------------------------
reset
check_exit "usage: unknown subcommand exits 2" 2 bogus
expect_contains "usage: prints usage" "usage:" "$TMP/err"

reset
check_exit "usage: radio without on|off exits 2" 2 radio bogus
expect_contains "usage: radio usage" "radio on|off" "$TMP/err"

reset
LOG_LEVEL=debug
check_ok "debug: radio status succeeds" radio-status
expect_contains "debug: radio status emits debug line" "[debug] slot-wifi:" "$TMP/err"

echo
if [ "$FAILS" -gt 0 ]; then
	echo "FAILED: $FAILS test(s)"
	exit 1
fi
echo "PASSED: all slot-wifi.sh tests"
exit 0
