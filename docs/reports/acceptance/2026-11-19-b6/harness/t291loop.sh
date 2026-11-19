#!/bin/bash
# t291loop.sh <first> <last> <variant> : pairs u6-<variant>-<i> through the UI; variants normal, nodial1, nodial2, load, loadnodial
source /tmp/claude-1000/b6/env.sh
cd /tmp/claude-1000/b6/harness
V=$3
for i in $(seq $1 $2); do
  T=u6-$V-$i
  case $V in
    nodial1) O='{"host":true,"env1":{"ACC_NO_DIAL":"1"}}';;
    nodial2|loadnodial) O='{"host":true,"env2":{"ACC_NO_DIAL":"1"}}';;
    *) O='{"host":true}';;
  esac
  LOAD=()
  if [[ $V == load* ]]; then for k in $(seq 1 $(nproc)); do (timeout 60 sh -c 'while :; do :; done' &) ; done; fi
  node up.mjs $T "$O" > /dev/null && sleep 2
  timeout 120 node f1.mjs $T > f1-$T.out 2>&1
  grep -E 'FALLA|ERROR' f1-$T.out
  grep -E 'paired: both' f1-$T.out
  node t291.mjs $T ${4:-70} $V
  node down.mjs $T
  pkill -f "while :; do :; done" 2>/dev/null
  sleep 2
done
