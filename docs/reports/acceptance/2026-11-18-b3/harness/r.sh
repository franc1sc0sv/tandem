#!/bin/bash
source /tmp/claude-1000/acc3b/env.sh
export ACC_SHOTS=$WT/docs/reports/acceptance/2026-11-18-b3
cd /tmp/claude-1000/acc3b/harness
timeout "${T:-300}" node "$@" 2>&1 | cut -c1-"${W:-500}" | tail -n "${N:-40}"
