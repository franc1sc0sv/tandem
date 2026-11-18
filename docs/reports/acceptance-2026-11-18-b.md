# Aceptación de Tandem v0.3 — parte B (flujos de punta a punta)

Fecha: 2026-11-18 · Base: `origin/develop` en `04f2967` (T-131) · Agente: acceptance (parte B de 3). Fuera de alcance: paridad píxel a píxel (parte A) y calidad base de CI (parte C).

## Veredicto

**FALLA.** 136 comprobaciones corridas, 120 pasan y 16 fallan; las fallas se agrupan en 10 tareas de corrección (T-150 a T-159). Un solo ítem sin pasar basta para no declarar la app terminada.

## Cómo se probó

- Dos `tandemd` reales por loopback (QUIC real, orquestador, IPC) con la UI de producción (`pnpm build`) en Chromium con Playwright. Como WebKitGTK no está en el entorno, el pegamento de Tauri se sustituyó por un **puente HTTP** (`apps/tandem-ui/src-tauri/examples/acc_bridge.rs`) que usa el código real de la app (`session`, `shortcuts`, `setup`, `files`, `admin_service`, `update`) y entrega los eventos por SSE; la ventana ve un `__TAURI_INTERNALS__` de prueba. Cada máquina tiene su puente y su navegador.
- Agentes: (1) el binario `tandemd --sim-backends` de verdad (emparejamiento, acomodo guardado, túneles reales por `tandem mcp`); (2) como `--sim-backends` no trae motor de entrada, monitores ni portapapeles, un **anfitrión** (`crates/tandemd/examples/acc_host.rs`, el mismo agente con `daemon::start`) con los dobles de las pruebas del repo: motor de entrada con backends falsos, bus DDC falso de `monitor-ddc` (estándar y de fábrica, con cambio fallido inyectable), portapapeles falso y sistema operativo declarado (`--os mac|windows`). Un puerto de control hace de «mundo físico» (mover el mouse, conectar el hub, agregar o quitar monitores).
- Red con dos y tres caminos: proxies UDP entre los agentes (usb4, ethernet, wifi) que se cortan y se curan a voluntad.
- El cliente MCP es un script JSON-RPC contra `tandem mcp`.
- Efecto real comprobado en el agente (estado por IPC, `tandem.toml`, `ui.toml`, `update.toml`, bus del monitor falso, `sim-steps.jsonl`, directorio del workspace, ramas de git), no solo en pantalla.
- Pares probados: macOS+Windows (declarados), Windows+Windows y macOS+macOS (G2). Modo demo: todas las rutas y variantes.
- Código del arnés: `docs/reports/acceptance/2026-11-18-b/harness/`. Capturas: `docs/reports/acceptance/2026-11-18-b/<flujo>/<paso>.png`.

### Límites del entorno (no son fallas de la app, pero quedan sin verificar)

- WebKitGTK, la ventana de Tauri, el ícono de la bandeja y los atajos globales del sistema no existen aquí: el puente registra lo que la app pediría a la ventana (`ui_show_main`, `ui_quit`, `files_open`). El updater real (plugin de Tauri) se sustituyó por un doble con el mismo plan (`update::plan`/`run`) contra un `latest.json` de prueba; no se instala nada.
- Permisos de macOS reales, UAC, Wake-on-LAN real, DDC real, cruce con mouse real y arrastre de archivos con un compositor real: `[hw]`. El arrastre de archivos entre máquinas se comprobó con sus pruebas de integración con dobles (`cargo test -p tandemd --test drag --test files`: 4 pruebas pasan), no con la UI.
- «Datos pausados si la entrada se degrada» (F6): se comprobó con `cargo test -p tandem-sim --test network` (4 pruebas pasan, incluida `la_medición_se_corta_si_la_entrada_pasa_su_objetivo`); la UI muestra el caso de «solo Wi-Fi» («Los envíos de datos esperan al cable»).
- La suite de hardware (T-123) no se corrió: H1–H5 quedan fuera de esta parte.

## Veredicto por ítem de `docs/ACCEPTANCE.md`

