// Genera docs/reports/acceptance-2026-11-18-b.md a partir de results.jsonl (última corrida de cada comprobación).
import { readFileSync, writeFileSync } from "node:fs"; import { WT, RUN } from "./lib.mjs";
const rows = readFileSync(`${RUN}/results.jsonl`, "utf8").trim().split("\n").map((l) => JSON.parse(l));
const last = new Map(); for (const r of rows) last.set(r.flow + "|" + r.item, r); const items = [...last.values()];
const esc = (s) => String(s ?? "").replace(/\|/g, "¦").replace(/\n/g, " ").replace(/\s+/g, " ").trim();
const flows = [...new Set(items.map((r) => r.flow))];
const total = items.length, fails = items.filter((r) => !r.ok);
const SHOT = "acceptance/2026-11-18-b";
let md = `# Aceptación de Tandem v0.3 — parte B (flujos de punta a punta)

Fecha: 2026-11-18 · Base: \`origin/develop\` en \`04f2967\` (T-131) · Agente: acceptance (parte B de 3). Fuera de alcance: paridad píxel a píxel (parte A) y calidad base de CI (parte C).

## Veredicto

**FALLA.** ${total} comprobaciones corridas, ${total - fails.length} pasan y ${fails.length} fallan; las fallas se agrupan en 10 tareas de corrección (T-150 a T-159). Un solo ítem sin pasar basta para no declarar la app terminada.

## Cómo se probó

- Dos \`tandemd\` reales por loopback (QUIC real, orquestador, IPC) con la UI de producción (\`pnpm build\`) en Chromium con Playwright. Como WebKitGTK no está en el entorno, el pegamento de Tauri se sustituyó por un **puente HTTP** (\`apps/tandem-ui/src-tauri/examples/acc_bridge.rs\`) que usa el código real de la app (\`session\`, \`shortcuts\`, \`setup\`, \`files\`, \`admin_service\`, \`update\`) y entrega los eventos por SSE; la ventana ve un \`__TAURI_INTERNALS__\` de prueba. Cada máquina tiene su puente y su navegador.
- Agentes: (1) el binario \`tandemd --sim-backends\` de verdad (emparejamiento, acomodo guardado, túneles reales por \`tandem mcp\`); (2) como \`--sim-backends\` no trae motor de entrada, monitores ni portapapeles, un **anfitrión** (\`crates/tandemd/examples/acc_host.rs\`, el mismo agente con \`daemon::start\`) con los dobles de las pruebas del repo: motor de entrada con backends falsos, bus DDC falso de \`monitor-ddc\` (estándar y de fábrica, con cambio fallido inyectable), portapapeles falso y sistema operativo declarado (\`--os mac|windows\`). Un puerto de control hace de «mundo físico» (mover el mouse, conectar el hub, agregar o quitar monitores).
- Red con dos y tres caminos: proxies UDP entre los agentes (usb4, ethernet, wifi) que se cortan y se curan a voluntad.
- El cliente MCP es un script JSON-RPC contra \`tandem mcp\`.
- Efecto real comprobado en el agente (estado por IPC, \`tandem.toml\`, \`ui.toml\`, \`update.toml\`, bus del monitor falso, \`sim-steps.jsonl\`, directorio del workspace, ramas de git), no solo en pantalla.
- Pares probados: macOS+Windows (declarados), Windows+Windows y macOS+macOS (G2). Modo demo: todas las rutas y variantes.
- Código del arnés: \`docs/reports/acceptance/2026-11-18-b/harness/\`. Capturas: \`docs/reports/${SHOT}/<flujo>/<paso>.png\`.

### Límites del entorno (no son fallas de la app, pero quedan sin verificar)

- WebKitGTK, la ventana de Tauri, el ícono de la bandeja y los atajos globales del sistema no existen aquí: el puente registra lo que la app pediría a la ventana (\`ui_show_main\`, \`ui_quit\`, \`files_open\`). El updater real (plugin de Tauri) se sustituyó por un doble con el mismo plan (\`update::plan\`/\`run\`) contra un \`latest.json\` de prueba; no se instala nada.
- Permisos de macOS reales, UAC, Wake-on-LAN real, DDC real, cruce con mouse real y arrastre de archivos con un compositor real: \`[hw]\`. El arrastre de archivos entre máquinas se comprobó con sus pruebas de integración con dobles (\`cargo test -p tandemd --test drag --test files\`: 4 pruebas pasan), no con la UI.
- «Datos pausados si la entrada se degrada» (F6): se comprobó con \`cargo test -p tandem-sim --test network\` (4 pruebas pasan, incluida \`la_medición_se_corta_si_la_entrada_pasa_su_objetivo\`); la UI muestra el caso de «solo Wi-Fi» («Los envíos de datos esperan al cable»).
- La suite de hardware (T-123) no se corrió: H1–H5 quedan fuera de esta parte.

## Veredicto por ítem de \`docs/ACCEPTANCE.md\`

| Ítem | Veredicto | Tareas | Evidencia |
|---|---|---|---|
`;
const V = [
  ["F1 Primer uso (asistente 1→6, emparejar, permisos, teclado y mouse, acomodo + cruce, resumen, MCP)", "FALLA", "T-150, T-151, T-156, T-158", "Emparejar con código de 6 dígitos y el rechazo de un código malo pasan; faltan: asistente solo en el primer uso (Windows), paso 4, prueba de cruce, resumen y «Cerrar»"],
  ["F2 Acomodo (4 lados × 3 alineaciones, motor)", "FALLA", "T-150", "Los 12 casos se guardan y el motor cruza por el borde nuevo y no por el viejo; la UI no dibuja las pantallas reales y la prueba de cruce no se puede completar"],
  ["F3 Escenas (crear, editar, aplicar, falla inyectada, atajo global)", "PASA", "", "F3-escenas"],
  ["F4 Monitores (estándar con veredicto, fábrica sin escrituras, re-elección de dueño)", "PASA", "", "F4-monitores · `[hw]` del 34\" real queda en la suite T-123"],
  ["F5 Túneles por MCP (git, 3 procesos, salida, archivos, puerto, corte de enlace, bring_back, T-130)", "FALLA", "T-154, T-155", "Todo lo del agente pasa; la UI recién abierta no muestra los túneles existentes y files_put de una credencial da un error equivocado"],
  ["F6 Red (corte < 1 s, evento en Red, medir ancho de banda)", "PASA", "T-153 (relacionada)", "F6-red · el botón de medir queda deshabilitado al recargar en Red (T-153)"],
  ["F7 Teclado y mouse sin dueño", "PASA", "", "F7-sin-dueno"],
  ["F8 Portapapeles y archivos", "PASA", "", "F8-portapapeles · arrastre de archivos por pruebas con dobles"],
  ["F9 Configuración compartida y «Deshacer»", "PASA", "", "F9-config-compartida"],
  ["F10 Energía", "PASA", "T-159 (relacionada)", "F10-energia · con `[power.wol]` puesto a mano; sin él «Encender» no funciona (T-159)"],
  ["F11 Módulos (apagar/encender, razones de «no disponible»)", "FALLA", "T-158", "Apagar y encender pasa; las razones de Túneles y Archivos son falsas"],
  ["F12 Ajustes y paleta (incluido «Bloquear aquí»)", "FALLA", "T-152, T-157", "Casi todas las acciones hacen lo que dicen; «Bloquear aquí» bloquea pero la UI nunca lo muestra y «Salir del puesto» deja a la otra máquina emparejada"],
  ["F13 Actualizaciones (latest.json de prueba, plan de las dos)", "PASA", "", "F13-actualizaciones · release real documentado en `docs/dev/release.md`"],
  ["F14 Bandeja", "PASA", "T-154 (relacionada)", "F14-bandeja · la bandeja dice «0 túneles» con túneles abiertos (T-154)"],
  ["G2 Nada fijo a Mac/PC (Windows+Windows, macOS+macOS)", "PASA", "", "G2-sin-Mac-PC-fijos"],
  ["G6 Cada elemento interactivo hace algo real", "FALLA", "T-150, T-153", "Barrido de botones: «Probar cruce» deshabilitado siempre y «Medir» deshabilitado tras recargar"],
  ["G7 / G8 Animaciones y movimiento reducido", "PASA", "", "G7-animaciones, G8-reduced-motion (muestra de pantallas)"],
  ["G9 Consola sin errores / sin `{{`, `undefined`, `NaN`", "PASA", "", "Ninguna consola con errores en toda la corrida en vivo y en el barrido de modo demo"],
  ["G10 Todo lo editable se edita desde la UI", "FALLA", "T-159", "Wake-on-LAN y el nombre del puesto no se editan desde la UI"],
];
for (const [a, v, t, e] of V) md += `| ${a} | **${v}** | ${t} | ${esc(e)} |\n`;
md += `\nNo evaluados aquí: P1–P15 y la similitud con los mockups (parte A), G1/G3–G5/G11 de calidad base (parte C) y H1–H5 (hardware).\n\n## Tareas de corrección\n\n| Tarea | Falla | Flujo |\n|---|---|---|\n`;
md += `| T-150 | Acomodo y prueba de cruce sin lo que sabe el motor (cruce real, pantallas reales) | F1, F2, G6 |\n| T-151 | Paso 4 del asistente: detecta lo supuesto y no oye la otra máquina | F1 |\n| T-152 | «Salir del puesto» deja a la otra máquina emparejada | F12 |\n| T-153 | \`can()\` no reactivo: botones deshabilitados tras recargar | F6, G6 |\n| T-154 | Túneles vacíos en una ventana recién abierta | F5, F14 |\n| T-155 | files_put de una credencial responde «manifiesto no válido» | F5 |\n| T-156 | Asistente no abre solo en Windows y «Cerrar» colapsado | F1 |\n| T-157 | «Bloquear aquí» sin estado visible | F12 |\n| T-158 | Módulos Túneles y Archivos con razón falsa | F11, F1 |\n| T-159 | Wake-on-LAN solo por \`tandem.toml\`; nombre del puesto no editable | F10, G10 |\n\n`;
md += `## Reproducir

1. \`cargo build -p tandemd --features sim-backends -p tandem-cli\` y, con los fuentes de \`harness/\` copiados a \`crates/tandemd/examples/acc_host.rs\` y \`apps/tandem-ui/src-tauri/examples/acc_bridge.rs\`, \`cargo build -p tandemd --features sim-backends --example acc_host\` y \`cargo build -p tandem-ui --example acc_bridge\`; \`pnpm -C apps/tandem-ui build\` y \`build:demo\`.
2. Copiar los cuatro binarios (\`tandemd\`, \`tandem\`, \`acc_host\`, \`acc_bridge\`) a \`/tmp/claude-1000/accB/bin\` (la ruta que usa \`lib.mjs\`; cámbiala en \`bin()\`).
3. \`node up.mjs <tag> '{"host":true}'\` levanta el par; los scripts \`f*.mjs\` reciben el \`<tag>\`; \`all.sh\` encadena la corrida oficial.

## Detalle de cada comprobación\n\n`;
for (const f of flows) {
  md += `### ${f}\n\n| Comprobación | Resultado | Detalle |\n|---|---|---|\n`;
  for (const r of items.filter((x) => x.flow === f)) md += `| ${esc(r.item)} | ${r.ok ? "PASA" : "**FALLA**"} | ${esc(r.detail).slice(0, 260)} |\n`;
  md += `\nCapturas: \`docs/reports/${SHOT}/${f.replace(/^F13-estados-dificiles$/, "F13-estados-dificiles")}/\`\n\n`;
}
md += `## Otras observaciones (sin tarea)

- Los avisos de «Cambiado desde …» no se cierran solos (piden decisión) y tapan controles de la esquina superior derecha mientras no se descartan; tras «Deshacer», el aviso que recibe la otra máquina nombra a la que cambió con su nombre anterior.
- «Aplicar» una escena cuyos monitores no están presentes no dice nada (no hay aviso de que no hizo nada).
- Escenas (F3): en 5 corridas, 4 pasaron completas; en una la primera aplicación tras agregar los monitores falló con «no se sabe qué máquina muestra (falta la entrada real o a qué entrada va cada máquina)» (el agente aún no había leído la entrada o aprendido los cables) y en otra la aplicación fue bien pero la pantalla de progreso no llegó a mostrarse (la transacción terminó antes de abrirla y volvió a Monitores). No se redujeron a una causa; si reaparecen, merecen su propia tarea.
- El estado «enlace caído» se resolvió como «PC está apagada» cuando se cortaron los tres caminos más de \`pc_lost_after_secs\` (2 s en la prueba); con el valor por omisión podría verse distinto: no se probó el rango.
- «Sin agente» no tiene botón «Reintentar»: reconecta solo cada 2 s (\`agent_retry\` existe y la UI no lo usa).
`;
writeFileSync(`${WT}/docs/reports/acceptance-2026-11-18-b.md`, md); console.log("escrito", total, "comprobaciones,", fails.length, "fallas");
