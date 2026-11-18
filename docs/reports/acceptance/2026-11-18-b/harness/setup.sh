#!/bin/bash
# setup.sh <tag> : pareja nueva con anfitrión, emparejada por la UI, sin guía y con monitores dobles
source /tmp/claude-1000/accB/env.sh
cd /tmp/claude-1000/accB/harness
node up.mjs $1 '{"host":true}' > /dev/null && sleep 2
node f1.mjs $1 > /tmp/claude-1000/accB/harness/f1-$1.out 2>&1
node fin.mjs $1 > /dev/null
node t3.mjs $1
sleep 9
