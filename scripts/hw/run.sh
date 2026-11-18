#!/usr/bin/env bash
# Hardware suite (T-123). Run by hand from WSL; NEVER in CI.
#
#   scripts/hw/run.sh [--sim] [--with-monitors] [--only a,b] [--skip-build] [--keep-cache] [--ref REF]
#                     [--mac HOST] [--mac-link DIR] [--pc-ip IP] [--mac-ip IP] [--plan]
#                     [--mac-manual [--mac-results FILE] [--repo URL] [--mac-command-file FILE]]
#
# Builds the Windows binaries (mingw, in WSL) and the macOS ones (over ssh, on the Mac), starts
# a test agent on each machine with preprovisioned identities and data in temporary
# folders, runs the `tandem-hw` checks and leaves the report in
# docs/reports/hw-<date>.md. When done, even if something fails, it kills what it started and deletes
# its folders (trap). It does not touch the user's real install or configuration: if there is
# a real `tandemd` running on either machine, it aborts before starting.
#
#   --sim            everything on this machine with simulated backends (no hardware): validates the
#                    orchestrator and the link and tunnel checks.
#   --with-monitors  adds the two checks that change the PC monitor's input.
#                    They have an automatic rollback (guard + `tandem-monitor-guard` watcher)
#                    and are skipped if the watcher is missing.
#   --only LIST     only these groups: link,tunnels,input,monitors (comma). `monitors` is
#                    ignored without --with-monitors.
#   --keep-cache     keeps ~/.cache/tandem-hw-src and ~/.cache/tandem-hw-target on the Mac
#                    (build cache); by default they are deleted when done.
#   --mac-link DIR   link-local address of the Mac's USB4 cable (or TANDEM_HW_MAC_LINK). If
#                    `mac-runner` does not answer (Wi-Fi down), this address is used with the same
#                    user and key as the ssh alias. No fixed IPs in the repo (D10).
#   --mac-manual     no ssh to the Mac (SOC2, T-198): runs only the PC half with long windows,
#                    prints the one-line command to paste on the Mac (it runs scripts/hw/mac-side.sh
#                    with bash, so zsh is fine) and, when the Mac prints its results, takes
#                    them from FILE (--mac-results; paste the block between the markers) and
#                    merges them into the usual report. Needs --mac-ip; --pc-ip is detected.
#                    The ref must be pushed: the Mac clones it. With --sim, both halves are local.
#   --plan           prints the steps and exits.
#
# Every call to powershell.exe and ssh carries `< /dev/null` and `timeout`; long steps
# run in the background with `wait`, so Ctrl-C and SIGTERM always trigger the cleanup.
#
# Exit: 0 no failures, 1 with failures, 2 if the environment could not be set up.

set -uo pipefail

ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
PS=/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe
PORT_A=47871
PORT_B=47872
SIM=0 MONITORS=0 SKIP_BUILD=0 PLAN=0 KEEP_CACHE=0
ONLY=link,tunnels,input
REF=origin/develop
MAC_HOST=${TANDEM_HW_MAC:-mac-runner}
PC_IP="" MAC_IP=""
MAC_LINK=${TANDEM_HW_MAC_LINK:-}
MANUAL=0 MAC_RESULTS="" REPO="" MAC_CMD_FILE=""
# Windows of the manual mode: they tolerate minutes of skew between the halves.
M_PAIR_WAIT=${TANDEM_HW_PAIR_WAIT:-900} M_WINDOW=${TANDEM_HW_WINDOW:-420} M_KEEP=${TANDEM_HW_KEEP:-120}
T_MAC_WAIT=${TANDEM_HW_MAC_WAIT:-1800}   # how long to wait for the Mac results (s)
T_CALL=${TANDEM_HW_TIMEOUT:-60}      # short calls to powershell.exe / ssh (s)
T_HW=${TANDEM_HW_STEP_TIMEOUT:-900}  # one tandem-hw run (s)
T_BUILD=${TANDEM_HW_BUILD_TIMEOUT:-3600}

