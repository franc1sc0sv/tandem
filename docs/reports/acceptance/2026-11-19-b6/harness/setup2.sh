#!/bin/bash
source /tmp/claude-1000/b6/env.sh
cd /tmp/claude-1000/b6/harness
node up.mjs $1 "$2" > /dev/null && sleep 2
node f1.mjs $1 > /tmp/claude-1000/b6/harness/f1-$1.out 2>&1
node fin.mjs $1 > /dev/null