| Ítem | Veredicto | Tareas | Evidencia |
|---|---|---|---|
| F1 Primer uso (asistente 1→6, emparejar, permisos, teclado y mouse, acomodo + cruce, resumen, MCP) | **FALLA** | T-150, T-151, T-156, T-158 | Emparejar con código de 6 dígitos y el rechazo de un código malo pasan; faltan: asistente solo en el primer uso (Windows), paso 4, prueba de cruce, resumen y «Cerrar» |
| F2 Acomodo (4 lados × 3 alineaciones, motor) | **FALLA** | T-150 | Los 12 casos se guardan y el motor cruza por el borde nuevo y no por el viejo; la UI no dibuja las pantallas reales y la prueba de cruce no se puede completar |
| F3 Escenas (crear, editar, aplicar, falla inyectada, atajo global) | **PASA** |  | F3-escenas |
| F4 Monitores (estándar con veredicto, fábrica sin escrituras, re-elección de dueño) | **PASA** |  | F4-monitores · `[hw]` del 34" real queda en la suite T-123 |
| F5 Túneles por MCP (git, 3 procesos, salida, archivos, puerto, corte de enlace, bring_back, T-130) | **FALLA** | T-154, T-155 | Todo lo del agente pasa; la UI recién abierta no muestra los túneles existentes y files_put de una credencial da un error equivocado |
| F6 Red (corte < 1 s, evento en Red, medir ancho de banda) | **PASA** | T-153 (relacionada) | F6-red · el botón de medir queda deshabilitado al recargar en Red (T-153) |
| F7 Teclado y mouse sin dueño | **PASA** |  | F7-sin-dueno |
| F8 Portapapeles y archivos | **PASA** |  | F8-portapapeles · arrastre de archivos por pruebas con dobles |
| F9 Configuración compartida y «Deshacer» | **PASA** |  | F9-config-compartida |
| F10 Energía | **PASA** | T-159 (relacionada) | F10-energia · con `[power.wol]` puesto a mano; sin él «Encender» no funciona (T-159) |
| F11 Módulos (apagar/encender, razones de «no disponible») | **FALLA** | T-158 | Apagar y encender pasa; las razones de Túneles y Archivos son falsas |
| F12 Ajustes y paleta (incluido «Bloquear aquí») | **FALLA** | T-152, T-157 | Casi todas las acciones hacen lo que dicen; «Bloquear aquí» bloquea pero la UI nunca lo muestra y «Salir del puesto» deja a la otra máquina emparejada |
| F13 Actualizaciones (latest.json de prueba, plan de las dos) | **PASA** |  | F13-actualizaciones · release real documentado en `docs/dev/release.md` |
| F14 Bandeja | **PASA** | T-154 (relacionada) | F14-bandeja · la bandeja dice «0 túneles» con túneles abiertos (T-154) |
| G2 Nada fijo a Mac/PC (Windows+Windows, macOS+macOS) | **PASA** |  | G2-sin-Mac-PC-fijos |
| G6 Cada elemento interactivo hace algo real | **FALLA** | T-150, T-153 | Barrido de botones: «Probar cruce» deshabilitado siempre y «Medir» deshabilitado tras recargar |
| G7 / G8 Animaciones y movimiento reducido | **PASA** |  | G7-animaciones, G8-reduced-motion (muestra de pantallas) |
| G9 Consola sin errores / sin `{{`, `undefined`, `NaN` | **PASA** |  | Ninguna consola con errores en toda la corrida en vivo y en el barrido de modo demo |
| G10 Todo lo editable se edita desde la UI | **FALLA** | T-159 | Wake-on-LAN y el nombre del puesto no se editan desde la UI |

No evaluados aquí: P1–P15 y la similitud con los mockups (parte A), G1/G3–G5/G11 de calidad base (parte C) y H1–H5 (hardware).

## Tareas de corrección

| Tarea | Falla | Flujo |
|---|---|---|
| T-150 | Acomodo y prueba de cruce sin lo que sabe el motor (cruce real, pantallas reales) | F1, F2, G6 |
| T-151 | Paso 4 del asistente: detecta lo supuesto y no oye la otra máquina | F1 |
| T-152 | «Salir del puesto» deja a la otra máquina emparejada | F12 |
| T-153 | `can()` no reactivo: botones deshabilitados tras recargar | F6, G6 |
| T-154 | Túneles vacíos en una ventana recién abierta | F5, F14 |
| T-155 | files_put de una credencial responde «manifiesto no válido» | F5 |
| T-156 | Asistente no abre solo en Windows y «Cerrar» colapsado | F1 |
| T-157 | «Bloquear aquí» sin estado visible | F12 |
| T-158 | Módulos Túneles y Archivos con razón falsa | F11, F1 |
| T-159 | Wake-on-LAN solo por `tandem.toml`; nombre del puesto no editable | F10, G10 |

## Reproducir

1. `cargo build -p tandemd --features sim-backends -p tandem-cli` y, con los fuentes de `harness/` copiados a `crates/tandemd/examples/acc_host.rs` y `apps/tandem-ui/src-tauri/examples/acc_bridge.rs`, `cargo build -p tandemd --features sim-backends --example acc_host` y `cargo build -p tandem-ui --example acc_bridge`; `pnpm -C apps/tandem-ui build` y `build:demo`.
2. Copiar los cuatro binarios (`tandemd`, `tandem`, `acc_host`, `acc_bridge`) a `/tmp/claude-1000/accB/bin` (la ruta que usa `lib.mjs`; cámbiala en `bin()`).
3. `node up.mjs <tag> '{"host":true}'` levanta el par; los scripts `f*.mjs` reciben el `<tag>`; `all.sh` encadena la corrida oficial.

## Detalle de cada comprobación

### F1-asistente

| Comprobación | Resultado | Detalle |
|---|---|---|
| primer uso en máquina no macOS: el asistente se abre solo | **FALLA** | no se abre solo; hay que ir a Ajustes → «Volver a correr el asistente» |
| las dos UIs muestran el mismo código de 6 dígitos | PASA | UI m2 muestra 009207; agente m1 {"code":"009207","confirmed":false,"kind":"comparing","peer":"PC"} |
| un código equivocado se rechaza con aviso | PASA |  |
| emparejadas: las dos agentes lo dicen y con lados distintos | PASA | m1=a m2=b |
| paso 4: actividad en la otra máquina se marca «Actividad detectada» | **FALLA** | estés usando. ¦ Mac ¦ Teclado y mouse ¦ PC ¦ Teclado y mouse ¦ Atrás ¦ Continuar |
| resumen: el comando MCP se copia | PASA | claude mcp add tandem -- tandem mcp |
| el botón «Cerrar» del asistente es visible y se puede pulsar | **FALLA** | caja 24×2 px; lo que recibe el clic: brand svelte-1b2mlu3 |
| resumen final: «Cómputo remoto» refleja que los túneles funcionan | **FALLA** | Tu puesto está listo ¦ Esto se comprobó de verdad, no se supuso. ¦ Mac y ¦ Simultaneo-1 en «Mi puesto» ¦ 2 máquinas ¦ Enlace ¦ USB-C · 1.4 ms ¦ Teclado y mouse ¦ cruce sin probar ¦ Monitores ¦ Std-34 compatible · dueño Simultaneo-1 ¦ Cómputo remoto ¦ falta el  |
| paso 4: una máquina sin teclado ni mouse conectados no aparece con «Teclado y mouse» detectado | **FALLA** | hub_here m1/m2 = false,false; filas de la UI: ["Mac ¦ Teclado y mouse","Simultaneo-1 ¦ Teclado y mouse"] |

