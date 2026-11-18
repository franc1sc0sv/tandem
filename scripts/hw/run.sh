#!/usr/bin/env bash
# Suite de hardware (T-123). Se corre a mano desde WSL; NUNCA en CI.
#
#   scripts/hw/run.sh [--sim] [--with-monitors] [--only a,b] [--skip-build] [--ref REF]
#                     [--mac HOST] [--pc-ip IP] [--mac-ip IP] [--plan]
#
# Compila los binarios de Windows (mingw, en WSL) y de macOS (por ssh, en la Mac), arranca
# un agente de prueba en cada máquina con identidades preprovisionadas y datos en carpetas
# temporales, corre las comprobaciones de `tandem-hw` y deja el informe en
# docs/reports/hw-<fecha>.md. Al terminar, aunque algo falle, mata lo que arrancó y borra
# sus carpetas (trap). No toca la instalación ni la configuración real del usuario: si hay
# un `tandemd` real corriendo en alguna máquina, aborta antes de empezar.
#
#   --sim            todo en esta máquina con los backends simulados (sin hardware): valida el
#                    orquestador y las comprobaciones de enlace y túneles.
#   --with-monitors  agrega las dos comprobaciones que cambian la entrada del monitor de la
#                    PC. Llevan vuelta automática (guardia + vigilante `tandem-monitor-guard`)
#                    y se omiten si el vigilante no está.
#   --only LISTA     solo estos grupos: link,tunnels,input,monitors (coma).
#   --plan           imprime los pasos y sale.
#
# Salida: 0 sin fallos, 1 con fallos, 2 si no se pudo armar el entorno.

set -uo pipefail

ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
PS=/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe
PORT_A=47871
PORT_B=47872
SIM=0 MONITORS=0 SKIP_BUILD=0 PLAN=0
ONLY=link,tunnels,input
REF=origin/develop
MAC_HOST=${TANDEM_HW_MAC:-mac-runner}
PC_IP="" MAC_IP=""

while [ $# -gt 0 ]; do
  case $1 in
    --sim) SIM=1 ;;
    --with-monitors) MONITORS=1 ;;
    --skip-build) SKIP_BUILD=1 ;;
    --plan) PLAN=1 ;;
    --only) ONLY=$2; shift ;;
    --ref) REF=$2; shift ;;
    --mac) MAC_HOST=$2; shift ;;
    --pc-ip) PC_IP=$2; shift ;;
    --mac-ip) MAC_IP=$2; shift ;;
    -h|--help) sed -n 2,25p "$0"; exit 0 ;;
    *) echo "opción desconocida: $1" >&2; exit 2 ;;
  esac
  shift
done
[ "$MONITORS" = 1 ] && ONLY="$ONLY,monitors"
has() { case ",$ONLY," in *",$1,"*) return 0 ;; esac; return 1; }
[ "$SIM" = 1 ] && ONLY=link,tunnels   # sin entrada ni monitores: no hay dispositivos

if [ "$PLAN" = 1 ]; then
  echo "modo: $([ "$SIM" = 1 ] && echo simulado || echo hardware real); grupos: $ONLY; ref: $REF"
  echo "1 comprobar que no hay un tandemd real  2 compilar  3 aprovisionar y arrancar"
  echo "4 pair,ping  5 túneles PC->Mac y Mac->PC  6 entrada sintética en las dos direcciones"
  echo "7 monitores con vuelta automática (solo --with-monitors)  8 informe  9 limpieza"
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

# --- Ayudantes por máquina ----------------------------------------------------------------
psq() { local a out=""; for a in "$@"; do out+="'${a//\'/\'\'}' "; done; printf '%s' "$out"; }
pc() { "$PS" -NoProfile -NonInteractive -Command "$1" | tr -d '\r'; }
mac() { ssh -o BatchMode=yes -o ConnectTimeout=10 "$MAC_HOST" "$@"; }
macq() { local a out=""; for a in "$@"; do out+="$(printf '%q ' "$a")"; done; printf '%s' "$out"; }

