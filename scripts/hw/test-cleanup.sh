#!/usr/bin/env bash
# Cleanup tests of the manual hardware suite (T-242). NEVER in CI; run by hand:
#
#   scripts/hw/test-cleanup.sh [--no-run-sh]   # --no-run-sh skips the part that builds (release)
#
# 1. mac-side.sh in real mode (git and cargo replaced by stubs in a throwaway HOME): killing run A
#    must not delete run B's folder or the shared build cache; the last run out removes the cache.
# 2. run.sh --sim killed with SIGTERM in the middle of a step still prints "cleanup", leaves no
#    process behind and removes its folders.
set -uo pipefail
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
W=$(mktemp -d "${TMPDIR:-/tmp}/tdhw-cleanup.XXXXXX")
trap 'kill $(jobs -p) 2>/dev/null; rm -rf "$W"' EXIT
fail=0
ok() { echo "ok: $1"; }
bad() { echo "FAIL: $1"; fail=1; }
wait_for() { local n=0; while ! eval "$1"; do n=$((n+1)); [ $n -ge "${2:-240}" ] && return 1; sleep 0.5; done; }

# --- 1. mac-side.sh: concurrent runs --------------------------------------------------
C1=aaaaaaaaaaaa1 C2=bbbbbbbbbbbb2   # two commits: their caches are separate
H=$W/home; mkdir -p "$H/.cache/tandem-hw-src-${C1:0:12}/.git" "$H/.cache/tandem-hw-src-${C2:0:12}/.git" "$W/stubs"
cat >"$W/stubs/git" <<'STUB'
#!/usr/bin/env bash
case " $* " in *" rev-parse "*) echo abc1234 ;; esac
exit 0
STUB
cat >"$W/stubs/cargo" <<'STUB'
#!/usr/bin/env bash
mkdir -p "$CARGO_TARGET_DIR/release"
printf '#!/usr/bin/env bash\nsleep 300\n' >"$CARGO_TARGET_DIR/release/tandemd"
printf '#!/usr/bin/env bash\ncase $1 in run) sleep 300 ;; *) exit 0 ;; esac\n' >"$CARGO_TARGET_DIR/release/tandem-hw"
chmod +x "$CARGO_TARGET_DIR/release/"*
STUB
chmod +x "$W/stubs/"*
mac() { # mac <dir> <port> <commit>   (the dir is under /tmp, like a real run)
  HOME=$H PATH="$W/stubs:$PATH" bash "$ROOT/scripts/hw/mac-side.sh" --commit "$3" --pc-ip 127.0.0.1 \
    --pc-hw /x --pc-work /y --mac-dir "$1" --port "$2" --groups tunnels --hold 300 >"$1.out" 2>&1 &
}
A=$W/a B=$W/b
mac "$A" 47911 $C1; PA=$!
wait_for "[ -x $A/bin/tandem-hw ] && grep -q 'tunnel-process' $A.out" || bad "run A did not reach its checks"
mac "$B" 47912 $C1; PB=$!
wait_for "[ -x $B/bin/tandem-hw ] && grep -q 'tunnel-process' $B.out" || bad "run B did not reach its checks"
kill -TERM "$PA"; wait "$PA" 2>/dev/null
[ ! -e "$A" ] && ok "run A removed its own folder" || bad "run A left its folder"
[ -x "$B/bin/tandem-hw" ] && ok "run B's folder survives run A's cleanup" || bad "run A deleted run B's binaries"
[ -d "$H/.cache/tandem-hw-target-${C1:0:12}" ] && ok "the build cache survives while run B (same commit) is alive" || bad "the cache was deleted under run B"
kill -0 "$PB" 2>/dev/null && ok "run B is still running" || bad "run B died"
kill -TERM "$PB"; wait "$PB" 2>/dev/null
[ ! -e "$B" ] && ok "run B removed its folder" || bad "run B left its folder"
[ ! -e "$H/.cache/tandem-hw-target-${C1:0:12}" ] && [ ! -e "$H/.cache/tandem-hw-src-${C1:0:12}" ] && ok "the last run out removed the cache" || bad "the cache is still there"
pgrep -f "$W/[ab]/bin/" >/dev/null && bad "processes left over" || ok "no processes left over"

