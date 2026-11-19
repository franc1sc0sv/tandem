#!/bin/bash
# h.sh <script.mjs> args... : runs a harness script (the round 1 one plus these) with the round 2 environment.
# The tandemd socket does not allow long paths (SUN_LEN): ACC_RUN must be short.
# WT: worktree root; the binaries (tandemd, tandem, acc_host, acc_bridge) go in the directory that `bin()` of lib.mjs reads.
: "${WT:?set WT to the worktree root}"
export ACC_SHOTS=${ACC_SHOTS:-$WT/docs/reports/acceptance/2026-11-19-b4}
export ACC_RUN=${ACC_RUN:-/tmp/claude-1000/acc4b/run}
cd "$(dirname "$0")"
exec node "$@"