Capturas: `docs/reports/acceptance/2026-11-18-b/F1-asistente/`

### F2-acomodo

| Comprobación | Resultado | Detalle |
|---|---|---|
| Mac «Derecha», alineación start: llega a las dos máquinas y a tandem.toml | PASA |  |
| Mac «Derecha», alineación center: llega a las dos máquinas y a tandem.toml | PASA |  |
| Mac «Derecha», alineación end: llega a las dos máquinas y a tandem.toml | PASA |  |
| Mac «Arriba», alineación start: llega a las dos máquinas y a tandem.toml | PASA |  |
| Mac «Arriba», alineación center: llega a las dos máquinas y a tandem.toml | PASA |  |
| Mac «Arriba», alineación end: llega a las dos máquinas y a tandem.toml | PASA |  |
| Mac «Abajo», alineación start: llega a las dos máquinas y a tandem.toml | PASA |  |
| Mac «Abajo», alineación center: llega a las dos máquinas y a tandem.toml | PASA |  |
| Mac «Abajo», alineación end: llega a las dos máquinas y a tandem.toml | PASA |  |
| Mac «Izquierda», alineación start: llega a las dos máquinas y a tandem.toml | PASA |  |
| Mac «Izquierda», alineación center: llega a las dos máquinas y a tandem.toml | PASA |  |
| Mac «Izquierda», alineación end: llega a las dos máquinas y a tandem.toml | PASA |  |
| motor: con Mac «Derecha» de la PC el cruce por el borde nuevo pasa (PC→Mac y Mac→PC) | PASA | PC→Mac true, Mac→PC true |
| motor: con Mac «Arriba» de la PC el cruce por el borde nuevo pasa (PC→Mac y Mac→PC) | PASA | PC→Mac true, Mac→PC true |
| motor: el borde viejo («Derecha») ya no cruza | PASA |  |
| motor: con Mac «Abajo» de la PC el cruce por el borde nuevo pasa (PC→Mac y Mac→PC) | PASA | PC→Mac true, Mac→PC true |
| motor: el borde viejo («Arriba») ya no cruza | PASA |  |
| motor: con Mac «Izquierda» de la PC el cruce por el borde nuevo pasa (PC→Mac y Mac→PC) | PASA | PC→Mac true, Mac→PC true |
| motor: el borde viejo («Abajo») ya no cruza | PASA |  |
| motor: el borde viejo («Izquierda») ya no cruza | PASA |  |

Capturas: `docs/reports/acceptance/2026-11-18-b/F2-acomodo/`

### F3-escenas

| Comprobación | Resultado | Detalle |
|---|---|---|
| crear escena: llega a tandem.toml de las dos máquinas | PASA | ["id = \"prueba-b\"\nname = \"Prueba B\"\nicon = \"puesto\"\nmonitors = "] |
| crear escena: vuelve a Monitores | PASA | #/monitores |
| crear escena: aparece como tarjeta con su atajo | PASA | Prueba B ¦ Std-34 → Mac ¦ ⌃⌥9 ¦ Aplicar |
| editar escena: el cambio llega a las dos máquinas | PASA | ["id = \"prueba-b\"\nname = \"Prueba B2\"\nicon = \"puesto\"\nmonitors = "] |
| aplicar con éxito: el monitor cambió de verdad (bus del doble) y el progreso lo muestra | PASA | bus={"std":["DisplayPort","Hdmi"],"fail":["DisplayPort","DisplayPort"]} ¦ Cambios en progreso ¦ Todo lo que toca a otra máquina es una transacción: pasos visibles, un tiempo máximo y vuelta atrás si algo falla. ¦ Aplicando la escena «Solo Std» ¦ 0.0 s ¦ Guarda |
| aplicar con éxito: la escena queda «Activa» en la UI de las dos máquinas | PASA | ["arrangement","cables","data","machines","modules","prefs","scenes","screens","shortcuts"] |
| aplicar con éxito: la otra máquina también la ve «Activa» | PASA |  |
| falla inyectada: la UI muestra la vuelta atrás | PASA | Cambios en progreso ¦ Todo lo que toca a otra máquina es una transacción: pasos visibles, un tiempo máximo y vuelta atrás si algo falla. ¦ Aplicando la escena «Prueba B2» ¦ 0.0 s ¦ Guardar cómo está todo ¦ listo ¦ Std-34 → Mac (HDMI) ¦ deshecho ¦ Zfail-27 → Ma |
| falla inyectada: el puesto vuelve a como estaba (monitores en DP, escena anterior) | PASA | {"std":["DisplayPort","DisplayPort"],"fail":["DisplayPort","DisplayPort"]} |
| atajo global de escena (Ctrl+Alt+7 en m1) aplica la escena | PASA | antes Hdmi,Hdmi → después DisplayPort,Hdmi |

Capturas: `docs/reports/acceptance/2026-11-18-b/F3-escenas/`

### F4-monitores

| Comprobación | Resultado | Detalle |
|---|---|---|
| monitor de fábrica se muestra «No compatible» con su razón | PASA |  |
| monitor de fábrica: ninguna escritura DDC en el bus | PASA | Set vistos: 0; total ops 41 |
| se apaga el dueño del monitor: el otro agente se vuelve dueño | PASA | dueño antes a, después b |
| monitor estándar falso: cambiar la entrada desde su tarjeta, con el veredicto del monitor | PASA | bus: Hdmi → DisplayPort; tarjeta: Std-34 ¦ Estándar Tandem ¦ Simultaneo-1 · DP ¦ Simultaneo-1 ¦ DP ¦ Mac ¦ HDMI ¦ 10:46 ¦ Cambio a DP confirmado ¦ 10:46 ¦ Señal detectada ¦ 10:46 ¦ Entrada → DP, con los botones |