while [ $# -gt 0 ]; do
  case $1 in
    --sim) SIM=1 ;;
    --with-monitors) MONITORS=1 ;;
    --skip-build) SKIP_BUILD=1 ;;
    --keep-cache) KEEP_CACHE=1 ;;
    --plan) PLAN=1 ;;
    --only) ONLY=$2; shift ;;
    --ref) REF=$2; shift ;;
    --mac) MAC_HOST=$2; shift ;;
    --mac-link) MAC_LINK=$2; shift ;;
    --pc-ip) PC_IP=$2; shift ;;
    --mac-ip) MAC_IP=$2; shift ;;
    --mac-manual) MANUAL=1 ;;
    --mac-results) MAC_RESULTS=$2; shift ;;
    --mac-command-file) MAC_CMD_FILE=$2; shift ;;
    --repo) REPO=$2; shift ;;
    -h|--help) sed -n 2,30p "$0"; exit 0 ;;
    *) echo "unknown option:$1" >&2; exit 2 ;;
  esac
  shift
done
if [ "$MONITORS" = 1 ]; then
  case ",$ONLY," in *,monitors,*) ;; *) ONLY="$ONLY,monitors" ;; esac
else
  # Real opt-in: without --with-monitors, `--only monitors` changes no input.
  ONLY=$(printf '%s' ",$ONLY," | sed 's/,monitors,/,/g; s/^,//; s/,$//')
  [ -n "$ONLY" ] || { echo "nothing to run: monitors requires --with-monitors" >&2; exit 2; }
fi
[ "$MANUAL" = 1 ] && T_HW=$((M_PAIR_WAIT + 2 * M_WINDOW + 600))
has() { case ",$ONLY," in *",$1,"*) return 0 ;; esac; return 1; }
[ "$SIM" = 1 ] && ONLY=link,tunnels   # no input or monitors: there are no devices

if [ "$PLAN" = 1 ]; then
  echo "mode: $([ "$SIM" = 1 ] && echo simulated || echo real hardware); groups: $ONLY; ref: $REF"
  echo "1 check there is no real tandemd  2 build  3 provision and start"
  echo "4 pair,ping  5 tunnels PC->Mac and Mac->PC  6 synthetic input in both directions"
  echo "7 monitors with automatic rollback (--with-monitors only)  8 report  9 cleanup"
  if [ "$MANUAL" = 1 ]; then
    echo "manual Mac (no ssh): only the PC half (pair wait ${M_PAIR_WAIT}s, windows ${M_WINDOW}s); prints the Mac command;"
    echo "waits for the Mac results in ${MAC_RESULTS:-<run>/mac-results.txt} and merges them into the report"
  else
    echo "Mac: ssh $MAC_HOST; cable fallback: ${MAC_LINK:-none (use --mac-link or TANDEM_HW_MAC_LINK)}"
  fi
  exit 0
fi

STAMP=$(date +%Y%m%d-%H%M%S)
SHORT=$(date +%H%M%S)
RUN=$HOME/.cache/tandem-hw/$STAMP
SRC=$RUN/src
OUT=$RUN/out
mkdir -p "$SRC" "$OUT"
export CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_BUILD_JOBS=4
LOG_LEVEL='tandemd=debug,tandem_link=info'

