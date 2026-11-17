//! Actualización de las dos máquinas (T-122, DESIGN §13, DECISIONS D19).
//!
//! El updater de Tauri baja e instala el paquete de **esta** máquina (UI y `tandemd`, que
//! va dentro). Lo de aquí decide el resto y es portable:
//!
//! - [`Version`]: comparar versiones (`x.y.z` y `x.y.z-rc.n`, con el orden de SemVer).
//! - [`plan`] y [`run`]: «Actualizar todas». Primero la otra máquina y, cuando vuelve con la
//!   versión nueva, esta. Nunca se reinician las dos a la vez: mientras una se actualiza la
//!   otra sigue con el teclado y el mouse, y esta (la que guía) no se toca hasta que la otra
//!   volvió. Si la otra no vuelve, vuelta atrás: esta se queda en su versión y nada más
//!   cambia, así el puesto sigue usable con la máquina que funciona.
//! - [`drives_auto`] y [`requested_install`]: quién actualiza solo y cuándo la otra pidió
//!   instalar.
//! - [`Settings`]: el interruptor «Actualizar solo» en `update.toml`.

use std::cmp::Ordering;
use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tandem_core::Machine;
use tandem_proto::UpdateStatus;

/// Cuánto se espera a que la otra máquina vuelva con la versión nueva: bajar el paquete,
/// instalarlo y arrancar de nuevo. Pasado esto se da por perdida y esta no se actualiza.
pub const PEER_RETURN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Cada cuánto se busca una versión nueva con «Actualizar solo».
pub const AUTO_CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Archivo del interruptor, junto a la configuración del agente.
pub const SETTINGS_FILE: &str = "update.toml";

// ---------------------------------------------------------------------------------------
// Versiones

/// Una versión de Tandem: `mayor.menor.parche` con un sufijo de prerelease opcional
/// (`0.3.0-rc.2`). Se ordena como SemVer: una prerelease va antes que su versión final y
/// sus partes numéricas se comparan como números.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Las partes del sufijo, separadas por `.`; vacío en una versión final.
    pub pre: Vec<Ident>,
}

/// Una parte del sufijo de prerelease.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ident {
    Num(u64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("«{0}» no es una versión (se espera x.y.z o x.y.z-rc.n)")]
pub struct BadVersion(pub String);

impl FromStr for Version {
    type Err = BadVersion;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || BadVersion(s.to_owned());
        // Los tags llevan `v`; los metadatos de compilación (`+...`) no cuentan para el orden.
        let t = s.trim();
        let t = t.strip_prefix('v').unwrap_or(t);
        let t = t.split_once('+').map_or(t, |(v, _)| v);
        let (core, pre) = match t.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (t, None),
        };
        let mut nums = core.split('.').map(number);
        let (Some(Some(major)), Some(Some(minor)), Some(Some(patch)), None) =
            (nums.next(), nums.next(), nums.next(), nums.next())
        else {
            return Err(bad());
        };
        let pre = match pre {
            None => Vec::new(),
            Some(p) => p
                .split('.')
                .map(|part| match number(part) {
                    Some(n) => Some(Ident::Num(n)),
                    // Solo dígitos pero no un número: ceros a la izquierda, que SemVer no admite.
                    None if part.bytes().any(|b| !b.is_ascii_digit())
                        && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') =>
                    {
                        Some(Ident::Text(part.to_owned()))
                    }
                    None => None,
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(bad)?,
        };
        Ok(Version {
            major,
            minor,
            patch,
            pre,
        })
    }
}

