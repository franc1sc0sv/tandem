# Deskflow — cómo controlarlo desde fuera

Investigado sobre `deskflow/deskflow` master (`c46781f`, 2026-10-14, después de v1.27.0).
Lo marcado **(sin verificar)** no se probó en un equipo real.

## Procesos

- Binarios: `deskflow` (GUI), `deskflow-core` (servidor y cliente) y, solo en Windows, `deskflow-daemon` (servicio que corre como SYSTEM).
- Arranque: `deskflow-core server|client --settings <archivo.conf> [--new-instance]`. Es el único CLI que existe; todo lo demás va en el INI.
- En macOS el core está en `Deskflow.app/Contents/MacOS/deskflow-core`.

## Configuración (INI `Deskflow.conf`)

- `[core]`: `coreMode` (1 = cliente, 2 = servidor), `interface`, `port` (24800), `computerName`, `preventSleep`.
- `[client]`: `remoteHost`, que acepta **una lista separada por comas** (round-robin, desde la 1.26). Ahí va nuestra lista de respaldo: USB4 → Ethernet → Wi-Fi.
- `[security]`: `tlsEnabled`, `checkPeerFingerprints`, `certificate`.
- Desde la 1.27, cada equipo se declara como grupo `[computer_<nombre>]`.
- Layout del servidor: `<settingsPath>/deskflow-server.conf`, con secciones `links` y `options`. La sección `screens` ya se ignora.

## Cambios en vivo

- macOS: `SIGHUP` recarga el layout sin reiniciar.
- Windows: hay que reiniciar el core.
- Cambiar de rol = reescribir el INI y reiniciar (≈1–3 s).
- Detección de caída: keep-alive cada 3 s, peer muerto tras 3 fallos (≈9 s). Nuestro agente puede detectarla antes y forzar el cambio.

## IPC (protocolo interno, sin garantía de estabilidad)

- Socket local de Qt `deskflow-core`; en Windows también `deskflow-daemon`. Mensajes de texto `cmd=args\n`.
- Eventos: `connectionState`, `connectedClients`, `peerFingerprint`, `connectionRefused`, `secureSocket`.
- Comandos del core: solo `stop`. Comandos del daemon: `configFile=`, `start`, `stop`, `logLevel=`.
- → Fijamos la versión y aislamos esto en un adaptador.

## TLS

- Carpeta `tls/` con `deskflow.pem`, `trusted-servers` y `trusted-clients`; formato de línea `v2:sha256:<hex>`. Se relee en cada handshake.
- Rutas: macOS `~/Library/Deskflow/tls/`, Windows `C:\ProgramData\Deskflow\tls\`.
- **Se puede preprovisionar** desde nuestro emparejamiento, así que el usuario nunca ve el diálogo de huellas.

## Otros

- Arrastrar archivos: **eliminado** (#8459). Transferencia de archivos: no se va a hacer (#8104, #9648). → La hace Tandem por el plano de datos.
- Portapapeles: texto, HTML e imagen BMP; no copia archivos.
- Sin mDNS (#8965 cerrado). → El descubrimiento lo hace nuestro agente.
- macOS: requiere permiso de Accesibilidad. Quizás nuestro agente también lo necesite, por ser el proceso que lo lanza **(sin verificar)**.
- Windows: para que funcione en la pantalla de inicio de sesión y en UAC hay que pasar por el daemon.
- Licencia GPL-2.0 con excepción OpenSSL. Lo instalamos aparte (`brew tap deskflow/tap`, `winget install Deskflow.Deskflow`), no lo empaquetamos.