say() { printf '[hw %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { say "ERROR: $*"; exit 2; }

# --- Per-machine helpers ----------------------------------------------------------------
psq() { local a out=""; for a in "$@"; do out+="'${a//\'/\'\'}' "; done; printf '%s' "$out"; }
# Always with stdin closed and a time limit: powershell.exe and ssh must not be able to hang the suite.
pc() { timeout -k 5 "${TMO:-$T_CALL}" "$PS" -NoProfile -NonInteractive -Command "$1" </dev/null | tr -d '\r'; }
MAC_OPTS=(-o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=10 -o ServerAliveCountMax=3)
mac() { timeout -k 5 "${TMO:-$T_CALL}" ssh "${MAC_OPTS[@]}" "$MAC_HOST" "$@" </dev/null; }
mac_in() { timeout -k 5 "${TMO:-$T_CALL}" ssh "${MAC_OPTS[@]}" "$MAC_HOST" "$@"; }   # reads stdin (tar)
macq() { local a out=""; for a in "$@"; do out+="$(printf '%q ' "$a")"; done; printf '%s' "$out"; }

# Kills a process and all its descendants (a background `cargo` does not die with its subshell).
killtree() {
  local p=$1 c
  for c in $(pgrep -P "$p" 2>/dev/null); do killtree "$c"; done
  kill "$p" 2>/dev/null
}
# Runs a long command in the background and waits with `wait`: bash runs the trap
# immediately when a signal arrives, which it does not do while a foreground child is alive.
bgwait() {
  "$@" &
  local p=$!
  BG_PIDS+=("$p")
  wait "$p"
}

# If `mac-runner` does not answer (sometimes the Wi-Fi), use the cable's link-local address.
MAC_VIA_CABLE=0
pick_mac_route() {
  mac true >/dev/null 2>&1 && return 0
  [ -n "$MAC_LINK" ] || return 1
  say "no answer from $MAC_HOST; trying the cable at $MAC_LINK"
  MAC_OPTS+=(-o "HostName=$MAC_LINK")
  MAC_VIA_CABLE=1
  mac true >/dev/null 2>&1
}

PC_DIR="" PC_BIN="" MAC_DIR="" SIM_BIN=""
PC_PID="" MAC_PID="" SIM_PIDS=() PC_ORIG_INPUT=""
BG_PIDS=()

# tandem-hw on each "machine" (with --sim, both are local).
pc_hw() {
  if [ "$SIM" = 1 ]; then "$SIM_BIN/tandem-hw" "$@"
  else TMO=$T_HW pc "& $(psq "$PC_DIR\\bin\\tandem-hw.exe" "$@")"; fi
}
mac_hw() {
  if [ "$SIM" = 1 ]; then "$SIM_BIN/tandem-hw" "$@"
  else TMO=$T_HW mac "$(macq "$MAC_DIR/bin/tandem-hw" "$@")"; fi
}

# --- Cleanup: always, even if something fails -------------------------------------------------
cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  say "cleanup"
  local p
  for p in "${BG_PIDS[@]:-}"; do [ -n "$p" ] && killtree "$p"; done
  for p in "${SIM_PIDS[@]:-}"; do [ -n "$p" ] && killtree "$p"; done
  if [ "$SIM" = 0 ]; then
    # 1. First tandem-hw (owner of the guard) and the agents; the watcher is NOT touched: it has
    #    to see its input close and roll back on its own.
    [ -n "$PC_PID" ] && pc "taskkill /F /T /PID $PC_PID" >/dev/null 2>&1
    [ -n "$PC_DIR" ] && pc "Get-Process | Where-Object { \$_.Path -like '$PC_DIR\\*' -and \$_.Path -notlike '*tandem-monitor-guard*' } | Stop-Process -Force -ErrorAction SilentlyContinue" >/dev/null 2>&1
    [ "$MANUAL" = 0 ] && [ -n "$MAC_PID" ] && mac "kill $MAC_PID 2>/dev/null; pkill -f '$MAC_DIR/bin/' 2>/dev/null; true" >/dev/null 2>&1
    if [ -n "$PC_DIR" ] && [ "$MONITORS" = 1 ]; then
      # 2. Wait (up to 60 s) for the watcher to finish on its own.
      local guard_q="Get-Process tandem-monitor-guard -ErrorAction SilentlyContinue | Where-Object { \$_.Path -like '$PC_DIR\\*' } | Select-Object -First 1 -ExpandProperty Id"
      local n=0
      while [ $n -lt 30 ] && [ -n "$(pc "$guard_q" 2>/dev/null)" ]; do sleep 2; n=$((n+1)); done
      # 3. If the input is not the original, explicit restore with the guard.
      if [ -n "$PC_ORIG_INPUT" ]; then
        cur=$(pc_hw monitor-input 2>/dev/null | tr -d '[:space:]')
        if [ "$cur" != "$PC_ORIG_INPUT" ]; then
          say "the monitor input is '$cur', not '$PC_ORIG_INPUT': restoring with the watcher"
          pc_hw monitor-restore "$PC_ORIG_INPUT" >/dev/null 2>&1 || say "WARNING: could not restore the input; check the monitor by hand"
        fi
      fi
      # Only now, if the watcher is still alive, is it killed.
      pc "Get-Process tandem-monitor-guard -ErrorAction SilentlyContinue | Where-Object { \$_.Path -like '$PC_DIR\\*' } | Stop-Process -Force -ErrorAction SilentlyContinue" >/dev/null 2>&1
    fi
    sleep 1
    [ -n "$PC_DIR" ] && pc "Remove-Item -Recurse -Force -LiteralPath '$PC_DIR' -ErrorAction SilentlyContinue" >/dev/null 2>&1
    [ "$MANUAL" = 0 ] && [ -n "$MAC_DIR" ] && mac "rm -rf '$MAC_DIR'" >/dev/null 2>&1
    [ "$MANUAL" = 0 ] && [ "$KEEP_CACHE" = 0 ] && mac 'rm -rf ~/.cache/tandem-hw-src ~/.cache/tandem-hw-target' >/dev/null 2>&1
  fi
  say "done (code $rc); run in $RUN"
  exit "$rc"
}
trap cleanup EXIT INT TERM

