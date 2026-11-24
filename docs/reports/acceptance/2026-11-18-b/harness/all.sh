#!/bin/bash
# Official acceptance B run on a new pair (tag fin).
source /tmp/claude-1000/accB/env.sh
cd /tmp/claude-1000/accB/harness
T=fin
L=/tmp/claude-1000/accB/harness/logs; mkdir -p $L
rm -f /tmp/claude-1000/accB/run/results.jsonl
step() { echo "== $1 $(date +%T)"; shift; timeout 600 "$@" > $L/$(echo "$1 $2" | tr ' /' '__').log 2>&1; }
node down.mjs fin 2>/dev/null; for p in $(pgrep -f '^node px.mjs'); do kill $p; done; sleep 1
node up.mjs $T '{"host":true}' > /dev/null; sleep 2
step F1 node f1.mjs $T; step F1c node f1c.mjs $T; step F1d node f1d.mjs $T
node fin.mjs $T > /dev/null; node t3.mjs $T; sleep 22
step F2 node f2.mjs $T; step F2g node f2g.mjs $T
step F3 node f3.mjs $T; step F3b node f3b.mjs $T
step F4 node f4.mjs $T
node restart.mjs $T m1 > /dev/null; node restart.mjs $T m2 > /dev/null; sleep 6; node t3.mjs $T; sleep 9
step F9 node f9.mjs $T
(nohup node px.mjs $T > $L/px.log 2>&1 &); sleep 2
python3 - <<PY
p='/tmp/claude-1000/accB/run/$T/m1/cfg/Tandem/tandem.toml'
s=open(p).read()
if 'power.wol' not in s: s+='\n[power.wol]\nmac = "aa:bb:cc:dd:ee:ff"\n'; open(p,'w').write(s)
PY
node netcfg.mjs $T; sleep 9; node t3.mjs $T; sleep 9
step F6a node f6a.mjs $T; step F6b node f6b.mjs $T
step F5 node f5.mjs $T
step F10 node f10.mjs $T
step F11 node f11.mjs $T
step F12 node f12.mjs $T
step F12b node f12b.mjs $T
step F7F8 node f7f8.mjs $T
step F13s node f13s.mjs $T
node restart.mjs $T m1 > /dev/null; node restart.mjs $T m2 > /dev/null; sleep 6; node t3.mjs $T; sleep 9
step F13u node f13u.mjs $T
node rebridge.mjs $T > /dev/null
step F14 node f14.mjs $T
step MISC node misc.mjs $T
step FIND node findings.mjs $T
step F1b node f1b.mjs $T
step F1e node f1e.mjs $T
step F4c node f4c.mjs $T
step F16 node f16.mjs $T
step G2 node g2.mjs
step SWEEP node sweep.mjs $T
step DEMO node demo-sweep.mjs
step F15 node f15.mjs $T
echo "== FIN $(date +%T)"
