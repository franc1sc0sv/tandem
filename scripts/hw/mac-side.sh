#!/usr/bin/env bash
# Mac half of the hardware suite, WITHOUT ssh (T-198). Run it ON the Mac; `run.sh --mac-manual`
# prints the exact one-line command to paste. It is bash on purpose: the one-liner starts it
# with `bash`, so it works the same when the user's shell is zsh (no word-splitting traps).
#
#   mac-side.sh --commit SHA --pc-ip IP [--mac-ip IP] --pc-hw PATH --pc-work DIR [--pc-sep S]
#               --mac-dir DIR [--groups link,tunnels,input] [--plan]
#               [--repo URL] [--port N] [--peer-port N] [--pair-wait SECS] [--window SECS]
#               [--keep SECS] [--hold SECS] [--sim --bin-dir DIR]
#
# Clones or updates the source in ~/.cache/tandem-hw-src at SHA (git over the CLI, so the user's
# own credentials apply), builds with absolute paths, provisions a test agent in DIR (a short
# /tmp path: the Unix socket has a 104-byte limit), starts it by absolute path, runs the checks in
# the same order as run.sh with windows that tolerate minutes of skew against the PC half, and
# prints the results between markers. The trap ALWAYS kills the agent and removes /tmp/tdhw*
# and ~/.cache/tandem-hw-*. Afterwards, remove the privacy permissions you granted by hand
# (Accessibility / Input Monitoring) in System Settings.
#
# --sim: local test on Linux (no clone, no build; binaries from --bin-dir; loopback; the trap only
# removes --mac-dir).
set -uo pipefail

COMMIT="" PC_IP="" MAC_IP="" PC_HW="" PC_WORK="" PC_SEP='\' MAC_DIR="" GROUPS_="link,tunnels"
REPO="" PORT=47871 PEER_PORT="" PAIR_WAIT=900 WINDOW=420 KEEP=120 HOLD=180 SIM=0 BIN_DIR="" PLAN=0
while [ $# -gt 0 ]; do
  case $1 in
    --commit) COMMIT=$2; shift ;;
    --pc-ip) PC_IP=$2; shift ;;
    --mac-ip) MAC_IP=$2; shift ;;
    --pc-hw) PC_HW=$2; shift ;;
    --pc-work) PC_WORK=$2; shift ;;
    --pc-sep) PC_SEP=$2; shift ;;
    --mac-dir) MAC_DIR=$2; shift ;;
    --groups) GROUPS_=$2; shift ;;
    --repo) REPO=$2; shift ;;
    --port) PORT=$2; shift ;;
    --peer-port) PEER_PORT=$2; shift ;;
    --pair-wait) PAIR_WAIT=$2; shift ;;
    --window) WINDOW=$2; shift ;;
    --keep) KEEP=$2; shift ;;
    --hold) HOLD=$2; shift ;;
    --sim) SIM=1 ;;
    --bin-dir) BIN_DIR=$2; shift ;;
    --plan) PLAN=1 ;;
    -h|--help) sed -n 2,22p "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
PEER_PORT=${PEER_PORT:-$PORT}
MAC_DIR=${MAC_DIR:-/tmp/tdhw-$(date +%H%M%S)}
SRC=$HOME/.cache/tandem-hw-src
TARGET=$HOME/.cache/tandem-hw-target
has() { case ",$GROUPS_," in *",$1,"*) return 0 ;; esac; return 1; }

if [ "$PLAN" = 1 ]; then
  echo "mode: $([ "$SIM" = 1 ] && echo simulated || echo Mac); groups: $GROUPS_; commit: ${COMMIT:-?}"
  echo "1 clone/update $SRC at the commit  2 build into $TARGET  3 provision in $MAC_DIR and start tandemd"
  echo "4 pair,ping (wait up to ${PAIR_WAIT}s)  5 tunnels  6 synthetic input (windows ${WINDOW}s, keep ${KEEP}s)"
  echo "7 print results  8 hold ${HOLD}s for the PC half  9 cleanup (agent, /tmp/tdhw*, ~/.cache/tandem-hw-*)"
  exit 0
fi

