//! Configuración automática en el primer arranque (T-082, T-087): si no hay `tandem.toml`,
//! el agente lo escribe solo.
//!
//! - `machine` empieza en `a`: el lado de verdad lo deciden las claves al emparejar
//!   (DECISIONS A1), no el sistema operativo.
//! - **El puesto empieza vacío**: sin `[[screens]]`, cada máquina usa las pantallas que lee
//!   el motor de entrada y A va a la izquierda de B (DESIGN §7).
//! - Red, entrada y energía con sus valores por omisión y nada inventado: sin enlaces
//!   fijos, sin Wake-on-LAN, sin hub y sin decir dónde está el teclado (no hay forma de
//!   adivinar una MAC, un VID:PID ni el teclado que usa la persona).
//!
//! Un archivo que ya existe nunca se toca, aunque sea inválido.

use std::io;
use std::path::Path;

use tandem_core::Machine;

use crate::config::{AgentConfig, ConfigError, DEFAULT_PORT};
use crate::paths;

/// El lado con que se genera la configuración: `A` hasta que el emparejamiento diga otra
/// cosa. `None` donde Tandem no corre de verdad (Linux): ahí no se genera sola.
pub fn initial_side() -> Option<Machine> {
    cfg!(any(target_os = "macos", windows)).then_some(Machine::A)
}

/// `tandem.toml` con el lado `machine`, comentado y con el puesto vacío. Tampoco dice dónde
/// está el teclado: sin hub ni respuesta del usuario es «sin definir». Siempre es válido.
pub fn generate(machine: Machine) -> String {
    format!(
        "# Configuración generada por tandemd al arrancar por primera vez (T-082, T-087).
# Se puede editar: el agente no vuelve a generarla mientras el archivo exista.
# El lado de esta máquina (`a` o `b`). Lo decide la clave pública al emparejar: la menor
# es `a`. El agente lo corrige solo.
machine = \"{side}\"
# El acomodo de pantallas para el teclado y el mouse (DESIGN §7). Sin `[[screens]]`, cada
# máquina usa las pantallas que lee ella misma, y A va a la izquierda de B. Para
# declararlo a mano (vale para las dos máquinas, se sincroniza solo):
# [[screens]]
# machine = \"b\"
# id = \"34\"
# x = 0
# y = 0
# width = 3440
# height = 1440
# scale = 1.0
#
# [arrangement]
# a_side = \"left\"
# align = \"center\"
#
# Lo compartido (acomodo, atajos, preferencias, módulos) se cambia desde la app de
# cualquiera de las dos máquinas; el agente lo escribe aquí.

[network]
# Puerto UDP del agente. Si no se puede abrir, el agente prueba otros y anuncia el real
# por mDNS: la otra máquina lo encuentra igual en la red local.
port = {DEFAULT_PORT}
# Enlaces directos (USB4 y Ethernet) con la dirección fija de la otra máquina, si los hay:
# peers = [{{ kind = \"usb4\", addr = \"<ip>:<puerto>\" }}]

[input]
# Dónde está el teclado y el mouse lo dice el hub USB o el asistente (se guarda aquí como
# `keyboard = \"a\"` o `\"b\"`). Sin eso, la app dice «sin definir».
# Para que los roles cambien solos al mover el hub, agrega [input.hub] con el VID:PID
# del teclado y el mouse (`keyboard` y `mouse`).

[power]
# Sin ningún enlace con la otra máquina durante esto, se la da por apagada.
peer_lost_after_secs = 15
# Esta máquina se despierta por Wake-on-LAN (se le anuncia a la otra).
wakeable = true
# Para encender la otra máquina desde esta, agrega [power.wol] con la MAC de su tarjeta
# de red (`mac`).
",
        side = machine.as_str()
    )
}

#[derive(Debug, thiserror::Error)]
pub enum AutoConfigError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("no se pudo escribir la configuración generada en {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error(
        "no hay configuración en {path} y en este sistema no se genera sola (solo en macOS y Windows): pásala con --config"
    )]
    Unsupported { path: String },
}

