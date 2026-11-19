#!/bin/bash
source /tmp/claude-1000/acc4b/env.sh
mkdir -p /tmp/claude-1000/acc4b/run/demo/{cfg,data}
cd /tmp/claude-1000/acc4b/run/demo
export XDG_CONFIG_HOME=$PWD/cfg XDG_DATA_HOME=$PWD/data HOME=$PWD TANDEM_DATA_DIR=$PWD/data/Tandem
nohup /tmp/claude-1000/acc4b/bin/acc_bridge --port 38123 --static $WT/apps/tandem-ui/dist-demo > bridge.out 2>&1 &
echo $! > bridge.pid
