#!/bin/sh
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
SCRIPT="$HERE/System/linkSlot/shell/linkslot-link.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
FAILS=0

fail() {
	echo "FAIL: $*"
	FAILS=$((FAILS + 1))
}

make_fake() {
	name="$1"
	out="$2"
	shift 2
	{
		echo '#!/bin/sh'
		echo "printf '%s\\n' \"\$*\" >> \"$out\""
		for line in "$@"; do echo "$line"; done
		echo 'exit 0'
	} > "$TMP/bin/$name"
	chmod 755 "$TMP/bin/$name"
}

reset() {
	rm -rf "$TMP/net" "$TMP/run" "$TMP/ctrl" "$TMP"/*.log "$TMP/bin"
	mkdir -p "$TMP/net/wlan0" "$TMP/net/wlan1" "$TMP/run" "$TMP/ctrl" "$TMP/modules" "$TMP/bin"
	: > "$TMP/modules/8821cs.ko"
	if [ "$1" = yes ]; then
		echo "8821cs 2863104 0 - Live 0x0" > "$TMP/proc-modules"
	else
		: > "$TMP/proc-modules"
	fi
	make_fake wpa_cli "$TMP/cli.log" 'echo "wpa_state=${FAKE_STATE:-COMPLETED}"'
	make_fake wpa_supplicant "$TMP/sup.log"
	make_fake ip "$TMP/ip.log"
	make_fake insmod "$TMP/mod.log"
	make_fake legacy "$TMP/legacy.log"
}

run() {
	shift
	PATH="$TMP/bin:$PATH" \
		AGS_MODULES="$TMP/modules" AGS_NET_SYS="$TMP/net" AGS_RUN="$TMP/run" \
		AGS_CTRL_DIR="$TMP/ctrl" AGS_PROC_MODULES="$TMP/proc-modules" \
		AGS_WPA_SUPPLICANT="$TMP/bin/wpa_supplicant" AGS_WPA_CLI="$TMP/bin/wpa_cli" \
		AGS_IP="$TMP/bin/ip" AGS_INSMOD="$TMP/bin/insmod" \
		AGS_LINK_WAIT_S="${WAIT_S:-1}" AGS_LINK_PIN_WAIT_S="${WAIT_S:-1}" \
		AGS_LINK_TRACE="${LINK_TRACE:-}" \
		AGS_LINK_PAUSE_HOME_WIFI="${LINK_PAUSE_HOME_WIFI:-1}" \
		AGS_LINK_RESTORE_WIFI="${LINK_RESTORE_WIFI:-1}" \
		SLOT_ROOT="${SLOT_ROOT:-}" \
		sh "$SCRIPT" "$@"
}

reset yes
run absent link host && rc=0 || rc=$?
[ "$rc" = 0 ] || fail "host exited $rc"
grep -q 'ap_scan=2' "$TMP/run/slotlink-ap.conf" || fail "host AP scans before starting"
grep -q 'frequency=5745' "$TMP/run/slotlink-ap.conf" || fail "host AP not on 5745"
grep -q 'mode=2' "$TMP/run/slotlink-ap.conf" || fail "host config is not an AP"
grep -q 'addr add 10.42.0.1/24 dev wlan1' "$TMP/ip.log" || fail "host not addressed"
[ -f "$TMP/run/slotlink.session" ] || fail "host left no session mark"

reset yes
export FAKE_STATE=SCANNING
run absent link host && rc=0 || rc=$?
unset FAKE_STATE
[ "$rc" = 1 ] || fail "dead AP exited $rc, want 1"
grep -q 'addr add' "$TMP/ip.log" 2>/dev/null && fail "dead AP was addressed"

reset yes
run absent link join && rc=0 || rc=$?
[ "$rc" = 0 ] || fail "join exited $rc"
grep -q 'scan_freq=' "$TMP/run/slotlink-sta.conf" \
	&& fail "the first join attempt should scan every channel, not pin 5745"
grep -q 'addr add 10.42.0.2/24 dev wlan0' "$TMP/ip.log" || fail "join not addressed"

reset yes
export FAKE_STATE=SCANNING
run absent link join && rc=0 || rc=$?
unset FAKE_STATE
[ "$rc" = 3 ] || fail "join with no host exited $rc, want 3"
[ "$(grep -c -- '-i wlan0' "$TMP/sup.log")" = 2 ] || fail "join did not make two attempts"
grep -q 'scan_freq=5745' "$TMP/run/slotlink-sta.conf" \
	|| fail "the pinned retry did not ask for 5745"

reset yes
run absent link join >/dev/null
run absent link down && rc=0 || rc=$?
[ "$rc" = 0 ] || fail "down exited $rc"
for f in slotlink-ap.conf slotlink-sta.conf slotlink.session; do
	[ -e "$TMP/run/$f" ] && fail "down left $f"
done

reset yes
run absent link cool && rc=0 || rc=$?
[ "$rc" = 0 ] || fail "cool exited $rc"
[ -e "$TMP/mod.log" ] && fail "cool touched the driver"

reset no
run absent link warm >/dev/null || true
[ -e "$TMP/mod.log" ] && fail "insmod on BaseOS"

reset yes
make_fake wpa_cli "$TMP/cli.log" 'sleep 0.3' 'echo "wpa_state=SCANNING"'
t0=$(date +%s)
run absent link join >/dev/null && rc=0 || rc=$?
t1=$(date +%s)
[ "$rc" = 3 ] || fail "slow join exited $rc, want 3"
[ $((t1 - t0)) -le 4 ] || fail "a 2 s join took $((t1 - t0)) s with a slow wpa_cli"

reset yes
run absent link sideways 2>/dev/null && rc=0 || rc=$?
[ "$rc" = 2 ] || fail "bad verb exited $rc, want 2"

reset yes
LINK_TRACE="$TMP/link-trace.log" run absent link host && rc=0 || rc=$?
[ "$rc" = 0 ] || fail "traced host exited $rc"
grep -q '\[debug\] \[link\] slotlink host: ready' "$TMP/link-trace.log" \
	|| fail "host trace did not record readiness"
grep -q '\[debug\] \[link\] slotlink clear: no stale join config' "$TMP/link-trace.log" \
	|| fail "host trace did not record whether wlan0 was cleared"
grep -q 'pause: home WiFi' "$TMP/link-trace.log" \
	|| fail "host trace did not record pausing home WiFi"
grep -q 'addr flush dev wlan0' "$TMP/ip.log" \
	|| fail "host did not flush the home WiFi address"
grep -q 'link set wlan0 down' "$TMP/ip.log" \
	|| fail "host did not take wlan0 down"
[ -f "$TMP/run/slotlink-wifi-paused" ] \
	|| fail "host did not leave a WiFi-paused marker"
grep -q 'wlan1 present' "$TMP/link-trace.log" || fail "host trace did not record wlan1"
grep -q 'supplicant exit=' "$TMP/link-trace.log" || fail "host trace did not record supplicant"
if grep -q 'psk=\|link_psk' "$TMP/link-trace.log"; then
	fail "the link trace leaked the PSK"
fi
LINK_TRACE="$TMP/link-trace.log" run absent link down
[ -f "$TMP/run/slotlink-wifi-paused" ] \
	&& fail "link down left the WiFi-paused marker"
grep -q 'restore: no linkslot.sh' "$TMP/link-trace.log" \
	|| fail "link down did not attempt WiFi restore"

reset yes
LINK_PAUSE_HOME_WIFI=0 LINK_TRACE="$TMP/no-pause-trace.log" run absent link host && rc=0 || rc=$?
[ "$rc" = 0 ] || fail "host with WiFi pause disabled exited $rc"
grep -q 'pause: disabled by AGS_LINK_PAUSE_HOME_WIFI=0' "$TMP/no-pause-trace.log" \
	|| fail "host did not record the WiFi pause opt-out"
[ -f "$TMP/run/slotlink-wifi-paused" ] && fail "pause opt-out still marked WiFi paused"
run absent link down

reset yes
mkdir -p "$TMP/card/System"
printf '#!/bin/sh\nprintf "restore\\n" >> "%s/restore.log"\n' "$TMP" \
	> "$TMP/card/System/linkSlot/shell/linkslot.sh"
SLOT_ROOT="$TMP/card" LINK_TRACE="$TMP/restore-trace.log" run absent link host && rc=0 || rc=$?
[ "$rc" = 0 ] || fail "host for restore test exited $rc"
[ -f "$TMP/run/slotlink-wifi-paused" ] || fail "restore test did not pause WiFi"
SLOT_ROOT="$TMP/card" LINK_TRACE="$TMP/restore-trace.log" run absent link down
i=0
while [ ! -f "$TMP/restore.log" ] && [ "$i" -lt 40 ]; do
	sleep 0.05 2>/dev/null || sleep 1
	i=$((i + 1))
done
grep -q '^restore$' "$TMP/restore.log" 2>/dev/null \
	|| fail "link down did not run the saved-WiFi restore command"
[ -f "$TMP/run/slotlink-wifi-restoring" ] && fail "restore left its running marker"

reset yes
: > "$TMP/run/slotlink-sta.conf"
LINK_TRACE="$TMP/stale-trace.log" run absent link host && rc=0 || rc=$?
[ "$rc" = 0 ] || fail "host with a stale join config exited $rc"
grep -q 'stale join config; taking wlan0 away' "$TMP/stale-trace.log" \
	|| fail "host did not report taking wlan0 from a stale join config"
grep -q 'wpa_cli.*-i wlan0 terminate\|-i wlan0 terminate' "$TMP/cli.log" \
	|| fail "host did not terminate the stale wlan0 supplicant"

reset yes
LINK_TRACE="$TMP/link-trace.log" run absent link join && rc=0 || rc=$?
[ "$rc" = 0 ] || fail "traced join exited $rc"
grep -q '\[debug\] \[link\] slotlink join: ready' "$TMP/link-trace.log" \
	|| fail "join trace did not record readiness"
grep -q 'supplicant_exit=' "$TMP/link-trace.log" || fail "join trace did not record supplicant"
LINK_TRACE="$TMP/link-trace.log" run absent link down
grep -q '\[debug\] \[link\] slotlink down: end' "$TMP/link-trace.log" \
	|| fail "down trace did not record cleanup"

if [ "$FAILS" -eq 0 ]; then
	echo "slotlink: all passed"
else
	echo "slotlink: $FAILS failed"
	exit 1
fi
