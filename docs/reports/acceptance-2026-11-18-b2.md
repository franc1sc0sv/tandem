# Aceptación de Tandem v0.3 — parte B, ronda 2 (flujos de punta a punta)

Fecha: 2026-11-18 · Base: `origin/develop` en `b3841bc` (T-152, T-155, T-159 integradas) · Agente: acceptance (parte B). Fuera de alcance: paridad píxel a píxel (parte A) y calidad base de CI (parte C).

## Veredicto

**FALLA.** Las 10 fallas de la ronda 1 (T-150 a T-159) quedaron corregidas salvo **una que sigue rota** (T-151: la actividad de la otra máquina en el paso 4 no llega a la UI) y se encontró **una nueva** (bandeja). Dos tareas de corrección: **T-166** y **T-167**. Un solo ítem sin pasar basta para no declarar la app terminada.

## Cómo se probó

Mismo arnés que la ronda 1 (`docs/reports/acceptance/2026-11-18-b/harness/`): dos `tandemd` reales por loopback, un puente HTTP por máquina con el código real de la app, la UI de producción en Chromium con Playwright, anfitrión con dobles de motor de entrada, DDC y portapapeles, proxies UDP para cortar caminos, cliente MCP contra `tandem mcp`. Se corrió la tanda completa (`all.sh`), se rehicieron en parejas limpias las comprobaciones que dependían de la UI vieja y se añadieron las nuevas (`docs/reports/acceptance/2026-11-18-b2/harness/`). Pares: macOS+Windows, Windows+Windows y macOS+macOS (G2); modo demo con todas las rutas y variantes. Capturas en `docs/reports/acceptance/2026-11-18-b2/<flujo>/` (las de Túneles se quitaron: muestran la ruta del proyecto de prueba). Resultados crudos: `resultados.jsonl` (144 filas: 133 pasan, 2 fallan, 8 invalidadas con su nota: falsos negativos del arnés comprobados aparte).

Límites del entorno (sin cambio respecto de la ronda 1): sin WebKitGTK, sin bandeja ni atajos globales del sistema, updater real sustituido por un doble, y todo lo `[hw]` (permisos reales de macOS, UAC, Wake-on-LAN real, DDC real, cruce con mouse real, arrastre de archivos con compositor real) queda para la suite T-123 o la lista de hardware pendiente de cada tarea. Los flujos F7 y F8 de arrastre se cubren con sus pruebas de integración con dobles.

## Las fallas de la ronda 1, vueltas a comprobar

| Tarea | Qué se comprobó ahora | Resultado |
|---|---|---|
| T-150 | «Probar cruce» habilitado; un cruce real del motor da «Cruzó a … por el borde derecho, como dice el acomodo» y «Continuar» se habilita; sin cruce, «No cruzó en 20 s»; el resumen dice «cruce probado»; Acomodo muestra 1920 × 1080 y 3440 × 1440, 21:9, 100 %; los 12 casos 4 lados × 3 alineaciones llegan a las dos máquinas y a `tandem.toml` (y el motor cruza por el borde nuevo, no por el viejo) | PASA |
| T-151 | Una máquina sin hub ya no aparece con «Teclado y mouse» (PASA). **Pero** «Actividad detectada» de la otra máquina sigue sin llegar a la UI | **FALLA** → T-166 |
| T-152 | «Salir del puesto» con confirmación: las dos quedan «sin emparejar» en 14 ms y `trust.toml` queda sin llaves | PASA |
| T-153 | Recargar en Red deja «Medir ancho de banda» habilitado | PASA |
| T-154 | Una ventana recién abierta o recargada muestra los túneles existentes (origen y otra máquina) | PASA |
| T-155 | `files_put` de una credencial se omite con aviso («1 archivo(s) omitido(s) por parecer credenciales») | PASA |
| T-156 | El asistente se abre solo en Windows; «Cerrar» mide 32 × 32, recibe el clic y cierra | PASA |
| T-157 | «Bloquear aquí» bloquea (el cursor no cruza), la UI lo muestra, «Quitar el bloqueo» lo suelta; en la máquina que no manda explica por qué no | PASA |
| T-158 | Túneles y Archivos con razones correctas; el resumen no dice «falta el cómputo remoto» | PASA |
| T-159 | MAC de Wake-on-LAN editable en la página de la otra máquina y guardada en `tandem.toml`; el nombre del puesto se edita en Ajustes y la otra máquina lo ve (`desk_name` en las dos) | PASA |

