//! Lo mínimo de los monitores que el núcleo necesita para planear (DESIGN §10): qué
//! entradas existen y a cuál va el cable de cada máquina. El control real vive en
//! `tandem-monitor`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::model::Machine;

/// Una entrada de video de un monitor compatible (el estándar del firmware propio).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorInput {
    #[serde(alias = "dp")]
    DisplayPort,
    Hdmi,
}

impl MonitorInput {
    pub const ALL: [MonitorInput; 2] = [MonitorInput::DisplayPort, MonitorInput::Hdmi];

    pub fn as_str(self) -> &'static str {
        match self {
            MonitorInput::DisplayPort => "DP",
            MonitorInput::Hdmi => "HDMI",
        }
    }

    /// La otra entrada: el estándar tiene exactamente dos.
    pub fn other(self) -> Self {
        match self {
            MonitorInput::DisplayPort => MonitorInput::Hdmi,
            MonitorInput::Hdmi => MonitorInput::DisplayPort,
        }
    }
}

/// El extremo de una máquina en un monitor: a qué entrada va su cable y cuál de sus pantallas
/// es ese monitor (el id de sus `[[screens]]` o de las que lee ella).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct CableEnd {
    /// `None`: no se declaró (vale lo que diga el EDID, o lo contrario de la otra).
    #[serde(default)]
    pub input: Option<MonitorInput>,
    /// `None`: no se sabe cuál de sus pantallas es, y la geometría no la puede sacar.
    #[serde(default)]
    pub screen: Option<String>,
}

/// Cómo está conectado un monitor compatible a las dos máquinas (`[[cables]]` del puesto).
/// Con esto un paso de escena («que el 34" muestre B») sabe qué entrada poner, y la geometría
/// sabe qué pantalla deja de ser de una máquina cuando el monitor muestra a la otra.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct MonitorCable {
    /// El id del monitor (`<modelo>-<hash>`).
    pub monitor: String,
    #[serde(default)]
    pub a: CableEnd,
    #[serde(default)]
    pub b: CableEnd,
}

impl MonitorCable {
    pub fn new(monitor: impl Into<String>) -> Self {
        MonitorCable {
            monitor: monitor.into(),
            a: CableEnd::default(),
            b: CableEnd::default(),
        }
    }

    pub fn end(&self, machine: Machine) -> &CableEnd {
        match machine {
            Machine::A => &self.a,
            Machine::B => &self.b,
        }
    }

    pub fn end_mut(&mut self, machine: Machine) -> &mut CableEnd {
        match machine {
            Machine::A => &mut self.a,
            Machine::B => &mut self.b,
        }
    }

    /// El mismo cableado con los lados cambiados.
    pub fn swapped(self) -> Self {
        MonitorCable {
            monitor: self.monitor,
            a: self.b,
            b: self.a,
        }
    }

    /// Qué falla en el cableado por sí solo.
    pub fn problem(&self) -> Option<String> {
        if self.monitor.trim().is_empty() {
            return Some("un cable no dice de qué monitor es".to_owned());
        }
        if self.a.input.is_some() && self.a.input == self.b.input {
            return Some(format!(
                "las dos máquinas no pueden ir a la misma entrada del monitor `{}`",
                self.monitor
            ));
        }
        if [&self.a, &self.b]
            .iter()
            .any(|e| e.screen.as_ref().is_some_and(|s| s.trim().is_empty()))
        {
            return Some(format!(
                "una pantalla del monitor `{}` no tiene id",
                self.monitor
            ));
        }
        None
    }
}

/// A qué entrada va el cable de cada máquina, ya resuelto ([`Inputs::resolve`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Inputs {
    pub a: Option<MonitorInput>,
    pub b: Option<MonitorInput>,
}

impl Inputs {
    /// Lo declarado en el puesto manda; sin nada declarado, lo que cada máquina leyó de su
    /// EDID (`learned`). Sabiendo la de una, la otra es la otra entrada. Si las dos dan la
    /// misma, no se sabe ninguna: nunca se adivina a qué máquina se cambia.
    pub fn resolve(
        declared: Option<&MonitorCable>,
        learned: impl Fn(Machine) -> Option<MonitorInput>,
    ) -> Inputs {
        let declared = declared.map(|c| (c.a.input, c.b.input));
        let (a, b) = match declared {
            Some((a, b)) if a.is_some() || b.is_some() => (a, b),
            _ => (learned(Machine::A), learned(Machine::B)),
        };
        match (a, b) {
            (Some(x), Some(y)) if x == y => Inputs::default(),
            (Some(x), None) => Inputs {
                a: Some(x),
                b: Some(x.other()),
            },
            (None, Some(y)) => Inputs {
                a: Some(y.other()),
                b: Some(y),
            },
            (a, b) => Inputs { a, b },
        }
    }