PC_DIR="" PC_BIN="" MAC_DIR="" SIM_BIN=""
PC_PID="" MAC_PID="" SIM_PIDS=()
BG_PIDS=()

# tandem-hw en cada "máquina" (en --sim, las dos son locales).
pc_hw() {
  if [ "$SIM" = 1 ]; then "$SIM_BIN/tandem-hw" "$@"
  else pc "& $(psq "$PC_DIR\\bin\\tandem-hw.exe" "$@")"; fi
}
mac_hw() {
  if [ "$SIM" = 1 ]; then "$SIM_BIN/tandem-hw" "$@"
  else mac "$(macq "$MAC_DIR/bin/tandem-hw" "$@")"; fi
}

# --- Limpieza: siempre, aunque algo falle -------------------------------------------------
cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  say "limpieza"
  local p
  for p in "${BG_PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
  for p in "${SIM_PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
  if [ "$SIM" = 0 ]; then
    [ -n "$PC_PID" ] && pc "Stop-Process -Id $PC_PID -Force -ErrorAction SilentlyContinue" >/dev/null 2>&1
    # Hijos sueltos de las pruebas (solo los de la carpeta de esta corrida).
    [ -n "$PC_DIR" ] && pc "Get-Process | Where-Object { \$_.Path -like '$PC_DIR\\*' } | Stop-Process -Force -ErrorAction SilentlyContinue" >/dev/null 2>&1
    [ -n "$MAC_PID" ] && mac "kill $MAC_PID 2>/dev/null; pkill -f '$MAC_DIR/bin/' 2>/dev/null; true" >/dev/null 2>&1
    sleep 1
    [ -n "$PC_DIR" ] && pc "Remove-Item -Recurse -Force -LiteralPath '$PC_DIR' -ErrorAction SilentlyContinue" >/dev/null 2>&1
    [ -n "$MAC_DIR" ] && mac "rm -rf '$MAC_DIR'" >/dev/null 2>&1
  fi
  say "listo (código $rc); corrida en $RUN"
  exit "$rc"
}
trap cleanup EXIT INT TERM

# --- 1. Preparar el código ----------------------------------------------------------------
git -C "$ROOT" fetch -q origin 2>/dev/null
git -C "$ROOT" archive "$REF" | tar -x -C "$SRC" || die "no se pudo extraer $REF"
HW_TARGET=$HOME/.cache/tandem-target-hw

if [ "$SIM" = 1 ]; then
  say "modo simulado: compilando en Linux"
  ( cd "$SRC" && CARGO_TARGET_DIR=$HW_TARGET cargo build --release -p tandem-hw -p tandemd --features tandemd/sim-backends ) >"$OUT/build-sim.log" 2>&1 \
    || { tail -20 "$OUT/build-sim.log" >&2; die "falló la compilación simulada"; }
  SIM_BIN=$RUN/bin; mkdir -p "$SIM_BIN"
  cp "$HW_TARGET/release/tandem-hw" "$HW_TARGET/release/tandemd" "$SIM_BIN/"
  REPORT_BIN=$SIM_BIN/tandem-hw
