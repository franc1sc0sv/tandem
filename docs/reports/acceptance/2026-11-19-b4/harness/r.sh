#!/bin/bash
source /tmp/claude-1000/acc4b/env.sh
export ACC_SHOTS=$WT/docs/reports/acceptance/2026-11-19-b4
cd /tmp/claude-1000/acc4b/harness
timeout "${T:-300}" node "$@" 2>&1 | cut -c1-"${W:-500}" | tail -n "${N:-40}"
