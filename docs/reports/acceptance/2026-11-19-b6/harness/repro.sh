#!/bin/bash
source /tmp/claude-1000/b6/env.sh
cd /tmp/claude-1000/b6/harness
T=fin4
node up.mjs $T '{"host":true}' > /dev/null; sleep 2
node f1.mjs $T > repro-f1.out 2>&1; node f1c.mjs $T > repro-f1c.out 2>&1; node f1d.mjs $T > repro-f1d.out 2>&1
node fin.mjs $T > /dev/null; node t3.mjs $T; sleep 22
node f2.mjs $T > repro-f2.out 2>&1; node f2g.mjs $T > repro-f2g.out 2>&1; node f3.mjs $T > repro-f3.out 2>&1
echo done > repro.done
