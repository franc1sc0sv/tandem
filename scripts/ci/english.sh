#!/usr/bin/env bash
# Fails if Spanish comes back into the repository (T-210): accented Spanish characters or a
# list of common Spanish words in tracked text files, and Spanish words in file names.
# Exceptions live in scripts/ci/english-allow.txt (path plus an optional line regex).
set -u
cd "$(git rev-parse --show-toplevel)"

ALLOW=scripts/ci/english-allow.txt
ACCENTS='[áéíóúñüçÁÉÍÓÚÑÜÇ¿¡]'
WORDS='que|para|con|una|los|las|del|sin|esta|este|cuando|tambien|porque|aqui|ahora|otra|otro|maquina|puesto|pantalla|prueba|pruebas|carpeta|archivo|enlace|tarea|llave|encendido|escritorio|ventana|trabajo|escena|acomodo|ajustes|atajos|bandeja|pero|como|desde|donde|tuneles|mascaras|asistente|cerrar|colapsado|estados|capturas|monitores|permisos|energia|modulos|agregar|lado'
# Whole words (an underscore or hyphen ends a word, so `clear_errors` is split), lowercase or capitalised. "Del" and "Con" stay lowercase-only (the Del key,
# the Windows device name CON), and ALL-CAPS identifiers are not Spanish prose.
WORD_RE="(?<![A-Za-z0-9.])(?:$WORDS|$(printf '%s' "$WORDS" | tr '|' '\n' | grep -vxE 'del|con' | sed -E 's/^(.)/\U\1/' | paste -sd'|'))(?![A-Za-z0-9])"
SKIP=(':!scripts/ci/english.sh' ':!scripts/ci/english-allow.txt')

# allowed FILE TEXT: true when english-allow.txt lists FILE (a shell glob) with no regex, or with a regex that matches TEXT.
allowed() {
  local file=$1 text=$2 path re
  while read -r path re; do
    [ -z "$path" ] && continue
    case $path in '#'*) continue ;; esac
    case $file in $path) ;; *) continue ;; esac
    [ -z "$re" ] && return 0
    printf '%s\n' "$text" | grep -qP -- "$re" && return 0
  done <"$ALLOW"
  return 1
}

failed=0
# git grep -I skips binary files.
while IFS= read -r hit; do
  file=${hit%%:*}
  rest=${hit#*:}
  line=${rest%%:*}
  text=${rest#*:}
  if allowed "$file" "$text"; then continue; fi
  printf '%s:%s: %s\n' "$file" "$line" "$text"
  failed=1
done < <(git grep -nIP -e "$ACCENTS|$WORD_RE" -- . "${SKIP[@]}")

# File names: any path component that is a Spanish word or has an accent.
words_re="^(?:$WORDS)\$"
while IFS= read -r path; do
  if allowed "$path" ""; then continue; fi
  if printf '%s\n' "$path" | tr '/._-' '\n\n\n\n' | grep -qiP -e "$ACCENTS|$words_re"; then
    printf '%s:0: Spanish file name\n' "$path"
    failed=1
  fi
done < <(git ls-files -- . "${SKIP[@]}")

[ "$failed" -eq 0 ] && echo "english: no findings"
exit "$failed"
