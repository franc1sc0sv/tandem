# Hardware tests

## Windows PC (this machine)

- From WSL: `/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe`. The sandbox blocks the WSL→Windows bridge, so each use requires approval (this is intentional).
- Windows binaries: `cargo build --target x86_64-pc-windows-gnu` (requires `sudo apt install -y mingw-w64`), copy them to `C:\Users\PC\AppData\Local\Temp\tandem\` and run them through interop.
- **Monitors:** Tandem does control them over DDC/CI (DESIGN §8, D11). The tests that change inputs (T-110, T-123) are opt-in with `--with-monitors` and carry automatic revert: an independent watchdog restores the input even if the test fails (A8). They are never run without that revert, because the user could be left unable to see the PC.

## Mac

- `ssh mac-runner` (defined in `~/.ssh/config` with `HostName <mac-ip>` and `User <user>`; that data is not versioned), with the WSL key authorized (T-051, 2026-11-16). Over Wi-Fi.
- MacBook Pro Apple Silicon, macOS 27.0.1. Tools: Xcode CLT; install rustup for the user if missing (`curl https://sh.rustup.rs -sSf | sh -s -- -y`).
- If it stops responding: check that **Remote Login** is still on (`sudo systemsetup -getremotelogin`) and the IP (`ipconfig getifaddr en0`).

## Automatic suite

`scripts/hw/run.sh` (T-123) runs the tests of both machines and leaves a report; see `docs/dev/hardware-suite.md`. Monitors are only touched with `--with-monitors` and with automatic revert.

## Without ssh to the Mac (SOC2, T-198)

The company policy forbids ssh to the Mac, so `run.sh` cannot drive it. Manual mode splits the suite in two halves that the user connects with one pasted command (`docs/reports/hw-2026-11-18-cable.md` is the first manual run and the source of the pitfalls below).

1. On the PC side (WSL): `scripts/hw/run.sh --mac-manual --mac-ip <mac-ip> [--pc-ip <pc-ip>] [--only link,tunnels,input] [--mac-results FILE]`. It builds the Windows binaries, provisions and starts only the PC agent, and prints **one command**. The ref (default `origin/develop`) must be pushed: the Mac clones it (`--repo URL` overrides the origin URL).
2. Paste that command on the Mac, in a Claude session or a terminal. It runs `bash -c ...`, so it works in zsh. It clones or updates `~/.cache/tandem-hw-src` at the commit, builds (with `CARGO_NET_GIT_FETCH_WITH_CLI=true`), provisions and starts `tandemd` by absolute path, runs `scripts/hw/mac-side.sh` and prints the results between `TANDEM-HW MAC RESULTS BEGIN/END` markers. The trap kills the agent and removes `/tmp/tdhw*` and `~/.cache/tandem-hw-*`.
3. Save the Mac output in the `--mac-results` file (printed by `run.sh`). The PC half keeps its agent up until the block appears (up to 30 min, `TANDEM_HW_MAC_WAIT`), then merges both sides into the usual report.
4. Remove the privacy permissions granted to the test binaries (Accessibility, Input Monitoring) in System Settings.

Windows tolerate skew between the halves: pair wait 15 min, input windows 7 min (+5 on the PC side), injection 2 min; each is overridable with `TANDEM_HW_PAIR_WAIT`, `TANDEM_HW_WINDOW` and `TANDEM_HW_KEEP`. The `input` group needs the user at the Mac (Accessibility and Input Monitoring).

Pitfalls it avoids: zsh does not split `$VAR` (the command is plain single-quoted words run by `bash`); `pkill -f` missed an agent started with a relative path (everything uses absolute paths); `--data-dir` must be absolute (T-197).

Local check without hardware: `scripts/hw/test-manual-sim.sh` runs both halves on Linux with `--sim` backends (not in CI; it builds in release).
