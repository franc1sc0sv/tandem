# S2 — Quitar y devolver pantallas en macOS

> Estado (2026-10-15): implementado en `tandem-display` (T-012), compilado en CI `macos-latest`.
> **Todavía no se ha probado en la Mac real**: lo de abajo es el diseño y lo que hay que
> comprobar. Los resultados de la Mac se agregan en la sección final.

## Qué hace falta (DESIGN §8 y §8.1)

- **Fantasma**: quitar del escritorio de macOS el monitor externo que está mostrando la PC.
- **Pantalla integrada**: Apagada (predeterminado), Espejo o Extendida.
- Nunca dejar la Mac sin pantalla.

## APIs

| Qué | API | Pública | Notas |
|---|---|---|---|
| Listar | `CGGetOnlineDisplayList` | sí | Ids `CGDirectDisplayID`. Incluye espejos; puede **omitir** las desactivadas. |
| Datos | `CGDisplayIsBuiltin/IsMain/IsActive/MirrorsDisplay/VendorNumber/ModelNumber/SerialNumber/Bounds` | sí | |
| Nombre del modelo | `CoreDisplay_DisplayCreateInfoDictionary` → `DisplayProductName` | **no** | Diccionario `{ locale: nombre }` con el nombre del EDID (`LC34G55T`). |
| Quitar/devolver | `CGSConfigureDisplayEnabled(config, id, bool)` | **no** (SkyLight) | La que usan BetterDisplay y otras. Va dentro de `CGBeginDisplayConfiguration` / `CGCompleteDisplayConfiguration`. |
| Espejo | `CGConfigureDisplayMirrorOfDisplay(config, id, master)` | sí | `master = kCGNullDirectDisplay` deshace el espejo. |

Decisiones:

- **FFI directo, sin crates nuevos.** Las dos funciones privadas se resuelven con `dlsym`
  (primero en las imágenes cargadas, luego con `dlopen` de SkyLight / CoreDisplay). Si Apple
  las quita, el binario sigue cargando: `set_enabled` devuelve `PrivateApiUnavailable` y la
  pantalla integrada cae a **Espejo** (el respaldo que pide §8.1). El nombre queda en `None`
  y el monitor no se puede identificar por modelo.
- **Alcance de la configuración: `kCGConfigureForSession`.** `ForAppOnly` revertiría al salir el
  proceso; parece una red de seguridad, pero un CLI de diagnóstico no podría dejar nada
  aplicado y no está claro que SkyLight respete ese alcance para pantallas desactivadas.
  La vuelta atrás la da Tandem (`restore_all`, `RestoreGuard`). Revisar tras probar en la Mac.
- **"Activa" = `CGDisplayIsActive` o en espejo.** En un grupo de espejo por hardware solo la
  principal cuenta como activa; la que refleja no es un fantasma.

## Lógica (portable, `tandem_display::quartz::MacTopology`)

- Identifica los monitores por el modelo del EDID (`MonitorConfig::edid_model`), sin
  distinguir mayúsculas; la integrada nunca coincide. Dos iguales = error `Ambiguous`.
- **Recuerda lo que desactivó** con su última foto: si macOS deja de listar la pantalla, se
  sigue pudiendo encontrar y devolver por su id.
- Antes de desactivar: rechaza si es la última pantalla activa (`LastDisplay`); deshace los
  espejos que dependen de ella (si la integrada reflejaba al 34" y el 34" se va, la integrada
  pasa a extendida en vez de quedarse negra) y saca a la propia pantalla de su grupo de espejo.
- Espejo de la integrada: refleja al externo principal o, si la principal es la integrada,
  al primer externo activo.
- Todo es idempotente: repetir un paso no vuelve a configurar nada.
- Pruebas con `FakeQuartz` (incluye una de propiedades: con cualquier secuencia de pasos y
  fallos, nunca queda la Mac sin pantalla, nada refleja a una desactivada y `restore_all`
  lo devuelve todo).

## Dudas que solo resuelve la Mac

1. ¿`CGSConfigureDisplayEnabled` existe y funciona en el macOS actual del M5, para un
   externo por HDMI **y** para la integrada?
2. Una pantalla desactivada, ¿sigue en `CGGetOnlineDisplayList`? (La lógica soporta ambos.)
3. Al reactivarla, ¿conserva el mismo `CGDirectDisplayID`?
4. ¿`CoreDisplay_DisplayCreateInfoDictionary` existe en Apple Silicon y da `DisplayProductName`?
   Si no, plan B: `NSScreen.localizedName` (solo pantallas activas) o IORegistry.
5. Una pantalla en reposo (display sleep) aparece como no activa: ¿afecta a la regla de
   "última pantalla"?
6. ¿La configuración sobrevive a dormir/despertar la Mac y a desconectar el cable?

## Resultados en la Mac

_Pendiente: ejecutar `cargo test -p tandem-display -- --ignored` en `mac-runner` (agente
hardware-verifier) y anotar aquí la versión de macOS y la respuesta a cada duda._
