#!/bin/bash
# Round 6 follow-ups: F1 steps 4-6 on Windows+Windows and macOS+macOS (named Mac/PC for the
# harness), then the per-pair scripts on win2/mac2 with neutral names, then G2 and tray3.
source /tmp/claude-1000/b6/env.sh
cd /tmp/claude-1000/b6/harness
L=logs/followup; mkdir -p $L
r() { echo "== $* $(date +%T)"; timeout 600 node "$@" > $L/$(echo "$*" | tr ' /' '__').log 2>&1; grep -h -E 'PASA|FALLA|ERROR' $L/$(echo "$*" | tr ' /' '__').log | cut -c1-260; }
for spec in 'ww2|{"host":true,"a":["windows","Mac","#7aa7e6"],"b":["windows","PC","#a99be8"]}' 'mm2|{"host":true,"a":["macos","Mac","#7aa7e6"],"b":["macos","PC","#a99be8"]}'; do
  T=${spec%%|*}; O=${spec#*|}
  node up.mjs $T "$O" > /dev/null; sleep 2
  r f1.mjs $T; r f1c.mjs $T; r f1d.mjs $T
  node down.mjs $T
done
bash setup2.sh win2 '{"host":true,"a":["windows","Tower-1","#7aa7e6"],"b":["windows","Tower-2","#a99be8"]}'; node t3.mjs win2; sleep 9
bash setup2.sh mac2 '{"host":true,"a":["macos","Book-1","#7aa7e6"],"b":["macos","Book-2","#a99be8"]}'; node t3.mjs mac2; sleep 9
for T in win2 mac2; do
  r t166.mjs $T; r t166b.mjs $T; r wiz5.mjs $T; r f2cross.mjs $T; r t244.mjs $T; r t270.mjs $T; r es.mjs $T; r f16b.mjs $T
done
r g2.mjs; r tray3.mjs
echo FOLLOWUP-END