# --- 1. Prepare the code ----------------------------------------------------------------
git -C "$ROOT" fetch -q origin 2>/dev/null
git -C "$ROOT" archive "$REF" | tar -x -C "$SRC" || die "could not extract $REF"
HW_TARGET=${TANDEM_HW_TARGET:-$HOME/.cache/tandem-target-hw}

if [ "$SIM" = 1 ]; then
  say "simulated mode: building on Linux"
  bgwait timeout "$T_BUILD" bash -c 'cd "$1" && CARGO_TARGET_DIR=$2 cargo build --release -p tandem-hw -p tandemd --features tandemd/sim-backends' _ "$SRC" "$HW_TARGET" >"$OUT/build-sim.log" 2>&1 \
    || { tail -20 "$OUT/build-sim.log" >&2; die "simulated build failed"; }
  SIM_BIN=$RUN/bin; mkdir -p "$SIM_BIN"
  cp "$HW_TARGET/release/tandem-hw" "$HW_TARGET/release/tandemd" "$SIM_BIN/"
  REPORT_BIN=$SIM_BIN/tandem-hw
else
  # --- 2. First of all: nothing real running ---
  if [ -n "$(pc 'Get-Process tandemd,tandemw -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Id')" ]; then
    die "there is a real tandemd running on the PC; the suite does not touch it. Close it and run again"
  fi
  if [ "$MANUAL" = 0 ]; then
    pick_mac_route || die "no ssh to $MAC_HOST or over the cable (see docs/dev/hardware.md; --mac-link or TANDEM_HW_MAC_LINK)"
    [ "$MAC_VIA_CABLE" = 1 ] && say "the Mac answers over the cable ($MAC_LINK)"
    mac 'pgrep -x tandemd >/dev/null' && die "there is a real tandemd running on the Mac; close it and run again"
  else
    [ -n "$MAC_IP" ] || die "--mac-manual needs --mac-ip (the cable or LAN address of the Mac)"
  fi

  if [ "$SKIP_BUILD" = 0 ]; then
    say "building for Windows (mingw) and for Linux (the report)"
    bgwait timeout "$T_BUILD" bash -c 'cd "$1" && export CARGO_TARGET_DIR=$2 && cargo build --release --target x86_64-pc-windows-gnu -p tandemd -p tandem-hw -p tandem-monitor && cargo build --release -p tandem-hw' _ "$SRC" "$HW_TARGET" >"$OUT/build-pc.log" 2>&1 \
      || { tail -20 "$OUT/build-pc.log" >&2; die "Windows build failed"; }
    if [ "$MANUAL" = 0 ]; then
    # The Mac has no GitHub credentials (T-130) and must stay that way: git dependencies
    # (private repos) are vendored here, where there is access, and travel with the code.
    if grep -q 'source = "git+' "$SRC/Cargo.lock"; then
      say "vendoring the git dependencies for the Mac"
      mkdir -p "$SRC/.cargo"
      bgwait timeout "$T_BUILD" bash -c 'cd "$1" && cargo vendor --locked vendor' _ "$SRC" >"$SRC/.cargo/config.toml" 2>"$OUT/vendor.log" \
        || { tail -20 "$OUT/vendor.log" >&2; die "cargo vendor failed"; }
    fi
    say "building on the Mac"
    tar -C "$SRC" -c . | mac_in 'rm -rf ~/.cache/tandem-hw-src && mkdir -p ~/.cache/tandem-hw-src && tar -x -C ~/.cache/tandem-hw-src' || die "could not copy the code to the Mac"
    TMO=$T_BUILD bgwait mac 'source ~/.cargo/env 2>/dev/null; cd ~/.cache/tandem-hw-src && CARGO_TARGET_DIR=$HOME/.cache/tandem-hw-target cargo build --release -p tandemd -p tandem-hw -p tandem-monitor' >"$OUT/build-mac.log" 2>&1 \
      || { tail -20 "$OUT/build-mac.log" >&2; die "macOS build failed"; }
    fi
  fi
  REPORT_BIN=$HW_TARGET/release/tandem-hw

  # --- 3. Temporary folders and binary install ---
  PC_TEMP=$(pc '[IO.Path]::GetTempPath()')
  PC_DIR="${PC_TEMP%\\}\\tandem-hw-$SHORT"
  MAC_DIR=/tmp/tdhw-$SHORT      # short: the Unix socket has a 104-byte limit
  PC_WSL=$(wslpath -u "$PC_DIR")
  mkdir -p "$PC_WSL/bin" "$PC_WSL/work"
  for f in tandemd tandem-hw tandem-monitor-guard; do
    cp "$HW_TARGET/x86_64-pc-windows-gnu/release/$f.exe" "$PC_WSL/bin/" || die "missing $f.exe"
  done
  [ "$MANUAL" = 0 ] && { mac "mkdir -p '$MAC_DIR/bin' '$MAC_DIR/work' && cd ~/.cache/tandem-hw-target/release && cp tandemd tandem-hw tandem-monitor-guard '$MAC_DIR/bin/'" || die "could not install the binaries on the Mac"; }

  # Over the cable, the link between agents also goes over the link-local addresses.
  if [ "$MAC_VIA_CABLE" = 1 ]; then
    [ -n "$MAC_IP" ] || MAC_IP=$MAC_LINK
    [ -n "$PC_IP" ] || PC_IP=$(pc '(Get-NetIPAddress -AddressFamily IPv4 -PrefixOrigin WellKnown,Manual,Dhcp -ErrorAction SilentlyContinue | Where-Object { $_.IPAddress -like "169.254.*" } | Select-Object -First 1).IPAddress')
  fi
  if [ "$MANUAL" = 1 ] && [ -z "$PC_IP" ]; then
    case $MAC_IP in 169.254.*) PC_IP=$(pc '(Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue | Where-Object { $_.IPAddress -like "169.254.*" } | Select-Object -First 1).IPAddress') ;; esac
  fi
  [ -n "$PC_IP" ] || PC_IP=$(pc '(Get-NetIPConfiguration | Where-Object { $_.IPv4DefaultGateway -ne $null -and $_.NetAdapter.Status -eq "Up" } | Select-Object -First 1).IPv4Address.IPAddress')
  [ -n "$MAC_IP" ] || [ "$MANUAL" = 1 ] || MAC_IP=$(mac 'ipconfig getifaddr en0 || ipconfig getifaddr en1')
  [ -n "$PC_IP" ] && [ -n "$MAC_IP" ] || die "could not determine the IP of both machines (use --pc-ip and --mac-ip)"
  say "PC $PC_IP, Mac $MAC_IP, test port $PORT_A"