else
  # --- 2. Antes de nada: nada real corriendo ---
  if [ -n "$(pc 'Get-Process tandemd,tandemw -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Id')" ]; then
    die "hay un tandemd real corriendo en la PC; la suite no lo toca. Ciérralo y vuelve a correr"
  fi
  mac 'pgrep -x tandemd >/dev/null' && die "hay un tandemd real corriendo en la Mac; ciérralo y vuelve a correr"
  mac 'true' || die "no hay ssh a $MAC_HOST (ver docs/dev/hardware.md)"

  if [ "$SKIP_BUILD" = 0 ]; then
    say "compilando para Windows (mingw) y para Linux (el informe)"
    ( cd "$SRC" && CARGO_TARGET_DIR=$HW_TARGET cargo build --release --target x86_64-pc-windows-gnu -p tandemd -p tandem-hw -p tandem-monitor \
        && CARGO_TARGET_DIR=$HW_TARGET cargo build --release -p tandem-hw ) >"$OUT/build-pc.log" 2>&1 \
      || { tail -20 "$OUT/build-pc.log" >&2; die "falló la compilación de Windows"; }
    say "compilando en la Mac"
    tar -C "$SRC" -c . | mac 'rm -rf ~/.cache/tandem-hw-src && mkdir -p ~/.cache/tandem-hw-src && tar -x -C ~/.cache/tandem-hw-src' || die "no se pudo copiar el código a la Mac"
    mac 'source ~/.cargo/env 2>/dev/null; cd ~/.cache/tandem-hw-src && CARGO_TARGET_DIR=$HOME/.cache/tandem-hw-target cargo build --release -p tandemd -p tandem-hw -p tandem-monitor' >"$OUT/build-mac.log" 2>&1 \
      || { tail -20 "$OUT/build-mac.log" >&2; die "falló la compilación de macOS"; }
  fi
  REPORT_BIN=$HW_TARGET/release/tandem-hw

  # --- 3. Carpetas temporales e instalación de los binarios ---
  PC_TEMP=$(pc '[IO.Path]::GetTempPath()')
  PC_DIR="${PC_TEMP%\\}\\tandem-hw-$SHORT"
  MAC_DIR=/tmp/tdhw-$SHORT      # corto: el socket Unix tiene un límite de 104 bytes
  PC_WSL=$(wslpath -u "$PC_DIR")
  mkdir -p "$PC_WSL/bin" "$PC_WSL/work"
  for f in tandemd tandem-hw tandem-monitor-guard; do
    cp "$HW_TARGET/x86_64-pc-windows-gnu/release/$f.exe" "$PC_WSL/bin/" || die "falta $f.exe"
  done
  mac "mkdir -p '$MAC_DIR/bin' '$MAC_DIR/work' && cd ~/.cache/tandem-hw-target/release && cp tandemd tandem-hw tandem-monitor-guard '$MAC_DIR/bin/'" || die "no se pudieron instalar los binarios en la Mac"

  [ -n "$PC_IP" ] || PC_IP=$(pc '(Get-NetIPConfiguration | Where-Object { $_.IPv4DefaultGateway -ne $null -and $_.NetAdapter.Status -eq "Up" } | Select-Object -First 1).IPv4Address.IPAddress')
  [ -n "$MAC_IP" ] || MAC_IP=$(mac 'ipconfig getifaddr en0 || ipconfig getifaddr en1')
  [ -n "$PC_IP" ] && [ -n "$MAC_IP" ] || die "no se pudo saber la IP de las dos máquinas (usa --pc-ip y --mac-ip)"
  say "PC $PC_IP, Mac $MAC_IP, puerto de prueba $PORT_A"
fi

# --- 4. Aprovisionar y arrancar los agentes de prueba -------------------------------------
if [ "$SIM" = 1 ]; then
  PC_DIR=$RUN/pc; MAC_DIR=$RUN/mac; mkdir -p "$PC_DIR/work" "$MAC_DIR/work"
  pc_hw provision --role pc --root "$PC_DIR/agent" --port $PORT_A --peer "127.0.0.1:$PORT_B" >/dev/null || die "provision pc"
  mac_hw provision --role mac --root "$MAC_DIR/agent" --port $PORT_B --peer "127.0.0.1:$PORT_A" >/dev/null || die "provision mac"
  for side in pc mac; do
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
  mac_hw provision --role mac --root "$MAC_DIR/agent" --port $PORT_A --peer "$PC_IP:$PORT_A" >/dev/null || die "provision mac"
  PC_PID=$(pc "\$env:TANDEM_LOG='$LOG_LEVEL'; (Start-Process -FilePath '$PC_DIR\\bin\\tandemd.exe' -ArgumentList '--config','$PC_DIR\\agent\\tandem.toml','--data-dir','$PC_DIR\\agent\\data' -WindowStyle Hidden -PassThru -RedirectStandardOutput '$PC_DIR\\agent.out' -RedirectStandardError '$PC_DIR\\agent.err').Id")
  MAC_PID=$(mac "cd '$MAC_DIR' && TANDEM_LOG='$LOG_LEVEL' nohup bin/tandemd --config agent/tandem.toml --data-dir agent/data >agent.out 2>&1 </dev/null & echo \$!")
  [ -n "$PC_PID" ] && [ -n "$MAC_PID" ] || die "no arrancaron los agentes de prueba"
  PC_DATA=$PC_DIR\\agent\\data; MAC_DATA=$MAC_DIR/agent/data
  PC_HW_PATH=$PC_DIR\\bin\\tandem-hw.exe; MAC_HW_PATH=$MAC_DIR/bin/tandem-hw
  PC_WORK=$PC_DIR\\work; MAC_WORK=$MAC_DIR/work
  PC_SEP='\'; MAC_SEP=/
