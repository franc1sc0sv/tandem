# Tandem — Documento de diseño

> Estado: **v0.3** (2026-11-16). Fuente de verdad del proyecto junto con [DECISIONS.md](DECISIONS.md) (que manda si chocan).
> Referencia visual: [`../mockups/v03/`](../mockups/v03/) (ver [`../mockups/README.md`](../mockups/README.md)).
>
> **Cambio de rumbo v0.3:** dos máquinas de igual a igual sin sistema fijo por lado; túneles para que Claude use la otra máquina; monitores por un estándar único (el firmware propio) con un crate Rust compartido; escenas en lugar de modos; toda la configuración desde una UI nueva con barra lateral.

## 1. Qué es

Tandem convierte **dos computadoras** (hoy una MacBook Pro y una PC Windows, pero cualquiera de las dos puede ser macOS o Windows) en un solo puesto de trabajo:

- **Teclado y mouse compartidos** con motor propio, sin dueño: se usan desde cualquiera de las dos y Tandem sigue al que se está usando; el cursor cruza por el borde que dice el acomodo.
- **Portapapeles compartido** y **arrastrar archivos** de una pantalla a la otra.
- **Túneles:** Claude (por MCP o CLI) abre un túnel hacia la otra máquina y dentro corre procesos, copia archivos, abre puertos y trabaja en un workspace de git que vuelve como rama.
- **Monitores:** cada monitor compatible con el estándar (firmware propio) se controla desde cualquiera de las dos; **escenas** guardan qué muestra cada monitor y el acomodo.
- **Energía:** encender y apagar la otra máquina.
- Un **enlace propio** cifrado que usa el mejor camino disponible (USB-C, Ethernet, Wi-Fi).

La UI es **hermosa, visual y animada**: entender el estado de un vistazo, leer poco, íconos en todo.

## 2. Hardware (de este usuario; el código no lo supone)

| | Máquina 1 | Máquina 2 |
|---|---|---|
| Equipo | MacBook Pro (Apple Silicon, macOS 27) | Gigabyte X870 GAMING X WIFI7 · Ryzen 7 9800X3D · RTX 5080 · Windows 11 25H2 |
| Red | Wi-Fi; 3× Thunderbolt/USB4 | Wi-Fi 7 · Realtek 2.5GbE · USB4 (ASMedia ASM4242, firmware 0.1a, BIOS F3) |
| Video | HDMI → monitores | DisplayPort → monitores |

| Monitor | Modelo | Estado |
|---|---|---|
| 34" | Samsung Odyssey G5 **LC34G55T** (MStar MST9U) | Firmware propio v3.1 (protocolo 2): reporta su entrada real y da veredicto de cada cambio. **Compatible.** |
| 32" | Samsung Odyssey G5 «FALCON» (Novatek NT68500) | **Sin servicio** desde 2026-11-16 (firmware dañado, se recupera por SPI). Al volver con firmware de fábrica: **no compatible** hasta su parche. |

- **Cable USB-C (spike S5, 2026-11-16):** con el cable de 10 Gbps las dos puntas negocian por PD y la PC anuncia USB4, pero el enlace USB4 no entrena; con un cable USB 2.0 es imposible. Se probará un cable Thunderbolt 4 / USB4 de 40 Gbps; no se actualiza el BIOS (decisión del usuario). Detalle en [research/usb4-link.md](research/usb4-link.md).
- Acceso para pruebas: la PC desde WSL (`powershell.exe`); la Mac por `ssh mac-runner` (Wi-Fi). Ver [dev/hardware.md](dev/hardware.md).

## 3. Principios

