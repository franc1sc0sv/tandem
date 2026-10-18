//! Configuración automática en el primer arranque (T-082, T-087): si no hay `tandem.toml`,
//! el agente lo escribe solo.
//!
//! - `machine` sale del sistema operativo (macOS = `mac`, Windows = `pc`).
//! - **El puesto empieza vacío** (T-087): la configuración generada no trae monitores. Los
//!   agrega el asistente de la app (o «Agregar monitor») a partir de lo que cada máquina
//!   ve conectado ahora ([`platform_scan`], [`seen`]); el agente no los inventa al
//!   arrancar ni al sincronizar con la otra máquina.
//! - Red, entrada y energía con sus valores por omisión y nada inventado: sin enlaces
//!   fijos, sin Wake-on-LAN, sin hub y sin decir dónde está el teclado (no hay forma de
//!   adivinar una MAC, un VID:PID ni el teclado que usa la persona).
//!
//! La pantalla integrada y los monitores sin EDID no se listan: no habría cómo encontrarlos
//! después. Un archivo que ya existe nunca se toca, aunque sea inválido.

use std::io;
use std::path::Path;

use tandem_core::{Input, Machine};
use tandem_display::ddc::{Edid, MonitorInfo};
use tandem_display::topology::Display;

use crate::capabilities::Seen;
use crate::config::{AgentConfig, ConfigError, DEFAULT_PORT};
use crate::paths;

/// Un monitor que esta máquina ve, con lo necesario para configurarlo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub edid: Edid,
    /// Borde izquierdo en el escritorio, si está en él: ordena de izquierda a derecha.
    pub x: Option<i32>,
    /// Por qué cable lo ve esta máquina, si el sistema lo dice.
    pub output: Option<Input>,
}

/// La máquina que es este sistema; `None` donde Tandem no corre de verdad (Linux).
pub fn machine_of_os() -> Option<Machine> {
    if cfg!(target_os = "macos") {
        Some(Machine::Mac)
    } else if cfg!(windows) {
        Some(Machine::Pc)
    } else {
        None
    }
}

/// Los monitores externos con EDID de una topología (activos o no).
pub fn from_displays<Id>(displays: &[Display<Id>]) -> Vec<Detected> {
    displays
        .iter()
        .filter(|d| !d.builtin)
        .filter_map(|d| {
            Some(Detected {
                edid: d.edid.clone()?,
                x: d.bounds.map(|b| b.x),
                output: d.output,
            })
        })
        .collect()
}

/// Los monitores con EDID del bus DDC (sin lugar en el escritorio).
pub fn from_ddc(monitors: &[MonitorInfo]) -> Vec<Detected> {
    monitors
        .iter()
        .filter_map(|m| {
            Some(Detected {
                edid: m.edid.clone()?,
                x: None,
                output: None,
            })
        })
        .collect()
}

/// `primary` más los de `extra` que no estén ya. Dos monitores son el mismo si coinciden
/// fabricante, producto y serie; se cuenta de a uno, así dos iguales sin serie siguen
/// siendo dos.
pub fn merge(primary: Vec<Detected>, extra: Vec<Detected>) -> Vec<Detected> {
    let key = |e: &Edid| (e.manufacturer.clone(), e.product_code, e.serial_number);
    let mut unclaimed: Vec<_> = primary.iter().map(|d| Some(key(&d.edid))).collect();
    let mut out = primary;
    for d in extra {
        let k = key(&d.edid);
        match unclaimed.iter_mut().find(|u| u.as_ref() == Some(&k)) {
            Some(slot) => *slot = None,
            None => out.push(d),
        }
    }
    out
}

