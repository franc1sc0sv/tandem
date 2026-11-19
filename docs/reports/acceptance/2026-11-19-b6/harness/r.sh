#!/bin/bash
source /tmp/claude-1000/b6/env.sh
export ACC_SHOTS=$WT/docs/reports/acceptance/2026-11-19-b6
cd /tmp/claude-1000/b6/harness
timeout "${T:-300}" node "$@" 2>&1 | cut -c1-"${W:-500}" | tail -n "${N:-40}"