1. **Una sola app, todo en Rust** (agente) + Tauri 2 con Svelte 5 (UI).
2. **La entrada es lo primero.** Nada (túneles, archivos, UI, monitores) puede frenar el camino captura → enlace → inyección. Cada cambio en ese camino se mide (§7.1).
3. **Dos lados iguales.** `Machine::{A, B}`, ordenados por clave pública. Ningún módulo pregunta «¿es la Mac?»: pregunta por **capacidades** (§4.3).
4. **Cada acción con efecto en la otra máquina es una transacción**: pasos, tiempos máximos y vuelta atrás (energía, escenas, cambios de entrada).
5. **Nada de adivinar.** Un monitor se controla solo si dice la verdad (firmware estándar). Un estado desconocido se muestra como desconocido.
6. **Configuración desde la UI**, sincronizada; el archivo es almacenamiento.
7. **Seguro por defecto.** Emparejamiento único con código, todo cifrado y con autenticación mutua.
8. **Nada sin sentido en la UI.** Cada elemento tiene un propósito y funciona de verdad; si algo no está disponible se dice por qué. Sin textos de relleno ni explicaciones de más.

## 4. Arquitectura

```
┌──────────── Máquina A ─────────────┐        ┌──────────── Máquina B ─────────────┐
│ Tandem UI (Tauri + Svelte)         │        │ Tandem UI                          │
│        │ IPC local                 │        │        │ IPC local                 │
│ tandemd                            │◄──────►│ tandemd                            │
│  ├ link    (caminos, salud, QoS)   │ QUIC   │  ├ link                            │
│  ├ input   (captura / inyección)   │ cifrado│  ├ input                           │
│  ├ clipboard + files (arrastrar)   │        │  ├ clipboard + files               │
│  ├ tunnels (procesos, archivos,    │        │  ├ tunnels                         │
│  │          puertos, workspaces)   │        │  │                                 │
│  ├ monitors (monitor-ddc)          │        │  ├ monitors                        │
│  ├ desk    (config compartida,     │        │  ├ desk                            │
│  │          escenas, acomodo)      │        │  │                                 │
│  ├ power, peripherals, resources   │        │  ├ …                               │
│ tandem mcp / tandem (CLI) ── IPC ──┘        │                                    │
└────────────────────────────────────┘        └────────────────────────────────────┘
```

### 4.1 Stack

- **Agente `tandemd`:** Rust, edición 2024, tokio. FFI directo a cada sistema.
- **UI:** Tauri 2 + **Svelte 5 + Vite + TypeScript** (A3). Tokens, botones, íconos y animaciones de los mockups v0.3.
- **Protocolo:** mensajes tipados (serde) sobre QUIC (quinn + rustls), identidades Ed25519.
- **Clientes de Claude:** `tandem mcp` (servidor MCP por stdio) y `tandem` (CLI), ambos hablan con el `tandemd` local por IPC.

### 4.2 Workspace

```
crates/
  tandem-core        modelo de dominio: Machine {A,B}, config compartida, escenas, transacciones
  tandem-proto       mensajes entre agentes y con la UI/CLI, versionado
  tandem-link        descubrimiento, caminos, salud, prioridad y QoS
  tandem-input       motor de teclado y mouse
  tandem-clipboard   portapapeles
  tandem-files       arrastrar archivos y transferencias (fragmentos, hash, reanudar)
  tandem-tunnel      túneles: sesiones, procesos, archivos, puertos, workspaces de git
  tandem-env         entornos detectados (shells, WSL, Docker, toolchains) y recursos en vivo
  tandem-monitor     integración de monitor-ddc: dueño, eventos, cambios con veredicto
  tandem-peripherals detección de teclado y mouse (actividad)
  tandem-power       Wake-on-LAN, apagado
  tandem-sim         puesto simulado para pruebas
  tandemd            binario del agente + `tandem` (CLI) + `tandem mcp`
apps/
  tandem-ui          Tauri 2 + Svelte 5
tools/
  mockup-render      renderizador local de mockups/v03 para comparar capturas
```

Dependencia externa propia: `monitor-ddc` (repo `monitor-firmware`, A5), fijada por commit.

### 4.3 Capacidades y módulos

