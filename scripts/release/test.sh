#!/usr/bin/env bash
# Pruebas de version.sh y notes.sh sobre repos de git desechables. Las corre el job `preparar`
# del workflow de release y el CI de Ubuntu. Uso: scripts/release/test.sh
# shellcheck disable=SC2015,SC2016  # `ok`/`mal` nunca fallan; los backticks son Markdown literal.
set -euo pipefail

aqui=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# Repos de prueba herméticos: sin la config global ni la del sistema de quien corre esto.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=prueba GIT_AUTHOR_EMAIL=prueba@example.com
export GIT_COMMITTER_NAME=prueba GIT_COMMITTER_EMAIL=prueba@example.com

fallos=0
ok() { echo "ok   $1"; }
mal() { echo "FALLO $1" >&2; fallos=$((fallos + 1)); }
contiene() { # nombre, texto, patrón (literal)
  if grep -qF -- "$3" <<<"$2"; then ok "$1"; else mal "$1: falta «$3»"; echo "$2" >&2; fi
}
no_contiene() {
  if grep -qF -- "$3" <<<"$2"; then mal "$1: sobra «$3»"; echo "$2" >&2; else ok "$1"; fi
}

# --- version.sh -------------------------------------------------------------------------
versiones() { # dir, cargo, tauri, npm
  mkdir -p "$1/apps/tandem-ui/src-tauri"
  printf '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "%s"\nedition = "2024"\n\n[workspace.dependencies]\nfoo = { version = "9.9.9" }\n' "$2" >"$1/Cargo.toml"
  printf '{ "productName": "Tandem", "version": "%s" }\n' "$3" >"$1/apps/tandem-ui/src-tauri/tauri.conf.json"
  printf '{ "name": "tandem-ui", "version": "%s" }\n' "$4" >"$1/apps/tandem-ui/package.json"
}

versiones "$tmp/v1" 1.2.3 1.2.3 1.2.3
[ "$("$aqui/version.sh" --raiz "$tmp/v1")" = 1.2.3 ] && ok "version: lee la del workspace" || mal "version: lee la del workspace"
"$aqui/version.sh" --raiz "$tmp/v1" v1.2.3 >/dev/null && ok "version: acepta el tag igual" || mal "version: acepta el tag igual"
if "$aqui/version.sh" --raiz "$tmp/v1" v1.2.4 >/dev/null 2>&1; then mal "version: rechaza otro tag"; else ok "version: rechaza otro tag"; fi
if "$aqui/version.sh" --raiz "$tmp/v1" 1.2.3 >/dev/null 2>&1; then mal "version: exige la v del tag"; else ok "version: exige la v del tag"; fi
versiones "$tmp/v2" 1.2.3 1.2.4 1.2.3
if "$aqui/version.sh" --raiz "$tmp/v2" >/dev/null 2>&1; then mal "version: rechaza tauri distinto"; else ok "version: rechaza tauri distinto"; fi
versiones "$tmp/v3" 1.2.3 1.2.3 0.0.1
if "$aqui/version.sh" --raiz "$tmp/v3" >/dev/null 2>&1; then mal "version: rechaza package.json distinto"; else ok "version: rechaza package.json distinto"; fi

# --- notes.sh ---------------------------------------------------------------------------
repo="$tmp/repo"
git init -q -b develop "$repo"
cd "$repo"
tarea() { # id, fase, casillas pendientes
  mkdir -p docs/tasks
  {
    printf -- '---\nid: %s\ntitle: "x"\nphase: %s\nstatus: done\n---\n\n# %s\n\n## Criterios\n\n- [ ] no cuenta\n' "$1" "$2" "$1"
    if [ "$3" -gt 0 ]; then
      printf '\n## Verificación en hardware pendiente\n\n'
      for _ in $(seq "$3"); do printf -- '- [ ] algo\n'; done
      printf -- '- [x] ya hecho\n\n## Notas\n\n- [ ] tampoco cuenta\n'
    fi
  } >"docs/tasks/$1.md"
}
commit() { git add -A && git commit -q --allow-empty -m "$1"; }

