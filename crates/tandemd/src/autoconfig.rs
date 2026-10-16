//! Configuración automática en el primer arranque (T-082): si no hay `tandem.toml`, el
//! agente lo escribe solo con lo que ve.
//!
//! - `machine` sale del sistema operativo (macOS = `mac`, Windows = `pc`).
//! - Los monitores salen del EDID: en la PC, de la topología de Windows y del bus DDC
//!   (dxva2); en la Mac, de Quartz. El id es el id PnP en minúsculas (`sam711a`), que es el
//!   mismo en las dos máquinas porque lo da el propio monitor; `edid_model` es el id PnP,
//!   la etiqueta es el tamaño (`34"`) o, sin tamaño, el modelo. Si la base de perfiles
//!   (T-079) conoce el modelo, sus códigos DDC van en `ddc`.
//! - Cableado por omisión: DisplayPort a la PC y HDMI a la Mac.
//! - Red, entrada y energía con sus valores por omisión y nada inventado: sin enlaces
//!   fijos, sin Wake-on-LAN y sin hub (no hay forma de adivinar una MAC ni un VID:PID).
//!
//! La pantalla integrada y los monitores sin EDID no entran: no habría cómo encontrarlos
//! después. Un monitor que esta máquina no ve (por ejemplo, uno que solo está cableado a
//! la otra) se agrega al sincronizar con ella (T-083).
//!
//! Un archivo que ya existe nunca se toca, aunque sea inválido.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use tandem_core::{Machine, MonitorConfig, MonitorId};
use tandem_display::ddc::profiles::ProfileDb;
use tandem_display::ddc::{Edid, MonitorInfo};
use tandem_display::topology::Display;

use crate::config::{AgentConfig, ConfigError, DEFAULT_PORT};
use crate::paths;

/// Un monitor que esta máquina ve, con lo necesario para configurarlo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub edid: Edid,
    /// Borde izquierdo en el escritorio, si está en él: ordena de izquierda a derecha.
    pub x: Option<i32>,
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

/// Los monitores de la configuración, de izquierda a derecha.
pub fn monitors(detected: &[Detected], db: &ProfileDb) -> Vec<MonitorConfig> {
    let mut sorted: Vec<&Detected> = detected.iter().collect();
    // Estable: los que no tienen lugar quedan al final, en el orden en que llegaron.
    sorted.sort_by_key(|d| (d.x.is_none(), d.x.unwrap_or(0)));
    let ids = ids(&sorted);
    let labels = labels(&sorted);
    sorted
        .iter()
        .zip(ids)
        .zip(labels)
        .map(|((d, id), label)| {
            let info = MonitorInfo {
                index: 0,
                description: String::new(),
                device: String::new(),
                edid: Some(d.edid.clone()),
            };
            let ddc = db.find(&info).map(|(e, _)| tandem_core::DdcProfile {
                hdmi: e.profile.codes.hdmi,
                display_port: e.profile.codes.display_port,
            });
            MonitorConfig {
                id,
                label,
                edid_model: Some(d.edid.pnp_id()),
                wiring: default_wiring(),
                ddc,
            }
        })
        .collect()
}

/// DisplayPort a la PC y HDMI a la Mac: lo habitual (la MacBook sale por HDMI o USB-C a
/// HDMI; la tarjeta de la PC tiene sobre todo DisplayPort).
pub fn default_wiring() -> std::collections::BTreeMap<Machine, tandem_core::Input> {
    [
        (Machine::Pc, tandem_core::Input::DisplayPort),
        (Machine::Mac, tandem_core::Input::Hdmi),
    ]
    .into()
}

/// `sam711a`; con dos del mismo modelo, `sam711a-<serie>` si la serie los distingue, o
/// `-2`, `-3`... por orden.
fn ids(sorted: &[&Detected]) -> Vec<MonitorId> {
    let base: Vec<String> = sorted
        .iter()
        .map(|d| d.edid.pnp_id().to_lowercase())
        .collect();
    let mut used = BTreeSet::new();
    let mut out = Vec::with_capacity(sorted.len());
    for (i, d) in sorted.iter().enumerate() {
        let twins = base.iter().filter(|b| **b == base[i]).count();
        let mut id = if twins > 1 && d.edid.serial_number != 0 {
            format!("{}-{}", base[i], d.edid.serial_number)
        } else {
            base[i].clone()
        };
        let mut n = 2;
        while used.contains(&id) {
            id = format!("{}-{n}", base[i]);
            n += 1;
        }
        used.insert(id.clone());
        out.push(MonitorId(id));
    }
    out
}