fi

# --- 4. Provision and start the test agents -------------------------------------
if [ "$SIM" = 1 ]; then
  PC_DIR=$RUN/pc; MAC_DIR=$RUN/mac; mkdir -p "$PC_DIR/work" "$MAC_DIR/work"
  pc_hw provision --role pc --root "$PC_DIR/agent" --port $PORT_A --peer "127.0.0.1:$PORT_B" >/dev/null || die "provision pc"
  SIDES="pc mac"
  if [ "$MANUAL" = 1 ]; then SIDES=pc   # the Mac half is run by mac-side.sh (--sim)
  else mac_hw provision --role mac --root "$MAC_DIR/agent" --port $PORT_B --peer "127.0.0.1:$PORT_A" >/dev/null || die "provision mac"; fi
  for side in $SIDES; do
    d=$RUN/$side
    TANDEM_LOG=$LOG_LEVEL "$SIM_BIN/tandemd" --config "$d/agent/tandem.toml" --data-dir "$d/agent/data" --sim-backends >"$d/agent.out" 2>&1 &
    SIM_PIDS+=($!)
  done
  PC_DATA=$PC_DIR/agent/data; MAC_DATA=$MAC_DIR/agent/data
  PC_HW_PATH=$SIM_BIN/tandem-hw; MAC_HW_PATH=$SIM_BIN/tandem-hw
  PC_WORK=$PC_DIR/work; MAC_WORK=$MAC_DIR/work
  PC_SEP=/; MAC_SEP=/