/// La configuración de `path`, o la que se genera si no existe.
#[derive(Debug)]
pub struct Loaded {
    pub config: AgentConfig,
    /// Se acaba de generar.
    pub generated: bool,
}

/// Carga `path`; si no existe y `machine` es `Some`, la genera (ver [`generate`]) y la
/// escribe. Nunca reemplaza un archivo existente.
pub fn load_or_generate(path: &Path, machine: Option<Machine>) -> Result<Loaded, AutoConfigError> {
    match AgentConfig::load(path) {
        Ok(config) => {
            return Ok(Loaded {
                config,
                generated: false,
            });
        }
        Err(ConfigError::Read { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let shown = path.display().to_string();
    let Some(machine) = machine else {
        return Err(AutoConfigError::Unsupported { path: shown });
    };
    let text = generate(machine);
    let config = AgentConfig::from_toml(&text)?;
    let written = write_new(path, &text).map_err(|source| AutoConfigError::Write {
        path: shown.clone(),
        source,
    })?;
    if !written {
        // Otro (la UI, el usuario) la escribió entre medio: esa es la que vale.
        return Ok(Loaded {
            config: AgentConfig::load(path)?,
            generated: false,
        });
    }
    tracing::info!("configuración generada en {shown}: lado {machine} hasta emparejar");
    Ok(Loaded {
        config,
        generated: true,
    })
}

/// `false` si ya existía (no se toca).
fn write_new(path: &Path, text: &str) -> io::Result<bool> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        paths::create_private_dir(dir)?;
    }
    // `create_new`: no pisa uno que aparezca entre la lectura y la escritura.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            io::Write::write_all(&mut file, text.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_generated_config_has_an_empty_desk_and_no_keyboard() {
        for machine in [Machine::B, Machine::A] {
            let text = generate(machine);
            let cfg = AgentConfig::from_toml(&text).unwrap();
            assert_eq!(cfg.machine, machine);
            assert_eq!(
                cfg.desk,
                tandem_core::Config::default(),
                "el puesto empieza vacío"
            );
            assert!(cfg.warnings().is_empty());
            assert_eq!(cfg.network.port, DEFAULT_PORT);
            assert!(cfg.network.peers.is_empty());
            assert_eq!(cfg.power.wol, None, "sin MAC inventada");
            assert_eq!(cfg.input.hub, None, "sin VID:PID inventado");
            assert!(!cfg.input.keyboard_defined(), "nunca se supone el teclado");
            // Ni un dato de ejemplo que parezca real.
            for fake in ["9c:6b", "046d", "10.77.", "monitors", "\"mac\"", "\"pc\""] {
                assert!(!text.contains(fake), "{fake} en\n{text}");
            }
        }
    }

    #[test]
    fn writes_once_and_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Tandem").join("tandem.toml");
        let first = load_or_generate(&path, Some(Machine::B)).unwrap();
        assert!(first.generated);
        assert!(first.config.desk.screens.is_empty());
        assert_eq!(AgentConfig::load(&path).unwrap(), first.config);
        // La segunda vez se lee el archivo tal cual: aunque alguien le agregue pantallas.
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(
            "\n[[screens]]\nmachine = \"b\"\nid = \"a\"\nx = 0\ny = 0\nwidth = 10\nheight = 10\n",
        );
        std::fs::write(&path, &text).unwrap();
        let again = load_or_generate(&path, Some(Machine::B)).unwrap();
        assert!(!again.generated);
        assert_eq!(again.config.desk.screens.len(), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn an_invalid_file_is_an_error_not_a_new_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandem.toml");
        std::fs::write(&path, "machine = \"linux\"").unwrap();
        let err = load_or_generate(&path, Some(Machine::B)).unwrap_err();
        assert!(
            matches!(err, AutoConfigError::Config(ConfigError::Parse(_))),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "machine = \"linux\""
        );
    }

    #[test]
    fn without_a_known_os_it_asks_for_a_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandem.toml");
        let err = load_or_generate(&path, None).unwrap_err();
        assert!(matches!(err, AutoConfigError::Unsupported { .. }), "{err}");
        assert!(err.to_string().contains("--config"), "{err}");
        assert!(!path.exists());
    }
}