/// `34"`; sin tamaño en el EDID, el modelo; si dos quedan iguales, se les agrega el modelo.
fn labels(sorted: &[&Detected]) -> Vec<String> {
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

/// `tandem.toml` para `machine` con esos monitores, comentado. Siempre es válido.
pub fn generate(machine: Machine, detected: &[Detected], db: &ProfileDb) -> String {
    let monitors = monitors(detected, db);
    // Antes de la primera tabla: si no, sería una clave de esa tabla.
    let none = if monitors.is_empty() {
        "# No se detectó ningún monitor externo con EDID. Se agregan con «Agregar monitor»
# o al sincronizar con la otra máquina.
monitors = []
"
    } else {
        ""
    };
    let mut out = format!(
        "# Configuración generada por tandemd al arrancar por primera vez (T-082).
# Se puede editar: el agente no vuelve a generarla mientras el archivo exista. Para
# detectar los monitores de nuevo, bórrala y reinicia el agente.
machine = \"{machine}\"
{none}
[network]
# Puerto UDP del agente. Si no se puede abrir, el agente prueba otros y anuncia el real
# por mDNS: la otra máquina lo encuentra igual en la red local.
port = {DEFAULT_PORT}
# Enlaces directos (USB4 y Ethernet) con la dirección fija de la otra máquina, si los hay:
# peers = [{{ kind = \"usb4\", addr = \"<ip>:<puerto>\" }}]

[input]
# Quién tiene el teclado y el mouse al arrancar, hasta que el hub USB diga otra cosa.
server = \"mac\"
lock_shortcut = [\"ScrollLock\"]
clipboard = true
# Para que los roles cambien solos al mover el hub, agrega [input.hub] con el VID:PID
# del teclado y el mouse (`keyboard` y `mouse`).

[power]
# En la Mac: sin ningún enlace con la PC durante esto, se la da por apagada.
pc_lost_after_secs = 15
# Para encender la PC desde la Mac, agrega [power.wol] con la MAC de la tarjeta de red
# de la PC (`mac`).
"
    );
    for m in &monitors {
        out.push_str("\n[[monitors]]\n");
        out.push_str(&format!("id = {}\n", quote(&m.id.0)));
        out.push_str(&format!("label = {}\n", quote(&m.label)));
        if let Some(model) = &m.edid_model {
            out.push_str(&format!("edid_model = {}\n", quote(model)));
        }
        out.push_str("# Qué entrada del monitor va a cada máquina; se cambia desde la UI.\n");
        out.push_str("wiring = { pc = \"displayport\", mac = \"hdmi\" }\n");
        if let Some(ddc) = &m.ddc {
            out.push_str("# Códigos DDC de la base de perfiles (profiles/ddc).\n");
            out.push_str(&format!(
                "ddc = {{ hdmi = {:#04X}, display_port = {:#04X} }}\n",
                ddc.hdmi, ddc.display_port
            ));
        }
    }
    out
}

/// Cadena TOML: literal (`'34"'`) si se puede, para que se lea como se escribe a mano.
fn quote(s: &str) -> String {
    if s.contains('\'') || s.chars().any(char::is_control) {
        toml_edit::Value::from(s).to_string().trim().to_owned()
    } else {
        format!("'{s}'")
    }
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

/// Carga `path`; si no existe y `machine` es `Some`, la genera con lo que dé `scan` (si
/// falla, sin monitores: se agregan después) y la escribe. Nunca reemplaza un archivo
/// existente.
pub fn load_or_generate(
    path: &Path,
    machine: Option<Machine>,
    scan: impl FnOnce() -> Result<Vec<Detected>, String>,
    db: &ProfileDb,
) -> Result<Loaded, AutoConfigError> {
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
    let detected = scan().unwrap_or_else(|e| {
        tracing::warn!("no se pudieron detectar los monitores: {e}");
        Vec::new()
    });
    let text = generate(machine, &detected, db);
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
    use proptest::prelude::*;
    use tandem_core::Input;
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

    #[test]
    fn generates_the_real_desk_on_the_pc() {
        let text = generate(Machine::Pc, &pc_desk(), ProfileDb::builtin());
        let cfg = AgentConfig::from_toml(&text).unwrap();
        assert_eq!(cfg.machine, Machine::Pc);
        assert_eq!(cfg.network.port, DEFAULT_PORT);
        assert!(cfg.network.peers.is_empty());
        assert_eq!(cfg.power.wol, None, "sin MAC inventada");
        assert_eq!(cfg.input.hub, None, "sin VID:PID inventado");
        let ms = &cfg.desk.monitors;
        assert_eq!(ms.len(), 2);
        // De izquierda a derecha: el 34" primero.
        assert_eq!(ms[0].id, "sam711a".into());
        assert_eq!(ms[0].label, "34\"");
        assert_eq!(ms[0].edid_model.as_deref(), Some("SAM711A"));
        assert_eq!(ms[0].input_for(Machine::Pc), Some(Input::DisplayPort));
        assert_eq!(ms[0].input_for(Machine::Mac), Some(Input::Hdmi));
        // Los códigos verificados de la base (T-079), no los de la plantilla vieja.
        assert_eq!(
            ms[0].ddc,
            Some(tandem_core::DdcProfile {
                hdmi: 0x06,
                display_port: 0x09
            })
        );
        assert_eq!(ms[1].id, "sam7489".into());
        assert_eq!(ms[1].label, "32\"");
        assert_eq!(ms[1].edid_model.as_deref(), Some("SAM7489"));
        assert_eq!(
            ms[1].ddc,
            Some(tandem_core::DdcProfile {
                hdmi: 0x05,
                display_port: 0x0F
            })
        );
        assert!(text.contains("label = '34\"'"), "{text}");
        assert!(
            text.contains("ddc = { hdmi = 0x06, display_port = 0x09 }"),
            "{text}"
        );
        // Ni un dato de ejemplo que parezca real.
        for fake in ["9c:6b", "046d", "10.77."] {
            assert!(!text.contains(fake), "{fake} en\n{text}");
        }
    }

    #[test]
    fn the_mac_generates_the_same_desk() {
        // En la Mac la integrada no entra y los ids son los mismos que en la PC.
        let mac = from_displays(&[
            display(None, Some(-1512), true),
            display(Some(m34()), Some(0), false),
            display(Some(m32()), Some(3440), false),
        ]);
        let pc = AgentConfig::from_toml(&generate(Machine::Pc, &pc_desk(), ProfileDb::builtin()))
            .unwrap();
        let cfg =
            AgentConfig::from_toml(&generate(Machine::Mac, &mac, ProfileDb::builtin())).unwrap();
        assert_eq!(cfg.machine, Machine::Mac);
        assert_eq!(cfg.desk, pc.desk);
    }

    #[test]
    fn the_mac_starts_with_what_it_sees() {
        // El 32" solo cableado a la PC: la Mac no lo ve y arranca con el 34".
        let mac = from_displays(&[display(Some(m34()), Some(0), false)]);
        let cfg =
            AgentConfig::from_toml(&generate(Machine::Mac, &mac, ProfileDb::builtin())).unwrap();
        let ids: Vec<_> = cfg.desk.monitors.iter().map(|m| m.id.0.as_str()).collect();
        assert_eq!(ids, ["sam711a"]);
    }

    #[test]
    fn no_monitors_is_still_a_valid_config() {
        let text = generate(Machine::Mac, &[], ProfileDb::builtin());
        let cfg = AgentConfig::from_toml(&text).unwrap();
        assert!(cfg.desk.monitors.is_empty());
    }

    #[test]
    fn unknown_monitors_get_no_ddc_and_the_model_as_label() {
        let dell = edid(("DEL", 0xA0EC), Some("DELL U2723QE"), 7, None);
        let cfg = AgentConfig::from_toml(&generate(
            Machine::Pc,
            &[Detected {
                edid: dell,
                x: None,
            }],
            ProfileDb::builtin(),
        ))
        .unwrap();
        let m = &cfg.desk.monitors[0];
        assert_eq!(m.id, "dela0ec".into());
        assert_eq!(m.label, "DELL U2723QE");
        assert_eq!(m.ddc, None);
        // Sin nombre ni tamaño: el id PnP.
        let bare = edid(("DEL", 0xA0EC), None, 7, None);
        let ms = monitors(
            &[Detected {
                edid: bare,
                x: None,
            }],
            ProfileDb::builtin(),
        );
        assert_eq!(ms[0].label, "DELA0EC");
    }

    #[test]
    fn twins_get_distinct_ids_and_labels() {
        let a = edid(("SAM", 0x711A), Some("LC34G55T"), 11, Some((80, 34)));
        let b = edid(("SAM", 0x711A), Some("LC34G55T"), 12, Some((80, 34)));
        let c = edid(("GSM", 0x5B7F), Some("LG ULTRAWIDE"), 0, Some((80, 34)));
        let d = edid(("GSM", 0x5B7F), Some("LG ULTRAWIDE"), 0, Some((80, 34)));
        let ms = monitors(
            &[a, b, c, d]
                .into_iter()
                .map(|edid| Detected { edid, x: None })
                .collect::<Vec<_>>(),
            ProfileDb::builtin(),
        );
        let ids: Vec<_> = ms.iter().map(|m| m.id.0.as_str()).collect();
        assert_eq!(ids, ["sam711a-11", "sam711a-12", "gsm5b7f", "gsm5b7f-2"]);
        assert_eq!(ms[0].label, "34\" LC34G55T");
        assert_eq!(ms[2].label, "34\" LG ULTRAWIDE");
    }

    #[test]
    fn ddc_fills_what_the_topology_misses() {
        let topo = vec![Detected {
            edid: m34(),
            x: Some(0),
        }];
        let ddc = vec![
            Detected {
                edid: m34(),
                x: None,
            },
            Detected {
                edid: m32(),
                x: None,
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
    fn labels_with_quotes_stay_valid_toml() {
        assert_eq!(quote("34\""), "'34\"'");
        let q = quote("it's");
        let v: toml::Value = toml::from_str(&format!("x = {q}")).unwrap();
        assert_eq!(v["x"].as_str(), Some("it's"));
    }

    #[test]
    fn writes_once_and_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Tandem").join("tandem.toml");
        let db = ProfileDb::builtin();
        let first = load_or_generate(&path, Some(Machine::Pc), || Ok(pc_desk()), db).unwrap();
        assert!(first.generated);
        assert_eq!(first.config.desk.monitors.len(), 2);
        assert_eq!(AgentConfig::load(&path).unwrap(), first.config);
        // La segunda vez se lee el archivo: no se vuelve a detectar.
        let again =
            load_or_generate(&path, Some(Machine::Pc), || panic!("no debía detectar"), db).unwrap();
        assert!(!again.generated);
        assert_eq!(again.config, first.config);
    }

    #[test]
    fn an_invalid_file_is_an_error_not_a_new_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandem.toml");
        std::fs::write(&path, "machine = \"linux\"").unwrap();
        let err = load_or_generate(
            &path,
            Some(Machine::Pc),
            || Ok(vec![]),
            ProfileDb::builtin(),
        )
        .unwrap_err();
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
    fn a_failed_scan_still_starts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandem.toml");
        let got = load_or_generate(
            &path,
            Some(Machine::Mac),
            || Err("sin Quartz".into()),
            ProfileDb::builtin(),
        )
        .unwrap();
        assert!(got.generated);
        assert!(got.config.desk.monitors.is_empty());
    }

    #[test]
    fn without_a_known_os_it_asks_for_a_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tandem.toml");
        let err = load_or_generate(&path, None, || Ok(vec![]), ProfileDb::builtin()).unwrap_err();
        assert!(matches!(err, AutoConfigError::Unsupported { .. }), "{err}");
        assert!(err.to_string().contains("--config"), "{err}");
        assert!(!path.exists());
    }

    fn arb_edid() -> impl Strategy<Value = Edid> {
        (
            prop::sample::select(vec!["SAM", "DEL", "GSM"]),
            0u16..4,
            0u32..3,
            proptest::option::of("[A-Za-z0-9 '\"]{0,13}"),
            proptest::option::of((1u8..120, 1u8..80)),
        )
            .prop_map(|(m, p, serial, name, size)| Edid {
                manufacturer: m.into(),
                product_code: p,
                serial_number: serial,
                name,
                serial_text: None,
                size_cm: size,
            })
    }

    proptest! {
        /// Con cualquier conjunto de monitores, la configuración generada es válida, tiene
        /// uno por monitor y ningún id repetido.
        #[test]
        fn generated_config_is_always_valid(
            edids in proptest::collection::vec((arb_edid(), proptest::option::of(-4000i32..8000)), 0..6),
            pc in any::<bool>(),
        ) {
            let detected: Vec<Detected> =
                edids.into_iter().map(|(edid, x)| Detected { edid, x }).collect();
            let machine = if pc { Machine::Pc } else { Machine::Mac };
            let text = generate(machine, &detected, ProfileDb::builtin());
            let cfg = AgentConfig::from_toml(&text).map_err(|e| TestCaseError::fail(format!("{e}\n{text}")))?;
            prop_assert_eq!(cfg.machine, machine);
            prop_assert_eq!(cfg.desk.monitors.len(), detected.len());
        }
    }
}