Capturas: `docs/reports/acceptance/2026-11-18-b/F4-monitores/`

### F9-config-compartida

| Comprobación | Resultado | Detalle |
|---|---|---|
| renombrar una máquina en A llega a B (agente y tandem.toml) | PASA | [["Escritorio-PC","Mac"],["Escritorio-PC","Mac"]] |
| B muestra el aviso «Cambiado desde … · Deshacer» | PASA | Cambiado desde Mac ¦ Máquinas y escenas |
| «Deshacer» restaura el nombre en las dos máquinas | PASA | {"antes":["PC","Mac"],"despues":[["PC","Mac"],["PC","Mac"]]} |
| cambiar el color de una máquina en A llega a B | PASA | {"antes":["#a99be8","#7aa7e6"],"despues":[["#5fbfb0","#7aa7e6"],["#5fbfb0","#7aa7e6"]]} |
| cambiar el atajo de bloqueo en A llega a B | PASA | ["lock = [\"Pause\"]","lock = [\"Pause\"]"] |
| cambiar una preferencia (Cmd↔Ctrl) en A llega a B | PASA | {"antes":["[prefs] cmd_ctrl = true drag_files = true "],"despues":["[prefs] cmd_ctrl = false drag_files = true ","[prefs] cmd_ctrl = false drag_files = true "]} |
| apagar «Portapapeles compartido» en A lo apaga en las dos | PASA | ["off","off"] |
| cambio simultáneo converge al mismo valor en las dos máquinas | PASA | [["Simultaneo-1","Mac"],["Simultaneo-1","Mac"]] |

Capturas: `docs/reports/acceptance/2026-11-18-b/F9-config-compartida/`

### F6-red

| Comprobación | Resultado | Detalle |
|---|---|---|
| «Medir ancho de banda» da un valor | PASA | Tandem ¦ Puesto ¦ Túneles ¦ Monitores ¦ Acomodo ¦ Red ¦ MÁQUINAS ¦ Mac ¦ PC ¦ Ajustes ¦ Red ¦ Medir ancho de banda ¦ Mac ¦ USB-C ¦ 1.5 ms ¦ Ethernet ¦ 1.3 ms ¦ Wi-Fi ¦ 1.3 ms ¦ PC ¦ Teclado y mouse ¦ — ¦ sin entrada ¦ Ancho de banda ¦ 480 Mbps ¦ por USB-C ¦ La |
| con dos caminos: USB-C principal y Wi-Fi de respaldo | PASA | ms ¦ Pasó de Ethernet a USB-C ¦ Cable USB-C ¦ Principal ¦ Latencia ¦ 1.2 ms ¦ Banda ¦ — ¦ Ethernet ¦ Respaldo ¦ Latencia ¦ 1.3 ms ¦ Banda ¦ — ¦ Wi-Fi ¦ Respaldo ¦ Latencia ¦ 1.4 ms ¦ Banda ¦ — |
| cortar el camino principal: la entrada pasa al siguiente en < 1 s | PASA | 650 ms |
| el evento del cambio de camino es visible en Red | PASA | d ¦ Medir ancho de banda ¦ Simultaneo-1 ¦ USB-C · caído ¦ Ethernet ¦ 1.7 ms ¦ Wi-Fi ¦ 1.6 ms ¦ Mac ¦ Teclado y mouse ¦ — ¦ sin entrada ¦ Ancho de banda ¦ — ¦ sin medir ¦ Latencia · 60 s ¦ objetivo 4 ms ¦ Se desconectó el cable · pasó a Ethernet en 1.3 s ¦ Cabl |
| tras el corte «Medir ancho de banda» mide el camino que lleva la entrada (o explica por qué no) | PASA | jetivo 4 ms ¦ Pico al desconectar el cable · pasó a Ethernet en 1.3 s ¦ Cable USB-C ¦ Caído ¦ Latencia ¦ — ¦ Banda ¦ — ¦ Ethernet ¦ Principal ¦ Latencia ¦ 1.3 ms ¦ Banda ¦ 220 Mbps ¦ Wi-Fi ¦ Respaldo ¦ Latencia ¦ 1.4 ms ¦ Banda ¦ — ¦ Ethernet: 220 Mbps |
| al volver el cable, el camino principal retoma la entrada | PASA | camino ahora: usb4 |

Capturas: `docs/reports/acceptance/2026-11-18-b/F6-red/`

### F5-tuneles-mcp