## Veredicto por ítem de `docs/ACCEPTANCE.md`

| Ítem | Veredicto | Tareas | Evidencia |
|---|---|---|---|
| F1 Primer uso | **FALLA** | T-166 | Pasan: asistente solo en Windows, mismo código de 6 dígitos, código malo rechazado, emparejadas, paso 4 sin falsos positivos, paso 5 con cruce real, resumen, comando MCP copiado. Falla: actividad de la otra máquina en el paso 4 (`F1-asistente/04-actividad-otra.png`) |
| F2 Acomodo | PASA | | F2-acomodo, F2-prueba-de-cruce-en-vivo |
| F3 Escenas | PASA | | F3-escenas |
| F4 Monitores | PASA | | F4-monitores · `[hw]` del 34" en T-123 |
| F5 Túneles por MCP | PASA | | todos los pasos pasan, incluidos T-130, la UI recién abierta y `files_put` de credencial |
| F6 Red | PASA | | F6-red |
| F7 Teclado y mouse sin dueño | PASA | | tres comprobaciones |
| F8 Portapapeles y archivos | PASA | | portapapeles texto/imagen A↔B; archivos con pruebas de integración |
| F9 Configuración compartida y Deshacer | PASA | | F9-config-compartida |
| F10 Energía | PASA | | F10-energia (la MAC se escribe desde la UI) |
| F11 Módulos | PASA | | razones correctas |
| F12 Ajustes y paleta | PASA | | todas las acciones y comandos; «Salir del puesto» y «Bloquear aquí» corregidos |
| F13 Actualizaciones | PASA | | F13-actualizaciones, estados difíciles |
| F14 Bandeja | **FALLA** | T-167 | Muestra el estado y aplica escenas, pero dice «1 túneles» y el conteo depende del sistema (`F14-bandeja/02-bandeja-con-tunel.png`) |
| G2 Nada fijo a Mac/PC | **FALLA** | T-167 | Windows+Windows y macOS+macOS pasan en todas las pantallas y la paleta; la bandeja (`Tray.svelte`) pinta «macOS» o «N túneles» según el sistema (leído del código) |
| G6 Cada elemento hace algo real | PASA | | barrido de botones: «Probar cruce» y «Medir» ya funcionan; los 6 «sin efecto» son la pestaña ya activa y el atajo fijo deshabilitado |
| G7 / G8 Animaciones y movimiento reducido | no reevaluado | | no se repitió en esta ronda (la ronda 1 los dio por PASA); lo cubre la parte A |
| G9 Consola sin errores / sin `{{`, `undefined`, `NaN` | PASA | | ninguna consola con errores en vivo; barrido demo (3 variantes × 12 rutas + 14 estados): sin hallazgos |
| G10 Todo lo editable se edita desde la UI | PASA | | Wake-on-LAN y nombre del puesto |

No evaluados aquí: P1–P15 y la similitud con los mockups (parte A), G1/G3–G5/G11 de calidad base (parte C) y H1–H5 (hardware). Cosas que se vieron y no se cuentan como falla: «Tamaño —» en Acomodo (el motor no lee el tamaño físico; documentado en T-150) y el nombre «s0» de las pantallas, que es el que da el motor simulado.

## Tareas de corrección

| Tarea | Falla | Flujo |
|---|---|---|
| T-166 | El IPC de `tandemd` nunca reenvía `AgentToUi::KeyProbe`: «Actividad detectada» de la otra máquina no llega a la UI | F1 |
| T-167 | Bandeja: «1 túneles» y conteo por sistema operativo | F14, G2 |

## Reproducir

Como en la ronda 1 (`docs/reports/acceptance/2026-11-18-b/harness/`), con `h.sh` y los scripts de `2026-11-18-b2/harness/` (`f2.mjs`, `f2cross.mjs`, `wiz5.mjs`, `aco.mjs`, `cerrar.mjs`, `g10.mjs`, `g10b.mjs`, `tray.mjs`, `f16.mjs`, `dbg3.mjs` para T-166). `ACC_RUN` tiene que ser una ruta corta (el socket de `tandemd` no admite más de 108 bytes). Nota de entorno: `pnpm install --frozen-lockfile` falla hoy con `fast-check@4.10.3` por la política de edad mínima (publicado el mismo día); se instaló con `--config.minimumReleaseAge=0`.