else
  pc_hw provision --role pc --root "$PC_DIR\\agent" --port $PORT_A --peer "$MAC_IP:$PORT_A" >/dev/null || die "provision pc"
  [ "$MANUAL" = 1 ] || mac_hw provision --role mac --root "$MAC_DIR/agent" --port $PORT_A --peer "$PC_IP:$PORT_A" >/dev/null || die "provision mac"
  # Decoupled: a direct `Start-Process` on tandemd.exe through the WSL interop bridge made
  # powershell never return (the child inherits the handles). Here cmd redirects its own
  # output to a file and `Start-Process` carries no redirections; it stays in the user's interactive
  # session (SendInput needs it, unlike Win32_Process.Create, which goes to session 0).
  inner="set \"TANDEM_LOG=$LOG_LEVEL\" && \"$PC_DIR\\bin\\tandemd.exe\" --config \"$PC_DIR\\agent\\tandem.toml\" --data-dir \"$PC_DIR\\agent\\data\" >\"$PC_DIR\\agent.out\" 2>&1"
  PC_PID=$(pc "(Start-Process -FilePath cmd.exe -ArgumentList '/d /s /c \"$(printf '%s' "$inner" | sed "s/'/''/g")\"' -WindowStyle Hidden -PassThru).Id")
  if [ "$MANUAL" = 0 ]; then
    MAC_PID=$(mac "cd '$MAC_DIR' && TANDEM_LOG='$LOG_LEVEL' nohup bin/tandemd --config agent/tandem.toml --data-dir agent/data >agent.out 2>&1 </dev/null & echo \$!")
    [ -n "$PC_PID" ] && [ -n "$MAC_PID" ] || die "the test agents did not start"
  else
    [ -n "$PC_PID" ] || die "the PC test agent did not start"
  fi
  PC_DATA=$PC_DIR\\agent\\data; MAC_DATA=$MAC_DIR/agent/data
  PC_HW_PATH=$PC_DIR\\bin\\tandem-hw.exe; MAC_HW_PATH=$MAC_DIR/bin/tandem-hw
  PC_WORK=$PC_DIR\\work; MAC_WORK=$MAC_DIR/work
  PC_SEP='\'; MAC_SEP=/
fi
say "test agents running"

# --- 4b. Manual mode: the command the user pastes on the Mac --------------------------------
# Single-quoted words: they work the same in zsh and bash, with no splitting or expansion.
sq() { local a q="'\\''" out=""; for a in "$@"; do out+="'${a//\'/$q}' "; done; printf '%s' "${out% }"; }
MAC_GROUPS=$(printf '%s' "$ONLY" | sed 's/,monitors//; s/monitors,//; s/^monitors$//')
if [ "$MANUAL" = 1 ]; then
  [ -n "$MAC_RESULTS" ] || MAC_RESULTS=$RUN/mac-results.txt
  COMMIT=$(git -C "$ROOT" rev-parse "$REF" 2>/dev/null) || die "unknown ref $REF"
  if [ "$SIM" = 1 ]; then
    MAC_CMD="bash $(sq "$ROOT/scripts/hw/mac-side.sh" --sim --bin-dir "$SIM_BIN" --port $PORT_B --peer-port $PORT_A \
      --pc-ip 127.0.0.1 --pc-hw "$PC_HW_PATH" --pc-work "$PC_WORK" --pc-sep / --mac-dir "$MAC_DIR" \
      --groups "$MAC_GROUPS" --hold 5)"
  else
    [ -n "$REPO" ] || REPO=$(git -C "$ROOT" remote get-url origin 2>/dev/null)
    git -C "$ROOT" branch -r --contains "$COMMIT" 2>/dev/null | grep -q . \
      || say "WARNING: $COMMIT is not on any remote branch: the Mac cannot fetch it. Push first."
    MAC_CMD="bash -c $(sq 'd=$HOME/.cache/tandem-hw-src; r=$1; c=$2; shift 2; export CARGO_NET_GIT_FETCH_WITH_CLI=true; [ -d "$d/.git" ] || git clone -q "$r" "$d" || exit 1; git -C "$d" fetch -q origin; git -C "$d" fetch -q origin "$c" 2>/dev/null; git -C "$d" checkout -q -f "$c" || exit 1; exec bash "$d/scripts/hw/mac-side.sh" --commit "$c" --repo "$r" "$@"') _ $(sq "$REPO" "$COMMIT" \
      --pc-ip "$PC_IP" --mac-ip "$MAC_IP" --pc-hw "$PC_HW_PATH" --pc-work "$PC_WORK" --pc-sep '\' \
      --mac-dir "$MAC_DIR" --groups "$MAC_GROUPS" --pair-wait "$M_PAIR_WAIT" --window "$M_WINDOW" --keep "$M_KEEP")"
  fi
  [ -n "$MAC_CMD_FILE" ] && printf '%s\n' "$MAC_CMD" >"$MAC_CMD_FILE"
  printf '%s\n' "$MAC_CMD" >"$OUT/mac-command.txt"
  {
    echo
    echo "=== Paste this ONE command on the Mac (Claude session or terminal; works in zsh) ==="
    echo "$MAC_CMD"
    echo "=== Then save what it prints (the block between the TANDEM-HW MAC RESULTS markers) in:"
    echo "    $MAC_RESULTS"
    echo "    This run waits for it up to $((T_MAC_WAIT / 60)) min. Start the PC half now: the windows tolerate minutes of skew. ==="
    echo
  } >&2