    pub fn of(&self, machine: Machine) -> Option<MonitorInput> {
        match machine {
            Machine::A => self.a,
            Machine::B => self.b,
        }
    }

    /// La máquina cuyo cable va a `input`.
    pub fn machine_on(&self, input: MonitorInput) -> Option<Machine> {
        Machine::ALL
            .into_iter()
            .find(|&m| self.of(m) == Some(input))
    }

    /// Se sabe la entrada de las dos.
    pub fn complete(&self) -> bool {
        self.a.is_some() && self.b.is_some()
    }
}

impl fmt::Display for MonitorInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn cable(a: Option<MonitorInput>, b: Option<MonitorInput>) -> MonitorCable {
        let mut c = MonitorCable::new("m34");
        c.a.input = a;
        c.b.input = b;
        c
    }

    #[test]
    fn declared_inputs_win_and_one_side_gives_the_other() {
        use MonitorInput::{DisplayPort as Dp, Hdmi};
        let none = |_| None;
        assert_eq!(Inputs::resolve(None, none), Inputs::default());
        let edid = |m| (m == Machine::B).then_some(Dp);
        let got = Inputs::resolve(None, edid);
        assert_eq!((got.a, got.b), (Some(Hdmi), Some(Dp)));
        assert_eq!(got.machine_on(Hdmi), Some(Machine::A));
        // Lo declarado manda sobre el EDID.
        let got = Inputs::resolve(Some(&cable(None, Some(Hdmi))), edid);
        assert_eq!((got.a, got.b), (Some(Dp), Some(Hdmi)));
        // Contradicción: no se sabe ninguna.
        let both = |_| Some(Hdmi);
        assert!(!Inputs::resolve(None, both).complete());
        assert_eq!(Inputs::resolve(None, both).machine_on(Hdmi), None);
    }

    #[test]
    fn problems_of_a_cable() {
        use MonitorInput::Hdmi;
        assert!(cable(Some(Hdmi), Some(Hdmi)).problem().is_some());
        assert!(MonitorCable::new(" ").problem().is_some());
        let mut c = cable(Some(Hdmi), None);
        assert!(c.problem().is_none());
        c.b.screen = Some(String::new());
        assert!(c.problem().is_some());
        let t: MonitorCable = toml::from_str(
            "monitor = \"m34\"\na = { input = \"hdmi\", screen = \"pantalla-2\" }\nb = { input = \"dp\" }",
        )
        .unwrap();
        assert_eq!(t.b.input, Some(MonitorInput::DisplayPort));
        assert_eq!(t.a.screen.as_deref(), Some("pantalla-2"));
        assert_eq!(t.clone().swapped().b, t.a);
    }

    fn input() -> impl Strategy<Value = Option<MonitorInput>> {
        prop_oneof![
            Just(None),
            Just(Some(MonitorInput::DisplayPort)),
            Just(Some(MonitorInput::Hdmi))
        ]
    }

    proptest! {
        /// Nunca quedan las dos máquinas en la misma entrada; si se sabe una, se saben las dos,
        /// y cada entrada lleva a su máquina.
        #[test]
        fn resolved_inputs_never_collide(
            da in input(), db in input(), la in input(), lb in input()
        ) {
            let declared = cable(da, db);
            let got = Inputs::resolve(Some(&declared), |m| if m == Machine::A { la } else { lb });
            prop_assert_eq!(got.a.is_some(), got.b.is_some());
            if let (Some(x), Some(y)) = (got.a, got.b) {
                prop_assert_ne!(x, y);
            }
            if declared.problem().is_none() && got.complete() {
                prop_assert!(da.is_none_or(|x| got.a == Some(x)));
                prop_assert!(db.is_none_or(|y| got.b == Some(y)));
            }
            for m in Machine::ALL {
                if let Some(i) = got.of(m) {
                    prop_assert_eq!(got.machine_on(i), Some(m));
                }
            }
        }
    }

    #[test]
    fn inputs_read_and_show_as_the_user_knows_them() {
        assert_eq!(MonitorInput::Hdmi.to_string(), "HDMI");
        let dp: MonitorInput = serde_json::from_str("\"dp\"").unwrap();
        assert_eq!(dp, MonitorInput::DisplayPort);
        assert_eq!(
            serde_json::to_string(&MonitorInput::DisplayPort).unwrap(),
            "\"display_port\""
        );
    }
}
