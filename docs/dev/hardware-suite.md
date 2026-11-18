# Suite de hardware (T-123)

Pruebas de punta a punta en la PC y la Mac reales, pensadas para correr sin supervisión (DECISIONS A8). **Nunca corren en CI** y no forman parte de `cargo test` rutinario.

## Cómo se corre

Desde WSL, con la Mac por `ssh mac-runner` y la PC por `powershell.exe` (cada uso del puente pide aprobación):

```
scripts/hw/run.sh                    # enlace, túneles y entrada sintética
scripts/hw/run.sh --with-monitors    # además cambia la entrada del 34" (con vuelta automática)
scripts/hw/run.sh --keep-cache        # conserva la caché de compilación de la Mac
# La limpieza no mata al vigilante: espera a que vuelva solo (60 s), verifica la entrada y, si
# no es la original, la restaura con él. `--only monitors` exige `--with-monitors`.
scripts/hw/run.sh --sim              # todo en Linux, backends simulados: valida el orquestador
scripts/hw/run.sh --plan             # imprime los pasos
```

Opciones: `--only link,tunnels,input,monitors`, `--mac-link DIR`, `--ref REF` (por omisión `origin/develop`; el código de la suite tiene que estar en ese ref), `--skip-build`, `--mac HOST`, `--pc-ip`, `--mac-ip`. Salida: 0 sin fallos, 1 con fallos, 2 si no se pudo armar el entorno. El informe queda en `docs/reports/hw-<fecha>.md`.

## Qué hace

1. Aborta si hay un `tandemd` real corriendo en alguna máquina (la tubería con nombre de Windows es una por usuario y chocaría). Nunca lo detiene.
2. Extrae `REF` con `git archive`, compila para Windows en WSL (`x86_64-pc-windows-gnu`, requiere `mingw-w64`) y en la Mac por ssh (`~/.cache/tandem-hw-src` y `~/.cache/tandem-hw-target`; se borran al terminar salvo con `--keep-cache`).
3. Copia los binarios a una carpeta temporal por corrida (`%TEMP%\tandem-hw-<hora>` y `/tmp/tdhw-<hora>`), aprovisiona identidades de prueba (semillas constantes, sin relación con las reales) con la confianza mutua ya cargada y arranca un agente en el puerto UDP 47871 (distinto del 52000 real).
4. Corre las comprobaciones de `tandem-hw run` en cada máquina y junta una línea JSON por resultado.
5. `trap`: pase lo que pase mata los agentes y los procesos de la carpeta de la corrida, y borra las carpetas temporales de las dos máquinas.

| Comprobación | Cubre | Qué verifica |
|---|---|---|
| `pair` | H1 | el agente de prueba ve a la otra máquina emparejada y por qué enlace |
| `ping` | T-093 | 50 pings de control; falla si se pierde alguno o el p99 pasa de 200 ms |
| `tunnel-process` | H2, T-103, T-104 | proceso en la otra máquina, su salida, `kill` |
| `tunnel-files` | H2, T-106 | archivo de 3 MiB de ida y vuelta idéntico |
| `tunnel-port` | H2, T-106 | puerto reenviado hasta un servidor de eco de la otra máquina |
| `tunnel-workspace` | H2, T-107 | repo de git al otro lado, commit allá, vuelve como rama |
| `input-cross` + `input-latency` | H4, T-093 | movimiento sintético (`SendInput` / `CGEventPost`) contra cada borde hasta el aviso `Crossed`; la otra máquina juzga p50/p99 (Wi-Fi 5/20 ms, cable 1/4 ms) |
| `monitor-roundtrip` | H3, T-109, T-110 | cambia el 34" a la otra entrada y vuelve |
| `monitor-kill` | T-110 | mata el proceso a mitad del cambio: el vigilante tiene que volver solo |

Los túneles se corren en las dos direcciones (PC a Mac y Mac a PC). La entrada sintética también.

## Monitores: vuelta automática