/// Lo que ve esta máquina, con los backends del sistema. Bloquea (abre la topología y
/// el bus DDC): fuera del runtime.
pub fn platform_scan(machine: Machine) -> Result<Vec<Detected>, String> {
    match machine {
        #[cfg(windows)]
        Machine::Pc => {
            use tandem_display::ddc::DdcBus;
            use tandem_display::topology::DisplayTopology;
            let topo = tandem_display::windows::topology::WindowsTopology
                .displays()
                .map(|d| from_displays(&d));
            // El bus DDC solo ve los monitores activos; la topología, todos los conectados.
            let ddc = tandem_display::windows::ddc::WindowsDdc::open()
                .map(|bus| from_ddc(bus.monitors()));
            match (topo, ddc) {
                (Ok(t), Ok(d)) => Ok(merge(t, d)),
                (Ok(t), Err(e)) => {
                    tracing::warn!("bus DDC no disponible al detectar monitores: {e}");
                    Ok(t)
                }
                (Err(e), Ok(d)) => {
                    tracing::warn!("topología no disponible al detectar monitores: {e}");
                    Ok(d)
                }
                (Err(t), Err(d)) => Err(format!("topología: {t}; DDC: {d}")),
            }
        }
        #[cfg(target_os = "macos")]
        Machine::Mac => {
            use tandem_display::topology::DisplayTopology;
            tandem_display::macos::MacDisplays::new(tandem_display::macos::CgApi::new())
                .displays()
                .map(|d| from_displays(&d))
                .map_err(|e| e.to_string())
        }
        _ => Err(format!(
            "este sistema no es el de la {machine}: no se pueden detectar sus monitores"
        )),
    }
}

/// Lo que esta máquina anuncia de los monitores que ve (T-084): el id PnP y el mismo
/// nombre que tendrían en una configuración generada, de izquierda a derecha.
pub fn seen(detected: &[Detected]) -> Vec<Seen> {
    let mut sorted: Vec<&Detected> = detected.iter().collect();
    sorted.sort_by_key(|d| (d.x.is_none(), d.x.unwrap_or(0)));
    let labels = labels(&sorted);
    sorted
        .iter()
        .zip(labels)
        .map(|(d, name)| Seen {
            pnp_id: Some(d.edid.pnp_id()),
            model: d
                .edid
                .name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_owned),
            name,
            serial: Some(d.edid.serial_number).filter(|n| *n != 0),
            inches: d.edid.diagonal_inches(),
            output: d.output,
        })
        .collect()
}

/// `34"`; sin tamaño en el EDID, el modelo; si dos quedan iguales, se les agrega el modelo.
pub fn labels(sorted: &[&Detected]) -> Vec<String> {
    let model = |e: &Edid| {
        e.name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map_or_else(|| e.pnp_id(), str::to_owned)
    };
    let first: Vec<String> = sorted
        .iter()
        .map(|d| match d.edid.diagonal_inches() {
            Some(inches) => format!("{inches}\""),
            None => model(&d.edid),
        })
        .collect();
    first
        .iter()
        .zip(sorted)
        .map(|(label, d)| {
            let m = model(&d.edid);
            if first.iter().filter(|l| *l == label).count() > 1 && *label != m {
                format!("{label} {m}")
            } else {
                label.clone()
            }
        })
        .collect()
}

