# Tandem — Documento de diseño

> Estado: borrador v0.1 (2026-10-15). Fuente de verdad del proyecto; las decisiones nuevas se agregan aquí.

## 1. Qué es

Tandem convierte una **MacBook Pro M5** y una **PC Windows** en un solo puesto de trabajo:

- Dos monitores que se reparten entre las dos máquinas **con un clic**, sin tocar el menú del monitor.
- Un solo teclado y mouse, con **motor propio** dentro de la app (sin Deskflow), siempre activo.
- Encender y apagar la PC desde la Mac.
- Un enlace de red propio entre las dos máquinas: rápido por cable, con Wi-Fi como respaldo solo para control.
- Más adelante: carpetas compartidas, SSH, usar la RTX 5080 desde la Mac y mandar archivos.

La UI debe ser **hermosa y visual**: entender el estado de un vistazo, leer poco.

## 2. Hardware

| | Mac | PC |
|---|---|---|
| Equipo | MacBook Pro M5 | Gigabyte X870 GAMING X WIFI7 · Ryzen 7 9800X3D · RTX 5080 |
| Red | Wi-Fi; 3× Thunderbolt/USB-C | Wi-Fi 7 · Realtek **2.5GbE** · **USB4** |
| Video | HDMI → ambos monitores (uno por cable USB-C→HDMI) | DisplayPort → ambos monitores |

| Monitor | Modelo | Entradas | DDC |
|---|---|---|---|
| 34" | Samsung Odyssey G5 **LC34G55T** | DP (PC) + HDMI (Mac) | ✅ desde la PC — ver [research/ddc-34-LC34G55T.md](research/ddc-34-LC34G55T.md) |
| 32" | Samsung Odyssey G5 (modelo por confirmar) | DP (PC) + HDMI (Mac) | ⏳ sin probar |

Periféricos: teclado y mouse en un **hub USB-C** que se conecta físicamente a una máquina u otra.

Compras pendientes: adaptador USB-C→Ethernet 2.5G (Mac), cable Cat6, cable USB4/TB 40 Gbps.

## 3. Principios

1. **Una sola app, todo en Rust.** Teclado y mouse compartidos con un motor propio (`tandem-input`) que viaja por nuestro enlace cifrado. Deskflow solo sirve de referencia: cómo resolvió cada caso raro. Ver [research/deskflow.md](research/deskflow.md).
2. **Cada cambio de modo es una transacción**: pasos, timeouts y vuelta atrás. Nunca dejar al usuario con una pantalla negra sin explicación.
3. **El estado lo lleva Tandem.** Los monitores mienten sobre su entrada, así que el agente guarda dónde está cada uno.
4. **Dos planos de tráfico** (sección 6). El Wi-Fi nunca lleva datos.
5. **Mecanismos intercambiables.** La forma de cambiar un monitor (DDC desde la PC, DDC local, manual, un KVM futuro) va detrás de una interfaz.
6. **Seguro por defecto.** Las máquinas se emparejan una vez, el tráfico va cifrado y nadie más en la red puede dar órdenes.

## 4. Arquitectura

```
┌──────────────── Mac ────────────────┐        ┌─────────────── PC ────────────────┐
│  Tandem UI (Tauri, barra de menú)   │        │  Tandem UI (Tauri, bandeja)        │
│            │ IPC local              │        │            │ IPC local             │
│  tandemd (agente, servicio)         │◄──────►│  tandemd (agente, servicio)        │
│   ├ orchestrator (modos, txns)      │ control│   ├ orchestrator                   │
│   ├ link  (USB4/Eth/Wi-Fi, salud)   │ + datos│   ├ link                           │
│   ├ display (fantasma macOS)        │        │   ├ display (DDC dxva2 + topología)│
│   ├ input (captura/inyección)       │        │   ├ input (captura/inyección)      │
│   ├ peripherals (¿hub aquí?)        │        │   ├ peripherals                    │
│   └ power (Wake-on-LAN)             │        │   └ power (apagado limpio)         │
│   └ clipboard                       │        │   └ clipboard + helper de escritorio seguro │
└─────────────────────────────────────┘        └────────────────────────────────────┘
                                                        │ DDC/CI por DisplayPort
                                                        ▼
                                               34" · 32"  (la PC controla las entradas)
```

### 4.1 Stack

