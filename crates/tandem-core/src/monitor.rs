//! The minimum about monitors the core needs for planning (DESIGN §10): which
//! inputs exist and which one each machine's cable goes to. The real control lives in
//! `tandem-monitor`.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::model::Machine;

/// A video input of a compatible monitor (the standard of the in-house firmware).
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

    /// The other input: the standard has exactly two.
    pub fn other(self) -> Self {
        match self {
            MonitorInput::DisplayPort => MonitorInput::Hdmi,
            MonitorInput::Hdmi => MonitorInput::DisplayPort,
        }
    }
}

/// A machine's end on a monitor: which input its cable goes to and which of its screens
/// that monitor is (the id in its `[[screens]]` or in those it reads itself).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct CableEnd {
    /// `None`: not declared (whatever the EDID says applies, or the opposite of the other).
    #[serde(default)]
    pub input: Option<MonitorInput>,
    /// `None`: it is not known which of its screens it is, and the geometry cannot take it out.
    #[serde(default)]
    pub screen: Option<String>,
}

/// How a compatible monitor is connected to the two machines (`[[cables]]` of the desk).
/// With this a scene step ("make the 34" show B") knows which input to set, and the geometry
/// knows which screen stops belonging to one machine when the monitor shows the other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct MonitorCable {
    /// The monitor id (`<model>-<hash>`).
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

    /// The same cabling with the sides swapped.
    pub fn swapped(self) -> Self {
        MonitorCable {
            monitor: self.monitor,
            a: self.b,
            b: self.a,
        }
    }

    /// What fails in the cabling on its own.
    pub fn problem(&self) -> Option<String> {
        if self.monitor.trim().is_empty() {
            return Some("a cable does not say which monitor it is for".to_owned());
        }
        if self.a.input.is_some() && self.a.input == self.b.input {
            return Some(format!(
                "the two machines cannot go to the same input of monitor `{}`",
                self.monitor
            ));
        }
        if [&self.a, &self.b]
            .iter()
            .any(|e| e.screen.as_ref().is_some_and(|s| s.trim().is_empty()))
        {
            return Some(format!("a screen of monitor `{}` has no id", self.monitor));
        }
        None
    }
}

/// Which input each machine's cable goes to, already resolved ([`Inputs::resolve`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Inputs {
    pub a: Option<MonitorInput>,
    pub b: Option<MonitorInput>,
}

impl Inputs {
    /// What is declared on the desk rules; with nothing declared, what each machine read from its
    /// EDID (`learned`). Knowing one machine's, the other's is the other input. If both give the
    /// same one, neither is known: which machine to switch to is never guessed.
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

    /// The machine whose cable goes to `input`.
    pub fn machine_on(&self, input: MonitorInput) -> Option<Machine> {
        Machine::ALL
            .into_iter()
            .find(|&m| self.of(m) == Some(input))
    }

    /// The input of both is known.
    pub fn complete(&self) -> bool {
        self.a.is_some() && self.b.is_some()
    }
}

/// The identity a display announces in its EDID: manufacturer, product code and serial number
/// (bytes 8–15 of the base block). The system reports the same three numbers for each of its
/// screens (`CGDisplayVendorNumber`/`ModelNumber`/`SerialNumber` on macOS, the EDID of the
/// display path on Windows), so a monitor seen over DDC is linked to its screen even when it
/// has no channel (T-340).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayKey {
    /// The three-letter PNP code packed as in the EDID (big endian, `SAM` = `0x4C2D`).
    pub vendor: u16,
    pub product: u16,
    pub serial: u32,
}

impl DisplayKey {
    /// From an EDID base block; `None` without the fixed header or without a manufacturer.
    pub fn from_edid(raw: &[u8]) -> Option<Self> {
        const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
        if raw.get(..8)? != HEADER {
            return None;
        }
        let vendor = u16::from_be_bytes([*raw.get(8)?, *raw.get(9)?]);
        let product = u16::from_le_bytes([*raw.get(10)?, *raw.get(11)?]);
        let serial =
            u32::from_le_bytes([*raw.get(12)?, *raw.get(13)?, *raw.get(14)?, *raw.get(15)?]);
        (vendor != 0).then_some(DisplayKey {
            vendor,
            product,
            serial,
        })
    }
}

/// Which of this machine's screens each monitor is: `monitors` are the monitors it sees (id
/// and EDID identity), `screens` its screens with the identity the system gives. Only a single
/// match counts: with two identical monitors the screen is not guessed.
pub fn link_screens(
    monitors: &BTreeMap<String, DisplayKey>,
    screens: &[(String, DisplayKey)],
) -> BTreeMap<String, String> {
    monitors
        .iter()
        .filter_map(|(id, key)| {
            let mut found = screens.iter().filter(|(_, k)| k == key);
            match (found.next(), found.next()) {
                (Some((screen, _)), None) => Some((id.clone(), screen.clone())),
                _ => None,
            }
        })
        .collect()
}

