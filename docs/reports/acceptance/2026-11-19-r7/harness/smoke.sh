#!/bin/bash
# r7 smoke: smoke.sh <tag> <pair options>: pairing (incl. T-291), T-280 from the non-owner, crossing, MCP tunnels, updates (T-251).
source /tmp/claude-1000/r7/env.sh
cd /tmp/claude-1000/r7/harness
T=$1; L=logs/$T; mkdir -p $L
step() { echo "== $1 $(date +%T)"; shift; timeout 900 "$@" > $L/$(echo "$1 $2" | tr ' /' '__').log 2>&1; grep -hE 'PASA|FALLA|ERROR' $L/$(echo "$1 $2" | tr ' /' '__').log | cut -c1-220; }
node down.mjs $T 2>/dev/null
bash setup2.sh $T "$2"; grep -hE 'PASA|FALLA|ERROR' f1-$T.out | cut -c1-220
node t3.mjs $T
step T291 node t291.mjs $T 70 smoke
step T280a node t280r.mjs $T
export WIN=nonowner; step T280 node t250.mjs $T 2; unset WIN
step F2 node f2.mjs $T
step F2g node f2g.mjs $T
(nohup node px.mjs $T > $L/px.log 2>&1 &); sleep 2
node netcfg.mjs $T; sleep 9; node t3.mjs $T; sleep 9
step F5 node f5.mjs $T
node restart.mjs $T m1 > /dev/null; node restart.mjs $T m2 > /dev/null; sleep 6; node t3.mjs $T; sleep 9
step F13u node f13u.mjs $T
node rebridge.mjs $T > /dev/null; step F16b node f16b.mjs $T
node down.mjs $T >/dev/null 2>&1; for p in $(pgrep -f "^node px.mjs $T"); do kill $p; done
echo "== SMOKE_DONE $T"
