# DDC/CI — Samsung Odyssey G5 32" (EDID "Odyssey G5", PnP id SAM7489)

Tests of 2026-10-16 from Windows (PC → monitor over DisplayPort) with `tandem-ddc`, with the user watching the monitor (T-050).

## Capabilities string

```
(prot(monitor)type(lcd)model(FALCON)cmds(01 02 03 07 0C E3 F3)vcp(02 04 05 08 10 12 14(05 08 0B 0C) 16 18 1A 52
 60(01 03 04 11 12 0F 10) 62 8D FF)mswhql(1)mccs_ver(2.0)asset_eep(32)mpu_ver(01))
```

It declares `60 = {01, 03, 04, 11, 12, 0F, 10}`. Like the 34", the code it obeys for HDMI **is not in that list**.

## Results

| VCP 0x60 | Effect |
|---|---|
| `0x0F` | ✅ switches to DisplayPort (PC) |
| `0x05` | ✅ switches to HDMI (Mac) |
| `0x06` | goes to an input with no signal (black); `0x0F` recovers it |
| `0x11`, `0x12`, `0x03`, `0x04`, `0x01` | no effect |

## The clue that solved it

Reading `0x60` while on HDMI returned `0x05`, and the 34" always returns `0x09`, which is its real DP code. These Samsung monitors **report the code they use for the input they are on**, even though they do not report well when they change. For a new monitor, reading `0x60` on each input is the first candidate to try.

## Profile

`hdmi = 0x05`, `display_port = 0x0F` ([`profiles/ddc/SAM7489.toml`](../../profiles/ddc/SAM7489.toml), configs in `docs/examples/`). The name "Odyssey G5" is shared by several models, so the profile is applied by PnP id (`SAM7489`).

## Pending: does the timing report tell the input? (T-085)

After a change from the monitor's menu, `0x60` does not update (it keeps the last code written over DDC), so Tandem cannot know by itself what the 32" is showing: the state starts unconfirmed and "Apply mode" or the user confirms it (T-085).

It remains to test **with the user present** whether the *timing report* (dxva2's `GetTimingReport`, or the sync-status VCP) changes according to the input it shows: with the 32" on DP (PC), on HDMI (Mac) and on black, read it from the PC and compare frequencies, polarity and the "out of range / no signal" bit. On the 34" it is of no use (it always reports its DP's even when it shows HDMI). If on the 32" it does tell them apart, it would be a new per-profile capability, "input detection", and that monitor could start confirmed without asking.

- [ ] Read the 32"'s timing report on DP, on HDMI and on black, and note the values here.

## The EDID changes depending on the input (T-087)

A fact from the first test with both machines: the same monitor has a different PnP id on each input, so the PC (DisplayPort) and the Mac (HDMI) see it with different ids. The EDID serial number usually matches and serves to link the two identities; if not, it is linked by model or size and the user confirms it in the setup.

| | PC (DisplayPort) | Mac (HDMI) |
|---|---|---|
| 32" Odyssey G5 | `SAM7489` | `SAM748A` |

That is why the profile carries `pnp_aliases` (`profiles/ddc/`) and the desk, `edid_aliases` in each `[[monitors]]`: either identity finds the monitor (DDC, Windows topology and Quartz). Before T-087 the sync treated them as two monitors and the desk ended up duplicated.
