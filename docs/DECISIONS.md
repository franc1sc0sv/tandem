# Decisiones

Registro de decisiones de producto y arquitectura. Lo de aquí manda sobre cualquier tarea; `DESIGN.md` lo desarrolla.
Cada entrada lleva fecha y quién decidió: **usuario** (respuesta explícita) o **agente** (decidido sin el usuario, a revisar).

Los agentes que trabajan solos (por ejemplo de noche) **no preguntan**: buscan la respuesta en este archivo, luego en `DESIGN.md`, luego en los mockups (`mockups/v03/`). Si nada la resuelve, deciden lo más simple que respete todo lo anterior y agregan una entrada al final de la sección «Decididas por agentes», con la razón.

## v0.3 (grilling del 2026-11-16)

### Producto

| # | Decisión | Quién |
|---|---|---|
| D1 | **Exactamente dos máquinas, de igual a igual (P2P).** No hay tercera máquina ni «agregar máquina»; sí se puede **cambiar** la otra (volver a emparejar). | usuario |
| D2 | **Nada fijo a Mac o PC.** Cada lado puede ser macOS o Windows; el código no supone qué sistema hay en cada lado. Lo que una máquina sabe hacer sale de sus **capacidades** anunciadas. | usuario |
| D3 | **Módulos compilados por sistema + capacidades.** Cada módulo existe solo donde compila; en ejecución anuncia si está activo, apagado o no disponible, con su razón. | usuario |
| D4 | **Toda la configuración desde la UI**, sincronizada entre las dos. `tandem.toml` queda como almacenamiento, no como interfaz. | usuario |
| D5 | **Conflictos de configuración: gana el último que guarda.** La otra máquina muestra un aviso con «Deshacer» (historial corto). Solo hay un usuario: no se pregunta. | agente (el usuario dijo que no pasará) |
| D6 | **Escenas en lugar de modos.** Una escena guarda qué muestra cada monitor y el acomodo; se aplica como transacción. | usuario |
| D7 | **Teclado y mouse sin dueño:** funcionan desde cualquier máquina y Tandem sigue al que se está usando. No hay «Teclado aquí» en la UI. | usuario |
| D8 | **Túneles:** Claude abre túneles entre las dos máquinas por **MCP + CLI** y dentro hace lo que sea: varios procesos, archivos, puertos, workspaces de git que vuelven como rama. Sobreviven a la caída del enlace. Se ejecutan con los máximos privilegios que tenga el agente. Entornos detectados (shells, WSL, Docker, toolchains). | usuario |
| D9 | **La UI de túneles es de solo lectura:** los maneja Claude; la app muestra proyectos, túneles, procesos que están corriendo y su salida. | usuario |
| D10 | **Nada específico de un equipo** (nada de «la RTX 5080»): todo sale del escaneo de la máquina. | usuario |
| D11 | **Monitores: un solo estándar.** Se controla un monitor si habla el protocolo del firmware propio (el del 34"); si no, aparece como «no compatible» hasta que tenga su parche. Sin perfiles de fábrica ni adivinar. | usuario |
| D12 | **El control de monitores es Rust, en un crate del repo `monitor-firmware`**, usado por los dos agentes (mismo código en macOS y Windows). | usuario |
| D13 | **Un dueño por monitor, elegido solo:** la máquina con canal DDC funcionando lo lee y lo controla; la otra pide por el plano de control. Si el dueño se va, se elige otro. | usuario |
| D14 | **Módulo nuevo: arrastrar archivos entre máquinas** al cruzar el borde. Nada de audio ni notificaciones por ahora. | usuario |
| D15 | **Paleta ⌘K / Ctrl+K: solo comandos** de Tandem (sin lenguaje natural). | usuario |
| D16 | **Navegación con barra lateral** (Puesto, Túneles, Monitores, Acomodo, Red, una página por máquina, Ajustes). | usuario |
| D17 | **Color por máquina:** se asigna solo al emparejar, editable. | usuario |
| D18 | **Acomodo:** la otra máquina se coloca en **cualquiera de los 4 lados** con alineación (inicio, centro, fin); sin X/Y a mano. Mockup `V3-Acomodo`. | usuario |
| D19 | **Release sin firmar + actualización automática** desde GitHub Releases. Sin extras de energía nuevos. | usuario |
| D20 | **Orden:** cimientos → entrada → túneles → monitores → UI viva → USB-C → release. | usuario |

### Arquitectura (decididas por el agente al planear, 2026-11-16)

| # | Decisión | Razón |
|---|---|---|
| A1 | **`Machine::{A, B}` en lugar de `Machine::{Mac, Pc}`.** Los dos lados se ordenan por la clave pública Ed25519 (la menor es `A`), así las dos máquinas coinciden sin coordinarse. El sistema, el nombre y el color de cada lado vienen de su anuncio. | D1 + D2 con el menor cambio: el código ya es de dos lados (`machine.other()`); solo se quita el supuesto de sistema. |
| A2 | **Los datos pueden ir por Wi-Fi.** Se elimina «el Wi-Fi nunca lleva datos». La entrada siempre tiene prioridad: los datos (túneles, archivos) van en streams de prioridad baja y con tope de ancho de banda en Wi-Fi para no meter latencia a teclado y mouse. | D8 pide túneles robustos por cualquier red, y hoy no hay cable USB-C funcionando (ver A6). |
| A3 | **UI nueva en Svelte 5 + Vite** (dentro de Tauri 2). La lógica pura que ya existe en TypeScript se conserva y se prueba con vitest. | El rediseño tiene ~20 pantallas con estado y animaciones; el DOM a mano no escala. Svelte trae transiciones y es liviano. |
| A4 | **Tokens de diseño de los mockups v0.3** (`mockups/v03/`): colores, sistema de botones, íconos de línea, animaciones con `prefers-reduced-motion`. Los mockups son la referencia visual; cada pantalla se compara con su mockup por capturas. | Fuente única de verdad visual. |
| A5 | **El crate de monitores se llama `monitor-ddc`** y vive en `monitor-firmware/crates/monitor-ddc`. Tandem lo usa como dependencia git fijada por commit. El repo `monitor-firmware` usa la rama de integración `develop` con las mismas reglas que Tandem (sin tocar `main`). | D12. |
| A6 | **El USB-C no da enlace hoy (spike S5, 2026-11-16).** Con el cable de 10 Gbps las dos puntas negocian por PD y la PC anuncia USB4, pero el enlace USB4 nunca entrena; con uno de USB 2.0 es imposible. El usuario no quiere actualizar el BIOS (F3, firmware ASM4242 de 2024-05). Se probará un cable Thunderbolt 4 / USB4 de 40 Gbps. Mientras tanto el enlace diario es Wi-Fi (y Ethernet si se agrega). T-094 se programa y se prueba con simuladores; lo físico queda en su lista de hardware. | Hechos medidos con `ioreg` en la Mac y PnP/registro en Windows. |
| A7 | **Privilegios de los túneles:** el proceso corre con los privilegios del agente. En Windows el agente arranca elevado (tarea programada «con los privilegios más altos», sin servicio SYSTEM mientras T-027 espere al usuario); en macOS corre como el usuario. | D8 («siempre como admin») sin depender de T-027. |
| A8 | **Pruebas de hardware nocturnas permitidas**, con vuelta automática obligatoria en todo lo que cambie una entrada de monitor (un vigilante independiente la restaura). La Mac se usa por `ssh mac-runner` (Wi-Fi). | El usuario lo autorizó. |
| A9 | **Mockups con tres máquinas** (`F-AgregarHost`, la MacBook Air de ejemplo) quedan fuera por D1: en su lugar, «Cambiar la otra máquina» (emparejar de nuevo). | D1. |

## Decididas por agentes (a revisar por el usuario)

Formato: `- AAAA-MM-DD · T-xxx · decisión · razón`.
- 2026-11-16 · plan · Playwright (Chromium headless) como herramienta de pruebas del repo para capturas y flujos (T-125, T-112, `acceptance`) · es código de pruebas, no navegación del agente; el usuario prefiere Claude in Chrome para tareas de navegador interactivas.
- 2026-11-16 · plan · Puerta final = agente `acceptance` con `docs/ACCEPTANCE.md` · pedido del usuario: nada se da por terminado sin su FINAL_OK.
- 2026-11-17 · T-125 · Las capturas de referencia de los mockups se sacan sin red: Inter y JetBrains Mono salen de `@fontsource/*` (no de Google Fonts), con DejaVu Sans para los símbolos que esos subconjuntos no traen (⌃ ⌥ ⌘ → ↵ ≈ ✓ ➜), en Chromium headless de Linux, con las animaciones finitas terminadas y las infinitas quitadas (estilo base) · dos corridas tienen que dar el mismo PNG para que la comparación con la app signifique algo; la app (T-112) debe capturarse igual (`launchBrowser` + `capture` de `@tandem/mockup-render/harness`) y empaquetar las mismas fuentes.
- 2026-11-17 · T-125 · `compare()` da, además de la similitud global (umbral ≥ 0.99 de DESIGN §12), `worstTile`: el cuadro de 64×64 con menos parecido (rejilla solapada, paso 32, para que un cambio que cruza cuadros no se diluya) · entre dos estados del mismo mockup suele cambiar menos del 1 % de los píxeles (cambiar la entrada en `V3-Monitores` = 99.84 %), así que la similitud global sola aprueba una pantalla en el estado equivocado; las pruebas de pantalla (T-113…T-120) deberían mirar también el peor cuadro.
- 2026-11-17 · T-125 · `F-AgregarHost` también se renderiza y se captura, aunque D1 lo deja fuera · es uno de los 23 mockups del criterio de T-125; queda como referencia histórica, no como pantalla a igualar (A9).
- 2026-11-17 · T-125 · En el runtime propio de los mockups, `{{x}}` que da `undefined`, `null` o un booleano se muestra vacío en el texto, y se avisa en `window.__dc.missing` (la prueba de los mockups exige que no haya ninguno) · lo más simple y parecido a React; ningún mockup v0.3 depende de otra cosa.
- 2026-11-17 · T-125 · Chromium se abre con `--disable-partial-raster` para las capturas (mockups y app), y solo se exige que sean idénticas bit a bit en la misma máquina; entre WSL y CI hoy también lo son (sha1 de las 72 del PR #70), pero CI las compara con la tolerancia de `compare()` (≥ 0.999) · con «partial raster» el PNG dependía de los cuadros intermedios de una transición (±1 de color en la pantalla con `opacity: .4` de `V3-Acomodo`, hasta ~10 600 píxeles y 12–13 capturas distintas entre dos corridas); otra imagen del runner o de Chromium podría rasterizar distinto.
- 2026-11-17 · T-097 · El lado de esta máquina se guarda en `tandem.toml` (`machine = "a"`) como lo último resuelto; sin par vale `A`. Al emparejar, si las claves dicen que es `B`, el agente cambia los lados en caliente (estado, puesto, teclado, archivo) y reinicia solo el motor de entrada · un lado tiene que existir antes de emparejar y el emparejamiento ocurre una sola vez; reiniciar el proceso no es fiable en Windows (sin supervisor).
- 2026-11-17 · T-097 · Emparejamiento simétrico: las dos máquinas esperan en su puerto de emparejamiento y buscan a la otra (enlaces fijos con su puerto + 1, el `pairing_port` propio y mDNS); vale solo la conexión que abre `A`, y `B` deja de buscar al descubrir que es `B` · los papeles fijos Mac-busca/PC-espera chocaban con D2 (dos Windows).
- 2026-11-17 · T-097 · Por omisión el teclado arranca en `A` y el portapapeles lo abre `A`; ante un empate de hubs, en `B` manda el servidor de `A` · son reglas que las dos máquinas conocen sin hablarse; antes eran «la Mac».
- 2026-11-17 · T-097 · `MachineInfo` de las dos máquinas se anota en el puesto al recibir el anuncio de capacidades de la otra, que se manda después del puesto: un solo cambio de versión y desde la versión más alta conocida · anotarlo al arrancar subía la versión en una instalación limpia y podía pisar un cambio real de la otra por el desempate de origen.
- 2026-11-17 · T-097 · En Linux (solo pruebas y simulación) `Os::local()` es Windows; las pruebas eligen el sistema con `AgentConfig::os` (no se guarda en el archivo) · el enum de sistemas es `{MacOs, Windows}` (tarea) y Linux no es plataforma de Tandem.
- 2026-11-17 · T-097 · Los ejemplos de `docs/examples/` pasan al formato de lados A/B; los de v0.2 quedan en `docs/examples/v0.2/` como referencia y para las pruebas de migración.
- 2026-11-17 · T-097 · Al cambiar de lado, el acomodo se invierte solo si ya se sincronizó y no es el de omisión; si no, queda «A a la izquierda de B» con los lados nuevos · invertir el de omisión dejaba a dos instalaciones nuevas con A a la derecha (revisión del PR #68).
- 2026-11-17 · T-097 · La UI vieja (TypeScript) se adapta a A/B en el PR de T-097 (alcance de T-126) en vez de esperar a T-112 · pedido de la revisión: `develop` no puede quedar con la UI rota.
- 2026-11-17 · T-112 · Svelte 5 con Vite 7 y `@sveltejs/vite-plugin-svelte` 6 (no Vite 8) · el plugin 7 exige Vite 8 y vitest 5; quedarse en Vite 7 / vitest 4 es el menor cambio y no toca Tauri.
- 2026-11-17 · T-112 · El modo demo (`?demo`) solo está en los builds que no son de producción (`__DEMO__`): `pnpm dev`, las pruebas y `pnpm build:demo`, que usa el arnés de capturas · el build de Tauri no debe traer datos de ejemplo (D10), y el arnés captura la misma app compilada.
- 2026-11-17 · T-112 · Un solo `Button` con alto fijo por tamaño (`sm` 32, `md` 36, `lg` 40, `xl` 44 px) y variables CSS (`--btn-h`, `--btn-px`, `--btn-fs`, `--btn-r`) · cada mockup usa otra altura de botón con la misma hoja; así cada pantalla calza con su mockup sin un segundo sistema de botones (G4).
- 2026-11-17 · T-112 · El arnés (`pnpm shots`) tapa en las dos imágenes, antes de comparar, lo que DECISIONS quita del mockup (hoy solo «Agregar máquina», D1), con la razón escrita en `scripts/shots-spec.ts` · ACCEPTANCE permite esas diferencias; sin taparlas, el peor cuadro de la barra lateral queda en 94.3 % solo por esa fila (similitud 99.77 %).
- 2026-11-17 · T-112 · El contador de Túneles en la barra lateral cuenta los túneles activos (con algo corriendo), no los abiertos · es lo que da el «3» de los mockups con los datos de `V3-Computo`.
- 2026-11-17 · T-112 · Con la paleta abierta la app queda al 22 % de opacidad bajo el velo · es como lo dibuja `F-Paleta`; con la app real a opacidad completa el fondo sería más claro que el del mockup.
- 2026-11-17 · T-112 · El ícono de cada máquina sale de su sistema (portátil para macOS, torre para Windows), no del lado · D2; T-115 puede afinarlo con el modelo del equipo cuando el escaneo lo informe.
- 2026-11-17 · T-112 · Bandeja: «Salir» es un comando nuevo `ui_quit` (igual que el menú del ícono); «Bloquear aquí» no se muestra hasta que la IPC tenga la acción (T-120) · G6: nada que parezca botón sin hacer algo real.
- 2026-11-17 · T-112 · El asistente queda como contenedor con los seis pasos de v0.3 (se recorre, sin contenido) hasta T-119; lo abren Ajustes y la paleta · la paleta y Ajustes necesitan dónde abrirlo; el contenido y la verificación de cada paso son de T-119.
- 2026-11-17 · T-112 · Fuentes de la app: solo los subconjuntos latín y latín extendido de `@fontsource/inter` y `@fontsource/jetbrains-mono` (los mismos pesos que piden los mockups) más las caras de DejaVu Sans para ⌃ ⌥ ⌘ → ↵ ≈ ✓ ➜, en el mismo orden que el renderizador · igual dibujo que las capturas de referencia sin empaquetar cirílico, griego ni vietnamita.
- 2026-11-17 · T-112 · La UI valida la forma de cada foto del agente (`checkSnapshot`) y, si no la entiende, muestra «sin agente» con la razón · un agente de otra versión no puede dejar la ventana colgada en «conectando» (revisión de #71).
- 2026-11-17 · T-112 · Los pasos «Teclado y mouse» y «Acomodo» del asistente aparecen deshabilitados en la lista y «Continuar» los salta hasta T-119 · G6: ningún paso sin contenido; emparejar y permisos ya funcionan con la lógica existente.