fi
say "agentes de prueba en marcha"

# --- 5. Comprobaciones --------------------------------------------------------------------
RES_PC=$OUT/results-pc.jsonl; RES_MAC=$OUT/results-mac.jsonl
: >"$RES_PC"; : >"$RES_MAC"

# step <pc|mac> <comprobaciones> [args extra de `run`]: guarda las líneas de resultado; si el
# proceso murió sin dar ninguna, deja un fallo por comprobación para que se vea en el informe.
step() {
  local side=$1 checks=$2; shift 2
  local data res tmp="$OUT/step.$side.$$.$RANDOM" c
  if [ "$side" = pc ]; then data=$PC_DATA; res=$RES_PC; else data=$MAC_DATA; res=$RES_MAC; fi
  say "$side: $checks"
  "${side}_hw" run --label "$side" --data-dir "$data" --checks "$checks" "$@" >"$tmp" 2>>"$OUT/step-$side.err"
  local rc=$?
  cat "$tmp" >>"$res"
  if [ $rc -ne 0 ] && ! grep -q '"outcome"' "$tmp"; then
    for c in ${checks//,/ }; do
      printf '{"check":"%s","machine":"%s","outcome":"fail","detail":"el proceso terminó sin resultado (código %s); ver registros","millis":0,"covers":[]}\n' "$c" "$side" "$rc" >>"$res"
    done
  fi
}

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

# Entrada sintética: el que recibe mide la latencia mientras el otro inyecta.
cross() {
  local from=$1 to=$2
  say "entrada sintética $from -> $to"
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

if has monitors; then
  # Solo la PC: la Mac no tiene canal DDC con el 34" por HDMI (S12). La vuelta automática la
  # arma cada comprobación; aquí no se cambia nada por otro camino.
  step pc monitor-roundtrip,monitor-kill
fi

# --- 6. Informe ---------------------------------------------------------------------------
fetch_logs() {
  if [ "$SIM" = 1 ]; then
    cp "$PC_DIR/agent.out" "$OUT/agent-pc.log" 2>/dev/null; cp "$MAC_DIR/agent.out" "$OUT/agent-mac.log" 2>/dev/null
  else
    cat "$PC_WSL/agent.out" "$PC_WSL/agent.err" >"$OUT/agent-pc.log" 2>/dev/null
    mac "cat '$MAC_DIR/agent.out'" >"$OUT/agent-mac.log" 2>/dev/null
  fi
}
fetch_logs
DATE=$(date +%F)
REPORT=$ROOT/docs/reports/hw-$DATE.md
[ "$SIM" = 1 ] && REPORT=$RUN/hw-sim-$DATE.md   # el simulado no pisa los informes reales
[ -e "$REPORT" ] && REPORT=${REPORT%.md}-$SHORT.md   # una segunda corrida del día no pisa la primera
"$REPORT_BIN" report --date "$DATE" --out "$REPORT" --log "agente PC=$OUT/agent-pc.log" --log "agente Mac=$OUT/agent-mac.log" "$RES_PC" "$RES_MAC"
code=$?
say "informe: $REPORT"
exit $code
