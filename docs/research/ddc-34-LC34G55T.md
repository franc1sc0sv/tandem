# DDC/CI — Samsung Odyssey G5 34" (LC34G55T)

Pruebas hechas el 2026-10-15 desde Windows (PC → monitor por DisplayPort, API `dxva2`).
Scripts en [`tools/windows/ddc/`](../../tools/windows/ddc/).

## Capabilities string (reportado por el monitor)

```
(prot(monitor)type(lcd)SAMSUNGcmds(01 02 03 07 0C E3 F3)
 vcp(02 04 05 08 10 12 14(05 08 0B 0C) 16 18 1A 52 60( 12 0F 10) AA(01 02 03 FF)
     AC AE B2 B6 C6 C8 C9 D6(01 04 05) DC(00 02 03 05 ) DF FD)
 mccs_ver(2.1)mswhql(1))
```

El monitor **declara** `60 = {12, 0F, 10}`, pero **no obedece** esos valores.

## Resultados

| VCP 0x60 escrito | Efecto observado |
|---|---|
| `0x03` | ✅ cambia a HDMI (imagen de la Mac) |
| `0x06` | ✅ cambia a HDMI (imagen de la Mac) |
| `0x09` / `0x0F` | ✅ regresa a DisplayPort (la secuencia mandó ambos; falta aislar cuál basta) |
| `0x09` estando ya en DP | provoca re-sincronización (~1–2 s en negro) |
| `0x01`, `0x02`, `0x04`, `0x05`, `0x07`, `0x08`, `0x0A`–`0x0E` | sin efecto |
| `0x10`–`0x1F` (incluye los declarados `0x12`, `0x10`) | sin efecto, también repitiendo la orden |

## Observaciones clave

- **Lectura de entrada inútil:** `0x60` siempre devuelve `0x09`, esté en DP o HDMI. El agente debe llevar el estado él mismo.
- **Timing report inútil para verificar:** sigue reportando 165 Hz de DP aunque el monitor esté mostrando HDMI.
- **El canal DDC por DP sigue vivo mientras el monitor muestra HDMI.** Windows sigue enumerando el monitor (por eso es "fantasma") y puede devolverlo a DP.
- **Auto Source Switch** (ahora activado) **no** cambia de entrada al perder la señal (se va a reposo) **ni** al reconectar el HDMI (solo muestra "HDMI connected").
- Errores sueltos de lectura `0xC0262589` aparecen al azar; no indican cambio.

## Lado Mac

BetterDisplay reporta "DDC channel: Unavailable" (falta anotar qué monitor y qué puerto). Hay que repetirlo
por el HDMI integrado y por el cable USB-C→HDMI.

## Pendiente

- [ ] Aislar si la vuelta funciona con `0x09` o con `0x0F`.
- [ ] Repetir todo con el 32" conectado por DP a la PC.
- [ ] Ver si Windows conserva el canal DDC con el monitor desactivado en la topología (`SetDisplayConfig`), o si hace falta I2C por NvAPI.
