#!/usr/bin/env bash
# End-to-end of the no-ssh mode on Linux (T-198): the PC half is `run.sh --sim --mac-manual` and the
# "Mac" half is the command it prints, run locally with --sim. Same files as the real flow: the
# Mac output is saved in the --mac-results file. NEVER in CI (it builds in release); run by hand.
#
#   scripts/hw/test-manual-sim.sh [--ref REF]   # default: HEAD
set -uo pipefail
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
REF=HEAD
[ "${1:-}" = --ref ] && REF=$2
W=$(mktemp -d "${TMPDIR:-/tmp}/tdhw-manual-sim.XXXXXX")
trap 'kill $(jobs -p) 2>/dev/null; rm -rf "$W"' EXIT

"$ROOT/scripts/hw/run.sh" --sim --mac-manual --ref "$REF" --mac-results "$W/mac-results.txt" \
  --mac-command-file "$W/mac-command.txt" >"$W/run.out" 2>"$W/run.err" &
RUN=$!

# The command appears once the PC half has built and started.
n=0
while [ ! -s "$W/mac-command.txt" ]; do
  kill -0 $RUN 2>/dev/null || { tail -20 "$W/run.err"; echo "FAIL: run.sh ended before printing the command"; exit 1; }
  [ $n -ge 1800 ] && { echo "FAIL: no Mac command after 30 min"; exit 1; }
  sleep 1; n=$((n+1))
done

# Skew between the halves: the "Mac" starts a few seconds later, like a user pasting the command.
sleep 3
bash -c "$(cat "$W/mac-command.txt")" >"$W/mac-results.txt" 2>"$W/mac.err"
MAC_RC=$?
wait $RUN
RUN_RC=$?

echo "mac-side exit: $MAC_RC; run.sh exit: $RUN_RC"
fail=0
check() { if grep -q "$1" "$2"; then echo "ok: $3"; else echo "FAIL: $3"; fail=1; fi; }
check 'MAC RESULTS END' "$W/mac-results.txt" "the Mac printed its results block"
for c in pair ping tunnel-process tunnel-files tunnel-port; do
  check "\"check\":\"$c\",\"machine\":\"mac\",\"outcome\":\"pass\"" "$W/mac-results.txt" "Mac $c passes"
done
REPORT=$(sed -n 's/.*report: //p' "$W/run.err" | tail -1)
[ -f "$REPORT" ] && check 'ping' "$REPORT" "the report includes the Mac results" || { echo "FAIL: no report"; fail=1; }
[ "$RUN_RC" = 0 ] || { echo "FAIL: run.sh code $RUN_RC"; tail -15 "$W/run.err"; fail=1; }
[ "$MAC_RC" = 0 ] || { echo "FAIL: mac-side code $MAC_RC"; tail -15 "$W/mac.err"; fail=1; }
pgrep -f "tandem-hw/[0-9]+-[0-9]+/bin/tandemd" >/dev/null && { echo "FAIL: processes left over"; fail=1; }
exit $fail