| Comprobación | Resultado | Detalle |
|---|---|---|
| P15: las herramientas de Claude existen en `tandem mcp` | PASA | machines,resources,tunnels,tunnel_open,tunnel_close,exec,processes,output,kill,files_put,files_get,port_forward,port_stop,bring_back |
| tunnel_open con workspace de git | PASA | {"created":1795020952451,"id":"e7a204e50952d1af","opened_by":{"kind":"Mcp","session":"mcp"},"origin":"a","project":{"name":"proj-demo","root":"/tmp/claude-1000/accB/proj-demo"},"skipped":[".aws/credentials",".env",".npmrc","deploy.pem","id_rsa"],"state":"Open" |
| T-130: la otra máquina no recibe credenciales (.env, .pem, id_rsa, .npmrc, .aws) | PASA | archivos en destino: .git,src,src/lib.rs,src/nuevo.rs ¦ filtrados: [".aws/credentials",".env",".npmrc","deploy.pem","id_rsa"] ¦ aviso: sí |
| T-130: el repo llega sin remotos ni credential.helper | PASA | [core] repositoryformatversion = 0 filemode = true bare = false logallrefupdates = true [user] email = tandem@localhost name = Tandem |
| T-130: exec rechaza variables de entorno que parecen secretos | PASA | "la otra máquina no lo aceptó: la variable «GH_TOKEN» parece un secreto: la otra máquina no recibe credenciales; hace falta permiso explícito en esta llamada (allow_secret_env)" |
| T-130: con allow_secret_env pasa solo lo pedido; GH_TOKEN y SSH_AUTH_SOCK del origen nunca | PASA | "GH= SSH= AWS=yy\n" |
| 3 procesos corriendo a la vez en el túnel | PASA | corriendo: 3 de [{"Exited":{"code":0}},"Running","Running","Running"] |
| la UI recién abierta muestra los túneles que ya existen (sin esperar a un cambio) | **FALLA** | ustes ¦ Túneles ¦ Sin túneles abiertos ¦ Cómo los abre Claude ¦ Sin túneles abiertos |
| la UI (origen) muestra el proyecto, el túnel y sus procesos en vivo | PASA | ustes ¦ Túneles ¦ 1 abierto · 1 proyecto ¦ Cómo los abre Claude ¦ proj-demo ¦ /tmp/claude-1000/accB/proj-demo ¦ 1/1 ¦ ws-aceptacion ¦ inactivo 0 s ¦ Mac ¦ PC ¦ proj-demo ¦ ws-aceptacion ¦ inactivo 0 s ¦ Mac ¦ sesión de Claude ¦ 0 B ¦ 0 B ¦ PC ¦ Windows ¦ Abier |
| la UI de la otra máquina también muestra el túnel | PASA | ustes ¦ Túneles ¦ 1 abierto · 1 proyecto ¦ Cómo los abre Claude ¦ proj-demo ¦ /tmp/claude-1000/accB/proj-demo ¦ 1/1 ¦ ws-aceptacion ¦ inactivo 2 s ¦ Mac ¦ PC ¦ proj-demo ¦ ws-aceptacion ¦ inactivo 2 s ¦ Mac ¦ sesión de Claude ¦ 0 B ¦ 0 B ¦ PC ¦ Windows ¦ Abier |
| leer la salida de cada proceso, sin mezclarse | PASA | ["p1-1,p1-2,p1-3,p1-4,p1-5,p1-6,","p2-1,p2-2,p2-3,p2-4,p2-5,p2-6,","p3-1,p3-2,p3-3,p3-4,p3-5,p3-6,"] |
| copiar un archivo al túnel (files_put) | PASA | {"bytes":17,"dirs":0,"files":1,"sent":17,"skipped":[]} |
| T-130: files_put de una credencial se omite con aviso | **FALLA** | "la otra máquina: manifiesto no válido: una copia de un archivo trae exactamente una entrada" |
| traer un archivo del túnel (files_get) | PASA | {"bytes":20,"dirs":0,"files":1,"sent":0,"skipped":[]} |
| abrir un puerto (port_forward) y hablar por él | PASA | {"direction":"LocalToRemote","id":1,"local_port":42677,"remote_host":"127.0.0.1","remote_port":41677,"tunnel":"e7a204e50952d1af"} → eco:hola |
| cortar el enlace y recuperarlo sin perder salida (24 líneas seguidas) | PASA | líneas 24; durante el corte: respondió |
| bring_back crea la rama tandem/<nombre> en el proyecto | PASA | {"branch":"tandem/ret","commits":[{"author":"t","id":"1f411dceb7af11e38afe1f308501deb4426c13f4","summary":"desde-b"}],"committed_pending":false} ¦ ramas: tandem/ret |
| bring_back: la rama trae el cambio hecho en la otra máquina | PASA |  |
| la UI abierta (o recargada) después de abrir un túnel lo muestra sin esperar otro cambio | **FALLA** | INAS ¦ Simultaneo-1 ¦ Mac ¦ Ajustes ¦ Túneles ¦ Sin túneles abiertos ¦ Cómo los abre Claude ¦ Sin túneles abiertos |

Capturas: `docs/reports/acceptance/2026-11-18-b/F5-tuneles-mcp/`

### F10-energia

| Comprobación | Resultado | Detalle |
|---|---|---|
| apagar la otra máquina: transacción con pasos en el agente (PowerOff) y estado apagada | PASA | pasos: [[],["{\"step\":\"power_off\",\"machine\":\"a\"}"]] estado {"on":{"a":false,"b":true},"scene":"trabajo"} |
| encender la otra máquina (Wake-on-LAN simulado por Ethernet): paso PowerOn y estado encendida | PASA | pasos: [["{\"step\":\"power_on\",\"machine\":\"a\"}"],["{\"step\":\"power_off\",\"machine\":\"a\"}"]] estado {"on":{"a":true,"b":true},"scene":null} |

Capturas: `docs/reports/acceptance/2026-11-18-b/F10-energia/`

### F11-modulos

| Comprobación | Resultado | Detalle |
|---|---|---|
| encender/apagar un módulo de la otra máquina desde su página (Portapapeles) | PASA | otra: active → off; (esta: active) |
| apagar y volver a encender «Teclado y mouse» de la otra máquina | PASA | active → off → active |
| razones de «no disponible» correctas: Túneles no dice «aún no está» si ya funciona | **FALLA** | Túneles: unavailable · «Aún no está en esta versión de Tandem» (los túneles funcionaron en F5) |
| razones de «no disponible» correctas: Archivos (arrastrar archivos, T-121) | **FALLA** | Archivos: unavailable · «Aún no está en esta versión de Tandem» |

Capturas: `docs/reports/acceptance/2026-11-18-b/F11-modulos/`

### F12-ajustes-paleta

| Comprobación | Resultado | Detalle |
|---|---|---|
| la paleta lista todos los comandos de D15 | PASA | faltan: |
| paleta «Monitores» navega a #/monitores | PASA | #/monitores |
| paleta «Red» navega a #/red | PASA | #/red |
| paleta «Acomodo» navega a #/acomodo | PASA | #/acomodo |
| paleta «Ajustes» navega a #/ajustes | PASA | #/ajustes |
| paleta «Túneles» navega a #/tuneles | PASA | #/tuneles |
| paleta «Puesto» navega a #/puesto | PASA | #/puesto |
| Ctrl+, abre Ajustes | PASA |  |
| paleta «Abrir el asistente» lo abre | PASA |  |
| paleta «Cambiar la otra máquina…» abre el asistente en «Tu puesto» | PASA | 2 Tu puesto |
| paleta «Medir el enlace» pide la medición al agente y la UI da el valor o la razón | PASA | #/ajustes INAS ¦ Simultaneo-1 ¦ Mac ¦ Ajustes ¦ Ajustes ¦ Puesto y máquinas ¦ Atajos ¦ Teclado y mouse ¦ Actualizaciones ¦ Avanzado ¦ Nombre del puesto ¦ Mi puesto ¦ Simultaneo-1 ¦ Mac · esta máquina ¦ Cambiar la otra máquina ¦ Emparejar de nuevo ¦ V |
| «Bloquear aquí» (en la máquina que tiene el teclado): el agente se bloquea y el cursor no cruza; «Quitar el bloqueo» lo suelta | PASA | cruzaba=true locked=true cruza_bloqueado=false cruza_tras_soltar=true |
| la UI indica que el cursor está bloqueado (aviso o marca visible) | **FALLA** | INAS ¦ Simultaneo-1 ¦ Mac ¦ Ajustes ¦ Puesto ¦ Buscar o ejecutar… ¦ Ctrl+K ¦ Simultaneo-1 ¦ Windows ¦ CPU ¦ 12 % ¦ RAM ¦ 2.9/23.5 GB ¦ VRAM ¦ 1/15.9 GB ¦ Mac ¦ macOS ¦ CPU ¦ 12 % ¦ RAM ¦ 2.9/23.5 GB ¦ VRAM ¦ 1/15.9 GB ¦ Simultaneo-1 ¦ Mac ¦ USB-C ¦ — · — ¦ Ent |
| «Bloquear aquí» en la máquina que no manda explica por qué no se puede | PASA | INAS ¦ Simultaneo-1 ¦ Mac ¦ Ajustes ¦ Puesto ¦ Buscar o ejecutar… ¦ Ctrl+K ¦ Simultaneo-1 ¦ Windows ¦ CPU ¦ 11 % ¦ RAM ¦ 2.9/23.5 GB ¦ VRAM ¦ 1/15.9 GB ¦ Mac ¦ macOS ¦ CPU ¦ 11 % ¦ RAM ¦ 2.9/23.5 GB ¦ VRAM ¦ 1/15.9 GB ¦ Simultaneo-1 ¦ Mac ¦ USB-C ¦ — · — ¦ Ent |
| Atajos: cambiar «Mostrar u ocultar Tandem» se guarda en ui.toml | PASA | [shortcuts] toggle_window = "Ctrl+Alt+Digit8" |
| Ajustes → «Salir del puesto» (con confirmación): las dos máquinas olvidan las llaves | **FALLA** | peer m1=unpaired m2=paired (m2 no se desemparejó en 20 s); trust.toml bytes=21,114 |

Capturas: `docs/reports/acceptance/2026-11-18-b/F12-ajustes-paleta/`

### F7-sin-dueno

| Comprobación | Resultado | Detalle |
|---|---|---|
| entrada física en A y luego en B: manda la que se usa | PASA | server tras usar m1=b, m2=a, m1=b (m1=b, m2=a) |
| al cambiar de dueño con una tecla apretada, no queda pegada en la otra máquina | PASA | inyectado antes: ["Key { usage: HidUsage { page: 7, id: 4 }, state: Pressed }"]; después: ["Key { usage: HidUsage { page: 7, id: 4 }, state: Released }","Key { usage: HidUsage { page: 7, id: 4 }, state: Pressed }"] |
| al caer el enlace con una tecla apretada, la otra máquina la suelta | PASA | eventos nuevos tras el corte: ["Key { usage: HidUsage { page: 7, id: 5 }, state: Released }"] |

Capturas: `docs/reports/acceptance/2026-11-18-b/F7-sin-dueno/`

### F8-portapapeles

| Comprobación | Resultado | Detalle |
|---|---|---|
| copiar texto en A→B y pegarlo en la otra | PASA | {"image":null,"text":"texto-A→B","writes":1} |
| copiar texto en B→A y pegarlo en la otra | PASA | {"image":null,"text":"texto-B→A","writes":1} |
| copiar una imagen en A y pegarla en B | PASA | {"image":[4,3],"text":null,"writes":2} |

Capturas: `docs/reports/acceptance/2026-11-18-b/F8-portapapeles/`

### F13-estados-dificiles

| Comprobación | Resultado | Detalle |
|---|---|---|
| permisos revocados: la UI lo dice y ofrece cómo concederlos | PASA | INAS ¦ Simultaneo-1 ¦ Mac ¦ Ajustes ¦ Puesto ¦ Buscar o ejecutar… ¦ Ctrl+K ¦ Faltan permisos en esta máquina ¦ Revisar permisos |
| permisos concedidos: vuelve el Puesto normal | PASA |  |
| enlace caído (la otra sigue encendida): la UI dice «Sin enlace con …» y ofrece volver a probar | PASA | INAS ¦ Simultaneo-1 ¦ Mac ¦ Ajustes ¦ Puesto ¦ Buscar o ejecutar… ¦ Ctrl+K ¦ Sin enlace con Simultaneo-1 ¦ Probar de nuevo |
| enlace vuelve: el Puesto vuelve solo | PASA |  |
| otra máquina perdida: estado difícil correcto con su acción | PASA | INAS ¦ Simultaneo-1 ¦ Mac ¦ Ajustes ¦ Puesto ¦ Buscar o ejecutar… ¦ Ctrl+K ¦ Simultaneo-1 está apagada ¦ Encender Simultaneo-1 |
| la otra vuelve: el Puesto vuelve solo | PASA | INAS ¦ Simultaneo-1 ¦ Mac ¦ Ajustes ¦ Puesto ¦ Buscar o ejecutar… ¦ Ctrl+K ¦ Simultaneo-1 ¦ Windows ¦ CPU ¦ 8 % ¦ RAM ¦ 2.5/23.5 GB ¦ VRAM ¦ |
| sin agente: la UI explica que tandemd no corre | PASA | tes ¦ Puesto ¦ Buscar o ejecutar… ¦ Ctrl+K ¦ tandemd no está corriendo: no hay nadie escuchando en la IPC local |
| al volver el agente, la UI reconecta sola | PASA | INAS ¦ Simultaneo-1 ¦ Mac ¦ Ajustes ¦ Puesto ¦ Buscar o ejecutar… ¦ Ctrl+K ¦ Simultaneo-1 ¦ Windows ¦ CPU ¦ 9 % ¦ RAM ¦ 2.7/23.5 GB ¦ VRAM ¦ |

Capturas: `docs/reports/acceptance/2026-11-18-b/F13-estados-dificiles/`

### F13-actualizaciones

| Comprobación | Resultado | Detalle |
|---|---|---|
| muestra las versiones de las dos máquinas | PASA | Tandem 0.1.0 ¦ Todas las máquinas del puesto en la misma versión. ¦ Buscar ahora ¦ Actualizar solo ¦ Origen ¦ GitHub Releases · sin firma |
| buscar contra un latest.json de prueba encuentra la versión nueva | PASA | Tandem 0.1.0 ¦ Todas las máquinas del puesto en la misma versión. ¦ Buscar de nuevo ¦ Hay una versión nueva: 0.2.0 ¦ Se instala en todas las máquinas, una a la vez, sin cortar el teclado. ¦ Actualizar todas ¦ Actualizar solo ¦ Origen ¦ GitHub Releases · sin fi |
| «Actualizar todas»: primero le pide a la otra máquina por tandemd (plan de las dos) | PASA | UI: Tandem 0.1.0 ¦ Todas las máquinas del puesto en la misma versión. ¦ Buscar de nuevo ¦ Hay una versión nueva: 0.2.0 ¦ Se instala en todas las máquinas, una a la vez, sin cortar el teclado. ¦ Actualizar todas ¦ Actualizar ¦ agente B: [] ¦ B.update={"auto":tr |
| si la otra no vuelve con la versión nueva, esta no se actualiza y la UI lo explica (vuelta atrás) | PASA | Tandem 0.1.0 ¦ Todas las máquinas del puesto en la misma versión. ¦ Buscar de nuevo ¦ Hay una versión nueva: 0.2.0 ¦ Se instala en todas las máquinas, una a la vez, sin cortar el teclado. ¦ Actualizar todas ¦ Actualizar solo ¦ Origen ¦ GitHub Releases · sin fi |
| un latest.json inexistente muestra el error al buscar | PASA | Tandem 0.1.0 ¦ Todas las máquinas del puesto en la misma versión. ¦ Buscar de nuevo ¦ Hay una versión nueva: 0.2.0 ¦ Se instala en todas las máquinas, una a la vez, sin cortar el teclado. ¦ Actualizar todas ¦ Actualizar solo ¦ Origen ¦ GitHub Releases · sin fi |
| «Actualizar solo» se guarda en update.toml | PASA | auto = false |

Capturas: `docs/reports/acceptance/2026-11-18-b/F13-actualizaciones/`

### F14-bandeja

| Comprobación | Resultado | Detalle |
|---|---|---|
| la mini escena de la bandeja muestra el estado del puesto | PASA | Mi puesto ¦ USB-C ¦ Simultaneo-1 ¦ 0 túneles ¦ Mac ¦ macOS ¦ Escenas ¦ Trabajo ¦ Solo Simultaneo-1 ¦ Solo Mac ¦ Prueba B2 ¦ Solo Std ¦ Std a PC ¦ Abrir |
| aplicar una escena desde la bandeja la aplica en el agente | PASA | Solo Std: trabajo → solo-std |
| «Abrir Tandem» y «Salir» de la bandeja llegan al núcleo de la app (ui_show_main / ui_quit) | PASA | ["ui_show_main","ui_quit"] |

Capturas: `docs/reports/acceptance/2026-11-18-b/F14-bandeja/`

### P6-prueba-de-cruce-demo

| Comprobación | Resultado | Detalle |
|---|---|---|
| demo: estado «cruzó bien» de la prueba de cruce | PASA | í ¦ 34" · PC-Escritorio ¦ el cursor llegó aquí ¦ Cruzó bien ¦ Entró a PC-Escritorio a la altura correcta (y = 612 de 1440). Latencia del cruce: 0.4 ms. ¦ Probar otra vez ¦ Listo |
| demo: estado «borde equivocado» de la prueba de cruce | PASA | í ¦ 34" · PC-Escritorio ¦ esperando el cursor ¦ Cruzó por otro borde ¦ Cruzó por arriba, no por la derecha: el acomodo no coincide con tu escritorio. Muévelo en el acomodo y prueba de nuevo. ¦ Editar |
| demo: estado «tiempo agotado» de la prueba de cruce | PASA | í ¦ 34" · PC-Escritorio ¦ esperando el cursor ¦ No cruzó en 20 s ¦ Quizá el 34" está en otro lado de tu escritorio. Muévelo en el acomodo y prueba de nuevo. ¦ Editar acomodo ¦ Probar otra vez |
| demo: estado «esperando» de la prueba de cruce | PASA | í ¦ 34" · PC-Escritorio ¦ el cursor llegó aquí ¦ Cruzó bien ¦ Entró a PC-Escritorio a la altura correcta (y = 612 de 1440). Latencia del cruce: 0.4 ms. ¦ Probar otra vez ¦ Listo |

Capturas: `docs/reports/acceptance/2026-11-18-b/P6-prueba-de-cruce-demo/`

### G8-reduced-motion

| Comprobación | Resultado | Detalle |
|---|---|---|
| con prefers-reduced-motion: reduce no hay animaciones ni transiciones | PASA |  |

Capturas: `docs/reports/acceptance/2026-11-18-b/G8-reduced-motion/`

### G7-animaciones

| Comprobación | Resultado | Detalle |
|---|---|---|
| con movimiento normal hay entradas/transiciones animadas en el Puesto | PASA | 39 elementos animados |

Capturas: `docs/reports/acceptance/2026-11-18-b/G7-animaciones/`

### F2-prueba-de-cruce-en-vivo

| Comprobación | Resultado | Detalle |
|---|---|---|
| Acomodo: «Probar cruce» está disponible con el agente real | **FALLA** | el botón está deshabilitado (store.can('crossing') siempre falso en LiveSource) |
| prueba de cruce con un CursorSample real del motor: se puede completar | **FALLA** | sor está aquí ¦ Simultaneo-1 ¦ esperando el cursor ¦ No se puede probar el cruce ¦ El agente todavía no informa el cruce del cursor ¦ Editar acomodo |
| el agente sí acepta watch_cursor (lo que falta es la UI) | PASA | {"kind":"done"} |

Capturas: `docs/reports/acceptance/2026-11-18-b/F2-prueba-de-cruce-en-vivo/`

### G6-botones-con-efecto

| Comprobación | Resultado | Detalle |
|---|---|---|
| recargar la ventana en Red deja «Medir ancho de banda» habilitado | **FALLA** | queda deshabilitado para siempre (can() no es reactivo) |
| cada botón visible de las pantallas en vivo produce un efecto observable | **FALLA** | 81 de 92 botones tienen efecto; de los 11 sin efecto, 10 son opciones ya seleccionadas (pestañas, entrada del monitor, pantalla, color) o el atajo de la paleta, deshabilitado por diseño; el único botón que no hace nada real es «Probar cruce» de Acomodo (deshab |

Capturas: `docs/reports/acceptance/2026-11-18-b/G6-botones-con-efecto/`

### P10-avisos

| Comprobación | Resultado | Detalle |
|---|---|---|
| una copia de archivos de un túnel muestra un aviso con progreso en la UI | PASA | avisos vistos: ["Enviando «grande.bin» a Simultaneo-1 ¦ 0 % · 0 KB de 63 MB","Enviando «grande.bin» a Simultaneo-1 ¦ 3 % · 2 MB de 63 MB"] |

Capturas: `docs/reports/acceptance/2026-11-18-b/P10-avisos/`

### G2-sin-Mac-PC-fijos

| Comprobación | Resultado | Detalle |
|---|---|---|
| par Windows+Windows: ninguna pantalla nombra el otro sistema ni usa atajos del otro sistema | PASA |  |
| par macOS+macOS: ninguna pantalla nombra el otro sistema ni usa atajos del otro sistema | PASA |  |

Capturas: `docs/reports/acceptance/2026-11-18-b/G2-sin-Mac-PC-fijos/`

### DEMO-barrido

| Comprobación | Resultado | Detalle |
|---|---|---|
| modo demo: ninguna consola con errores, ningún {{ undefined NaN ni inglés en todas las pantallas y variantes | PASA |  |

Capturas: `docs/reports/acceptance/2026-11-18-b/DEMO-barrido/`

## Otras observaciones (sin tarea)

- Los avisos de «Cambiado desde …» no se cierran solos (piden decisión) y tapan controles de la esquina superior derecha mientras no se descartan; tras «Deshacer», el aviso que recibe la otra máquina nombra a la que cambió con su nombre anterior.
- «Aplicar» una escena cuyos monitores no están presentes no dice nada (no hay aviso de que no hizo nada).
- Escenas (F3): en 5 corridas, 4 pasaron completas; en una la primera aplicación tras agregar los monitores falló con «no se sabe qué máquina muestra (falta la entrada real o a qué entrada va cada máquina)» (el agente aún no había leído la entrada o aprendido los cables) y en otra la aplicación fue bien pero la pantalla de progreso no llegó a mostrarse (la transacción terminó antes de abrirla y volvió a Monitores). No se redujeron a una causa; si reaparecen, merecen su propia tarea.
- El estado «enlace caído» se resolvió como «PC está apagada» cuando se cortaron los tres caminos más de `pc_lost_after_secs` (2 s en la prueba); con el valor por omisión podría verse distinto: no se probó el rango.
- «Sin agente» no tiene botón «Reintentar»: reconecta solo cada 2 s (`agent_retry` existe y la UI no lo usa).