/// Writes into `machine`'s end of each cable the screen it learned (`links`: monitor →
/// screen). An end that already names a screen is never touched: the user may have declared it
/// by hand. `true` if something changed.
pub fn learn_screens(
    cables: &mut [MonitorCable],
    machine: Machine,
    links: &BTreeMap<String, String>,
) -> bool {
    let mut changed = false;
    for cable in cables {
        let Some(screen) = links.get(&cable.monitor) else {
            continue;
        };
        let end = cable.end_mut(machine);
        if end.screen.is_none() {
            end.screen = Some(screen.clone());
            changed = true;
        }
    }
    changed
}

/// `new` without the screens the agents learned since `old` (ends that had none and now have one):
/// to tell a person's change from the agent's own.
pub fn without_learned_screens(old: &[MonitorCable], new: &[MonitorCable]) -> Vec<MonitorCable> {
    new.iter()
        .map(|c| {
            let before = old.iter().find(|o| o.monitor == c.monitor);
            let mut c = c.clone();
            for m in Machine::ALL {
                if before.is_none_or(|b| b.end(m).screen.is_none()) {
                    c.end_mut(m).screen = None;
                }
            }
            c
        })
        .collect()
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
        // What is declared rules over the EDID.
        let got = Inputs::resolve(Some(&cable(None, Some(Hdmi))), edid);
        assert_eq!((got.a, got.b), (Some(Dp), Some(Hdmi)));
        // Contradiction: neither is known.
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
            "monitor = \"m34\"\na = { input = \"hdmi\", screen = \"screen-2\" }\nb = { input = \"dp\" }",
        )
        .unwrap();
        assert_eq!(t.b.input, Some(MonitorInput::DisplayPort));
        assert_eq!(t.a.screen.as_deref(), Some("screen-2"));
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
        /// The two machines never end up on the same input; if one is known, both are,
        /// and each input leads to its machine.
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

    fn edid(vendor: &[u8; 3], product: u16, serial: u32) -> Vec<u8> {
        let mut raw = vec![0u8; 128];
        raw[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        let packed = vendor
            .iter()
            .fold(0u16, |acc, c| (acc << 5) | u16::from(c - b'A' + 1));
        raw[8..10].copy_from_slice(&packed.to_be_bytes());
        raw[10..12].copy_from_slice(&product.to_le_bytes());
        raw[12..16].copy_from_slice(&serial.to_le_bytes());
        raw
    }

    #[test]
    fn the_display_key_comes_from_the_edid_header() {
        let key = DisplayKey::from_edid(&edid(b"SAM", 0x7105, 0x3032_4E48)).unwrap();
        assert_eq!(key.vendor, 0x4C2D, "SAM as CoreGraphics reports it");
        assert_eq!((key.product, key.serial), (0x7105, 0x3032_4E48));
        let mut bad = edid(b"SAM", 1, 1);
        bad[0] = 1;
        assert_eq!(DisplayKey::from_edid(&bad), None);
        assert_eq!(DisplayKey::from_edid(&[0, 0xFF]), None);
        assert_eq!(DisplayKey::from_edid(&edid(b"SAM", 1, 1)[..12]), None);
    }

    fn key(product: u16) -> DisplayKey {
        DisplayKey {
            vendor: 0x4C2D,
            product,
            serial: 0,
        }
    }

    #[test]
    fn a_monitor_links_to_the_only_screen_with_its_identity() {
        let monitors = BTreeMap::from([
            ("lc34".to_owned(), key(0x7105)),
            ("m27".to_owned(), key(0x7154)),
            ("gone".to_owned(), key(1)),
        ]);
        let screens = [
            ("screen-1".to_owned(), key(0xA0A0)),
            ("screen-2".to_owned(), key(0x7105)),
            ("screen-3".to_owned(), key(0x7154)),
            ("screen-4".to_owned(), key(0x7154)),
        ];
        let got = link_screens(&monitors, &screens);
        assert_eq!(
            got,
            BTreeMap::from([("lc34".to_owned(), "screen-2".to_owned())]),
            "two identical 27\" are not guessed"
        );
    }

    #[test]
    fn a_learned_screen_never_replaces_a_declared_one() {
        let mut cables = vec![MonitorCable::new("lc34"), MonitorCable::new("m27")];
        cables[1].b.screen = Some("by-hand".into());
        let links = BTreeMap::from([
            ("lc34".to_owned(), "screen-2".to_owned()),
            ("m27".to_owned(), "screen-3".to_owned()),
        ]);
        assert!(learn_screens(&mut cables, Machine::B, &links));
        assert_eq!(cables[0].b.screen.as_deref(), Some("screen-2"));
        assert_eq!(cables[0].a.screen, None, "only this machine's end");
        assert_eq!(cables[1].b.screen.as_deref(), Some("by-hand"));
        assert!(
            !learn_screens(&mut cables, Machine::B, &links),
            "already learned"
        );
        let old = vec![MonitorCable::new("lc34"), cables[1].clone()];
        assert_eq!(without_learned_screens(&old, &cables), old);
        // A screen changed by hand is not an agent's.
        let mut edited = cables.clone();
        edited[1].b.screen = Some("other".into());
        assert_ne!(without_learned_screens(&cables, &edited), cables);
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
