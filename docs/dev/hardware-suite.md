# Hardware suite (T-123)

End-to-end tests on the real PC and Mac, designed to run unattended (DECISIONS A8). **They never run in CI** and are not part of the routine `cargo test`.

## How to run it

From WSL, with the Mac through `ssh mac-runner` and the PC through `powershell.exe` (each use of the bridge asks for approval):

```
scripts/hw/run.sh                    # link, tunnels and synthetic input
scripts/hw/run.sh --with-monitors    # also changes the 34" input (with automatic revert)
scripts/hw/run.sh --keep-cache        # keeps the Mac's build cache
# Cleanup does not kill the watchdog: it waits for it to return by itself (60 s), checks the input and, if
# it is not the original, restores it with it. `--only monitors` requires `--with-monitors`.
scripts/hw/run.sh --sim              # everything on Linux, simulated backends: validates the orchestrator
scripts/hw/run.sh --plan             # prints the steps
```

Options: `--only link,tunnels,input,monitors`, `--mac-link DIR`, `--ref REF` (by default `origin/develop`; the suite's code has to be in that ref), `--skip-build`, `--mac HOST`, `--pc-ip`, `--mac-ip`. Exit: 0 with no failures, 1 with failures, 2 if the environment could not be set up. The report is left in `docs/reports/hw-<date>.md`.

## What it does

1. Aborts if a real `tandemd` is running on either machine (Windows's named pipe is one per user and would clash). It never stops it.
2. Extracts `REF` with `git archive`, builds for Windows in WSL (`x86_64-pc-windows-gnu`, requires `mingw-w64`) and on the Mac over ssh (`~/.cache/tandem-hw-src` and `~/.cache/tandem-hw-target`; deleted at the end except with `--keep-cache`).
3. Copies the binaries to a temporary folder per run (`%TEMP%\tandem-hw-<time>` and `/tmp/tdhw-<time>`), provisions test identities (constant seeds, unrelated to the real ones) with mutual trust already loaded and starts an agent on UDP port 47871 (different from the real 52000).
4. Runs the `tandem-hw run` checks on each machine and gathers one JSON line per result.
5. `trap`: whatever happens it kills the agents and the processes of the run folder, and deletes the temporary folders on both machines.

| Check | Covers | What it verifies |
|---|---|---|
| `pair` | H1 | the test agent sees the other paired machine and over which link |
| `ping` | T-093 | 50 control pings; fails if any is lost or the p99 exceeds 200 ms |
| `tunnel-process` | H2, T-103, T-104 | process on the other machine, its output, `kill` |
| `tunnel-files` | H2, T-106 | a 3 MiB file round trip, identical |
| `tunnel-port` | H2, T-106 | port forwarded to an echo server on the other machine |
| `tunnel-workspace` | H2, T-107 | git repo on the other side, commit there, comes back as a branch |
| `input-cross` + `input-latency` | H4, T-093 | synthetic movement (`SendInput` / `CGEventPost`) against each edge until the `Crossed` notice; the other machine judges p50/p99 (Wi-Fi 5/20 ms, cable 1/4 ms) |
| `monitor-roundtrip` | H3, T-109, T-110 | switches the 34" to the other input and back |
| `monitor-kill` | T-110 | kills the process halfway through the change: the watchdog has to revert by itself |

Tunnels run in both directions (PC to Mac and Mac to PC). Synthetic input too.

## Monitors: automatic revert

Every check that changes an input arms the two safety nets of `tandem-monitor`: `InputGuard` in the process and `tandem-monitor-guard` as a separate process. If the watchdog is not next to `tandem-hw`, the check is **skipped** without touching anything. `monitor-kill` adds a third net: if the watchdog did not return within 45 s, the test itself restores it and fails. PC only: the Mac does not change inputs. Messages do not carry the monitor's identifier or EDID.

## What it does not cover

It is listed at the end of each report (an hour of use, ES/LatAm layout, screen scales, real hook loss, dragging files with Finder, elevated installer, USB4 cable, real Claude session).

## Standalone tests

The same checks exist as `#[ignore = "hardware"]` tests in `crates/tandem-hw/tests/hardware.rs`, against an already started test agent (`TANDEM_HW_DATA_DIR`). `tandem-hw`'s helpers (`emit`, `sleep`, `echo-server`) serve as the test program inside the tunnels, so they do not depend on anything installed on the other machine.

## Run robustness

A real run hung for more than an hour; these are the causes and what the script does now.

- **Agent startup on the PC.** `Start-Process -FilePath tandemd.exe` from `powershell.exe` through WSL's interop bridge did not return: the child inherits the bridge's stdout/stderr handles and powershell does not finish while they stay open. Now `cmd.exe /d /s /c "set TANDEM_LOG=... && tandemd.exe ... > agent.out 2>&1"` is launched with `Start-Process -WindowStyle Hidden -PassThru` and no redirections: output goes to a file inside cmd and no shared handle remains. `Invoke-CimMethod Win32_Process -MethodName Create` was discarded because that process runs in session 0 and `SendInput` (synthetic input) would not work. The saved PID is `cmd`'s; cleanup uses `taskkill /F /T`.
- **Nothing without a timeout or stdin.** Every call to `powershell.exe` and `ssh` carries `< /dev/null` and `timeout -k 5` (60 s by default, `TANDEM_HW_TIMEOUT`; 900 s per `tandem-hw` run, `TANDEM_HW_STEP_TIMEOUT`; 3600 s per build, `TANDEM_HW_BUILD_TIMEOUT`). Only the copying `tar | ssh` reads stdin.
- **Mac over cable.** If `ssh mac-runner` does not respond (Wi-Fi), with `--mac-link DIR` or `TANDEM_HW_MAC_LINK` it retries with `-o HostName=DIR` (same user and key as the alias). Over the cable, the link IP between agents is that link-local address and the PC's is taken from its `169.254.*` adapter (or `--pc-ip`). There are no fixed IPs in the repo (D10); without `--mac-link`, the failure remains an environment error (code 2).
- **Git dependencies without credentials on the Mac.** The Mac has no access to GitHub (T-130) and must stay that way. If `Cargo.lock` carries `git+` sources, before copying the code `cargo vendor --locked` is run in WSL (where there is access) inside the extracted copy; its output is `.cargo/config.toml` and `vendor/` travels in the tar. The Mac's build does not use the network.
- **Cleanup and signals.** Long steps (builds, `tandem-hw run`) run in the background and the script waits with `wait`; this way Ctrl-C and SIGTERM run the trap immediately. Cleanup kills the whole tree of each background process (`killtree`), not just its subshell.

`TANDEM_HW_TARGET` changes the build directory (by default `~/.cache/tandem-target-hw`). Validation without hardware: `bash -n`, `--plan` and `--sim --ref HEAD`.
