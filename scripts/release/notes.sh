#!/usr/bin/env bash
# Notas de un release en Markdown, a partir de las tareas integradas: cada PR de tarea entra a
# develop como un commit squash `T-<id>: <título> (#<pr>)`. Las agrupa por la fase del
# frontmatter de docs/tasks/T-<id>.md (tal como está en --hasta), pone aparte los commits que
# no son de una tarea y avisa de las tareas que aún tienen verificación en hardware pendiente.
#
# Por omisión el rango va desde el tag v* anterior a --hasta (o desde el inicio si no hay
# ninguno). Ver docs/dev/release.md.
#
# Uso: scripts/release/notes.sh [--version <x.y.z>] [--desde <ref>] [--hasta <ref>]
set -euo pipefail

version=
desde=
hasta=HEAD
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version=$2; shift 2 ;;
    --desde) desde=$2; shift 2 ;;
    --hasta) hasta=$2; shift 2 ;;
    *) echo "notes.sh: opción desconocida $1" >&2; exit 2 ;;
  esac
done

git rev-parse --verify --quiet "$hasta^{commit}" >/dev/null \
  || { echo "notes.sh: no existe $hasta" >&2; exit 1; }
if [ -z "$desde" ]; then
  # `$hasta^`: si --hasta es el propio tag del release, el anterior es otro.
  desde=$(git describe --tags --abbrev=0 --match 'v*' "$hasta^" 2>/dev/null || true)
fi
rango=$hasta
[ -n "$desde" ] && rango="$desde..$hasta"

nombre_fase() {
  case "$1" in
    F0) echo "F0 Spikes" ;;
    F1) echo "F1 Núcleo" ;;
    F2) echo "F2 Teclado y mouse" ;;
    F3) echo "F3 Monitores y modos" ;;
    F4) echo "F4 Energía" ;;
    F5) echo "F5 UI" ;;
    v2) echo "v2 Datos" ;;
    v3) echo "v3 Archivos" ;;
    "") echo "Sin fase" ;;
    *) echo "$1" ;;
  esac
}

declare -A titulo=() fase=() prs=()
orden=()    # tareas en el orden en que se integraron
otros=()
while IFS= read -r asunto; do
  if [[ "$asunto" =~ ^(T-[0-9]+):[[:space:]]+(.*)$ ]]; then
    id=${BASH_REMATCH[1]}
    resto=${BASH_REMATCH[2]}
    pr=
    if [[ "$resto" =~ ^(.*[^[:space:]])[[:space:]]+\((#[0-9]+)\)$ ]]; then
      resto=${BASH_REMATCH[1]}
      pr=${BASH_REMATCH[2]}
    fi
    # Una tarea con varios commits (un arreglo posterior) sale una vez, con todos sus PRs.
    if [ -z "${titulo[$id]+x}" ]; then
      orden+=("$id")
      titulo[$id]=$resto
      prs[$id]=$pr
      fase[$id]=$(git show "$hasta:docs/tasks/$id.md" 2>/dev/null \
        | sed -n '1,/^---$/{s/^phase:[[:space:]]*//p;}' | head -n 1 || true)
    elif [ -n "$pr" ]; then
      prs[$id]="${prs[$id]:+${prs[$id]}, }$pr"
    fi
  else
    otros+=("$asunto")
  fi
done < <(git log --no-merges --reverse --format=%s "$rango")

if [ -n "$version" ]; then echo "# Tandem $version"; else echo "# Tandem"; fi
echo
if [ -n "$desde" ]; then
  echo "Cambios desde \`$desde\`: ${#orden[@]} tareas integradas."
else
  echo "Primer release: ${#orden[@]} tareas integradas."
fi

# Fases en el orden del diseño; las desconocidas después, en orden alfabético, y al final las
# tareas sin archivo.
mapfile -t fases < <(for id in "${orden[@]}"; do echo "${fase[$id]}"; done \
  | awk '{ n = split("F0 F1 F2 F3 F4 F5 v2 v3", o, " "); p = ($0 == "") ? 100 : 99
         for (i = 1; i <= n; i++) if (o[i] == $0) p = i
         print p "\t" $0 }' \
  | sort -t "$(printf '\t')" -k1,1n -k2,2 -u | cut -f2-)
for f in "${fases[@]}"; do
  echo
  echo "## $(nombre_fase "$f")"
  echo
  for id in "${orden[@]}"; do
    [ "${fase[$id]}" = "$f" ] || continue
    echo "- **$id** ${titulo[$id]}${prs[$id]:+ (${prs[$id]})}"
  done
done

if [ ${#otros[@]} -gt 0 ]; then
  echo
  echo "## Otros cambios"
  echo
  for o in "${otros[@]}"; do echo "- $o"; done
fi

# Lo que solo se puede comprobar en el equipo real: casillas sin marcar de esa sección.
pendientes=()
for id in "${orden[@]}"; do
  archivo="$hasta:docs/tasks/$id.md"
  git cat-file -e "$archivo" 2>/dev/null || continue
  n=$(git show "$archivo" \
    | awk '/^## /{ dentro = ($0 ~ /^## Verificación en hardware pendiente/) } dentro && /^- \[ \]/{ n++ } END{ print n + 0 }')
  [ "$n" -gt 0 ] && pendientes+=("- **$id** ${titulo[$id]}: $n")
done
if [ ${#pendientes[@]} -gt 0 ]; then
  echo
  echo "## Verificación en hardware pendiente"
  echo
  echo "Comprobaciones que solo se pueden hacer en el equipo real (lista en \`docs/tasks/T-<id>.md\`):"
  echo
  printf '%s\n' "${pendientes[@]}"
fi
