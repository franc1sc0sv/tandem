# DDC/CI — Samsung Odyssey G5 34" (LC34G55T)

> **Update 2026-11-16 (v0.3):** the 34" now runs the **own firmware v3.1 (protocol 2)** from the `monitor-firmware` repo: `Get 0x60` **returns the real input** (patch v1, ADR-0009 of that repo) and the monitor gives change events and verdicts (VCP `0xEA`–`0xEF`). What follows describes the **factory** firmware and stays as history. Confirmed `Set 0x60` values: HDMI `0x06` (also `0x03`), DP `0x09` (`0x0F` also worked). DP = port 0, HDMI = port 1. From the Mac over HDMI the DDC channel only responds while HDMI is the active input (RC=01 otherwise).
>
> The 32" is not an MStar like the 34": it is the **Odyssey G5 "FALCON" (Novatek NT68500)**, EDID `0x7489` (DP) / `0x748A` (HDMI); **damaged on 2026-11-16** and being recovered over SPI. With factory firmware it will be "not supported" (DECISIONS D11).
>
> Contract with Tandem: `monitor-firmware/docs/integration/tandem.md`. In v0.3 control lives in the Rust crate `monitor-ddc` (DECISIONS A5), not in the Python HTTP service.

Tests done on 2026-10-15 from Windows (PC → monitor over DisplayPort, `dxva2` API).
Scripts in [`tools/windows/ddc/`](../../tools/windows/ddc/).

## Capabilities string (reported by the monitor)

```
(prot(monitor)type(lcd)SAMSUNGcmds(01 02 03 07 0C E3 F3)
 vcp(02 04 05 08 10 12 14(05 08 0B 0C) 16 18 1A 52 60( 12 0F 10) AA(01 02 03 FF)
     AC AE B2 B6 C6 C8 C9 D6(01 04 05) DC(00 02 03 05 ) DF FD)
 mccs_ver(2.1)mswhql(1))
```

The monitor **declares** `60 = {12, 0F, 10}`, but **does not obey** those values.

## Results

| VCP 0x60 written | Observed effect |
|---|---|
| `0x03` | ✅ switches to HDMI (the Mac's image) |
| `0x06` | ✅ switches to HDMI (the Mac's image) |
| `0x09` | ✅ returns to DisplayPort (isolated on 2026-10-16, T-050) |
| `0x09` while already on DP | causes a re-sync (~1–2 s of black) |
| `0x01`, `0x02`, `0x04`, `0x05`, `0x07`, `0x08`, `0x0A`–`0x0E` | no effect |
| `0x10`–`0x1F` (includes the declared `0x12`, `0x10`) | no effect, also when repeating the command |

## Key observations

- **(Factory firmware)** **Useless input reading:** `0x60` always returns `0x09`, whether on DP or HDMI. The agent must keep the state itself.
- **Timing report useless for verifying:** it keeps reporting DP's 165 Hz even though the monitor is showing HDMI.
- **The DDC channel over DP stays alive while the monitor shows HDMI.** Windows keeps enumerating the monitor (that is why it is a "ghost") and can return it to DP.
- **Auto Source Switch** (now enabled) does **not** change input when the signal is lost (it goes to standby) **nor** when HDMI is reconnected (it only shows "HDMI connected").
- Stray read errors `0xC0262589` appear at random; they do not indicate a change.

## Mac side

BetterDisplay reports "DDC channel: Unavailable" (which monitor and which port still need to be noted). It has to be repeated
over the built-in HDMI and over the USB-C→HDMI cable.

## Pending

- [x] Isolate whether the return works with `0x09` or with `0x0F`: `0x09` is enough (T-050).
- [x] Repeat everything with the 32" connected over DP to the PC: see `ddc-32-odyssey-g5.md`.
- [ ] See whether Windows keeps the DDC channel with the monitor disabled in the topology (`SetDisplayConfig`), or whether I2C through NvAPI is needed.

## The EDID changes depending on the input (T-087)

A fact from the first test with both machines: the same monitor has a different PnP id on each input, so the PC (DisplayPort) and the Mac (HDMI) see it with different ids. The EDID serial number usually matches and serves to link the two identities; if not, it is linked by model or size and the user confirms it in the setup.

| | PC (DisplayPort) | Mac (HDMI) |
|---|---|---|
| 34" LC34G55T | `SAM711A` | `SAM7119` |

That is why the profile carries `pnp_aliases` (`profiles/ddc/`) and the desk, `edid_aliases` in each `[[monitors]]`: either identity finds the monitor (DDC, Windows topology and Quartz). Before T-087 the sync treated them as two monitors and the desk ended up duplicated.
