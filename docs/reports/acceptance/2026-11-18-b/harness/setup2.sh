#!/bin/bash
source /tmp/claude-1000/accB/env.sh
cd /tmp/claude-1000/accB/harness
node up.mjs $1 "$2" > /dev/null && sleep 2
node f1.mjs $1 > /tmp/claude-1000/accB/harness/f1-$1.out 2>&1
node fin.mjs $1 > /dev/null
