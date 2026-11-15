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
| **Agente `tandemd`** | Ensamblado de punta a punta: red, orquestador de encendido y apagado, energía, entrada, portapapeles y roles. Probado con **dos procesos reales** que hablan entre sí (T-075) |
| **Monitores** | Quitado en T-092 (DESIGN §8): vuelve con el firmware propio del 34". La UI los muestra deshabilitados. El motor de entrada usa el acomodo declarado (`[[screens]]`) o las pantallas que lee cada sistema |
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
| 1 | Activar SSH a la Mac para que los agentes prueben ahí | T-051 | Tú, una vez |
| 2 | Instalar los dos instaladores y emparejar | T-070, T-071, T-073 | Las dos máquinas |
| 3 | Teclado y mouse: cruzar bordes, distribución ES (`ñ`, teclas muertas, AltGr), modificadores, scroll, multimedia, bloqueo con ScrollLock | T-022…T-025, T-074 | Las dos máquinas |
| 4 | Portapapeles y roles moviendo el hub | T-028, T-029 | Las dos máquinas |
| 5 | Red: mDNS entre las dos, firewall, y después los cables USB4 y Ethernet | T-006 | Cables |
| 6 | Wake-on-LAN y apagado limpio (BIOS: ErP off, WoL on; Inicio rápido desactivado) | T-030 | Cable Ethernet |

## Lo que necesita al usuario

| Tarea | Qué |
|---|---|
| **T-051** | Activar Sesión remota en la Mac y autorizar la llave de WSL (`docs/dev/hardware.md`) |
| **T-027** | Servicio SYSTEM de Windows para que el mouse funcione en UAC y en la pantalla de inicio de sesión. Necesita decidir un certificado Authenticode |
| **Pasar a `main`** | Revisar `develop` y abrir el PR a `main` |

## Riesgos y limitaciones conocidas

- **Nada se probó aún en hardware real.** Todo pasa con dobles de prueba, en el simulador del puesto y en CI. Lo físico (APIs de macOS, hooks de Windows, distribución de teclado) solo se confirma en tus equipos.
- **APIs privadas de macOS** (`SetsCursorInBackground`): pueden cambiar entre versiones de macOS.
- **Firma:** macOS usa firma ad-hoc (clic derecho → Abrir; los permisos TCC están atados a la firma, así que al reinstalar puede pedirlos otra vez). Windows sin firma Authenticode: SmartScreen va a avisar.
- **Windows sin el servicio SYSTEM (T-027):** el mouse remoto no funciona en diálogos UAC ni en la pantalla de inicio de sesión.
- **Escala en Windows:** la topología no da los DPI y las pantallas de la PC van con escala 1; la velocidad del cursor entre monitores puede sentirse distinta (T-074).
- **Corte de corriente durante un cambio que usa la Mac:** la Mac espera hasta ~130 s antes de marcar la PC como apagada (T-063).
- **Monitores en pausa:** desde T-092 Tandem no cambia entradas; lo hará el firmware propio del 34", que reporta su estado real.

## Cómo se construyó

Un piloto de agentes en paralelo (hasta 11 tareas a la vez), cada tarea en su worktree: implementar → revisión independiente → correcciones → CI en las tres plataformas → merge a `develop` desde la sesión principal. Las rondas de QA usan `tandem-sim`, un puesto simulado con fallos inyectados. Detalle en `docs/dev/agents.md`, estrategia de pruebas en `docs/TESTING.md`, diseño en `docs/DESIGN.md`.
