#!/bin/bash
# h.sh <script.mjs> args... : corre un script del arnés (el de la ronda 1 más estos) con el entorno de la ronda 2.
# El socket de tandemd no admite rutas largas (SUN_LEN): ACC_RUN tiene que ser corto.
# WT: raíz del worktree; los binarios (tandemd, tandem, acc_host, acc_bridge) van en el directorio que lee `bin()` de lib.mjs.
: "${WT:?define WT con la raíz del worktree}"
export ACC_SHOTS=${ACC_SHOTS:-$WT/docs/reports/acceptance/2026-11-18-b2}
export ACC_RUN=${ACC_RUN:-/tmp/claude-1000/accB2run}
cd "$(dirname "$0")"
exec node "$@"
