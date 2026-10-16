# Perfiles DDC

Un perfil dice qué valor de `0x60` (Input Select) obedece un modelo de monitor para cada entrada. Hace falta porque muchos monitores no obedecen los valores MCCS estándar ni los que declaran en su capabilities string (los dos Samsung de `docs/research/` son el ejemplo).

## Dónde viven

- **Base del repo:** `profiles/ddc/<PNP>.toml`, uno por modelo, verificados en un equipo real. Van embebidos en el binario (`BUILTIN` en `crates/tandem-display/src/ddc/profiles.rs`).
- **Del usuario:** `<datos>/profiles/ddc/*.toml`, donde `<datos>` es el directorio de datos de Tandem (`%APPDATA%\Tandem` en Windows, `~/Library/Application Support/Tandem` en macOS). Con el mismo id PnP, **el del usuario gana** sobre el del repo.

Un archivo inválido no impide usar los demás: se ignora y se avisa con su nombre y el motivo (`tandem-ddc: aviso: perfil ignorado: <archivo>: <error>`). Dos archivos del usuario con el mismo id PnP: gana el primero por orden alfabético y el otro se reporta.

## Cómo se elige el perfil de un monitor

1. Por **id PnP** del EDID (fabricante + producto, por ejemplo `SAM7489`): es lo que identifica al modelo.
2. Si no hay, por **nombre del EDID** (descriptor 0xFC), comparado sin mayúsculas, solo si el nombre no es genérico: el perfil no tiene `generic_name = true` y ningún otro perfil usa ese nombre.
3. Si tampoco, `switch` usa los códigos MCCS estándar (HDMI `0x11`, DP `0x0F`), que muchos monitores no obedecen.

`tandem-ddc profile <monitor>` muestra el perfil que se aplicaría, de dónde sale y por qué coincide. `tandem-ddc list` da el id PnP de cada monitor.

## Formato

```toml
pnp_id = "SAM7489"                 # obligatorio: 3 letras mayúsculas + 4 dígitos hex
name = "Odyssey G5 32\" (SAM7489)" # obligatorio: nombre para mostrar
edid_names = ["Odyssey G5"]        # nombres del EDID con los que se anuncia
generic_name = true                # el nombre lo comparten otros modelos: solo por id PnP
notes = "0x06 lleva a una entrada sin señal (negro)."
verified_by = "quien lo probó"
verified_on = "2026-10-16"         # AAAA-MM-DD

[codes]                            # obligatorio: el valor de 0x60 que obedece
hdmi = 0x05
display_port = 0x0F
# hdmi2 = 0x06                     # opcionales, si el monitor tiene una segunda entrada
# display_port2 = 0x10

[reported]                         # opcional: lo que LEE 0x60 estando en cada entrada
hdmi = 0x05
```

- Los códigos van en hexadecimal (`0x05`) y no pueden ser `0x00`.
- Las claves desconocidas son un error (así un error de tipeo no pasa en silencio).
- `[reported]` es la pista de T-050: varios monitores reportan en `0x60` el código de la entrada en la que están, aunque no obedezcan el valor estándar. Sirve a la calibración (T-077) para probar primero lo más probable.

## Cómo aportar un perfil

1. Encuentra los códigos con `tandem-ddc switch <monitor> hdmi|dp --return-after 10 --code <hex>` (siempre vuelve solo) mirando el monitor. Anota también qué devuelve `tandem-ddc get <monitor> 60` estando en cada entrada.
2. Escribe el perfil en `<datos>/profiles/ddc/<PNP>.toml` y comprueba con `tandem-ddc profile <monitor>` que se aplica, y con un `switch` sin `--code` que funciona.
3. `tandem-ddc profile export <monitor>` imprime el TOML normalizado. Para sumarlo al repo: guárdalo como `profiles/ddc/<PNP>.toml` (el nombre del archivo debe ser el id PnP), agrégalo a `BUILTIN` en `crates/tandem-display/src/ddc/profiles.rs` y abre un PR con lo que probaste. La prueba `every_repo_file_is_embedded_and_valid` falla si falta en `BUILTIN`, si el archivo no es válido o si su nombre no coincide con el id PnP.
