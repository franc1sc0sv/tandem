#!/usr/bin/env bash
# Release notes in Markdown, built from the merged tasks: each task PR lands on
# develop as a squash commit `T-<id>: <title> (#<pr>)`. It groups them by the phase in the
# frontmatter of docs/tasks/T-<id>.md (as it is at --to), lists non-task commits separately
# and flags tasks that still have pending hardware verification.
#
# By default the range goes from the v* tag before --to (or from the start if there is
# none). See docs/dev/release.md.
#
# Usage: scripts/release/notes.sh [--version <x.y.z>] [--from <ref>] [--to <ref>]
set -euo pipefail

version=
from_ref=
to_ref=HEAD
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version=$2; shift 2 ;;
    --from) from_ref=$2; shift 2 ;;
    --to) to_ref=$2; shift 2 ;;
    *) echo "notes.sh: unknown option $1" >&2; exit 2 ;;
  esac
done

git rev-parse --verify --quiet "$to_ref^{commit}" >/dev/null \
  || { echo "notes.sh: $to_ref does not exist" >&2; exit 1; }
if [ -z "$from_ref" ]; then
  # `$to_ref^`: if --to is the release tag itself, the previous one is a different tag.
  from_ref=$(git describe --tags --abbrev=0 --match 'v*' "$to_ref^" 2>/dev/null || true)
fi
range=$to_ref
[ -n "$from_ref" ] && range="$from_ref..$to_ref"

phase_name() {
  case "$1" in
    F0) echo "F0 Spikes" ;;
    F1) echo "F1 Core" ;;
    F2) echo "F2 Keyboard and mouse" ;;
    F3) echo "F3 Monitors and modes" ;;
    F4) echo "F4 Power" ;;
    F5) echo "F5 UI" ;;
    v2) echo "v2 Data" ;;
    v3) echo "v3 Files" ;;
    "") echo "No phase" ;;
    *) echo "$1" ;;
  esac
}

declare -A title=() phase=() prs=()
order=()    # tasks in the order they were merged
others=()
while IFS= read -r subject; do
  if [[ "$subject" =~ ^(T-[0-9]+):[[:space:]]+(.*)$ ]]; then
    id=${BASH_REMATCH[1]}
    rest=${BASH_REMATCH[2]}
    pr=
    if [[ "$rest" =~ ^(.*[^[:space:]])[[:space:]]+\((#[0-9]+)\)$ ]]; then
      rest=${BASH_REMATCH[1]}
      pr=${BASH_REMATCH[2]}
    fi
    # A task with several commits (a later fix) appears once, with all its PRs.
    if [ -z "${title[$id]+x}" ]; then
      order+=("$id")
      title[$id]=$rest
      prs[$id]=$pr
      phase[$id]=$(git show "$to_ref:docs/tasks/$id.md" 2>/dev/null \
        | sed -n '1,/^---$/{s/^phase:[[:space:]]*//p;}' | head -n 1 || true)
    elif [ -n "$pr" ]; then
      prs[$id]="${prs[$id]:+${prs[$id]}, }$pr"
    fi
  else
    others+=("$subject")
  fi
done < <(git log --no-merges --reverse --format=%s "$range")

if [ -n "$version" ]; then echo "# Tandem $version"; else echo "# Tandem"; fi
echo
if [ -n "$from_ref" ]; then
  echo "Changes since \`$from_ref\`: ${#order[@]} tasks merged."
else
  echo "First release: ${#order[@]} tasks merged."
fi

# Phases in the design order; unknown ones after, in alphabetical order, and last the
# tasks without a file.
mapfile -t phases < <(for id in "${order[@]}"; do echo "${phase[$id]}"; done \
  | awk '{ n = split("F0 F1 F2 F3 F4 F5 v2 v3", o, " "); p = ($0 == "") ? 100 : 99
         for (i = 1; i <= n; i++) if (o[i] == $0) p = i
         print p "\t" $0 }' \
  | sort -t "$(printf '\t')" -k1,1n -k2,2 -u | cut -f2-)
for f in "${phases[@]}"; do
  echo
  echo "## $(phase_name "$f")"
  echo
  for id in "${order[@]}"; do
    [ "${phase[$id]}" = "$f" ] || continue
    echo "- **$id** ${title[$id]}${prs[$id]:+ (${prs[$id]})}"
  done
done

if [ ${#others[@]} -gt 0 ]; then
  echo
  echo "## Other changes"
  echo
  for o in "${others[@]}"; do echo "- $o"; done
fi

# What can only be checked on the real machine: unchecked boxes in that section.
pending=()
for id in "${order[@]}"; do
  file="$to_ref:docs/tasks/$id.md"
  git cat-file -e "$file" 2>/dev/null || continue
  n=$(git show "$file" \
    | awk '/^## /{ inside = ($0 ~ /^## Pending hardware verification/) } inside && /^- \[ \]/{ n++ } END{ print n + 0 }')
  [ "$n" -gt 0 ] && pending+=("- **$id** ${title[$id]}: $n")
done
if [ ${#pending[@]} -gt 0 ]; then
  echo
  echo "## Pending hardware verification"
  echo
  echo "Checks that can only be done on the real machine (list in \`docs/tasks/T-<id>.md\`):"
  echo
  printf '%s\n' "${pending[@]}"
fi