/// Un número sin ceros a la izquierda (SemVer); `None` si no lo es.
fn number(s: &str) -> Option<u64> {
    let digits = !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let leading_zero = s.len() > 1 && s.starts_with('0');
    if digits && !leading_zero {
        s.parse().ok()
    } else {
        None
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (i, p) in self.pre.iter().enumerate() {
            f.write_str(if i == 0 { "-" } else { "." })?;
            match p {
                Ident::Num(n) => write!(f, "{n}")?,
                Ident::Text(t) => f.write_str(t)?,
            }
        }
        Ok(())
    }
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Ident::Num(a), Ident::Num(b)) => a.cmp(b),
            // Los numéricos van antes que los de texto.
            (Ident::Num(_), Ident::Text(_)) => Ordering::Less,
            (Ident::Text(_), Ident::Num(_)) => Ordering::Greater,
            (Ident::Text(a), Ident::Text(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // `1.0.0-rc.1` < `1.0.0`.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                // Parte por parte; con todas iguales, la más larga es mayor.
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// ---------------------------------------------------------------------------------------
// El plan de las dos máquinas

/// La otra máquina, vista para actualizar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Peer {
    /// Sin par: esta máquina se actualiza sola.
    Alone,
    /// Emparejada pero sin enlace.
    Unreachable,
    /// Conectada, pero todavía no dijo su versión.
    Unknown,
    /// Se conectó hablando otra versión del protocolo: corre otra versión de Tandem.
    OtherProtocol,
    At(Version),
}

/// Un paso de «Actualizar todas», en orden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Pedirle a la otra que instale la versión nueva.
    AskPeer,
    /// Esperar a que vuelva con ella.
    WaitPeer,
    /// Instalar aquí; la app se reinicia y con ella `tandemd`.
    InstallHere,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("las dos máquinas ya tienen la {0}")]
    UpToDate(Version),
    #[error("la otra máquina no está conectada: hay que actualizar las dos juntas")]
    PeerUnreachable,
    #[error("todavía no se sabe qué versión tiene la otra máquina")]
    PeerUnknown,
    #[error(
        "la otra máquina tiene otra versión de Tandem que no se entiende con esta: instala la {0} a mano en la que falte"
    )]
    PeerOtherProtocol(Version),
}

/// Los pasos para dejar las dos máquinas en `target`: primero la otra, después esta, y
/// solo las que tienen una versión anterior.
pub fn plan(here: &Version, peer: &Peer, target: &Version) -> Result<Vec<Step>, PlanError> {
    let here_behind = here < target;
    let peer_behind = match peer {
        Peer::Alone => false,
        Peer::Unreachable => return Err(PlanError::PeerUnreachable),
        Peer::Unknown => return Err(PlanError::PeerUnknown),
        Peer::OtherProtocol => return Err(PlanError::PeerOtherProtocol(target.clone())),
        Peer::At(v) => v < target,
    };
    let mut steps = Vec::new();
    if peer_behind {
        steps.extend([Step::AskPeer, Step::WaitPeer]);
    }
    if here_behind {
        steps.push(Step::InstallHere);
    }
    if steps.is_empty() {
        return Err(PlanError::UpToDate(target.clone()));
    }
    Ok(steps)
}

/// La otra máquina según la sesión con `tandemd`: `paired` y `link` salen del par
/// ([`crate::session::PeerView`]) y `status` de lo último que dijo el agente de las versiones.
pub fn peer_from(paired: bool, link: bool, status: Option<&UpdateStatus>) -> Peer {
    if !paired {
        return Peer::Alone;
    }
    // Primero el protocolo: si la otra habla otro, el enlace no llega a levantarse.
    if status.is_some_and(|s| s.peer_protocol.is_some()) {
        return Peer::OtherProtocol;
    }
    if !link {
        return Peer::Unreachable;
    }
    match status.and_then(|s| s.peer.as_deref()).map(str::parse) {
        Some(Ok(v)) => Peer::At(v),
        Some(Err(_)) | None => Peer::Unknown,
    }
}

/// La otra volvió con `target` (o una más nueva). Si ya no se entiende con esta, también:
/// solo pasa si cambió de versión, y se le pidió justamente eso.
pub fn came_back(peer: &Peer, target: &Version) -> bool {
    match peer {
        Peer::At(v) => v >= target,
        Peer::OtherProtocol => true,
        Peer::Alone | Peer::Unreachable | Peer::Unknown => false,
    }
}

