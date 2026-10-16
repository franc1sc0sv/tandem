# Tandem — Estado del proyecto

> Corte: 2026-10-16 · rama `develop` · CI en verde en Linux, Windows y macOS

## En una línea

**Todo lo que se puede construir y probar sin tu hardware está hecho.** Lo que falta es probarlo en tu Mac, tu PC y tus monitores reales, y 3 tareas que necesitan una acción tuya.

| | |
|---|---|
| Tareas integradas | **41 de 44** (las 3 restantes necesitan al usuario) |
| Pruebas | **909 de Rust + 60 de la UI**, 0 fallos |
| Código | ~55 mil líneas de Rust en 10 crates + UI en Tauri |
| CI por PR | Linux · Windows · macOS · UI x3 · cobertura · `.dmg` · instalador de Windows |
| Rondas de QA adversarial | 2, con 5 bugs reales encontrados y corregidos |

## Qué hay

| Área | Estado |
|---|---|
| **Red** | QUIC cifrado con autenticación mutua, enlaces USB4 → Ethernet → Wi-Fi con failover e histéresis, el Wi-Fi solo lleva control, descubrimiento mDNS, emparejamiento con código de 6 dígitos desde la UI |
| **Agente `tandemd`** | Ensamblado de punta a punta: red, orquestador de modos con vuelta atrás, monitores, energía, entrada, portapapeles y roles. Probado con **dos procesos reales** que hablan entre sí (T-075) |
| **Monitores** | DDC en Windows (`tandem-ddc`), topología unificada Windows + macOS, monitor fantasma, pantalla integrada (apagada/espejo/extendida/traer ventanas) |
| **Teclado y mouse** | Motor propio: captura e inyección en macOS y Windows, mapa de teclas HID, geometría y cruce de bordes, bloqueo del cursor, sesión por el enlace |
| **Portapapeles** | Texto, HTML e imagen, sin eco |
| **Roles** | El servidor es la máquina donde está el hub USB-C, con selector manual |
| **Energía** | Wake-on-LAN, apagado limpio, detección de corte de corriente |
| **UI** | Escena principal (el mockup, funcionando), barra de menú y bandeja, atajos globales, emparejamiento, avisos |
| **Instalación** | `.dmg` de macOS con LaunchAgent y guía de permisos; instalador de Windows con arranque automático; release en CI con un tag `v*` (en borrador) |

## Cómo probarlo

1. **Instaladores:** en GitHub → *Actions* → la última corrida de CI en `develop` → artefactos `dmg` e `instalador`. O empuja un tag `v0.1.0` y el workflow *release* crea un release en **borrador** con los dos.
2. **macOS:** abre el `.dmg`, arrastra a Aplicaciones y ábrelo con clic derecho → Abrir (la firma es ad-hoc). La guía te lleva a dar los permisos.
3. **Windows:** corre el instalador. `tandemd` arranca con tu sesión.
4. **Emparejar:** en la UI de cualquiera de las dos → Emparejar → confirma el mismo código en las dos.
5. **Configs de ejemplo:** `docs/examples/tandem.mac.toml` y `tandem.pc.toml`. Guías en `docs/dev/install-macos.md`, `install-windows.md` y `release.md`.

## Plan de prueba en hardware (en orden)

Cada tarea tiene su lista detallada en `docs/tasks/T-xxx.md`, en la sección *Verificación en hardware pendiente*. Este es el orden que recomiendo:

| # | Qué probar | Tareas | Necesita |
|---|---|---|---|
| 1 | `tandem-ddc` en la PC: listar, `caps` y `switch` con vuelta automática en el 34" | T-010, T-050 | Nada nuevo |
| 2 | Conectar el 32" por DP y sacar sus códigos DDC | T-050 | Tú, mirando el monitor |
| 3 | Activar SSH a la Mac para que los agentes prueben ahí | T-051 | Tú, una vez |
| 4 | Pantallas en la Mac: apagar/espejo de la integrada, quitar el fantasma, traer ventanas | T-012, T-013, T-015 | Mac |
| 5 | Instalar los dos instaladores y emparejar | T-070, T-071, T-073 | Las dos máquinas |
| 6 | Teclado y mouse: cruzar bordes, distribución ES (`ñ`, teclas muertas, AltGr), modificadores, scroll, multimedia, bloqueo con ScrollLock | T-022…T-025, T-074 | Las dos máquinas |
| 7 | Portapapeles y roles moviendo el hub | T-028, T-029 | Las dos máquinas |
| 8 | Modos completos: Solo Mac → Compartido → Solo Windows, con monitores reales | T-014, T-015 | Todo |
| 9 | Red: mDNS entre las dos, firewall, y después los cables USB4 y Ethernet | T-006 | Cables |
| 10 | Wake-on-LAN y apagado limpio (BIOS: ErP off, WoL on; Inicio rápido desactivado) | T-030 | Cable Ethernet |

## Lo que necesita al usuario

| Tarea | Qué |
|---|---|
| **T-050** | Conectar el 32" por DP a la PC para encontrar sus códigos DDC (y aislar si el 34" vuelve con `0x09` o `0x0F`) |
| **T-051** | Activar Sesión remota en la Mac y autorizar la llave de WSL (`docs/dev/hardware.md`) |
| **T-027** | Servicio SYSTEM de Windows para que el mouse funcione en UAC y en la pantalla de inicio de sesión. Necesita decidir un certificado Authenticode |
| **Pasar a `main`** | Revisar `develop` y abrir el PR a `main` |

## Riesgos y limitaciones conocidas

- **Nada se probó aún en hardware real.** Todo pasa con dobles de prueba, en el simulador del puesto y en CI. Lo físico (DDC, APIs privadas de macOS, hooks de Windows, distribución de teclado) solo se confirma en tus equipos.
- **APIs privadas de macOS** (`CGSConfigureDisplayEnabled`, `SetsCursorInBackground`): pueden cambiar entre versiones de macOS; el espejo es el respaldo.
- **Firma:** macOS usa firma ad-hoc (clic derecho → Abrir; los permisos TCC están atados a la firma, así que al reinstalar puede pedirlos otra vez). Windows sin firma Authenticode: SmartScreen va a avisar.
- **Windows sin el servicio SYSTEM (T-027):** el mouse remoto no funciona en diálogos UAC ni en la pantalla de inicio de sesión.
- **Escala en Windows:** la topología no da los DPI y las pantallas de la PC van con escala 1; la velocidad del cursor entre monitores puede sentirse distinta (T-074).
- **Corte de corriente durante un cambio que usa la Mac:** la Mac espera hasta ~130 s antes de marcar la PC como apagada (T-063).
- **El monitor no reporta su entrada:** el estado lo lleva Tandem. Si cambias la entrada a mano desde el menú del monitor, Tandem no se entera hasta el siguiente cambio de modo.

## Cómo se construyó

Un piloto de agentes en paralelo (hasta 11 tareas a la vez), cada tarea en su worktree: implementar → revisión independiente → correcciones → CI en las tres plataformas → merge a `develop` desde la sesión principal. Las rondas de QA usan `tandem-sim`, un puesto simulado con fallos inyectados. Detalle en `docs/dev/agents.md`, estrategia de pruebas en `docs/TESTING.md`, diseño en `docs/DESIGN.md`.
