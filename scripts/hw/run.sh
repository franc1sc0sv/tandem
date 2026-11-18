#!/usr/bin/env bash
# Suite de hardware (T-123). Se corre a mano desde WSL; NUNCA en CI.
#
#   scripts/hw/run.sh [--sim] [--with-monitors] [--only a,b] [--skip-build] [--keep-cache] [--ref REF]
#                     [--mac HOST] [--mac-link DIR] [--pc-ip IP] [--mac-ip IP] [--plan]
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
#   --only LISTA     solo estos grupos: link,tunnels,input,monitors (coma). `monitors` se
#                    ignora sin --with-monitors.
#   --keep-cache     conserva en la Mac ~/.cache/tandem-hw-src y ~/.cache/tandem-hw-target
#                    (caché de compilación); por defecto se borran al terminar.
#   --mac-link DIR   dirección link-local del cable USB4 de la Mac (o TANDEM_HW_MAC_LINK). Si
#                    `mac-runner` no responde (Wi-Fi caído), se usa esta dirección con el mismo
#                    usuario y llave del alias ssh. Sin IPs fijas en el repo (D10).
#   --plan           imprime los pasos y sale.
#
# Todas las llamadas a powershell.exe y ssh llevan `< /dev/null` y `timeout`; los pasos largos
# corren en segundo plano con `wait`, así Ctrl-C y SIGTERM disparan siempre la limpieza.
#
# Salida: 0 sin fallos, 1 con fallos, 2 si no se pudo armar el entorno.

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
T_CALL=${TANDEM_HW_TIMEOUT:-60}      # llamadas cortas a powershell.exe / ssh (s)
T_HW=${TANDEM_HW_STEP_TIMEOUT:-900}  # una corrida de tandem-hw (s)
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
    -h|--help) sed -n 2,30p "$0"; exit 0 ;;
    *) echo "opción desconocida: $1" >&2; exit 2 ;;
  esac
  shift
done
if [ "$MONITORS" = 1 ]; then
  case ",$ONLY," in *,monitors,*) ;; *) ONLY="$ONLY,monitors" ;; esac
else
  # Opt-in real: sin --with-monitors, `--only monitors` no cambia ninguna entrada.
  ONLY=$(printf '%s' ",$ONLY," | sed 's/,monitors,/,/g; s/^,//; s/,$//')
  [ -n "$ONLY" ] || { echo "nada que correr: monitors exige --with-monitors" >&2; exit 2; }
fi
has() { case ",$ONLY," in *",$1,"*) return 0 ;; esac; return 1; }
[ "$SIM" = 1 ] && ONLY=link,tunnels   # sin entrada ni monitores: no hay dispositivos

if [ "$PLAN" = 1 ]; then
  echo "modo: $([ "$SIM" = 1 ] && echo simulado || echo hardware real); grupos: $ONLY; ref: $REF"
  echo "1 comprobar que no hay un tandemd real  2 compilar  3 aprovisionar y arrancar"
  echo "4 pair,ping  5 túneles PC->Mac y Mac->PC  6 entrada sintética en las dos direcciones"
  echo "7 monitores con vuelta automática (solo --with-monitors)  8 informe  9 limpieza"
  echo "Mac: ssh $MAC_HOST; respaldo por cable: ${MAC_LINK:-ninguno (usa --mac-link o TANDEM_HW_MAC_LINK)}"
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
# Siempre con stdin cerrado y tope de tiempo: powershell.exe y ssh no deben poder colgar la suite.
pc() { timeout -k 5 "${TMO:-$T_CALL}" "$PS" -NoProfile -NonInteractive -Command "$1" </dev/null | tr -d '\r'; }
MAC_OPTS=(-o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=10 -o ServerAliveCountMax=3)
mac() { timeout -k 5 "${TMO:-$T_CALL}" ssh "${MAC_OPTS[@]}" "$MAC_HOST" "$@" </dev/null; }
mac_in() { timeout -k 5 "${TMO:-$T_CALL}" ssh "${MAC_OPTS[@]}" "$MAC_HOST" "$@"; }   # lee stdin (tar)
macq() { local a out=""; for a in "$@"; do out+="$(printf '%q ' "$a")"; done; printf '%s' "$out"; }

