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
