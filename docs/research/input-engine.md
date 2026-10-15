# Motor de teclado y mouse (`tandem-input`)

Investigación del 2026-10-15. Se leyó el código de lan-mouse (`f1b96bd`) y de Deskflow (`c46781f`), más la documentación de Microsoft y de Apple.
**Decisión: escribirlo desde cero.** lan-mouse y Deskflow sirven solo como referencia; **no se copia código de ninguno de los dos**, porque los dos son GPL.

## Por qué no usar lan-mouse

- Es GPL-3.0-or-later (crates `input-capture`, `input-emulation` e `input-event` 0.4.0). Usarlo obligaría a que Tandem también fuera GPL.
- Tiene huecos justo en nuestras dos plataformas:
  - En Windows el movimiento es relativo y con aceleración.
  - No maneja UIPI, UAC ni el escritorio seguro.
  - No tiene teclas multimedia ni portapapeles.
  - En macOS detecta los bordes con la caja que encierra a todas las pantallas.
  - Tiene bugs abiertos de modificadores, scroll y repetición de teclas.
- Su modelo de eventos se basa en evdev (Linux) y necesita un `LocalSet` de tokio.

## Modelo de eventos

- **Teclas:** código físico **USB HID** (página 0x07, y 0x0C para multimedia), equivalente a `KeyboardEvent.code`. Las tablas HID ↔ keycode de Mac ↔ scancode de Windows salen del crate `keycode` (MIT, tabla de Chromium).
- **Paso físico:** el sistema destino aplica su propia distribución de teclado, así que ES/LatAm, teclas muertas y AltGr/Option funcionan solos. Convertir a caracteres queda solo como respaldo opcional.
- **Modificadores:** reasignación configurable por dirección. Por defecto Mac→Win: Cmd→Ctrl, Ctrl→Win, Option→Alt, Option derecha→AltGr; al revés para Win→Mac. Se lleva el conjunto de teclas presionadas y **se sueltan todas** al salir, al desconectarse o por timeout.
- **Scroll:** dos tipos, v120 para la rueda y píxeles para el trackpad. Respetar el signo del "scroll natural".
- **Ojo con la Mac ISO:** las teclas `§`/`` ` `` están intercambiadas respecto a HID. Hay que comprobarlo con el teclado de la M5.

## macOS

- **Captura:** `CGEventTapCreate` en `kCGHIDEventTap`, tap activo (devolver NULL traga el evento), en un hilo propio con su CFRunLoop. Reactivarlo cuando llegue `kCGEventTapDisabledByTimeout`.
- **Permisos (TCC):** Accesibilidad para un tap activo y para inyectar; Input Monitoring si el tap es solo de escucha. El permiso queda atado a la firma del binario, así que hace falta una `.app` firmada de forma estable.
- **Congelar el cursor:** `CGDisplayHideCursor` + `SetsCursorInBackground` (API privada) + `CGAssociateMouseAndMouseCursorPosition(false)`, como hace Deskflow. Es mejor que lan-mouse, que reposiciona el cursor en cada movimiento.
- **Secure Event Input** (campos de contraseña): se detecta con `IsSecureEventInputEnabled()`. Se avisa en la UI y no se cruza de pantalla mientras esté activo.
- **Inyección:**
  - Teclado: `CGEventCreateKeyboardEvent` + `CGEventPost(kCGHIDEventTap)`, mandando también `kCGEventFlagsChanged` con los flags correctos. Las flechas llevan `NumericPad|SecondaryFn`. La repetición se marca con `kCGKeyboardEventAutorepeat`.
  - Mouse: eventos `*Dragged` mientras hay un botón presionado; `kCGMouseEventClickState` para doble y triple clic.
  - Scroll: `CGEventCreateScrollWheelEvent2` en píxeles.
  - Multimedia: `NSEvent` del tipo `SystemDefined` con subtipo 8.

## Windows

- **Captura:** hooks `WH_KEYBOARD_LL`/`WH_MOUSE_LL` para tragar la entrada, más Raw Input (`RIDEV_INPUTSINK`) para leer el movimiento sin aceleración.
  - **El hook se elimina en silencio si tarda más de `LowLevelHooksTimeout`** (máximo 1 s): hilo dedicado, callback mínimo que solo encola, y un watchdog que se autoprueba.
  - Ignorar los eventos con `LLMHF_INJECTED`/`LLKHF_INJECTED`, o marcar los nuestros con `dwExtraInfo`.
  - Filtrar el Ctrl falso que Windows manda junto con AltGr.
- **Inyección:**
  - Teclado: `SendInput` con `KEYEVENTF_SCANCODE` (+ `EXTENDEDKEY` para las teclas E0).
  - Mouse: **absoluto** (`MOVE|ABSOLUTE|VIRTUALDESK`, 0–65535), acumulando las fracciones. El proceso tiene que declararse **Per-Monitor-V2 DPI aware**.
  - Rueda: `mouseData` admite fracciones de 120.
- **UIPI y escritorio seguro:** un servicio SYSTEM duplica el token de `winlogon.exe`, pone `TokenUIAccess=1` y lanza el helper con `CreateProcessAsUser` en `winsta0`. El helper sigue los cambios de escritorio con `OpenInputDesktop`/`SetThreadDesktop`. Ctrl+Alt+Supr se manda con `SendSAS` desde el servicio. Todo esto exige un binario firmado (Authenticode) e instalado en Program Files.

## Geometría

- Se prueba el borde **de la pantalla donde está el cursor**, nunca la caja que encierra a todas.
- La posición se mapea como fracción a lo largo del borde compartido.
- Margen en las esquinas, atajo para bloquear el cursor en la pantalla actual, y velocidad escalada por DPI.
- La geometría se vuelve a leer cuando cambian las pantallas: `CGDisplayRegisterReconfigurationCallback` en Mac, `WM_DISPLAYCHANGE` en Windows.

## Transporte

- **QUIC (quinn):** el movimiento y el scroll van en **datagramas** (coalescidos, con número de secuencia y una resincronización absoluta periódica); teclas, botones, entrar/salir y portapapeles van en un **stream confiable y ordenado**.
- Latido de ~1 s. Al cambiar de enlace se sueltan todas las teclas y botones del cliente.
- Objetivos: menos de 5 ms de latencia agregada por cable y p99 menor a 10 ms por Wi-Fi.

## Portapapeles

- `arboard` 3.7 para leer y escribir: texto, HTML e imagen.
- Detección de cambios propia: `changeCount` de NSPasteboard en Mac, `AddClipboardFormatListener` en Windows.
- Evitar el eco recordando el hash de lo que escribimos nosotros. Los contenidos grandes se mandan perezosos: primero se anuncia el cambio y se transfiere al pegar.

## Estimación (un desarrollador con experiencia)

Unas **15–18 semanas** en total. Las partes más grandes son el servicio de escritorio seguro de Windows (2–3 semanas) y la captura de cada sistema (1.5–2 semanas cada una).

## Referencias (solo lectura, no copiar)

- lan-mouse: `input-capture/src/{macos.rs,windows/event_thread.rs}`, `input-emulation/src/{macos.rs,windows.rs}`.
- Deskflow: `src/lib/platform/{OSXComputer.mm,MSWindowsWatchdog.cpp,MSWindowsProcess.cpp,MSWindowsDesks.cpp,MSWindowsHook.cpp,MSWindowsKeyState.cpp}`.
- Microsoft: LowLevelKeyboardProc, SendInput, la política de uiAccess.