# Mata un proceso y todos sus descendientes (un `cargo` en segundo plano no muere con su subshell).
killtree() {
  local p=$1 c
  for c in $(pgrep -P "$p" 2>/dev/null); do killtree "$c"; done
  kill "$p" 2>/dev/null
}
# Corre un comando largo en segundo plano y espera con `wait`: bash ejecuta el trap de
# inmediato al llegar una señal, cosa que no hace mientras un hijo de primer plano sigue vivo.
bgwait() {
  "$@" &
  local p=$!
  BG_PIDS+=("$p")
  wait "$p"
}

# Si `mac-runner` no contesta (a veces el Wi-Fi), usa la dirección link-local del cable.
MAC_VIA_CABLE=0
pick_mac_route() {
  mac true >/dev/null 2>&1 && return 0
  [ -n "$MAC_LINK" ] || return 1
  say "sin respuesta de $MAC_HOST; probando el cable en $MAC_LINK"
  MAC_OPTS+=(-o "HostName=$MAC_LINK")
  MAC_VIA_CABLE=1
  mac true >/dev/null 2>&1
}

PC_DIR="" PC_BIN="" MAC_DIR="" SIM_BIN=""
PC_PID="" MAC_PID="" SIM_PIDS=() PC_ORIG_INPUT=""
BG_PIDS=()

# tandem-hw en cada "máquina" (en --sim, las dos son locales).
pc_hw() {
  if [ "$SIM" = 1 ]; then "$SIM_BIN/tandem-hw" "$@"
  else TMO=$T_HW pc "& $(psq "$PC_DIR\\bin\\tandem-hw.exe" "$@")"; fi
}
mac_hw() {
  if [ "$SIM" = 1 ]; then "$SIM_BIN/tandem-hw" "$@"
  else TMO=$T_HW mac "$(macq "$MAC_DIR/bin/tandem-hw" "$@")"; fi
}

# --- Limpieza: siempre, aunque algo falle -------------------------------------------------
cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  say "limpieza"
  local p
  for p in "${BG_PIDS[@]:-}"; do [ -n "$p" ] && killtree "$p"; done
  for p in "${SIM_PIDS[@]:-}"; do [ -n "$p" ] && killtree "$p"; done
  if [ "$SIM" = 0 ]; then
    # 1. Primero tandem-hw (dueño del guardia) y los agentes; el vigilante NO se toca: tiene
    #    que ver cerrarse su entrada y volver solo.
    [ -n "$PC_PID" ] && pc "taskkill /F /T /PID $PC_PID" >/dev/null 2>&1
    [ -n "$PC_DIR" ] && pc "Get-Process | Where-Object { \$_.Path -like '$PC_DIR\\*' -and \$_.Path -notlike '*tandem-monitor-guard*' } | Stop-Process -Force -ErrorAction SilentlyContinue" >/dev/null 2>&1
    [ -n "$MAC_PID" ] && mac "kill $MAC_PID 2>/dev/null; pkill -f '$MAC_DIR/bin/' 2>/dev/null; true" >/dev/null 2>&1
    if [ -n "$PC_DIR" ] && [ "$MONITORS" = 1 ]; then
      # 2. Esperar (tope 60 s) a que el vigilante termine solo.
      local guard_q="Get-Process tandem-monitor-guard -ErrorAction SilentlyContinue | Where-Object { \$_.Path -like '$PC_DIR\\*' } | Select-Object -First 1 -ExpandProperty Id"
      local n=0
      while [ $n -lt 30 ] && [ -n "$(pc "$guard_q" 2>/dev/null)" ]; do sleep 2; n=$((n+1)); done
      # 3. Si la entrada no es la original, restauración explícita con el guardia.
      if [ -n "$PC_ORIG_INPUT" ]; then
        cur=$(pc_hw monitor-input 2>/dev/null | tr -d '[:space:]')
        if [ "$cur" != "$PC_ORIG_INPUT" ]; then
          say "la entrada del monitor es '$cur', no '$PC_ORIG_INPUT': restaurando con el vigilante"
          pc_hw monitor-restore "$PC_ORIG_INPUT" >/dev/null 2>&1 || say "AVISO: no se pudo restaurar la entrada; revisar el monitor a mano"
        fi
      fi
      # Solo ahora, si el vigilante sigue vivo, se le mata.
      pc "Get-Process tandem-monitor-guard -ErrorAction SilentlyContinue | Where-Object { \$_.Path -like '$PC_DIR\\*' } | Stop-Process -Force -ErrorAction SilentlyContinue" >/dev/null 2>&1
    fi
    sleep 1
    [ -n "$PC_DIR" ] && pc "Remove-Item -Recurse -Force -LiteralPath '$PC_DIR' -ErrorAction SilentlyContinue" >/dev/null 2>&1
    [ -n "$MAC_DIR" ] && mac "rm -rf '$MAC_DIR'" >/dev/null 2>&1
    [ "$KEEP_CACHE" = 0 ] && mac 'rm -rf ~/.cache/tandem-hw-src ~/.cache/tandem-hw-target' >/dev/null 2>&1
  fi
  say "listo (código $rc); corrida en $RUN"
  exit "$rc"
}
trap cleanup EXIT INT TERM