tarea T-001 F1 0; commit "T-001: Esqueleto del workspace (#1)"
commit "Diseño inicial"
tarea T-040 F5 2; commit "T-040: esqueleto de la UI (#2)"
tarea T-010 F3 0; commit "T-010: DDC/CI en Windows (dxva2) (#3)"
git tag v0.1.0
tarea T-070 F5 1; commit "T-070: Instalador macOS (#4)"
commit "T-070: arreglar la firma (#5)"
tarea T-099 F9 0; commit "T-099: tarea de una fase rara (#6)"
commit "T-098: tarea sin archivo"
git switch -q -c otra && commit "Rama aparte" && git switch -q develop
git merge -q --no-ff -m "Merge branch 'otra'" otra
commit "Backlog: más tareas (#7)"
git tag v0.2.0

todo=$("$aqui/notes.sh" --version 0.1.0 --hasta v0.1.0)
contiene "notas: título con versión" "$todo" "# Tandem 0.1.0"
contiene "notas: primer release sin tag previo" "$todo" "Primer release: 3 tareas integradas."
contiene "notas: tarea con su PR" "$todo" "- **T-001** Esqueleto del workspace (#1)"
contiene "notas: fase del frontmatter" "$todo" "## F3 Monitores y modos"
contiene "notas: otros cambios" "$todo" "- Diseño inicial"
contiene "notas: hardware pendiente cuenta solo su sección" "$todo" "- **T-040** esqueleto de la UI: 2"
no_contiene "notas: sin tareas de después" "$todo" "T-070"
# Las fases salen en el orden del diseño (F1 antes que F3 antes que F5).
orden=$(grep '^## F' <<<"$todo" | tr '\n' '|')
[ "$orden" = "## F1 Núcleo|## F3 Monitores y modos|## F5 UI|" ] && ok "notas: orden de fases" || mal "notas: orden de fases ($orden)"

dos=$("$aqui/notes.sh" --hasta v0.2.0)
contiene "notas: desde el tag anterior" "$dos" 'Cambios desde `v0.1.0`: 3 tareas integradas.'
no_contiene "notas: excluye lo del release anterior" "$dos" "T-001"
contiene "notas: una tarea con varios PRs sale una vez" "$dos" "- **T-070** Instalador macOS (#4, #5)"
contiene "notas: tarea sin archivo va en sin fase" "$dos" "## Sin fase"
contiene "notas: tarea sin PR" "$dos" "- **T-098** tarea sin archivo"
contiene "notas: fase desconocida tal cual" "$dos" "## F9"
contiene "notas: commits de una rama integrada" "$dos" "- Rama aparte"
no_contiene "notas: sin commits de merge" "$dos" "Merge branch"
[ "$(grep -c 'T-070' <<<"$dos")" = 2 ] && ok "notas: T-070 en su fase y en hardware" || mal "notas: T-070 repetida"
ultima=$(grep '^## ' <<<"$dos" | tail -n 1)
[ "$ultima" = "## Verificación en hardware pendiente" ] && ok "notas: hardware al final" || mal "notas: hardware al final ($ultima)"
f9=$(grep -n '^## F9' <<<"$dos" | cut -d: -f1); sf=$(grep -n '^## Sin fase' <<<"$dos" | cut -d: -f1)
[ "$f9" -lt "$sf" ] && ok "notas: fases desconocidas al final" || mal "notas: fases desconocidas al final"

rango=$("$aqui/notes.sh" --desde v0.1.0 --hasta HEAD~1)
no_contiene "notas: --desde y --hasta explícitos" "$rango" "Backlog: más tareas"
if "$aqui/notes.sh" --hasta no-existe >/dev/null 2>&1; then mal "notas: ref inexistente"; else ok "notas: ref inexistente"; fi

if [ "$fallos" -gt 0 ]; then echo "$fallos pruebas fallaron" >&2; exit 1; fi
echo "todas las pruebas pasaron"
