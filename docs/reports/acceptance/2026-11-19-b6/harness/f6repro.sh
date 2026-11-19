#!/bin/bash
# f6repro.sh <tag>: the F6 part of all.sh on a new pair (three paths through px.mjs), then f6a and f6b.
source /tmp/claude-1000/b6/env.sh
cd /tmp/claude-1000/b6/harness
T=$1
bash setup.sh $T
(nohup node px.mjs $T > /tmp/claude-1000/b6/harness/px-$T.log 2>&1 &); sleep 2
node netcfg.mjs $T; sleep 9; node t3.mjs $T; sleep 9
node -e 'import("./lib.mjs").then(async L=>{const {m1,m2}=L.attach(process.argv[1]); for (const m of [m1,m2]){const s=await m.snapshot(); console.log(m.label,"side",s.agent,"peer",s.peer.kind,JSON.stringify(s.peer.link));}})' $T
node f6a.mjs $T | grep -E 'PASA|FALLA|ERROR' | cut -c1-200
node f6b.mjs $T | grep -E 'PASA|FALLA|ERROR' | cut -c1-200
grep -h -E 'MeasureLink|measuring the bandwidth|bandwidth measured' /tmp/claude-1000/b6/run/$T/m1/agent.out | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-200