/// `tandem.toml` para `machine`, comentado y sin monitores: el puesto empieza vacío y los
/// monitores los agrega el asistente (T-087). Tampoco dice dónde está el teclado: sin hub
/// ni respuesta del usuario es «sin definir». Siempre es válido.
pub fn generate(machine: Machine) -> String {
    format!(
        "# Configuración generada por tandemd al arrancar por primera vez (T-082, T-087).
# Se puede editar: el agente no vuelve a generarla mientras el archivo exista.
machine = \"{machine}\"
# El puesto empieza vacío. Los monitores se agregan desde el asistente de la app (o con
# «Agregar monitor»); el agente nunca los crea solo ni al sincronizar con la otra máquina.
monitors = []

[network]
# Puerto UDP del agente. Si no se puede abrir, el agente prueba otros y anuncia el real
# por mDNS: la otra máquina lo encuentra igual en la red local.
port = {DEFAULT_PORT}
# Enlaces directos (USB4 y Ethernet) con la dirección fija de la otra máquina, si los hay:
# peers = [{{ kind = \"usb4\", addr = \"<ip>:<puerto>\" }}]

[input]
# Dónde está el teclado y el mouse lo dice el hub USB o el asistente (se guarda aquí como
# `keyboard = \"pc\"` o `\"mac\"`). Sin eso, la app dice «sin definir».
lock_shortcut = [\"ScrollLock\"]
clipboard = true
# Para que los roles cambien solos al mover el hub, agrega [input.hub] con el VID:PID
# del teclado y el mouse (`keyboard` y `mouse`).

[power]
# Sin ningún enlace con la otra máquina durante esto, se la da por apagada.
peer_lost_after_secs = 15
# Esta máquina se despierta por Wake-on-LAN (se le anuncia a la otra).
wakeable = true
# Para encender la otra máquina desde esta, agrega [power.wol] con la MAC de su tarjeta
# de red (`mac`).
"
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

/// Carga `path`; si no existe y `machine` es `Some`, la genera (sin monitores, ver
/// [`generate`]) y la escribe. Nunca reemplaza un archivo existente.
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
    tracing::info!(
        "configuración generada en {shown}: máquina {machine}, {} monitores",
        config.desk.monitors.len()
    );
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
    use tandem_display::topology::Bounds;

    use super::*;

    pub fn edid(pnp: (&str, u16), name: Option<&str>, serial: u32, size: Option<(u8, u8)>) -> Edid {
        Edid {
            manufacturer: pnp.0.into(),
            product_code: pnp.1,
            serial_number: serial,
            name: name.map(Into::into),
            serial_text: None,
            size_cm: size,
        }
    }

    /// El 34" (Odyssey G5 LC34G55T, SAM711A): 80 x 34 cm.
    pub fn m34() -> Edid {
        edid(
            ("SAM", 0x711A),
            Some("LC34G55T"),
            0x4D30_4E30,
            Some((80, 34)),
        )
    }

    /// El 32" (Odyssey G5, SAM7489): 70 x 39 cm.
    pub fn m32() -> Edid {
        edid(
            ("SAM", 0x7489),
            Some("Odyssey G5"),
            0x4D30_4E31,
            Some((70, 39)),
        )
    }

    fn display(edid: Option<Edid>, x: Option<i32>, builtin: bool) -> Display<u32> {
        Display {
            id: 0,
            name: edid
                .as_ref()
                .and_then(|e| e.name.clone())
                .unwrap_or_default(),
            device_path: String::new(),
            edid,
            builtin,
            main: false,
            active: x.is_some(),
            mirror_of: None,
            output: None,
            bounds: x.map(|x| Bounds {
                x,
                y: 0,
                width: 2560,
                height: 1440,
            }),
        }
    }

    /// El puesto real tal como lo ve la PC: el 32" a la derecha llega primero.
    fn pc_desk() -> Vec<Detected> {
        from_displays(&[
            display(Some(m32()), Some(3440), false),
            display(Some(m34()), Some(0), false),
        ])
    }

    /// Hallazgo del PR #56: lo detectado es lo que se anuncia (no los `[[monitors]]` del
    /// puesto), atado a los monitores de la configuración.
    #[test]
    fn what_is_detected_is_what_is_announced() {
        let seen = seen(&pc_desk());
        assert_eq!(
            seen,
            [
                Seen {
                    pnp_id: Some("SAM711A".into()),
                    model: Some("LC34G55T".into()),
                    name: "34\"".into(),
                    serial: Some(0x4D30_4E30),
                    inches: Some(34),
                    output: None,
                },
                Seen {
                    pnp_id: Some("SAM7489".into()),
                    model: Some("Odyssey G5".into()),
                    name: "32\"".into(),
                    serial: Some(0x4D30_4E31),
                    inches: Some(32),
                    output: None,
                },
            ]
        );
    }

    #[test]
    fn the_output_cable_comes_from_the_topology() {
        let mut d = display(Some(m34()), Some(0), false);
        d.output = Some(Input::DisplayPort);
        let got = from_displays(&[d]);
        assert_eq!(seen(&got)[0].output, Some(Input::DisplayPort));
    }

    #[test]
    fn a_generated_config_has_no_monitors_and_no_keyboard() {
        for machine in [Machine::Pc, Machine::Mac] {
            let text = generate(machine);
            let cfg = AgentConfig::from_toml(&text).unwrap();
            assert_eq!(cfg.machine, machine);
            assert!(cfg.desk.monitors.is_empty(), "el puesto empieza vacío");
            assert_eq!(cfg.network.port, DEFAULT_PORT);
            assert!(cfg.network.peers.is_empty());
            assert_eq!(cfg.power.wol, None, "sin MAC inventada");
            assert_eq!(cfg.input.hub, None, "sin VID:PID inventado");
            assert!(!cfg.input.keyboard_defined(), "nunca se supone el teclado");
            // Ni un dato de ejemplo que parezca real.
            for fake in ["9c:6b", "046d", "10.77.", "[[monitors]]"] {
                assert!(!text.contains(fake), "{fake} en\n{text}");
            }
        }
    }

    #[test]
    fn twins_get_distinct_labels() {
        let a = edid(("SAM", 0x711A), Some("LC34G55T"), 11, Some((80, 34)));
        let b = edid(("SAM", 0x711A), Some("LC34G55T"), 12, Some((80, 34)));
        let d: Vec<Detected> = [a, b]
            .into_iter()
            .map(|edid| Detected {
                edid,
                x: None,
                output: None,
            })
            .collect();
        let refs: Vec<&Detected> = d.iter().collect();
        assert_eq!(labels(&refs), ["34\" LC34G55T", "34\" LC34G55T"]);
    }

    #[test]
    fn ddc_fills_what_the_topology_misses() {
        let topo = vec![Detected {
            edid: m34(),
            x: Some(0),
            output: Some(Input::DisplayPort),
        }];
        let ddc = vec![
            Detected {
                edid: m34(),
                x: None,
                output: None,
            },
            Detected {
                edid: m32(),
                x: None,
                output: None,
            },
        ];
        let merged = merge(topo, ddc);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].x, Some(0));
        assert_eq!(merged[1].edid, m32());
    }

    #[test]
    fn displays_and_buses_without_edid_are_skipped() {
        assert!(from_displays(&[display(None, Some(0), false)]).is_empty());
        let info = |edid| MonitorInfo {
            index: 0,
            description: "Monitor PnP genérico".into(),
            device: r"\\.\DISPLAY1".into(),
            edid,
        };
        assert_eq!(from_ddc(&[info(None), info(Some(m32()))]).len(), 1);
    }

    #[test]
    fn writes_once_and_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Tandem").join("tandem.toml");
        let first = load_or_generate(&path, Some(Machine::Pc)).unwrap();
        assert!(first.generated);
        assert!(first.config.desk.monitors.is_empty());
        assert_eq!(AgentConfig::load(&path).unwrap(), first.config);
        // La segunda vez se lee el archivo tal cual: aunque alguien le agregue monitores.
        let mut text = std::fs::read_to_string(&path).unwrap();
        text = text.replace("monitors = []", "");
        text.push_str("\n[[monitors]]\nid = \"a\"\nlabel = \"a\"\nwiring = { pc = \"hdmi\" }\n");
        std::fs::write(&path, &text).unwrap();
        let again = load_or_generate(&path, Some(Machine::Pc)).unwrap();
        assert!(!again.generated);
        assert_eq!(again.config.desk.monitors.len(), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn an_invalid_file_is_an_error_not_a_new_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandem.toml");
        std::fs::write(&path, "machine = \"linux\"").unwrap();
        let err = load_or_generate(&path, Some(Machine::Pc)).unwrap_err();
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