say() { printf '[mac %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { say "ERROR: $*"; exit 2; }
[ -n "$PC_IP" ] && [ -n "$PC_HW" ] && [ -n "$PC_WORK" ] || die "missing --pc-ip, --pc-hw or --pc-work"
[ "$SIM" = 1 ] || [ -n "$COMMIT" ] || die "missing --commit"
case $MAC_DIR in /*) ;; *) die "--mac-dir must be absolute" ;; esac

AGENT_PID="" BG_PIDS=()
# Always runs: kills the agent (by PID and by absolute path, so a relative start cannot escape) and
# removes the folders.
cleanup() {
  local rc=$?
  trap - EXIT INT TERM HUP
  say "cleanup"
  local p
  for p in "${BG_PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
  [ -n "$AGENT_PID" ] && kill "$AGENT_PID" 2>/dev/null
  pkill -f "$MAC_DIR/bin/" 2>/dev/null
  sleep 1
  pkill -9 -f "$MAC_DIR/bin/" 2>/dev/null
  if [ "$SIM" = 1 ]; then
    rm -rf "$MAC_DIR"
  else
    rm -rf /tmp/tdhw* "$HOME"/.cache/tandem-hw-*
    say "Remove the privacy permissions you granted (Accessibility, Input Monitoring) in System Settings."
  fi
  exit "$rc"
}
trap cleanup EXIT INT TERM HUP

# --- 1-2. Code and build ----------------------------------------------------------
if [ "$SIM" = 1 ]; then
  [ -d "$BIN_DIR" ] || die "--sim needs --bin-dir"
  SRC_BIN=$BIN_DIR
else
  export CARGO_NET_GIT_FETCH_WITH_CLI=true CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  command -v cargo >/dev/null || die "cargo not found (install rustup: curl https://sh.rustup.rs -sSf | sh -s -- -y)"
  pgrep -x tandemd >/dev/null && die "there is a real tandemd running on the Mac; close it and run again"
  if [ ! -d "$SRC/.git" ]; then
    [ -n "$REPO" ] || die "$SRC does not exist: pass --repo URL"
    rm -rf "$SRC"; git clone -q "$REPO" "$SRC" || die "git clone failed"
  fi
  git -C "$SRC" fetch -q origin 2>/dev/null; git -C "$SRC" fetch -q origin "$COMMIT" 2>/dev/null
  git -C "$SRC" checkout -q -f "$COMMIT" || die "commit $COMMIT is not available (is it pushed?)"
  say "building at $(git -C "$SRC" rev-parse --short HEAD)"
  (cd "$SRC" && CARGO_TARGET_DIR=$TARGET cargo build --release -p tandemd -p tandem-hw -p tandem-monitor) >"$HOME/.cache/tandem-hw-build.log" 2>&1 &
  BG_PIDS+=($!); wait $!
  [ -x "$TARGET/release/tandemd" ] || { tail -20 "$HOME/.cache/tandem-hw-build.log" >&2; die "build failed"; }
  SRC_BIN=$TARGET/release
fi
if [ -n "$MAC_IP" ] && [ "$SIM" = 0 ] && ! ifconfig 2>/dev/null | grep -q "inet $MAC_IP "; then
  say "WARNING: $MAC_IP is not an address of this Mac; the PC will try to reach it anyway"
fi

# --- 3. Provision and start (absolute paths) ---------------------------------------------
mkdir -p "$MAC_DIR/bin" "$MAC_DIR/work"
for f in tandemd tandem-hw tandem-monitor-guard; do
  [ -e "$SRC_BIN/$f" ] && cp "$SRC_BIN/$f" "$MAC_DIR/bin/"
done
HW=$MAC_DIR/bin/tandem-hw
DATA=$MAC_DIR/agent/data
"$HW" provision --role mac --root "$MAC_DIR/agent" --port "$PORT" --peer "$PC_IP:$PEER_PORT" >/dev/null || die "provision"
EXTRA=(); [ "$SIM" = 1 ] && EXTRA=(--sim-backends)
(cd "$MAC_DIR" && TANDEM_LOG='tandemd=debug,tandem_link=info' nohup "$MAC_DIR/bin/tandemd" \
  --config "$MAC_DIR/agent/tandem.toml" --data-dir "$DATA" ${EXTRA[@]+"${EXTRA[@]}"} >"$MAC_DIR/agent.out" 2>&1 </dev/null &
  echo $! >"$MAC_DIR/agent.pid")
AGENT_PID=$(cat "$MAC_DIR/agent.pid" 2>/dev/null)
[ -n "$AGENT_PID" ] || die "the agent did not start"
say "agent $AGENT_PID running; waiting for the PC (up to ${PAIR_WAIT}s)"

# --- 4. Checks -------------------------------------------------------------------------
RES=$MAC_DIR/results.jsonl; : >"$RES"
step() {
  local checks=$1; shift
  local tmp="$MAC_DIR/step.$RANDOM" c
  say "$checks"
  "$HW" run --label mac --data-dir "$DATA" --checks "$checks" "$@" >"$tmp" 2>>"$MAC_DIR/step.err" &
  BG_PIDS+=($!); wait $!
  local rc=$?
  cat "$tmp" >>"$RES"
  if [ $rc -ne 0 ] && ! grep -q '"outcome"' "$tmp"; then
    for c in ${checks//,/ }; do
      printf '{"check":"%s","machine":"mac","outcome":"fail","detail":"the process ended without a result (code %s); see logs","millis":0,"covers":[]}\n' "$c" "$rc" >>"$RES"
    done
  fi
}
if has link; then
  step pair --pair-wait-secs "$PAIR_WAIT"
  step ping
fi
if has tunnels; then
  step tunnel-process,tunnel-files,tunnel-port,tunnel-workspace --peer-exe "$PC_HW" --remote-dir "$PC_WORK" --remote-sep "$PC_SEP"
fi
if has input; then
  # PC -> Mac: this side receives (long window: the PC injects whenever its half gets there).
  step input-latency --window-secs "$WINDOW"
  # Mac -> PC: this side injects while the PC half measures (it waits for its window).
  step input-cross --keep-secs "$KEEP"
fi

# --- 5. Results (the user pastes them into a file for run.sh --mac-results) ----------------------
echo "=== TANDEM-HW MAC LOG BEGIN ==="
tail -n 60 "$MAC_DIR/agent.out" 2>/dev/null
echo "=== TANDEM-HW MAC LOG END ==="
echo "=== TANDEM-HW MAC RESULTS BEGIN ==="
cat "$RES"
echo "=== TANDEM-HW MAC RESULTS END ==="
say "results printed above: copy them to the PC side. Holding the agent ${HOLD}s for the PC half."
sleep "$HOLD" &
BG_PIDS+=($!); wait $!
exit 0