/// Lo que «Actualizar todas» necesita de afuera: la otra máquina (por `tandemd`) y el
/// updater de esta. En las pruebas, un doble.
pub trait Fleet: Send {
    /// Pedirle a la otra que instale `target`.
    fn ask_peer(&mut self, target: &Version) -> impl Future<Output = Result<(), String>> + Send;
    /// Espera el siguiente cambio de la otra (su versión, o que se perdió); `None` si ya no
    /// llegarán más (la sesión con `tandemd` terminó).
    fn peer_changed(&mut self) -> impl Future<Output = Option<Peer>> + Send;
    /// Baja e instala `target` aquí. En la app no vuelve si sale bien: se reinicia.
    fn install_here(&mut self, target: &Version)
    -> impl Future<Output = Result<(), String>> + Send;
}

/// Por qué «Actualizar todas» no terminó.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    #[error("la otra máquina no aceptó actualizarse: {0}")]
    PeerRefused(String),
    /// La vuelta atrás: esta no se instaló y el puesto sigue con la máquina que funciona.
    #[error(
        "la otra máquina no volvió con la {target} en {} min: esta se queda en su versión",
        .waited.as_secs() / 60
    )]
    PeerDidNotReturn { target: Version, waited: Duration },
    #[error("se perdió la conexión con tandemd mientras la otra máquina se actualizaba")]
    SessionLost,
    #[error("la otra máquina ya tiene la {target}, pero esta no se pudo actualizar: {reason}")]
    InstallHere { target: Version, reason: String },
}

/// Corre los pasos de [`plan`] en orden, contando cada uno a `on_step` antes de empezarlo.
pub async fn run<F: Fleet>(
    steps: &[Step],
    target: &Version,
    fleet: &mut F,
    timeout: Duration,
    mut on_step: impl FnMut(Step) + Send,
) -> Result<(), Failure> {
    for &step in steps {
        on_step(step);
        match step {
            Step::AskPeer => fleet.ask_peer(target).await.map_err(Failure::PeerRefused)?,
            Step::WaitPeer => {
                let wait = async {
                    loop {
                        match fleet.peer_changed().await {
                            Some(p) if came_back(&p, target) => return Ok(()),
                            Some(_) => {}
                            None => return Err(Failure::SessionLost),
                        }
                    }
                };
                match tokio::time::timeout(timeout, wait).await {
                    Ok(r) => r?,
                    Err(_) => {
                        return Err(Failure::PeerDidNotReturn {
                            target: target.clone(),
                            waited: timeout,
                        });
                    }
                }
            }
            Step::InstallHere => {
                fleet
                    .install_here(target)
                    .await
                    .map_err(|reason| Failure::InstallHere {
                        target: target.clone(),
                        reason,
                    })?
            }
        }
    }
    Ok(())
}

/// Con «Actualizar solo», quién guía: sin par, esta; con par, solo `A`. Así las dos
/// máquinas lo saben sin hablarse y nunca guían las dos a la vez (DECISIONS A1).
pub fn drives_auto(me: Machine, peer: &Peer) -> bool {
    matches!(peer, Peer::Alone) || me == Machine::A
}

/// La versión que la otra pidió instalar aquí, si es más nueva que la de esta.
pub fn requested_install(here: &Version, request: Option<&str>) -> Option<Version> {
    let v: Version = request?.parse().ok()?;
    (v > *here).then_some(v)
}

// ---------------------------------------------------------------------------------------
// «Actualizar solo»

/// Lo que se guarda en `update.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Buscar e instalar solo (por omisión, sí: así viene en `F-Ajustes`).
    pub auto: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto: true }
    }
}