# --- 1. Preparar el código ----------------------------------------------------------------
git -C "$ROOT" fetch -q origin 2>/dev/null
git -C "$ROOT" archive "$REF" | tar -x -C "$SRC" || die "no se pudo extraer $REF"
HW_TARGET=${TANDEM_HW_TARGET:-$HOME/.cache/tandem-target-hw}

if [ "$SIM" = 1 ]; then
  say "modo simulado: compilando en Linux"
  bgwait timeout "$T_BUILD" bash -c 'cd "$1" && CARGO_TARGET_DIR=$2 cargo build --release -p tandem-hw -p tandemd --features tandemd/sim-backends' _ "$SRC" "$HW_TARGET" >"$OUT/build-sim.log" 2>&1 \
    || { tail -20 "$OUT/build-sim.log" >&2; die "falló la compilación simulada"; }
  SIM_BIN=$RUN/bin; mkdir -p "$SIM_BIN"
  cp "$HW_TARGET/release/tandem-hw" "$HW_TARGET/release/tandemd" "$SIM_BIN/"
  REPORT_BIN=$SIM_BIN/tandem-hw
else
  # --- 2. Antes de nada: nada real corriendo ---
  if [ -n "$(pc 'Get-Process tandemd,tandemw -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Id')" ]; then
    die "hay un tandemd real corriendo en la PC; la suite no lo toca. Ciérralo y vuelve a correr"
  fi
  pick_mac_route || die "no hay ssh a $MAC_HOST ni por el cable (ver docs/dev/hardware.md; --mac-link o TANDEM_HW_MAC_LINK)"
  [ "$MAC_VIA_CABLE" = 1 ] && say "la Mac responde por el cable ($MAC_LINK)"
  mac 'pgrep -x tandemd >/dev/null' && die "hay un tandemd real corriendo en la Mac; ciérralo y vuelve a correr"

  if [ "$SKIP_BUILD" = 0 ]; then
    say "compilando para Windows (mingw) y para Linux (el informe)"
    bgwait timeout "$T_BUILD" bash -c 'cd "$1" && export CARGO_TARGET_DIR=$2 && cargo build --release --target x86_64-pc-windows-gnu -p tandemd -p tandem-hw -p tandem-monitor && cargo build --release -p tandem-hw' _ "$SRC" "$HW_TARGET" >"$OUT/build-pc.log" 2>&1 \
      || { tail -20 "$OUT/build-pc.log" >&2; die "falló la compilación de Windows"; }
    # La Mac no tiene credenciales de GitHub (T-130) y así debe seguir: las dependencias git
    # (repos privados) se vendorizan aquí, donde sí hay acceso, y viajan con el código.
    if grep -q 'source = "git+' "$SRC/Cargo.lock"; then
      say "vendorizando las dependencias git para la Mac"
      mkdir -p "$SRC/.cargo"
      bgwait timeout "$T_BUILD" bash -c 'cd "$1" && cargo vendor --locked vendor' _ "$SRC" >"$SRC/.cargo/config.toml" 2>"$OUT/vendor.log" \
        || { tail -20 "$OUT/vendor.log" >&2; die "falló cargo vendor"; }
    fi
    say "compilando en la Mac"
    tar -C "$SRC" -c . | mac_in 'rm -rf ~/.cache/tandem-hw-src && mkdir -p ~/.cache/tandem-hw-src && tar -x -C ~/.cache/tandem-hw-src' || die "no se pudo copiar el código a la Mac"
    TMO=$T_BUILD bgwait mac 'source ~/.cargo/env 2>/dev/null; cd ~/.cache/tandem-hw-src && CARGO_TARGET_DIR=$HOME/.cache/tandem-hw-target cargo build --release -p tandemd -p tandem-hw -p tandem-monitor' >"$OUT/build-mac.log" 2>&1 \
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

  # Por el cable, el enlace entre agentes también va por las direcciones link-local.
  if [ "$MAC_VIA_CABLE" = 1 ]; then
    [ -n "$MAC_IP" ] || MAC_IP=$MAC_LINK
    [ -n "$PC_IP" ] || PC_IP=$(pc '(Get-NetIPAddress -AddressFamily IPv4 -PrefixOrigin WellKnown,Manual,Dhcp -ErrorAction SilentlyContinue | Where-Object { $_.IPAddress -like "169.254.*" } | Select-Object -First 1).IPAddress')
  fi
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
  # Desacoplado: `Start-Process` directo sobre tandemd.exe con el puente de interop de WSL hacía
  # que powershell no regresara nunca (el hijo hereda los handles). Aquí cmd redirige su propia
  # salida a archivo y `Start-Process` no lleva redirecciones; sigue en la sesión interactiva del
  # usuario (SendInput lo necesita, a diferencia de Win32_Process.Create, que va a la sesión 0).
  inner="set \"TANDEM_LOG=$LOG_LEVEL\" && \"$PC_DIR\\bin\\tandemd.exe\" --config \"$PC_DIR\\agent\\tandem.toml\" --data-dir \"$PC_DIR\\agent\\data\" >\"$PC_DIR\\agent.out\" 2>&1"
  PC_PID=$(pc "(Start-Process -FilePath cmd.exe -ArgumentList '/d /s /c \"$(printf '%s' "$inner" | sed "s/'/''/g")\"' -WindowStyle Hidden -PassThru).Id")
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
  bgwait "${side}_hw" run --label "$side" --data-dir "$data" --checks "$checks" "$@" >"$tmp" 2>>"$OUT/step-$side.err"
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
  # La entrada original se anota antes de cambiar nada, para la restauración de la limpieza.
  PC_ORIG_INPUT=$(pc_hw monitor-input 2>/dev/null | tr -d '[:space:]')
  [ -n "$PC_ORIG_INPUT" ] || say "AVISO: no se leyó la entrada original; la limpieza no podrá verificarla"
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
"$REPORT_BIN" report --require-monitors "$MONITORS" --date "$DATE" --out "$REPORT" --log "agente PC=$OUT/agent-pc.log" --log "agente Mac=$OUT/agent-mac.log" "$RES_PC" "$RES_MAC"
code=$?
say "informe: $REPORT"
exit $code