fi

# --- 5. Checks --------------------------------------------------------------------
RES_PC=$OUT/results-pc.jsonl; RES_MAC=$OUT/results-mac.jsonl
: >"$RES_PC"; : >"$RES_MAC"

# step <pc|mac> <checks> [extra `run` args]: stores the result lines; if the
# process died without giving any, leaves one failure per check so it shows in the report.
step() {
  local side=$1 checks=$2; shift 2
  local data res tmp="$OUT/step.$side.$$.$RANDOM" c
  if [ "$side" = pc ]; then data=$PC_DATA; res=$RES_PC; else data=$MAC_DATA; res=$RES_MAC; fi
  say "$side: $checks"
  bgwait "${side}_hw" run --label "$side" --data-dir "$data" --checks "$checks" "$@" >"$tmp" 2>>"$OUT/step-$side.err"
  local rc=$?
  cat "$tmp" >>"$res"
  if [ $rc -ne 0 ] && ! grep -q '"outcome"' "$tmp"; then
    for c in ${checks//,/ }; do
      printf '{"check":"%s","machine":"%s","outcome":"fail","detail":"the process ended without a result (code %s); see logs","millis":0,"covers":[]}\n' "$c" "$side" "$rc" >>"$res"
    done
  fi
}

# Manual mode (no ssh): only the PC half, in the same order as the Mac's (mac-side.sh), with long
# windows. The pair/tunnel steps wait for the other half; for input, the receiving window of each
# side contains the other's injection with minutes to spare.
manual_checks() {
  if has link; then
    step pc pair --pair-wait-secs "$M_PAIR_WAIT"
    step pc ping
  fi
  if has tunnels; then
    step pc tunnel-process,tunnel-files,tunnel-port,tunnel-workspace --peer-exe "$MAC_HW_PATH" --remote-dir "$MAC_WORK" --remote-sep "$MAC_SEP"
  fi
  if has input; then
    step pc input-cross --keep-secs "$M_KEEP"
    # The Mac injects after its own window: this one covers it plus the skew.
    step pc input-latency --window-secs $((M_WINDOW + 300))
  fi
}

if [ "$MANUAL" = 1 ]; then
  manual_checks
else
if has link; then
  step pc pair --pair-wait-secs 90
  step mac pair --pair-wait-secs 90
  step pc ping
  step mac ping
fi

if has tunnels; then
  T=tunnel-process,tunnel-files,tunnel-port,tunnel-workspace
  step pc "$T" --peer-exe "$MAC_HW_PATH" --remote-dir "$MAC_WORK" --remote-sep "$MAC_SEP"
  step mac "$T" --peer-exe "$PC_HW_PATH" --remote-dir "$PC_WORK" --remote-sep "$PC_SEP"
fi

# Synthetic input: the receiver measures latency while the other injects.
cross() {
  local from=$1 to=$2
  say "synthetic input $from -> $to"
  step "$to" input-latency --window-secs 30 &
  BG_PIDS+=($!)
  local bg=$!
  sleep 4
  step "$from" input-cross --keep-secs 10
  wait "$bg"
}
if has input; then
  cross pc mac
  cross mac pc
fi

fi

if has monitors; then
  # PC only: the Mac has no DDC channel to the 34" over HDMI (S12). The automatic rollback is
  # armed by each check; nothing is changed here by any other path.
  # The original input is noted before changing anything, for the cleanup restore.
  PC_ORIG_INPUT=$(pc_hw monitor-input 2>/dev/null | tr -d '[:space:]')
  [ -n "$PC_ORIG_INPUT" ] || say "WARNING: the original input was not read; the cleanup will not be able to verify it"
  step pc monitor-roundtrip,monitor-kill
fi

# --- 6. Report ---------------------------------------------------------------------------
# Manual mode: wait for the Mac block in the file (the PC agent stays up meanwhile: the Mac half
# may still need it), then pull the result lines and the agent log tail out of the pasted text.
# Text around the markers (terminal noise, the prompt) is ignored.
collect_mac_results() {
  local waited=0
  say "waiting for the Mac results in $MAC_RESULTS (up to $((T_MAC_WAIT / 60)) min)"
  while ! grep -q 'TANDEM-HW MAC RESULTS END' "$MAC_RESULTS" 2>/dev/null; do
    [ $waited -ge "$T_MAC_WAIT" ] && break
    sleep 2; waited=$((waited + 2))
  done
  if grep -q 'TANDEM-HW MAC RESULTS END' "$MAC_RESULTS" 2>/dev/null; then
    sed -n '/TANDEM-HW MAC RESULTS BEGIN/,/TANDEM-HW MAC RESULTS END/p' "$MAC_RESULTS" | grep '^{.*"outcome"' >"$RES_MAC"
    sed -n '/TANDEM-HW MAC LOG BEGIN/,/TANDEM-HW MAC LOG END/p' "$MAC_RESULTS" | grep -v 'TANDEM-HW MAC LOG' >"$OUT/agent-mac.log"
  else
    say "WARNING: no Mac results after $((T_MAC_WAIT / 60)) min: the Mac checks go in the report as failures"
  fi
  if [ ! -s "$RES_MAC" ]; then
    local c
    for c in $(printf '%s' "$ONLY" | tr ',' ' '); do
      printf '{"check":"mac-half-%s","machine":"mac","outcome":"fail","detail":"no Mac results received","millis":0,"covers":[]}\n' "$c" >>"$RES_MAC"
    done
  fi
}
[ "$MANUAL" = 1 ] && collect_mac_results

fetch_logs() {
  if [ "$MANUAL" = 1 ]; then
    if [ "$SIM" = 1 ]; then cp "$PC_DIR/agent.out" "$OUT/agent-pc.log" 2>/dev/null
    else cat "$PC_WSL/agent.out" "$PC_WSL/agent.err" >"$OUT/agent-pc.log" 2>/dev/null; fi
  elif [ "$SIM" = 1 ]; then
    cp "$PC_DIR/agent.out" "$OUT/agent-pc.log" 2>/dev/null; cp "$MAC_DIR/agent.out" "$OUT/agent-mac.log" 2>/dev/null
  else
    cat "$PC_WSL/agent.out" "$PC_WSL/agent.err" >"$OUT/agent-pc.log" 2>/dev/null
    mac "cat '$MAC_DIR/agent.out'" >"$OUT/agent-mac.log" 2>/dev/null
  fi
}
fetch_logs
DATE=$(date +%F)
REPORT=$ROOT/docs/reports/hw-$DATE.md
[ "$SIM" = 1 ] && REPORT=$RUN/hw-sim-$DATE.md   # the simulated one does not overwrite real reports
[ -e "$REPORT" ] && REPORT=${REPORT%.md}-$SHORT.md   # a second run on the same day does not overwrite the first
LOGS=(--log "agent PC=$OUT/agent-pc.log")
[ -s "$OUT/agent-mac.log" ] && LOGS+=(--log "agent Mac=$OUT/agent-mac.log")
"$REPORT_BIN" report --require-monitors "$MONITORS" --date "$DATE" --out "$REPORT" "${LOGS[@]}" "$RES_PC" "$RES_MAC"
code=$?
say "report: $REPORT"
exit $code
