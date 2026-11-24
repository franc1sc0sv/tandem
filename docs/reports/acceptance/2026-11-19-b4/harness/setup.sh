#!/bin/bash
# setup.sh <tag> : new pair with a host, paired through the UI, without the tour and with fake monitors
source /tmp/claude-1000/acc4b/env.sh
cd /tmp/claude-1000/acc4b/harness
node up.mjs $1 '{"host":true}' > /dev/null && sleep 2
node f1.mjs $1 > /tmp/claude-1000/acc4b/harness/f1-$1.out 2>&1
node fin.mjs $1 > /dev/null
node t3.mjs $1
sleep 9