- **Agente `tandemd`: Rust.** Un binario nativo por sistema. Llama directo a las APIs del sistema: `dxva2`/`SetDisplayConfig`/NvAPI en Windows, IOKit/CoreGraphics en macOS. Por FFI no hace falta C ni C++ propios.
- **UI: Tauri 2 + frontend web** (TypeScript, animaciones). Una sola UI para los dos sistemas, ícono en la barra de menú y en la bandeja, ventana principal.
- **Protocolo**: mensajes tipados (serde) sobre QUIC o TLS (rustls) con autenticación mutua por claves Ed25519.

### 4.2 Workspace (propuesto)

```
crates/
  tandem-core        modelo de dominio, modos, máquina de estados, transacciones
  tandem-proto       mensajes entre agentes y UI, versionado
  tandem-link        descubrimiento, enlaces, salud, prioridad, política de planos
  tandem-display     trait MonitorSwitcher + backends (ddc-windows, ddc-macos, manual, kvm)
                     trait DisplayTopology + backends (windows, macos)
  tandem-input       motor de teclado y mouse: captura, inyección, mapa de teclas, bordes
  tandem-clipboard   sincronización del portapapeles
  tandem-peripherals detección del hub USB (VID/PID)
  tandem-power       Wake-on-LAN, apagado remoto
  tandemd            binario del agente (servicio launchd / Windows Service)
apps/
  tandem-ui          Tauri
tools/               scripts de diagnóstico (DDC, etc.)
```

## 5. Modos

| Modo | 34" | 32" | Teclado/mouse |
|---|---|---|---|
| **Solo Mac** | Mac | Mac | activo si la PC está prendida |
| **Compartido** | elegido en la UI | elegido en la UI | activo |
| **Solo Windows** | PC | PC | activo |