Toda comprobación que cambia una entrada arma las dos redes de `tandem-monitor`: `InputGuard` en el proceso y `tandem-monitor-guard` como proceso aparte. Si el vigilante no está junto a `tandem-hw`, la comprobación se **omite** sin tocar nada. `monitor-kill` agrega una tercera red: si el vigilante no volvió en 45 s, la restaura la propia prueba y falla. Solo la PC: la Mac no cambia entradas. Los mensajes no llevan el identificador ni el EDID del monitor.

## Qué no cubre

Está listado al final de cada informe (una hora de uso, distribución ES/LatAm, escalas de pantalla, pérdida real del hook, arrastrar archivos con Finder, instalador elevado, cable USB4, sesión real de Claude).

## Pruebas sueltas

Las mismas comprobaciones existen como pruebas `#[ignore = "hardware"]` en `crates/tandem-hw/tests/hardware.rs`, contra un agente de prueba ya arrancado (`TANDEM_HW_DATA_DIR`). Los ayudantes de `tandem-hw` (`emit`, `sleep`, `echo-server`) sirven de programa de prueba dentro de los túneles, así no dependen de nada instalado en la otra máquina.

## Robustez de la corrida

Una corrida real se colgó más de una hora; estas son las causas y lo que hace ahora el script.

- **Arranque del agente en la PC.** `Start-Process -FilePath tandemd.exe` desde `powershell.exe` por el puente de interop de WSL no regresaba: el hijo hereda los handles de stdout/stderr del puente y powershell no termina mientras sigan abiertos. Ahora se lanza `cmd.exe /d /s /c "set TANDEM_LOG=... && tandemd.exe ... > agent.out 2>&1"` con `Start-Process -WindowStyle Hidden -PassThru` y sin redirecciones: la salida va a archivo dentro de cmd y no queda ningún handle compartido. Se descartó `Invoke-CimMethod Win32_Process -MethodName Create` porque ese proceso corre en la sesión 0 y `SendInput` (entrada sintética) no funcionaría. El PID guardado es el de `cmd`; la limpieza usa `taskkill /F /T`.
- **Nada sin tope ni stdin.** Toda llamada a `powershell.exe` y a `ssh` lleva `< /dev/null` y `timeout -k 5` (60 s por omisión, `TANDEM_HW_TIMEOUT`; 900 s por corrida de `tandem-hw`, `TANDEM_HW_STEP_TIMEOUT`; 3600 s por compilación, `TANDEM_HW_BUILD_TIMEOUT`). Solo el `tar | ssh` de copia lee stdin.
- **Mac por cable.** Si `ssh mac-runner` no responde (Wi-Fi), con `--mac-link DIR` o `TANDEM_HW_MAC_LINK` se reintenta con `-o HostName=DIR` (mismo usuario y llave del alias). Por el cable, la IP del enlace entre agentes es esa dirección link-local y la de la PC se toma de su adaptador `169.254.*` (o `--pc-ip`). No hay IPs fijas en el repo (D10); sin `--mac-link`, el fallo sigue siendo un error de entorno (código 2).
- **Dependencias git sin credenciales en la Mac.** La Mac no tiene acceso a GitHub (T-130) y así debe seguir. Si `Cargo.lock` trae fuentes `git+`, antes de copiar el código se corre `cargo vendor --locked` en WSL (donde sí hay acceso) dentro de la copia extraída; su salida es `.cargo/config.toml` y `vendor/` viaja en el tar. La compilación de la Mac no usa la red.
- **Limpieza y señales.** Los pasos largos (compilaciones, `tandem-hw run`) corren en segundo plano y el script espera con `wait`; así Ctrl-C y SIGTERM ejecutan el trap de inmediato. La limpieza mata el árbol completo de cada proceso en segundo plano (`killtree`), no solo su subshell.

`TANDEM_HW_TARGET` cambia el directorio de compilación (por omisión `~/.cache/tandem-target-hw`). Validación sin hardware: `bash -n`, `--plan` y `--sim --ref HEAD`.
