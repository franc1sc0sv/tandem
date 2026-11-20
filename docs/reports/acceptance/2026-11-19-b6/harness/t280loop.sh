#!/bin/bash
# t280loop.sh <first> <last> <N> : fresh mac-win pairs r6-<i>; report check right after setup and 30 s later; t250 from the non-owner.
source /tmp/claude-1000/b6/env.sh
cd /tmp/claude-1000/b6/harness
for i in $(seq $1 $2); do
  T=r6-$i
  bash setup.sh $T
  grep -E 'FALLA|ERROR' f1-$T.out
  node t280r.mjs $T
  sleep 30
  node t280r.mjs $T
  WIN=nonowner timeout 900 node t250.mjs $T $3 | grep -E 'window|PASA|FALLA|ERROR'
  node down.mjs $T
  sleep 2
done