/// `update.toml` en el mismo directorio que la configuración del agente.
pub fn settings_file_for(agent_config: &Path) -> PathBuf {
    agent_config
        .parent()
        .map_or_else(|| PathBuf::from(SETTINGS_FILE), |d| d.join(SETTINGS_FILE))
}

/// Un archivo que falta o no se entiende vale lo de siempre: no es motivo para no
/// actualizar.
pub fn load_settings(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_settings(path: &Path, s: Settings) -> io::Result<()> {
    let text = toml::to_string(&s).map_err(io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Escribir aparte y renombrar: un corte a medias no deja el archivo vacío.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

// ---------------------------------------------------------------------------------------
// Lo que ve la ventana

/// La pestaña Actualizaciones de Ajustes (`F-Ajustes`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateView {
    /// La versión de esta máquina.
    pub here: String,
    /// La de la otra; `None` sin par o si todavía no se sabe.
    pub peer: Option<String>,
    /// Hay par: «Actualizar todas» incluye a la otra.
    pub paired: bool,
    /// La versión nueva que se encontró, si hay una más nueva que alguna de las dos.
    pub available: Option<String>,
    /// Buscando ahora.
    pub checking: bool,
    /// El paso de «Actualizar todas» en curso.
    pub step: Option<Step>,
    /// Lo último que salió mal (al buscar o al actualizar), para mostrarlo tal cual.
    pub error: Option<String>,
    /// «Actualizar solo».
    pub auto: bool,
}

/// La versión encontrada sirve si es más nueva que alguna de las dos máquinas.
pub fn worth_offering(found: &Version, here: &Version, peer: &Peer) -> bool {
    found > here || matches!(peer, Peer::At(v) if found > v)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use proptest::prelude::*;

    use super::*;

    fn v(s: &str) -> Version {
        s.parse().unwrap()
    }

    #[test]
    fn parses_and_prints() {
        for s in [
            "0.3.0",
            "1.20.3",
            "0.3.0-rc.1",
            "1.0.0-alpha.beta.10",
            "2.0.0-x-y",
        ] {
            assert_eq!(v(s).to_string(), s);
        }
        assert_eq!(v("v0.3.1"), v("0.3.1"));
        assert_eq!(v("0.3.1+build.7"), v("0.3.1"));
        assert_eq!(v(" 0.3.1 "), v("0.3.1"));
    }

    #[test]
    fn rejects_what_is_not_a_version() {
        for s in [
            "",
            "0.3",
            "0.3.1.2",
            "a.b.c",
            "01.2.3",
            "1.02.3",
            "0.3.0-",
            "0.3.0-rc..1",
            "0.3.0-rc.01",
            "-1.0.0",
            "0.3.0-ñ",
        ] {
            assert!(s.parse::<Version>().is_err(), "«{s}» no debería valer");
        }
    }

    #[test]
    fn semver_order() {
        // El ejemplo de la especificación de SemVer, de menor a mayor.
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "2.0.0",
        ];
        for w in ordered.windows(2) {
            assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        }
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("0.3.0-rc.10") > v("0.3.0-rc.9"));
    }

    fn version() -> impl Strategy<Value = Version> {
        let ident = prop_oneof![
            (0u64..20).prop_map(Ident::Num),
            "[a-z][a-z0-9-]{0,4}".prop_map(Ident::Text),
        ];
        (
            0u64..5,
            0u64..5,
            0u64..5,
            proptest::collection::vec(ident, 0..3),
        )
            .prop_map(|(major, minor, patch, pre)| Version {
                major,
                minor,
                patch,
                pre,
            })
    }

    proptest! {
        #[test]
        fn display_parses_back(a in version()) {
            prop_assert_eq!(a.to_string().parse::<Version>(), Ok(a));
        }

        #[test]
        fn order_is_total_and_consistent(a in version(), b in version(), c in version()) {
            prop_assert_eq!(a.cmp(&b), b.cmp(&a).reverse());
            prop_assert_eq!(a.cmp(&b) == Ordering::Equal, a == b);
            if a <= b && b <= c {
                prop_assert!(a <= c);
            }
        }

        #[test]
        fn a_prerelease_goes_before_its_release(a in version()) {
            let release = Version { pre: vec![], ..a.clone() };
            prop_assert!(a <= release);
            prop_assert_eq!(a == release, a.pre.is_empty());
        }

        /// El plan nunca reinicia esta antes que la otra, y solo pide lo que falta.
        #[test]
        fn plan_goes_other_first(here in version(), peer in version(), target in version()) {
            match plan(&here, &Peer::At(peer.clone()), &target) {
                Ok(steps) => {
                    let asks = steps.contains(&Step::AskPeer);
                    prop_assert_eq!(asks, peer < target);
                    prop_assert_eq!(steps.contains(&Step::InstallHere), here < target);
                    if asks {
                        prop_assert_eq!(&steps[..2], &[Step::AskPeer, Step::WaitPeer]);
                    }
                    if let Some(i) = steps.iter().position(|s| *s == Step::InstallHere) {
                        prop_assert_eq!(i, steps.len() - 1);
                    }
                }
                Err(e) => {
                    prop_assert_eq!(e, PlanError::UpToDate(target.clone()));
                    prop_assert!(here >= target && peer >= target);
                }
            }
        }
    }

    #[test]
    fn plan_cases() {
        use Step::*;
        let (old, new) = (v("0.3.0"), v("0.3.1"));
        let at = |s: &str| Peer::At(v(s));
        assert_eq!(
            plan(&old, &at("0.3.0"), &new),
            Ok(vec![AskPeer, WaitPeer, InstallHere])
        );
        // La otra ya se actualizó (por ejemplo, la vez anterior esta falló): solo esta.
        assert_eq!(plan(&old, &at("0.3.1"), &new), Ok(vec![InstallHere]));
        // Esta ya está: solo la otra.
        assert_eq!(plan(&new, &at("0.3.0"), &new), Ok(vec![AskPeer, WaitPeer]));
        assert_eq!(plan(&old, &Peer::Alone, &new), Ok(vec![InstallHere]));
        assert_eq!(
            plan(&new, &Peer::Alone, &new),
            Err(PlanError::UpToDate(new.clone()))
        );
        assert_eq!(
            plan(&old, &Peer::Unreachable, &new),
            Err(PlanError::PeerUnreachable)
        );
        assert_eq!(
            plan(&old, &Peer::Unknown, &new),
            Err(PlanError::PeerUnknown)
        );
        assert_eq!(
            plan(&old, &Peer::OtherProtocol, &new),
            Err(PlanError::PeerOtherProtocol(new.clone()))
        );
    }

    #[test]
    fn who_drives_auto_and_what_the_other_asked() {
        assert!(drives_auto(Machine::A, &Peer::At(v("0.3.0"))));
        assert!(!drives_auto(Machine::B, &Peer::At(v("0.3.0"))));
        assert!(!drives_auto(Machine::B, &Peer::Unreachable));
        assert!(drives_auto(Machine::B, &Peer::Alone));

        let here = v("0.3.0");
        assert_eq!(requested_install(&here, Some("0.3.1")), Some(v("0.3.1")));
        assert_eq!(requested_install(&here, Some("0.3.0")), None);
        assert_eq!(requested_install(&here, Some("0.2.9")), None);
        assert_eq!(requested_install(&here, Some("basura")), None);
        assert_eq!(requested_install(&here, None), None);
    }

    #[test]
    fn peer_from_the_session() {
        let status = |peer: Option<&str>, protocol: Option<u16>| UpdateStatus {
            here: "0.3.0".into(),
            peer: peer.map(Into::into),
            install: None,
            peer_protocol: protocol,
        };
        let s = status(Some("0.3.1"), None);
        assert_eq!(peer_from(false, false, Some(&s)), Peer::Alone);
        assert_eq!(peer_from(true, true, Some(&s)), Peer::At(v("0.3.1")));
        assert_eq!(peer_from(true, false, Some(&s)), Peer::Unreachable);
        assert_eq!(peer_from(true, true, None), Peer::Unknown);
        assert_eq!(
            peer_from(true, true, Some(&status(None, None))),
            Peer::Unknown
        );
        assert_eq!(
            peer_from(true, true, Some(&status(Some("rara"), None))),
            Peer::Unknown
        );
        // Con otro protocolo no hay enlace, pero se sabe que corre otra versión.
        assert_eq!(
            peer_from(true, false, Some(&status(Some("0.3.0"), Some(99)))),
            Peer::OtherProtocol
        );
    }

    #[test]
    fn worth_offering_if_either_is_behind() {
        let found = v("0.3.1");
        assert!(worth_offering(&found, &v("0.3.0"), &Peer::Alone));
        assert!(worth_offering(&found, &v("0.3.1"), &Peer::At(v("0.3.0"))));
        assert!(!worth_offering(&found, &v("0.3.1"), &Peer::At(v("0.3.1"))));
        assert!(!worth_offering(&found, &v("0.3.1"), &Peer::Unreachable));
    }

    // ---- run ----

    /// La otra máquina y el updater, simulados. `peer` son los cambios que irá anunciando
    /// la otra; vacía, se queda callada para siempre.
    #[derive(Default)]
    struct FakeFleet {
        log: Vec<String>,
        refuse: Option<String>,
        peer: VecDeque<Option<Peer>>,
        install_fails: Option<String>,
    }

    impl Fleet for FakeFleet {
        async fn ask_peer(&mut self, target: &Version) -> Result<(), String> {
            self.log.push(format!("pedir {target}"));
            match &self.refuse {
                Some(why) => Err(why.clone()),
                None => Ok(()),
            }
        }

        async fn peer_changed(&mut self) -> Option<Peer> {
            match self.peer.pop_front() {
                Some(p) => {
                    self.log.push(format!("otra: {p:?}"));
                    p
                }
                None => std::future::pending().await,
            }
        }

        async fn install_here(&mut self, target: &Version) -> Result<(), String> {
            self.log.push(format!("instalar {target}"));
            match &self.install_fails {
                Some(why) => Err(why.clone()),
                None => Ok(()),
            }
        }
    }

    const ALL: [Step; 3] = [Step::AskPeer, Step::WaitPeer, Step::InstallHere];
    const WAIT: Duration = Duration::from_secs(60);

    async fn go(fleet: &mut FakeFleet) -> (Result<(), Failure>, Vec<Step>) {
        let mut seen = Vec::new();
        let r = run(&ALL, &v("0.3.1"), fleet, WAIT, |s| seen.push(s)).await;
        (r, seen)
    }

    #[tokio::test(start_paused = true)]
    async fn other_first_then_this_once_it_is_back() {
        let mut f = FakeFleet {
            peer: VecDeque::from([
                // Se cae mientras se instala y vuelve con la nueva.
                Some(Peer::Unreachable),
                Some(Peer::At(v("0.3.1"))),
            ]),
            ..FakeFleet::default()
        };
        let (r, seen) = go(&mut f).await;
        assert_eq!(r, Ok(()));
        assert_eq!(seen, ALL);
        assert_eq!(
            f.log,
            [
                "pedir 0.3.1",
                "otra: Some(Unreachable)",
                "otra: Some(At(Version { major: 0, minor: 3, patch: 1, pre: [] }))",
                "instalar 0.3.1",
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_old_version_coming_back_is_not_enough() {
        let mut f = FakeFleet {
            // Volvió, pero con la de antes (el instalador falló allá): se sigue esperando.
            peer: VecDeque::from([Some(Peer::At(v("0.3.0")))]),
            ..FakeFleet::default()
        };
        let (r, _) = go(&mut f).await;
        assert_eq!(
            r,
            Err(Failure::PeerDidNotReturn {
                target: v("0.3.1"),
                waited: WAIT
            })
        );
        assert!(
            !f.log.iter().any(|l| l.starts_with("instalar")),
            "{:?}",
            f.log
        );
    }

    #[tokio::test(start_paused = true)]
    async fn rolls_back_if_the_other_never_returns() {
        let mut f = FakeFleet {
            peer: VecDeque::from([Some(Peer::Unreachable)]),
            ..FakeFleet::default()
        };
        let (r, seen) = go(&mut f).await;
        let err = r.unwrap_err();
        assert_eq!(
            err.to_string(),
            "la otra máquina no volvió con la 0.3.1 en 1 min: esta se queda en su versión"
        );
        // Esta no se tocó.
        assert_eq!(seen, [Step::AskPeer, Step::WaitPeer]);
        assert_eq!(f.log, ["pedir 0.3.1", "otra: Some(Unreachable)"]);
    }

    #[tokio::test(start_paused = true)]
    async fn another_protocol_counts_as_back() {
        let mut f = FakeFleet {
            peer: VecDeque::from([Some(Peer::OtherProtocol)]),
            ..FakeFleet::default()
        };
        assert_eq!(go(&mut f).await.0, Ok(()));
        assert_eq!(f.log.last().map(String::as_str), Some("instalar 0.3.1"));
    }

    #[tokio::test(start_paused = true)]
    async fn refused_or_lost_stops_before_touching_this() {
        let mut f = FakeFleet {
            refuse: Some("la otra máquina no está conectada".into()),
            ..FakeFleet::default()
        };
        let (r, seen) = go(&mut f).await;
        assert_eq!(
            r,
            Err(Failure::PeerRefused(
                "la otra máquina no está conectada".into()
            ))
        );
        assert_eq!(seen, [Step::AskPeer]);

        let mut f = FakeFleet {
            peer: VecDeque::from([None]),
            ..FakeFleet::default()
        };
        assert_eq!(go(&mut f).await.0, Err(Failure::SessionLost));
        assert!(!f.log.iter().any(|l| l.starts_with("instalar")));
    }

    #[tokio::test(start_paused = true)]
    async fn install_here_failing_is_reported_with_the_other_updated() {
        let mut f = FakeFleet {
            peer: VecDeque::from([Some(Peer::At(v("0.3.1")))]),
            install_fails: Some("sin red".into()),
            ..FakeFleet::default()
        };
        let err = go(&mut f).await.0.unwrap_err();
        assert_eq!(
            err.to_string(),
            "la otra máquina ya tiene la 0.3.1, pero esta no se pudo actualizar: sin red"
        );
    }

    #[test]
    fn settings_default_on_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_file_for(&dir.path().join("sub").join("tandem.toml"));
        assert_eq!(path, dir.path().join("sub").join(SETTINGS_FILE));
        assert_eq!(load_settings(&path), Settings { auto: true });
        save_settings(&path, Settings { auto: false }).unwrap();
        assert_eq!(load_settings(&path), Settings { auto: false });
        std::fs::write(&path, "auto = \"quizás\"").unwrap();
        assert_eq!(load_settings(&path), Settings::default());
    }

    #[test]
    fn view_json_shape() {
        let view = UpdateView {
            here: "0.3.0".into(),
            peer: Some("0.3.0".into()),
            paired: true,
            available: Some("0.3.1".into()),
            checking: false,
            step: Some(Step::WaitPeer),
            error: None,
            auto: true,
        };
        assert_eq!(
            serde_json::to_value(&view).unwrap(),
            serde_json::json!({
                "here": "0.3.0",
                "peer": "0.3.0",
                "paired": true,
                "available": "0.3.1",
                "checking": false,
                "step": "wait_peer",
                "error": null,
                "auto": true
            })
        );
    }
}
