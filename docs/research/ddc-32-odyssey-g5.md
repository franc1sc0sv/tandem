# DDC/CI — Samsung Odyssey G5 32" (EDID "Odyssey G5", id PnP SAM7489)

Pruebas del 2026-10-16 desde Windows (PC → monitor por DisplayPort) con `tandem-ddc`, con el usuario mirando el monitor (T-050).

## Capabilities string

```
(prot(monitor)type(lcd)model(FALCON)cmds(01 02 03 07 0C E3 F3)vcp(02 04 05 08 10 12 14(05 08 0B 0C) 16 18 1A 52
 60(01 03 04 11 12 0F 10) 62 8D FF)mswhql(1)mccs_ver(2.0)asset_eep(32)mpu_ver(01))
```

Declara `60 = {01, 03, 04, 11, 12, 0F, 10}`. Igual que el 34", el código que obedece para HDMI **no está en esa lista**.

## Resultados

| VCP 0x60 | Efecto |
|---|---|
| `0x0F` | ✅ pasa a DisplayPort (PC) |
| `0x05` | ✅ pasa a HDMI (Mac) |
| `0x06` | se va a una entrada sin señal (negro); `0x0F` lo recupera |
| `0x11`, `0x12`, `0x03`, `0x04`, `0x01` | sin efecto |

## La pista que lo resolvió

Leer `0x60` estando en HDMI devolvió `0x05`, y el 34" devuelve siempre `0x09`, que es su código real de DP. Estos Samsung **reportan el código que usan para la entrada en la que están**, aunque no reporten bien cuando cambian. Para un monitor nuevo, leer `0x60` en cada entrada es el primer candidato a probar.

## Perfil

`hdmi = 0x05`, `display_port = 0x0F` ([`profiles/ddc/SAM7489.toml`](../../profiles/ddc/SAM7489.toml), configs de `docs/examples/`). El nombre "Odyssey G5" lo comparten varios modelos, así que el perfil se aplica por id PnP (`SAM7489`).

## Pendiente: ¿el timing report distingue la entrada? (T-085)

Tras un cambio desde el menú del monitor, `0x60` no se actualiza (sigue con el último código escrito por DDC), así que Tandem no puede saber solo qué muestra el 32": el estado arranca sin confirmar y lo confirma «Aplicar modo» o el usuario (T-085).

Falta probar **con el usuario delante** si el *timing report* (`GetTimingReport` de dxva2, o el VCP de estado de sincronía) cambia según la entrada que muestra: con el 32" en DP (PC), en HDMI (Mac) y en negro, leerlo desde la PC y comparar frecuencias, polaridad y el bit de «fuera de rango / sin señal». En el 34" no sirve (siempre reporta lo de su DP aunque muestre HDMI). Si en el 32" sí distingue, sería una capacidad nueva por perfil, «detección de entrada», y ese monitor podría arrancar confirmado sin preguntar.

- [ ] Leer el timing report del 32" en DP, en HDMI y en negro, y anotar los valores aquí.