- En **Compartido** la UI muestra los dos monitores y el usuario los arrastra a cada máquina.
- Cambiar de modo es una transacción. Ejemplo, de Solo Mac a Compartido (34" → PC):
  1. Si la PC está apagada, Wake-on-LAN y esperar a que su agente responda (timeout 120 s).
  2. PC: DDC `0x60 = DP` en el 34".
  3. PC: activar el 34" en la topología de Windows. Mac: quitar el 34" de macOS (el fantasma).
  4. Recalcular la geometría de pantallas del motor de entrada (cambio en caliente, sin reinicio).
  5. Confirmar el estado y guardarlo. Si algún paso falla, deshacer en orden inverso y avisar en la UI.
- **Apagar la PC** también es transacción: poner todos los monitores en HDMI → la Mac los toma → ceder el teclado y mouse a la Mac → `shutdown /s /t 0`.
- Disparadores: botón en la UI, atajo de teclado global y horarios (opcional).

## 6. Red: enlaces y planos

| Enlace | Prioridad | Plano de control | Plano de datos |
|---|---|---|---|
| USB4 / Thunderbolt directo | 1 | ✅ | ✅ |
| Ethernet 2.5G directo | 2 | ✅ | ✅ |
| Wi-Fi (LAN de casa) | 3 | ✅ | ❌ **nunca** |

- **Control** = teclado, mouse, portapapeles, mensajes entre agentes, monitores, modos, apagado.
- **Datos** = SSH, carpetas compartidas, servicios GPU, archivos. Solo se publican en interfaces de cable. Si se cae el cable, se pausan y la UI los muestra deshabilitados ("requiere cable"). **No hay interruptor para forzarlos por Wi-Fi.**
- IPs estáticas en los enlaces directos (por ejemplo `10.77.1.0/30` USB4 y `10.77.2.0/30` Ethernet) y descubrimiento en la LAN.
- Revisión de salud 1 vez por segundo por enlace. El agente elige el mejor y mueve el tráfico de entrada al siguiente en cuanto uno falla, sin esperar timeouts largos.
- Wake-on-LAN solo por Ethernet. Requiere: BIOS con ErP **apagado** y WoL **encendido**, y Windows con Inicio rápido **desactivado** (para un arranque limpio de verdad).

## 7. Teclado y mouse (motor propio)

- **El servidor es la máquina donde está conectado el hub USB-C.** El agente detecta el teclado y el mouse por VID/PID y, con un pequeño retardo para no reaccionar a desconexiones breves, invierte los roles. La UI tiene un selector manual.
- **Captura** en el servidor: `CGEventTap` en macOS; hooks de bajo nivel o Raw Input en Windows. Cuando el foco está en la otra máquina, los eventos se tragan y el cursor local se bloquea y oculta.
- **Inyección** en el cliente: `CGEventPost` en macOS; `SendInput` con scancodes en Windows.
- **Teclas**: representación canónica por código físico (USB HID), así se respeta la distribución física ES/LatAm de cada sistema. Cmd↔Ctrl configurable.
- **Transporte**: el movimiento del mouse va en datagramas no confiables (coalescidos, baja latencia); las teclas y clics, en un stream confiable. Todo por el enlace cifrado de Tandem, con cambio de enlace inmediato.
  - Sobre la conexión del enlace activo, la entrada y el portapapeles van cada uno en su propio stream: quien lo abre manda `ControlMsg::Open(canal)` como primer mensaje y el otro agente se lo entrega al motor de entrada. El de entrada lo abre el servidor; el del portapapeles, la Mac.
  - Los roles se deciden en cada agente con lo que ve del hub y el reporte del otro (`ControlMsg::Hub`); las pantallas de cada máquina viajan con `ControlMsg::Screens` para que el servidor arme la geometría de las dos.
- **Geometría**: solo cuentan las pantallas que cada máquina está mostrando de verdad, así el cursor nunca pasa por un monitor fantasma. Se recalcula en caliente con cada cambio de modo.
- **Portapapeles**: texto, texto enriquecido e imágenes en v1; archivos en v3, por el plano de datos.
- **Windows, escritorio seguro**: el agente corre como servicio SYSTEM y lanza un helper en el escritorio activo (incluido Winlogon), para que el mouse funcione en UAC y en la pantalla de inicio de sesión.
- Diseño del motor: [research/input-engine.md](research/input-engine.md).

## 8. Monitores

- **Quién cambia las entradas: la PC**, por DDC sobre DisplayPort. El canal sigue vivo aunque el monitor esté mostrando HDMI.
- La Mac pide los cambios por el plano de control. Si se logra DDC desde la Mac (sección 11), se agrega como backend `ddc-macos` y la Mac podría operar con la PC apagada.
- Perfil por monitor con sus códigos reales (el 34": HDMI `0x06`, DP `0x0F`/`0x09`). El monitor no reporta su entrada; el estado lo guarda el agente.
- **Fantasma**: la máquina que no está mostrando un monitor lo quita de su escritorio.
  - Windows: `SetDisplayConfig`. Riesgo: perder el handle DDC → ver spike S3.
  - macOS: no hay API pública; usar la privada `CGSConfigureDisplayEnabled` (como BetterDisplay) → spike S2.
  - Plan C si falla: dejar la pantalla activa pero vacía, moviendo las ventanas fuera, y excluirla de la geometría del motor de entrada.

### 8.1 Pantalla integrada de la MacBook

La laptop se queda casi cerrada pero no del todo (se usan su micrófono y sus audífonos), así que su pantalla queda activa y las ventanas se van ahí sin que se vean.

- **Modos de la pantalla integrada** (selector en la UI, junto al dibujo de la laptop):
  - **Apagada** (predeterminado): se quita del escritorio de macOS. Las ventanas no pueden ir ahí y el motor de entrada la excluye. Es el mismo mecanismo de API privada que el fantasma (spike S2).
  - **Espejo**: refleja un monitor externo (API pública `CGConfigureDisplayMirrorOfDisplay`). Sirve como respaldo si la API privada falla.
  - **Extendida**: comportamiento normal de macOS.
  - **Traer ventanas**: acción puntual que mueve las ventanas de la pantalla integrada al monitor principal (API de Accesibilidad).
- **Regla automática**: si la Mac se queda sin monitores externos (modo Solo Windows o los monitores en DP), la pantalla integrada **se enciende sola**. Al volver un monitor externo, se aplica otra vez la preferencia del usuario.
- La geometría de pantallas del motor de entrada se calcula siempre a partir de este estado, para que el cursor nunca entre en una pantalla que no se está mirando.

## 9. Seguridad

- Emparejamiento único con un código de 6 dígitos que se muestra en las dos UIs.
- Cada agente tiene una identidad Ed25519 guardada en el llavero del sistema (Keychain / DPAPI).
- Toda conexión entre agentes usa autenticación mutua; el motor de entrada usa el mismo canal.
- El IPC local de la UI solo es accesible para el usuario actual.

## 10. UI

Dirección visual: oscura y sobria, una sola escena viva en vez de formularios. Colores apagados (un acento suave por máquina), sin brillos de neón, sin degradados llamativos ni fondos animados: que no parezca "app de IA".

- **Escena principal**: las dos máquinas dibujadas, los dos monitores con miniatura de color, y las líneas del enlace animadas: grosor y pulso según la velocidad, color según el tipo.
- Arrastrar un monitor de una máquina a otra = cambiarlo.
- Tarjetas de modo grandes (Solo Mac / Compartido / Solo Windows) con iconografía, sin texto largo.
- Botón de encendido de la PC con su estado (apagada, arrancando, lista).
- Chip de teclado y mouse (quién es servidor, conectado o no).
- Barra de menú / bandeja: mini versión de la escena + los tres modos.
- Feedback de transacción: una línea de progreso por pasos, con iconos.

Mockups en [`../mockups/`](../mockups/).

## 11. Riesgos y spikes (Fase 0)

| # | Spike | Por qué importa |
|---|---|---|
| S1 | DDC del 32" desde la PC | Saber si los dos monitores se pueden controlar |
| S2 | Quitar/devolver una pantalla en macOS por API privada (monitores externos **y** pantalla integrada); espejo como respaldo | Monitor fantasma y pantalla integrada |
| S3 | ¿DDC sobrevive si Windows desactiva el monitor? Si no, I2C por NvAPI | Monitor fantasma en Windows sin perder el control |
| S4 | DDC desde la Mac: HDMI integrado vs cable USB-C→HDMI (m1ddc / IOAVService) | Operar monitores con la PC apagada |
| S5 | Red USB4 Mac↔Windows (cuando llegue el cable) | Enlace prioritario |
| S6 | Wake-on-LAN desde apagado total en la X870 | Encendido remoto |
| S7 | Permisos de macOS (Accesibilidad / Input Monitoring) para capturar e inyectar | Experiencia de instalación |
| S9 | Escritorio seguro de Windows (UAC, Ctrl+Alt+Supr, inicio de sesión) con servicio SYSTEM + helper | El mouse no puede "morir" en UAC |
| S10 | Prototipo de captura e inyección + mapa de teclas ES/LatAm Mac↔Windows | Base del motor propio |
| S8 | Aislar el código de vuelta a DP (`0x09` vs `0x0F`) | Perfil del 34" |

## 12. Fases

| Fase | Contenido | Cómo se considera terminada |
|---|---|---|
| **F0 Spikes** | S1–S4, S6–S8 | Cada spike con nota en `docs/research/` |
| **F1 Núcleo** | workspace, tandemd en los dos sistemas, emparejamiento, enlace (Wi-Fi primero), protocolo, CI que compile mac y win | Los agentes se ven y hablan cifrado |
| **F2 Teclado y mouse** | `tandem-input` (captura, inyección, bordes, mapa de teclas), portapapeles, roles por hub USB, escritorio seguro | Uso diario sin Deskflow; cambiar el hub invierte los roles |
| **F3 Monitores y modos** | MonitorSwitcher, topología, transacciones, los tres modos | Un clic cambia el modo completo, con vuelta atrás |
| **F4 Energía** | Wake-on-LAN y apagado transaccional | Encender y apagar la PC desde la Mac |
| **F5 UI** | escena, arrastrar monitores, barra de menú, atajos | La UI de los mockups, funcionando |
| **v2 Datos** | SSH, carpetas compartidas (SMB), servicios GPU (Ollama, etc.) solo por cable | Accesos directos en la UI |
| **v3 Archivos** | arrastrar y soltar o "enviar a" entre máquinas | Mandar un archivo con un gesto |

## 13. Flujo de desarrollo

- Código en WSL (`~/projects/personal-projects/apps/tandem`), repo privado `franc1sc0sv/tandem`.
- CI en GitHub Actions: compila y prueba en `macos-latest` y `windows-latest`.
- Pruebas en hardware: SSH a la Mac y OpenSSH en Windows para instalar, correr y leer logs sin intervención del usuario. Las pruebas que mueven monitores siempre traen **vuelta automática**.
- El usuario revisa el avance; Claude ejecuta.

## 14. Preguntas abiertas

- Modelo exacto del 32".
- ¿Horarios automáticos en v1 o después?
- ¿Qué servicios GPU concretos en v2 (Ollama, ComfyUI, Jupyter)?