- Cada módulo se compila solo donde puede existir (`#[cfg]`) y en ejecución reporta un **estado**: `activo`, `apagado` (por el usuario), `no disponible` (con razón: falta permiso, falta hardware, no compatible).
- `ControlMsg::Capabilities` lleva: sistema, nombre, color, módulos con estado, entornos detectados, recursos y monitores con canal DDC. Se manda al conectar y con cada cambio.
- La UI muestra todos los módulos de cada máquina con su estado y razón (mockup `V3-Maquina`); nada se oculta sin explicar.

### 4.4 Configuración compartida

- Un **documento del puesto** (`Desk`) con secciones: máquinas (nombre, color), acomodo, escenas, atajos, preferencias de teclado/mouse, módulos encendidos. Lleva versión (`desk_rev`) y autor.
- La UI edita por IPC (`UiCommand::EditDesk { patch }`); el agente aplica, sube la versión, guarda en `tandem.toml` y lo manda al otro.
- **Gana el último** (D5). La otra UI muestra «Cambiado desde <máquina>» con **Deshacer** (historial de las últimas 20 versiones).
- Lo local de cada máquina (rutas, permisos, entornos) no se sincroniza.

## 5. Escenas y energía

- **Escena** = qué entrada muestra cada monitor compatible + acomodo de escritorios + (opcional) máquina que debe estar encendida. Ejemplos: «Trabajo» (34" con la PC, MacBook a la izquierda), «Solo MacBook», «Solo PC».
- **Aplicar una escena** es una transacción: encender la máquina si hace falta → cambiar cada monitor (con veredicto del firmware) → aplicar el acomodo → mover la geometría del motor de entrada. Si un paso falla, se vuelve atrás. Mockup `F-Transacciones`.
- Los monitores no compatibles no entran en escenas (se muestran, pero no se pueden asignar).
- **Encender** la otra máquina: Wake-on-LAN desde la que tenga su MAC y un camino por LAN; se espera a su agente (120 s).
- **Apagar** la otra: transacción (soltar teclas, apagado limpio). macOS solo si anuncia que puede.
- **Vigilancia del par:** si ningún camino la ve durante `peer_lost_after_secs`, se la da por apagada y se avisa.

## 6. Red: caminos y prioridades

| Camino | Prioridad | Entrada | Datos (túneles, archivos) |
|---|---|---|---|
| USB-C directo (red USB4 / Thunderbolt) | 1 | ✅ | ✅ |
| Ethernet directo | 2 | ✅ | ✅ |
| Wi-Fi (LAN) | 3 | ✅ | ✅ con tope (A2) |

- **La entrada manda:** teclado y mouse van en datagramas y en un stream de máxima prioridad; los datos van en streams de prioridad baja. En Wi-Fi los datos tienen un tope de ancho de banda configurable (por omisión 50 % del medido) y se pausan si la latencia de la entrada pasa su objetivo.
- **Salud y cambio de camino:** revisión 1 vez por segundo por camino; ante una falla la entrada pasa al siguiente en < 1 s; histéresis al volver (T-065).
- **Métricas por camino** para la pantalla Red: latencia p50/p99 (entrada) y RTT, ancho de banda medido a pedido, historial de 60 s, eventos de cambio («pasó a Wi-Fi en 0.6 s»). Mockup `V3-Red`.
- **USB-C (T-094):** detección automática de la interfaz (Thunderbolt Bridge en macOS; «USB4 P2P Network Adapter» en Windows), direcciones link-local, mDNS solo en esa interfaz, clasificación de cada extremo (vale la peor). Hoy sin enlace físico (§2).
- Wake-on-LAN solo por LAN cableada o Wi-Fi con la máquina en reposo (no por USB-C).

## 7. Teclado y mouse (motor propio)

- **Sin dueño (D7):** las dos máquinas capturan; la que recibe entrada física es la que manda en ese momento («sigue al que usas»). Si llega entrada física a las dos a la vez, gana la que se movió último después de 300 ms de quietud de la otra.
- **Captura:** `CGEventTap` en macOS; hooks de bajo nivel / Raw Input en Windows. Con el foco en la otra máquina, los eventos se tragan y el cursor local se oculta y bloquea.
- **Inyección:** `CGEventPost` en macOS; `SendInput` con scancodes en Windows.
- **Teclas:** código físico canónico (USB HID); respeta la distribución ES/LatAm de cada sistema. Cmd↔Ctrl configurable.
- **Transporte:** movimiento en datagramas no confiables (coalescidos); teclas y clics en stream confiable de máxima prioridad.
- **Geometría: acomodo declarado** (D18). Cada máquina declara sus pantallas (rectángulos en coordenadas nativas del cursor y escala); el acomodo dice en qué lado (arriba, abajo, izquierda, derecha) está el escritorio de la otra y con qué alineación (inicio, centro, fin). El borde de cruce es el tramo compartido. Por omisión, los límites que el sistema ya da al inyectar.
- **Escenas y geometría:** las pantallas que en la escena activa muestran a la otra máquina salen de la geometría.
- **Portapapeles:** texto, HTML e imagen. **Archivos:** arrastrar un archivo al cruzar el borde lo transfiere (`tandem-files`) y lo suelta en el destino (§9).
- **Windows, escritorio seguro** (T-027, necesita al usuario): servicio SYSTEM + helper para UAC y el inicio de sesión.
- Diseño del motor: [research/input-engine.md](research/input-engine.md).

### 7.1 Latencia y fiabilidad

| Camino | p50 | p99 |
|---|---|---|
| USB-C / Ethernet | ≤ 1 ms | ≤ 4 ms |
| Wi-Fi | ≤ 5 ms | ≤ 20 ms |

Medición siempre disponible (marcas de tiempo monotónicas, desfase estimado con los heartbeats), banco en `tandem-sim`, camino caliente sin colas compartidas, y los fallos conocidos cubiertos (hook quitado por Windows, tap desactivado en macOS, teclas pegadas, DPI por pantalla). Detalle en T-093.

## 8. Túneles (Claude usa la otra máquina)

- **Túnel** = sesión entre las dos máquinas abierta por un cliente (Claude por MCP o una persona por CLI), con id, título (el de la sesión de Claude), proyecto (repo/carpeta) y máquina destino. Varios túneles a la vez; varios procesos por túnel.
- **Dentro de un túnel:**
  - `exec`: lanzar procesos (shell elegido de los entornos detectados: zsh/bash, PowerShell, cmd, WSL, Docker), con cwd y variables; varios a la vez.
  - `output`: salida en vivo (stdout/stderr) con búfer circular; `kill`; código de salida.
  - `files_put` / `files_get`: copiar archivos y carpetas (fragmentos con hash, reanudable).
  - `port_forward`: un puerto local que llega a uno de la otra máquina (o al revés).
  - **Workspace de git:** `tunnel_open` con `workspace` copia el repo (rama actual + cambios sin commit, por `git bundle` + parche) a una carpeta de trabajo en la otra máquina; `bring_back` trae los commits hechos allá como una rama `tandem/<túnel>` en el repo de origen.
- **Sobreviven al enlace:** los procesos viven en el agente destino; si el enlace se cae, siguen corriendo y la salida se guarda; al volver, el cliente retoma desde el último byte recibido.
- **Privilegios (A7):** los del agente (elevado en Windows; usuario en macOS).
- **Entornos y recursos (`tandem-env`):** escaneo al arrancar y a pedido: shells, distros de WSL, Docker, git, toolchains (cargo, node, uv/python), GPU (modelo, VRAM, uso por `nvidia-smi` o Metal). Recursos en vivo: CPU, RAM, GPU, disco. Claude elige la máquina con estos datos.
- **MCP (`tandem mcp`):** herramientas `machines`, `resources`, `tunnel_open`, `tunnel_close`, `tunnels`, `exec`, `output`, `kill`, `files_put`, `files_get`, `port_forward`, `bring_back`. Mockup `F-ClaudeMCP`. La CLI `tandem` expone lo mismo.
- **UI (solo lectura, D9):** proyectos agrupados → túneles (título, rama, camino A→B) → procesos que están corriendo y su salida. Mockup `V3-Computo`.

## 9. Archivos: arrastrar entre máquinas

- Al cruzar el borde con un arrastre de archivos en curso (Finder / Explorador), la máquina origen lee las rutas arrastradas, `tandem-files` las transfiere y la destino inicia un arrastre equivalente con los archivos ya copiados en una carpeta temporal; al soltar, el sistema destino hace la copia normal.
- Si el arrastre se cancela, se borra lo transferido. Progreso en la UI (aviso con barra).
- Requiere cruzar con el botón presionado: esta función activa el cruce durante un arrastre de archivos aunque «Arrastrar entre máquinas» esté apagado para lo demás.

## 10. Monitores (estándar único)

- **Estándar:** el protocolo del firmware propio del 34" (MCCS + VCP `0xEA`–`0xEF` eventos y veredictos, `0xEE` estado en vivo, `0x60` con la entrada real). Un monitor es **compatible** si el sondeo (`Get 0xEA`, solo lecturas) lo confirma. Cualquier otro: **no compatible** (se muestra, no se controla). Nunca se mandan códigos de fabricante sin sondeo previo.
- **Crate `monitor-ddc`** (repo `monitor-firmware`, A5): trait `DdcBus` (Get/Set VCP, capacidades, EDID), backends Windows (`dxva2`) y macOS (`IOAVService` en Apple Silicon), doble de prueba con personalidades (estándar, fábrica, desconectado), sondeo, lectura de eventos con cursor, cambio de entrada con veredicto.
- **Dueño por monitor (D13):** cada agente reporta qué monitores ve con canal DDC vivo. El dueño es el que tiene canal; si los dos, el de menor latencia de respuesta; si el dueño se va o pierde el canal, se elige el otro. Solo el dueño lee eventos (el cursor de eventos es global del monitor).
- **Identidad:** `<modelo>-<hash corto del serial del EDID>`; vincula el id de DP y el de HDMI (el EDID cambia por entrada).
- **Cambios de entrada:** transacción con veredicto del firmware (`confirmed` / `failed`), tiempo máximo y vuelta atrás. En pruebas de hardware, **vuelta automática obligatoria**.
- **UI:** monitores con su entrada real, dueño y eventos; selector de entrada con indicador deslizante; los no compatibles en tarjeta propia. Mockup `V3-Monitores`.

## 11. Seguridad

- Emparejamiento con código de 6 dígitos en las dos UIs; identidades Ed25519 en el llavero del sistema.
- Autenticación mutua en toda conexión; el IPC local solo para el usuario actual.
- Túneles: solo los abre un cliente local autenticado por IPC (el MCP y la CLI corren como el usuario); la otra máquina solo ejecuta lo que llega por el enlace autenticado.

## 12. UI

- **Shell:** barra lateral (Puesto, Túneles, Monitores, Acomodo, Red; Máquinas: una página por cada una; Ajustes abajo). Barra de menú / bandeja con mini resumen.
- **Pantallas** (mockup de referencia en `mockups/v03/`):
  | Pantalla | Mockup |
  |---|---|
  | Puesto (inicio): máquinas, enlace vivo, escritorios en vivo a escala real, escenas | `V3-Puesto` |
  | Túneles (solo lectura) | `V3-Computo` |
  | Máquina: recursos (CPU, RAM, GPU, disco), módulos, entornos | `V3-Maquina`, `F-MaquinaMac` |
  | Monitores y escenas | `V3-Monitores`, `F-NuevaEscena` |
  | Acomodo (4 lados + alineación) y prueba de cruce | `V3-Acomodo`, `F-PruebaCruce` |
  | Red | `V3-Red` |
  | Asistente (6 pasos) | `F-Asis1`, `V3-Asistente`, `F-Asis3`…`F-Asis6` |
  | Escena/energía en progreso, avisos | `F-Transacciones`, `F-Avisos` |
  | Ajustes (pestañas con íconos), paleta ⌘K | `F-Ajustes`, `F-Paleta` |
  | Estados difíciles (sin agente, sin la otra máquina…) | `V3-Estados` |
  | Barra de menú / bandeja | `V3-Bandeja` |
- **Paridad con el mockup:** cada pantalla se captura en modo demo y se compara con el render del mockup (`tools/mockup-render`); objetivo ≥ 99 % de parecido visual, con las mismas animaciones y transiciones.
- **Reglas visuales:** oscura y sobria; acento por máquina; un solo sistema de botones (alto fijo, ícono siempre, elevación al pasar, brillo en el primario, foco visible); íconos en todo; animaciones y transiciones en todo, respetando `prefers-reduced-motion`; sin líneas de color sobre las tarjetas; sin textos explicativos de más; escritorios a escala real.
- **Modo demo:** datos simulados para cada pantalla y cada estado, para capturas y pruebas.

## 13. Release

- Instaladores sin firmar: `.dmg` (macOS) y NSIS (Windows), en GitHub Releases al empujar un tag `v*`.
- **Actualización automática:** `tauri-plugin-updater` con firma propia del updater (minisign; no es firma de código) contra GitHub Releases; actualiza la UI y el agente; las dos máquinas se actualizan una a la vez sin cortar el teclado.

## 14. Riesgos y spikes

| # | Spike | Estado |
|---|---|---|
| S5 | Red USB4 Mac↔Windows (ASM4242) | Negativo con cable de 10 Gbps; probar cable TB4/USB4 40 Gbps |
| S6 | Wake-on-LAN desde apagado en la X870 | Pendiente (necesita LAN cableada) |
| S7 | Permisos de macOS | Hecho (T-090) |
| S9 | Escritorio seguro de Windows | Necesita al usuario (T-027) |
| S11 | Latencia real captura → inyección | T-093 |
| S12 | DDC desde la Mac por HDMI (`IOAVService`) | Solo con HDMI activo; por probar |
| S13 | Arrastre de archivos entre sistemas (iniciar un drag sintético en el destino) | T-121 |

## 15. Fases (v0.3)

| Fase | Contenido | Tareas |
|---|---|---|
| H1 Cimientos | Lados A/B, capacidades y módulos, config compartida, escenas | T-097…T-100 |
| H2 Entrada y red | Latencia, datos por cualquier camino, métricas, USB-C | T-093, T-101, T-102, T-094 |
| H3 Túneles | Sesiones, procesos, entornos, archivos y puertos, workspaces, MCP/CLI | T-103…T-108 |
| H4 Monitores | `monitor-ddc`, integración, escenas con monitores | T-109…T-111 |
| H5 UI viva | Base nueva y todas las pantallas, renderizador de mockups | T-112…T-120, T-125 |
| H6 Archivos | Arrastrar entre máquinas | T-121 |
| H7 Release | Instaladores y actualización automática | T-122 |
| QA | Hardware nocturno, aceptación de punta a punta | T-123, T-124 |

## 16. Flujo de desarrollo

- Repo privado `franc1sc0sv/tandem`, integración en `develop` (nunca `main`). CI: Linux, Windows, macOS + UI.
- Trabajo autónomo: [dev/night-run.md](dev/night-run.md) (capitán, implementadores, revisores, aceptación, QA, hardware).
- Pruebas: [TESTING.md](TESTING.md).

## 17. Preguntas abiertas

- ¿El cable TB4/USB4 de 40 Gbps levanta el enlace con el firmware actual de la X870? (S5)
- ¿El 32" vuelve con firmware de fábrica o con el de desarrollo? (decide si es «no compatible» o entra al estándar)
