# Tandem — Documento de diseño

> Estado: v0.2 (2026-11-15). Fuente de verdad del proyecto; las decisiones nuevas se agregan aquí.
>
> **Cambio de rumbo v0.2:** el control de monitores por DDC de fábrica (perfiles, calibración, identidades por EDID, monitor fantasma, pantalla integrada, lectura de la topología) se elimina. Lo va a reemplazar un firmware propio del 34" que reporta su estado real ([§8](#8-monitores-en-pausa-firmware-propio)). Mientras tanto Tandem se concentra en teclado y mouse, el enlace por cable y la energía.

## 1. Qué es

Tandem convierte una **MacBook Pro M5** y una **PC Windows** en un solo puesto de trabajo:

- Un solo teclado y mouse, con **motor propio** dentro de la app (sin Deskflow), siempre activo, sin retraso perceptible y respetando el acomodo de las pantallas.
- Un enlace de red propio entre las dos máquinas: un **cable USB-C directo** como enlace principal, Ethernet como segundo y el Wi-Fi solo para control.
- Encender y apagar la otra máquina desde la que se está usando.
- Portapapeles compartido.
- **Más adelante:** los monitores, cuando el firmware propio esté listo ([§8](#8-monitores-en-pausa-firmware-propio)); carpetas compartidas, SSH, usar la RTX 5080 desde la Mac y mandar archivos.

La UI debe ser **hermosa y visual**: entender el estado de un vistazo, leer poco.

## 2. Hardware

| | Mac | PC |
|---|---|---|
| Equipo | MacBook Pro M5 | Gigabyte X870 GAMING X WIFI7 · Ryzen 7 9800X3D · RTX 5080 |
| Red | Wi-Fi; 3× Thunderbolt/USB-C | Wi-Fi 7 · Realtek **2.5GbE** · **USB4** (ASMedia ASM4242) |
| Video | HDMI → ambos monitores | DisplayPort → ambos monitores |

| Monitor | Modelo | Control |
|---|---|---|
| 34" | Samsung Odyssey G5 **LC34G55T** | Firmware propio en desarrollo (`personal-projects/monitor-firmware`) |
| 32" | Samsung Odyssey G5 | Sin control por ahora |

Periféricos: teclado y mouse en un **hub USB-C** que se conecta físicamente a una máquina u otra.

Cables: un cable USB-C a USB-C de un monitor Samsung de 27" (10 Gbps, con video) para el enlace directo ([§6.1](#61-enlace-directo-por-usb-c)). Pendiente: adaptador USB-C→Ethernet 2.5G para la Mac y cable Cat6.

## 3. Principios

1. **Una sola app, todo en Rust.** Teclado y mouse compartidos con un motor propio (`tandem-input`) que viaja por nuestro enlace cifrado. Deskflow solo sirve de referencia: cómo resolvió cada caso raro. Ver [research/deskflow.md](research/deskflow.md).
2. **La entrada es lo primero.** Nada en el agente (red, portapapeles, UI) puede frenar el camino captura → enlace → inyección. Cada cambio en ese camino se mide ([§7.1](#71-latencia-y-fiabilidad)).
3. **Cada acción con efecto en la otra máquina es una transacción**: pasos, timeouts y vuelta atrás (hoy: encender y apagar).
4. **Dos planos de tráfico** (sección 6). El Wi-Fi nunca lleva datos.
5. **Nada de adivinar el estado de un monitor.** Tandem no controla monitores hasta tener el firmware propio, que dice la verdad. No se vuelven a agregar perfiles, calibraciones ni preguntas al usuario sobre qué muestra un monitor.
6. **Seguro por defecto.** Las máquinas se emparejan una vez, el tráfico va cifrado y nadie más en la red puede dar órdenes.
7. **Pares, sin centro fijo (T-084).** Cualquiera de las dos puede ser la principal. Cada agente anuncia lo que tiene y sabe hacer (**capacidades**: hub, permisos, energía, servicios) y las acciones salen de esos anuncios, no del nombre de la máquina.

## 4. Arquitectura

```
┌──────────────── Mac ────────────────┐        ┌─────────────── PC ────────────────┐
│  Tandem UI (Tauri, barra de menú)   │        │  Tandem UI (Tauri, bandeja)        │
│            │ IPC local              │        │            │ IPC local             │
│  tandemd (agente, servicio)         │◄──────►│  tandemd (agente, servicio)        │
│   ├ orchestrator (energía, txns)    │ control│   ├ orchestrator                   │
│   ├ link  (USB-C/Eth/Wi-Fi, salud)  │ + datos│   ├ link                           │
│   ├ input (captura/inyección)       │        │   ├ input (captura/inyección)      │
│   ├ peripherals (¿hub aquí?)        │        │   ├ peripherals                    │
│   ├ power (Wake-on-LAN)             │        │   ├ power (apagado limpio)         │
│   └ clipboard                       │        │   └ clipboard                      │
└─────────────────────────────────────┘        └────────────────────────────────────┘
                                                  (futuro) tandem-monitor ── firmware del 34"
```

- **Anuncio de capacidades** (`ControlMsg::Capabilities`): al conectarse por cada enlace y con cada cambio. Con los dos anuncios cada agente sabe a quién pedirle cada paso (despertar, apagar). La UI habilita una acción solo si alguna máquina puede hacerla.
- **Puesto compartido** (`ControlMsg::Desk`): el acomodo de pantallas de cada máquina para el motor de entrada ([§7](#7-teclado-y-mouse-motor-propio)) y el atajo de bloqueo, con versión (`desk_rev`). Va al emparejar, en cada reconexión y con cada cambio; gana la versión más nueva.
- Un agente de una versión anterior se rechaza en el saludo con un error de versión que dice qué hacer. v0.2 sube la versión del protocolo: los mensajes de monitores desaparecen.

### 4.1 Stack

- **Agente `tandemd`: Rust.** Un binario nativo por sistema, con FFI directo a las APIs del sistema (hooks y `SendInput` en Windows; `CGEventTap`/`CGEventPost` en macOS).
- **UI: Tauri 2 + frontend web** (TypeScript, animaciones).
- **Protocolo**: mensajes tipados (serde) sobre QUIC (rustls) con autenticación mutua por claves Ed25519.

### 4.2 Workspace

```
crates/
  tandem-core        modelo de dominio, máquina de estados, transacciones
  tandem-proto       mensajes entre agentes y UI, versionado
  tandem-link        descubrimiento, enlaces, salud, prioridad, política de planos
  tandem-input       motor de teclado y mouse: captura, inyección, mapa de teclas, bordes
  tandem-clipboard   sincronización del portapapeles
  tandem-peripherals detección del hub USB (VID/PID)
  tandem-power       Wake-on-LAN, apagado remoto
  tandem-sim         puesto simulado para pruebas
  tandemd            binario del agente (servicio launchd / Windows Service)
  (futuro) tandem-monitor  cliente del firmware propio (§8)
apps/
  tandem-ui          Tauri
```

`tandem-display` y la CLI `tandem-ddc` se eliminan en T-092.

## 5. Energía y estado del puesto

Los modos Solo Mac / Compartido / Solo Windows dependían de cambiar monitores y **quedan en pausa** hasta el firmware. La UI los muestra deshabilitados («próximamente»).

- **Encender la otra máquina**: Wake-on-LAN desde la que tiene su MAC y un enlace por Ethernet o LAN; se espera a que su agente responda (timeout 120 s).
- **Apagar la otra máquina** es una transacción: el teclado y mouse vuelven a la que queda → la otra se apaga limpio (la PC con `shutdown /s /t 0`; la Mac todavía no sabe apagarse sola y no lo anuncia).
- **Vigilancia del par**: si ningún enlace ve a la otra durante `peer_lost_after_secs` sin que nadie la apagara (corte de corriente, suspensión), se la da por apagada, el motor de entrada la saca de la geometría y la UI lo avisa.
- **Asistente de inicio** (sin monitores): (1) esta máquina y sus permisos, (2) emparejar y ver el ping del enlace, (3) «presiona una tecla»: la máquina que la recibe tiene el teclado, (4) resumen. Con T-096 se agrega, antes del resumen, el acomodo de pantallas ([§7](#7-teclado-y-mouse-motor-propio)) con una prueba de cruce del mouse.
- **Teclado sin definir (T-087).** La configuración generada no supone dónde está el teclado: la UI dice «sin definir» hasta que lo diga el hub o el usuario. Solo en la UI: el motor de entrada arranca como si estuviera en la Mac (`InputConfig::keyboard()`).

## 6. Red: enlaces y planos

| Enlace | Prioridad | Plano de control | Plano de datos |
|---|---|---|---|
| USB-C directo (red USB4 / Thunderbolt) | 1 | ✅ | ✅ |
| Ethernet 2.5G directo | 2 | ✅ | ✅ |
| Wi-Fi (LAN de casa) | 3 | ✅ | ❌ **nunca** |

- **Control** = teclado, mouse, portapapeles, mensajes entre agentes, energía.
- **Datos** = SSH, carpetas compartidas, servicios GPU, archivos. Solo se publican en interfaces de cable. Si se cae el cable, se pausan y la UI los muestra deshabilitados ("requiere cable"). **No hay interruptor para forzarlos por Wi-Fi.**
- Revisión de salud 1 vez por segundo por enlace. El agente elige el mejor y mueve el tráfico de entrada al siguiente en cuanto uno falla, sin esperar timeouts largos (con histéresis al volver, T-065).
- Wake-on-LAN solo por Ethernet o LAN (no por USB-C: con la máquina apagada el controlador USB4 no tiene red). Requiere BIOS con ErP **apagado** y WoL **encendido**, y Windows con Inicio rápido **desactivado**.

### 6.1 Enlace directo por USB-C

**Qué es «Ethernet» en la tabla:** un cable de red normal (RJ45) entre el puerto 2.5GbE de la PC y la Mac con un adaptador USB-C→Ethernet. Es el respaldo si el USB-C no da red.

**El cable USB-C como red.** Dos computadoras con USB4/Thunderbolt conectadas con un cable USB-C forman una red punto a punto (*USB4 inter-domain networking*): en macOS aparece la interfaz **Thunderbolt Bridge** (`bridge0`); en Windows, un adaptador de red **USB4 / Thunderbolt Networking**. No es USB normal: un cable USB 3 entre dos hosts sin USB4 no hace nada.

- **El cable del monitor de 27":** si ese cable lleva video del monitor (DisplayPort alt mode), tiene los pares de alta velocidad, que son los mismos que usa USB4. Un cable pasivo de 10 Gbps (USB 3.2 Gen 2) normalmente negocia **USB4 Gen 2 (2×10 = 20 Gbps de enlace)**; descontando el protocolo, la red rinde del orden de **5–10 Gbps**. Para teclado, mouse y portapapeles sobra: basta con ~1 Mbps y lo que importa es la latencia, que en cable directo es de décimas de milisegundo.
- **Riesgo principal (spike S5):** la red USB4 entre hosts en Windows depende del controlador. La X870 usa un ASMedia ASM4242 y no está confirmado que Windows le dé el adaptador de red host a host. Se prueba primero con el hardware real; si no aparece la interfaz, el enlace principal es Ethernet.
- **Detección automática (T-094):** el agente vigila las interfaces del sistema y reconoce la del cable por su tipo (Thunderbolt Bridge en macOS; descripción USB4/Thunderbolt Networking en Windows), sin configurar nada a mano:
  1. Al aparecer la interfaz, usa las direcciones IPv6 *link-local* (siempre existen, sin DHCP) y, como respaldo, una dirección fija del rango `10.77.1.0/30` que cada agente se asigna según su lado.
  2. Descubre al par con mDNS **solo en esa interfaz** y abre la conexión QUIC. Cada extremo clasifica el enlace por su interfaz de salida y se intercambian la clasificación: **vale la peor de las dos** (si la PC sale por Ethernet al router y la Mac llega por Wi-Fi, el enlace es Wi-Fi y no lleva datos).
  3. Mide el enlace (latencia de ida y vuelta y, a pedido, el ancho de banda con una prueba corta **solo en enlaces de cable**: la prueba es tráfico de datos) y la UI lo muestra: «USB-C · 20 Gbps · 0.3 ms».
  4. Al desconectar el cable, el gestor de enlaces pasa la entrada al siguiente en menos de un segundo.
- Los `peers` estáticos de la configuración siguen funcionando como respaldo manual.

## 7. Teclado y mouse (motor propio)

- **El teclado está en la máquina donde está conectado el hub USB-C**, en cualquiera de las dos direcciones. El agente detecta el teclado y el mouse por VID:PID y, con un pequeño retardo para no reaccionar a desconexiones breves, lo pasa a la otra. Si la otra se da por apagada, la que queda se lo queda. En la UI no hay «servidor» ni «cliente»: «Teclado aquí» / «Teclado de la PC».
- **Captura** en la que tiene el teclado: `CGEventTap` en macOS; hooks de bajo nivel o Raw Input en Windows. Cuando el foco está en la otra máquina, los eventos se tragan y el cursor local se bloquea y oculta.
- **Inyección** en la otra: `CGEventPost` en macOS; `SendInput` con scancodes en Windows.
- **Teclas**: representación canónica por código físico (USB HID), así se respeta la distribución física ES/LatAm de cada sistema. Cmd↔Ctrl configurable.
- **Transporte**: el movimiento del mouse va en datagramas no confiables (coalescidos, baja latencia); las teclas y clics, en un stream confiable. Todo por el enlace cifrado de Tandem, con cambio de enlace inmediato.
  - Sobre la conexión del enlace activo, la entrada y el portapapeles van cada uno en su propio stream (`ControlMsg::Open(canal)`).
  - Los papeles se deciden en cada agente con lo que ve del hub y el reporte del otro (`ControlMsg::Hub`); las pantallas viajan con `ControlMsg::Screens`.
- **Geometría (v0.2): acomodo declarado.** Se elimina la topología de `tandem-display` (monitores, EDID, salidas activas, fantasma, pantalla integrada). La geometría sale del **acomodo** del puesto compartido: `[[screens]]` por máquina, cada pantalla con un id, su rectángulo en las **coordenadas nativas del cursor de esa máquina** (puntos en macOS, píxeles físicos del escritorio virtual en Windows) y su escala; más la posición de un escritorio respecto del otro (qué borde de cuál toca cuál, y el desplazamiento). Se edita a mano en `tandem.toml` (T-092) y con un editor visual en la UI (T-096).
  - **Valor por omisión:** los límites que el propio motor de entrada ya lee para clampear el cursor al inyectar (`CGDisplayBounds` en `tandem-input` para macOS; `SM_*VIRTUALSCREEN` en Windows). No es control de monitores: es lo mínimo que la inyección necesita, y evita que con un 34" de 3440×1440 el mouse cruce a media pantalla. Sin acomodo declarado, la Mac va a la izquierda de la PC.
  - `tandem_input::geometry` **sí cambia**: hoy depende de `tandem_core::{Config, State, MonitorId, BuiltinDisplay}` (dueño de cada monitor, pantalla integrada efectiva, orden de `cfg.monitors`). En T-092 pasa a recibir solo el acomodo (rectángulos por máquina y posición relativa); el algoritmo de cruce de bordes se conserva.
  - Si el acomodo no coincide con las pantallas reales, el cursor cruza en el lugar equivocado: el asistente termina con una prueba de cruce (cuando exista el editor, T-096). Cuando llegue el firmware, las pantallas que muestran la otra máquina salen de la geometría solas ([§8](#8-monitores-en-pausa-firmware-propio)).
- **Portapapeles**: texto, texto enriquecido e imágenes en v1; archivos en v3, por el plano de datos.
- **Windows, escritorio seguro** (T-027): el agente corre como servicio SYSTEM y lanza un helper en el escritorio activo, para que el mouse funcione en UAC y en la pantalla de inicio de sesión.
- Diseño del motor: [research/input-engine.md](research/input-engine.md).

### 7.1 Latencia y fiabilidad

Objetivo medible (T-093), de evento capturado a evento inyectado, en la máquina remota:

| Enlace | p50 | p99 |
|---|---|---|
| USB-C / Ethernet directo | ≤ 1 ms | ≤ 4 ms |
| Wi-Fi | ≤ 5 ms | ≤ 20 ms |

- **Medición siempre disponible:** cada datagrama y mensaje de entrada lleva una marca de tiempo del reloj monotónico del servidor; el cliente estima el desfase con los heartbeats y publica p50/p99 en la UI (chip del enlace) y en los logs. Un banco de pruebas (`tandem-sim` + loopback) mide la ruta interna: en CI con un umbral holgado que solo detecta regresiones grandes (los runners compartidos tienen pausas); el objetivo de 1 ms se comprueba como benchmark local.
- **Sin colas en el camino caliente:** la captura entrega a un canal dedicado; el envío del datagrama no espera a un runtime ocupado con el portapapeles o la UI (tarea con prioridad propia o hilo dedicado). Los datagramas de entrada no esperan al *pacing* del control de congestión de QUIC.
- **Fallos conocidos a cubrir:** Windows quita en silencio un hook lento (`LowLevelHooksTimeout`): el callback no hace trabajo y el watchdog lo reinstala; macOS desactiva el tap por timeout (`kCGEventTapDisabledByTimeout`): se reactiva y se registra; teclas pegadas al cambiar de máquina o caer el enlace: se sueltan todas en los dos lados; escala por monitor en Windows (DPI): el acomodo declarado lleva la escala.

## 8. Monitores (en pausa: firmware propio)

Todo el control de monitores de v0.1 se elimina en T-092: DDC por `dxva2`, perfiles y calibración, identidades por EDID, «¿qué muestra ahora?», monitor fantasma (`SetDisplayConfig`, `CGSConfigureDisplayEnabled`), pantalla integrada de la MacBook, lectura de la topología de pantallas y los modos que dependían de todo eso. La razón: los monitores de fábrica mienten sobre su entrada (el 34" responde con el puerto por donde llegó la pregunta, no con lo que muestra), así que todo era adivinar.

**El reemplazo es un firmware propio del 34"** (proyecto `personal-projects/monitor-firmware`, v3.1 ya flasheado). Sobre MCCS normal agrega:

| VCP | Qué da |
|---|---|
| `0x60` (corregido en v1) | La entrada real en pantalla, no el puerto de la pregunta |
| `0xEA`–`0xED` | Registro de eventos con número de secuencia y hora: cambio de entrada, señal, energía, ajustes del menú, y el **veredicto de un cambio** (`switch-done` / `switch-failed`) |
| `0xEE` | Estado en vivo: puerto, selector de fuente, energía, señal |
| `0xF0`–`0xF6` | Lectura de memoria, escritura de ajustes y aplicar opciones del menú |

**Integración prevista (T-095, después de que el firmware esté listo):**

- Crate nuevo `tandem-monitor` con el protocolo del firmware, detrás de un trait con doble de prueba. Sin perfiles ni calibración: si el monitor no habla el protocolo (VCP `0xEA` sin soporte), no se controla.
- **Un solo lector por monitor.** El cursor de eventos (`0xEB`) y la dirección (`0xF0`/`0xF1`) son estado global del monitor: dos lectores a la vez se pisan. Lee el agente de la máquina con canal DDC (hoy la PC) y reparte los eventos a la otra por el plano de control.
- El estado de cada monitor sale de los eventos; un cambio de entrada termina con el veredicto del firmware, no con un plazo ni con una pregunta.
- Vuelven los modos y el arrastre de monitores en la UI, y la geometría del motor de entrada saca sola las pantallas que muestran la otra máquina.
- Límite conocido: solo la PC tiene canal DDC hoy (la Mac no lo tiene por HDMI). Con la PC apagada no hay control, salvo que funcione el backend de macOS (`IOAVService`, sin probar).

## 9. Seguridad

- Emparejamiento único con un código de 6 dígitos que se muestra en las dos UIs.
- Cada agente tiene una identidad Ed25519 guardada en el llavero del sistema (Keychain / DPAPI).
- Toda conexión entre agentes usa autenticación mutua; el motor de entrada usa el mismo canal.
- El IPC local de la UI solo es accesible para el usuario actual.

## 10. UI

Dirección visual: oscura y sobria, una sola escena viva en vez de formularios. Colores apagados (un acento suave por máquina), sin brillos de neón, sin degradados llamativos ni fondos animados.

- **Escena principal**: las dos máquinas y el enlace entre ellas, vivo: el tipo de enlace activo (USB-C, Ethernet, Wi-Fi), su velocidad y la latencia de la entrada (p50/p99) con un pulso que sigue el tráfico real.
- **El cursor en vivo**: un punto que se mueve en el acomodo de pantallas mostrando en qué máquina está el foco, y un destello en el borde cuando cruza.
- **Acomodo de pantallas**: rectángulos arrastrables por máquina (§7); se guardan en el puesto compartido.
- Chip de teclado y mouse (dónde está el hub, conectado o no).
- Botón de encendido de la otra máquina con su estado, solo si se la puede despertar o sabe apagarse.
- **Monitores**: la tarjeta queda visible pero **deshabilitada** («próximamente: firmware propio»), igual que los modos.
- Barra de menú / bandeja: mini versión de la escena.

Mockups: [`../mockups/main.html`](../mockups/main.html) (v0.1) y [`../mockups/next.html`](../mockups/next.html) (v0.2).

## 11. Riesgos y spikes

| # | Spike | Por qué importa |
|---|---|---|
| S5 | Red USB4 host a host Mac↔Windows con el cable del monitor de 27" (ASM4242 en la X870) | Enlace principal; si no hay interfaz en Windows, el principal es Ethernet |
| S6 | Wake-on-LAN desde apagado total en la X870 | Encendido remoto |
| S7 | Permisos de macOS (Accesibilidad / Input Monitoring) | Experiencia de instalación |
| S9 | Escritorio seguro de Windows con servicio SYSTEM + helper | El mouse no puede "morir" en UAC |
| S11 | Latencia real captura → inyección en los dos sentidos, por cable y por Wi-Fi | Objetivo de §7.1 |
| S12 | Firmware: lector único de eventos desde Tandem y control del 32" | Integración de monitores (T-095) |

Los spikes de DDC de fábrica (S1–S4, S8) quedan cerrados: se reemplazan por el firmware propio.

## 12. Fases

| Fase | Contenido | Cómo se considera terminada |
|---|---|---|
| F0–F5 (v0.1) | Núcleo, entrada, monitores por DDC, energía, UI | Hecho; los monitores se retiran en v0.2 |
| **G1 Limpieza** | T-092: fuera monitores, DDC y pantallas | El agente y la UI compilan y prueban sin `tandem-display` |
| **G2 Entrada** | T-093: latencia medida y fallos conocidos cubiertos | Objetivos de §7.1 en el banco y en el puesto |
| **G3 Enlace USB-C** | T-094: detección automática del cable | Conectar el cable lo pone como enlace principal sin configurar |
| **G4 UI viva** | T-096: escena con enlace, cursor y acomodo de pantallas | La UI de `mockups/next.html`, funcionando |
| **G5 Monitores** | T-095: firmware propio | Cambios con veredicto del monitor, modos de vuelta |
| v2 Datos / v3 Archivos | SSH, SMB, GPU; mandar archivos | Igual que en v0.1 |

## 13. Flujo de desarrollo

- Código en WSL (`~/projects/personal-projects/apps/tandem`), repo privado `franc1sc0sv/tandem`.
- CI en GitHub Actions: compila y prueba en `macos-latest` y `windows-latest`.
- Pruebas en hardware: SSH a la Mac y OpenSSH en Windows para instalar, correr y leer logs sin intervención del usuario. Las pruebas que mueven monitores siempre traen **vuelta automática**.
- El usuario revisa el avance; Claude ejecuta.

## 14. Preguntas abiertas

- Modelo exacto del 32" y si su firmware se puede modificar igual que el del 34".
- ¿La X870 da red USB4 host a host en Windows? (S5)
- ¿Horarios automáticos en v1 o después?
- ¿Qué servicios GPU concretos en v2 (Ollama, ComfyUI, Jupyter)?
