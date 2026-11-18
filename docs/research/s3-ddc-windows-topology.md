# S3 — Does the DDC channel survive if Windows removes the monitor from the desktop?

Spike S3 of `docs/DESIGN.md` §11, done inside T-011. Code:
`crates/tandem-display/src/topology/` (portable) and `crates/tandem-display/src/windows/topology.rs`.

## Why it matters

In Shared or Mac-only mode, the PC has to remove from its desktop the monitor that the Mac is
showing (the "ghost", DESIGN §8). But the PC is the one that changes inputs over DDC.
If removing the monitor loses the DDC channel, the PC can no longer return it to DisplayPort.

## Real inventory (2026-10-15, read-only)

Test `windows::topology::tests::lists_real_displays_with_edid`, run on the PC from WSL:

```
LC34G55T (SAM711A, serial HNTX501396) at 0x15070/4355 [active]
Odyssey G5 (SAM7489, serial HNAX100823) at 0x15070/4357 [active]
```

- `QueryDisplayConfig(QDC_ALL_PATHS)` + `DisplayConfigGetDeviceInfo` give the name and the interface
  path of each monitor; the full EDID (with serial number) comes from the registry with the
  same path the DDC backend already uses. The two monitors are told apart without ambiguity by EDID.
- Both hang off the same adapter (the RTX 5080), each on its own target.

## What is known without disabling anything

1. **A new enumeration through `dxva2` cannot see a disabled monitor.** The Monitor
   Configuration API reaches physical monitors only from an `HMONITOR`
   (`GetPhysicalMonitorsFromHMONITOR`), and Windows only creates `HMONITOR`s for screens that
   are part of the desktop. This comes from the API's design, not from a test: with the
   monitor out of the topology, `WindowsDdc::open()` does not list it.
2. **A doubt remains that only the hardware answers:** whether a physical handle opened *before*
   disabling keeps working afterwards. Windows does not document it; the driver might invalidate it.
3. Reading `0x60` is no use to verify the input (it always returns `0x09`, see
   [ddc-34-LC34G55T.md](ddc-34-LC34G55T.md)); for S3 it is enough that the read *responds*.

## Consequences for the design (without changing it)

- The "monitor → PC" transaction cannot depend on DDC over a disabled monitor. The safe order
  is **enable first in the topology and then send DDC** (the reverse of how DESIGN §5's example
  lists it, steps 2 and 3). Enabling does not need DDC: Windows sees the monitor
  over the DP link even if it shows HDMI, because the target stays "available".
- The "monitor → Mac" transaction has no problem: DDC first (the PC still has it active)
  and then it is removed from the desktop.
- Pending on hardware: confirm that the target stays `targetAvailable` with the monitor on HDMI
  (if the 34" cuts DP hot-plug when switching input, the PC could not re-enable it and it would
  have to go through NvAPI or plan C).

## Alternative if `dxva2` is not enough: I2C through NvAPI

The PC has an NVIDIA (RTX 5080), so NvAPI applies:

- `NvAPI_GPU_GetConnectedDisplayIds` lists the **connected** monitors, active or not, by NVIDIA
  display id; it does not depend on the desktop.
- `NvAPI_I2CWriteEx` / `NvAPI_I2CReadEx` speak I2C on that output's DDC port. DDC/CI is
  address `0x37` (on the bus, `0x6E` for writing) with register `0x51`; building the
  message and the checksum would be Tandem's job (they are simple and already described in MCCS).
- It is loaded at runtime from `nvapi64.dll` (`nvapi_QueryInterface`), linking
  nothing. NVAPI's public headers are MIT.
- Cost: a new backend (`DdcBus` over NvAPI) and a mapping between the NVIDIA id and the topology's
  target. NVIDIA only; no use for other GPUs.

Plan C (DESIGN §8): do not remove the monitor, just empty it and exclude it from the geometry.

## Result

| Question | Result |
|---|---|
| Does `dxva2` enumerate a disabled monitor? | **No** (by API design: without an `HMONITOR` there is no physical monitor). |
| Does a handle opened before disabling respond? | **Pending on hardware** (`s3_ddc_while_detached`). |
| Does the target stay available with the monitor on HDMI? | **Pending on hardware.** |
| Can it be disabled and re-enabled through `SetDisplayConfig` identifying by EDID? | Implemented and tested with the double; **pending on hardware** (`detach_and_attach_round_trip`). |

Recommendation meanwhile: design the transactions with the order "enable → DDC" and
"DDC → disable", which do not depend on doubt 2. NvAPI is only needed if the hardware
shows that the target stops being available.

## How to repeat the tests

From WSL, with both monitors active over DP (the tests refuse to run with only one).
They remove the monitor for a few seconds and **return it by themselves**, also if they fail halfway; S3 only
reads VCP `0x60`, it does not change the input.

```
export CARGO_TARGET_DIR=/tmp/target-win
cargo test -p tandem-display --lib --target x86_64-pc-windows-gnu --no-run
# Run the .exe that cargo prints:
<path>/tandem_display-<hash>.exe --ignored --nocapture --test-threads=1 windows::topology
```

`TANDEM_TOPOLOGY_MONITOR` chooses the monitor (by default `LC34G55T`; any text from the
EDID works, such as the serial number).
