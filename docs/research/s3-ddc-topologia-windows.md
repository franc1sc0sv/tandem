# S3 — ¿Sobrevive el canal DDC si Windows quita el monitor del escritorio?

Spike S3 de `docs/DESIGN.md` §11, hecho dentro de T-011. Código:
`crates/tandem-display/src/topology/` (portable) y `crates/tandem-display/src/windows/topology.rs`.

## Por qué importa

En el modo Compartido o Solo Mac, la PC tiene que quitar de su escritorio el monitor que está
mostrando la Mac (el "fantasma", DESIGN §8). Pero la PC es la que cambia las entradas por DDC.
Si al quitar el monitor se pierde el canal DDC, la PC ya no puede devolverlo a DisplayPort.

## Inventario real (2026-10-15, solo lectura)

Prueba `windows::topology::tests::lists_real_displays_with_edid`, ejecutada en la PC desde WSL:

```
LC34G55T (SAM711A, serie HNTX501396) en 0x15070/4355 [activo]
Odyssey G5 (SAM7489, serie HNAX100823) en 0x15070/4357 [activo]
```

- `QueryDisplayConfig(QDC_ALL_PATHS)` + `DisplayConfigGetDeviceInfo` dan el nombre y la ruta de
  interfaz de cada monitor; el EDID completo (con número de serie) sale del registro con la
  misma ruta que ya usa el backend DDC. Los dos monitores se distinguen sin ambigüedad por EDID.
- Los dos cuelgan del mismo adaptador (la RTX 5080), cada uno en su target.

## Lo que se sabe sin desactivar nada

1. **Una enumeración nueva por `dxva2` no puede ver un monitor desactivado.** La Monitor
   Configuration API llega a los monitores físicos solo a partir de un `HMONITOR`
   (`GetPhysicalMonitorsFromHMONITOR`), y Windows solo crea `HMONITOR` para las pantallas que
   forman parte del escritorio. Esto sale del diseño de la API, no de una prueba: con el
   monitor fuera de la topología, `WindowsDdc::open()` no lo lista.
2. **Queda una duda que solo responde el hardware:** si un handle físico abierto *antes* de
   desactivar sigue funcionando después. Windows no lo documenta; el driver podría invalidarlo.
3. La lectura de `0x60` no sirve para verificar la entrada (siempre devuelve `0x09`, ver
   [ddc-34-LC34G55T.md](ddc-34-LC34G55T.md)); para S3 alcanza con que la lectura *responda*.

## Consecuencias para el diseño (sin cambiarlo)

- La transacción "monitor → PC" no puede depender de DDC sobre un monitor desactivado. El orden
  seguro es **activar primero en la topología y después mandar DDC** (al revés de como lo
  lista el ejemplo de DESIGN §5, pasos 2 y 3). Activar no necesita DDC: Windows ve el monitor
  por el enlace DP aunque muestre HDMI, porque el target sigue "disponible".
- La transacción "monitor → Mac" no tiene problema: primero DDC (la PC todavía lo tiene activo)
  y después se quita del escritorio.
- Pendiente de hardware: confirmar que el target sigue `targetAvailable` con el monitor en HDMI
  (si el 34" corta el hot-plug de DP al cambiar de entrada, la PC no podría reactivarlo y habría
  que ir por NvAPI o por el plan C).

## Alternativa si `dxva2` no basta: I2C por NvAPI

La PC tiene una NVIDIA (RTX 5080), así que NvAPI aplica:

- `NvAPI_GPU_GetConnectedDisplayIds` lista los monitores **conectados**, activos o no, por id
  de pantalla de NVIDIA; no depende del escritorio.
- `NvAPI_I2CWriteEx` / `NvAPI_I2CReadEx` hablan I2C en el puerto DDC de esa salida. DDC/CI es
  la dirección `0x37` (en el bus, `0x6E` para escribir) con el registro `0x51`; el armado del
  mensaje y el checksum los haría Tandem (son simples y ya están descritos en MCCS).
- Se carga en tiempo de ejecución desde `nvapi64.dll` (`nvapi_QueryInterface`), sin enlazar
  nada. Los encabezados públicos de NVAPI son MIT.
- Costo: un backend nuevo (`DdcBus` sobre NvAPI) y un mapeo entre el id de NVIDIA y el target
  de la topología. Solo NVIDIA; no sirve para otras GPUs.

Plan C (DESIGN §8): no quitar el monitor, solo vaciarlo y excluirlo de la geometría.

## Resultado

| Pregunta | Resultado |
|---|---|
| ¿`dxva2` enumera un monitor desactivado? | **No** (por diseño de la API: sin `HMONITOR` no hay monitor físico). |
| ¿Responde un handle abierto antes de desactivar? | **Pendiente en hardware** (`s3_ddc_while_detached`). |
| ¿Sigue disponible el target con el monitor en HDMI? | **Pendiente en hardware.** |
| ¿Se puede desactivar y reactivar por `SetDisplayConfig` identificando por EDID? | Implementado y probado con el doble; **pendiente en hardware** (`detach_and_attach_round_trip`). |

Recomendación mientras tanto: diseñar las transacciones con el orden "activar → DDC" y
"DDC → desactivar", que no dependen de la duda 2. NvAPI solo hace falta si el hardware
muestra que el target deja de estar disponible.

## Cómo repetir las pruebas

Desde WSL, con los dos monitores activos por DP (las pruebas se niegan si hay uno solo).
Quitan el monitor unos segundos y lo **devuelven solas**, también si fallan a la mitad; S3 solo
lee el VCP `0x60`, no cambia la entrada.

```
export CARGO_TARGET_DIR=/tmp/target-win
cargo test -p tandem-display --lib --target x86_64-pc-windows-gnu --no-run
# Ejecutar el .exe que imprime cargo:
<ruta>/tandem_display-<hash>.exe --ignored --nocapture --test-threads=1 windows::topology
```

`TANDEM_TOPOLOGY_MONITOR` elige el monitor (por defecto `LC34G55T`; vale cualquier texto del
EDID, como el número de serie).
