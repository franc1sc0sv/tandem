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
