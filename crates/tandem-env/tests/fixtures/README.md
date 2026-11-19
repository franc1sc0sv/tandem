# `tandem-env` fixtures

Outputs of the tools the scan reads, as they are (bytes included: `wsl.exe` writes
UTF-16LE and Windows ends lines with CRLF). Used by `tests/fixtures.rs`.

| File | Command | Source |
|---|---|---|
| `windows-wsl-list.utf16` | `wsl.exe -l -v` | Real PC (Windows 11) |
| `windows-wsl-ubuntu-probe.txt` | `wsl.exe -d Ubuntu-24.04 -e bash -lic <PROBE_SCRIPT>` | Real PC (user paths changed) |
| `windows-docker-version.json` | `docker version --format json` | Real PC (Docker Desktop) |
| `windows-docker-runtimes.txt` | `docker info --format '{{range $k, $v := .Runtimes}}{{$k}} {{end}}'` | Real PC |
| `windows-nvidia-smi.csv` | `nvidia-smi <NVIDIA_QUERY> --format=csv,noheader,nounits` | Real PC |
| `windows-cim-gpu.json` | `Get-CimInstance Win32_VideoController … \| ConvertTo-Json` | Real PC |
| `windows-powershell5-version.txt` | `powershell -Command $PSVersionTable.PSVersion.ToString()` | Real PC |
| `windows-cmd-ver.txt` | `cmd /d /c ver` | Real PC |
| `windows-tools.txt` | `<tool> --version`, one `@@<tool>` section for each | Real PC |
| `macos-zsh-probe.txt` | `zsh -lic <PROBE_SCRIPT>` | macOS format; the Mac was not within reach (replace during hardware verification) |
| `macos-system-profiler-displays.json` | `system_profiler SPDisplaysDataType -json` | Same |
| `macos-ioreg-accelerator.txt` | `ioreg -r -d 1 -c IOAccelerator` | Same |
| `macos-docker-version.json` | `docker version --format json` | Same |
