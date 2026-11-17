# Fixtures de `tandem-env`

Salidas de las herramientas que lee el escaneo, tal cual (bytes incluidos: `wsl.exe` escribe
UTF-16LE y Windows termina las líneas con CRLF). Las usa `tests/fixtures.rs`.

| Archivo | Comando | Origen |
|---|---|---|
| `windows-wsl-list.utf16` | `wsl.exe -l -v` | PC real (Windows 11) |
| `windows-wsl-ubuntu-probe.txt` | `wsl.exe -d Ubuntu-24.04 -e bash -lic <PROBE_SCRIPT>` | PC real (rutas de usuario cambiadas) |
| `windows-docker-version.json` | `docker version --format json` | PC real (Docker Desktop) |
| `windows-docker-runtimes.txt` | `docker info --format '{{range $k, $v := .Runtimes}}{{$k}} {{end}}'` | PC real |
| `windows-nvidia-smi.csv` | `nvidia-smi <NVIDIA_QUERY> --format=csv,noheader,nounits` | PC real |
| `windows-cim-gpu.json` | `Get-CimInstance Win32_VideoController … \| ConvertTo-Json` | PC real |
| `windows-powershell5-version.txt` | `powershell -Command $PSVersionTable.PSVersion.ToString()` | PC real |
| `windows-cmd-ver.txt` | `cmd /d /c ver` | PC real |
| `windows-tools.txt` | `<herramienta> --version`, una sección `@@<herramienta>` por cada una | PC real |
| `macos-zsh-probe.txt` | `zsh -lic <PROBE_SCRIPT>` | Formato de macOS; la Mac no estaba al alcance (reemplazar en la verificación en hardware) |
| `macos-system-profiler-displays.json` | `system_profiler SPDisplaysDataType -json` | Ídem |
| `macos-ioreg-accelerator.txt` | `ioreg -r -d 1 -c IOAccelerator` | Ídem |
| `macos-docker-version.json` | `docker version --format json` | Ídem |