# Different commits: each run has its own tree and cache; one run's cleanup never touches the other's.
A=$W/c D=$W/d
mkdir -p "$H/.cache/tandem-hw-src-${C1:0:12}/.git"   # the first part removed it
mac "$A" 47913 $C1; PA=$!
wait_for "[ -x $A/bin/tandem-hw ] && grep -q 'tunnel-process' $A.out" || bad "run C did not reach its checks"
mac "$D" 47914 $C2; PD=$!
wait_for "[ -x $D/bin/tandem-hw ] && grep -q 'tunnel-process' $D.out" || bad "run D did not reach its checks"
[ -d "$H/.cache/tandem-hw-target-${C1:0:12}" ] && [ -d "$H/.cache/tandem-hw-target-${C2:0:12}" ] && ok "one cache per commit" || bad "caches are not per commit"
kill -TERM "$PA"; wait "$PA" 2>/dev/null
[ ! -e "$H/.cache/tandem-hw-target-${C1:0:12}" ] && ok "run C (alone on its commit) removed its cache" || bad "run C left its cache"
[ -d "$H/.cache/tandem-hw-target-${C2:0:12}" ] && [ -x "$D/bin/tandem-hw" ] && ok "run D (other commit) is untouched" || bad "run C's cleanup touched run D"
kill -TERM "$PD"; wait "$PD" 2>/dev/null
[ -z "$(ls "$H/.cache/tandem-hw-locks" 2>/dev/null)" ] && ok "no lock files left" || bad "lock files left: $(ls "$H/.cache/tandem-hw-locks")"

# --mac-dir that could be more than a run folder is refused, and nothing is deleted.
mkdir -p "$H/keep"; : >"$H/keep/f"
for bad_dir in / "$H" "$H/" /var/tmp/x /tmp /tmp/ "/tmp/../etc"; do
  HOME=$H PATH="$W/stubs:$PATH" bash "$ROOT/scripts/hw/mac-side.sh" --commit $C1 --pc-ip 127.0.0.1 --pc-hw /x --pc-work /y \
    --mac-dir "$bad_dir" >"$W/refuse.out" 2>&1; rc=$?
  [ $rc = 2 ] && grep -q 'ERROR.*--mac-dir' "$W/refuse.out" && ok "--mac-dir '$bad_dir' is refused" || bad "--mac-dir '$bad_dir' was not refused (rc $rc)"
done
[ -f "$H/keep/f" ] && ok "nothing was deleted by the refused runs" || bad "a refused run deleted files"

# --- 2. run.sh --sim killed with SIGTERM mid-step -------------------------------------
if [ "${1:-}" != --no-run-sh ]; then
  "$ROOT/scripts/hw/run.sh" --sim --ref HEAD >"$W/run.out" 2>"$W/run.err" &
  R=$!
  wait_for "grep -q 'test agents running' $W/run.err" 3600 || { tail -20 "$W/run.err"; bad "run.sh did not start its agents"; }
  wait_for "grep -qE 'hw [0-9:]+\] (pc|mac): ' $W/run.err" 60 || bad "run.sh did not reach a step"
  kill -TERM "$R"; wait "$R"; rc=$?
  RUN_DIR=$(sed -n 's/.*run in //p' "$W/run.err" | tail -1)
  grep -q '\] cleanup' "$W/run.err" && ok "run.sh printed cleanup after SIGTERM" || bad "no cleanup line after SIGTERM"
  [ "$rc" = 143 ] && ok "exit code 143" || bad "exit code $rc, expected 143"
  left=$(pgrep -af "tandem-hw/[0-9]+-[0-9]+/bin/tandemd")
  [ -n "$left" ] && { echo "$left" | cut -c1-200; bad "agents left over"; } || ok "no agents left over"
  [ -s "$RUN_DIR/out/final-logs/pc/agent.out" ] && ok "the PC agent log was saved in out/final-logs" || bad "the PC agent log was not saved ($RUN_DIR)"
  [ -n "$RUN_DIR" ] && rm -rf "$RUN_DIR"
fi
exit $fail
