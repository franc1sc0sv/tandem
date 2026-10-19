//! Lo que hace falta del asistente de configuración inicial (T-087) y no depende del
//! sistema: juntar lo que ve cada máquina en monitores físicos, armar los monitores
//! nuevos del puesto y detectar de nuevo lo conectado.
//!
//! Cada máquina ve a un mismo monitor con un id PnP distinto según la entrada (el 34" es
//! `SAM711A` por DisplayPort y `SAM7119` por HDMI), así que el asistente propone el vínculo
//! entre las dos identidades y el usuario lo confirma: por número de serie del EDID si
//! coincide; si no, por modelo o tamaño. Nada de aquí cambia el escritorio de nadie.

use std::collections::BTreeMap;

use tandem_core::{Config, DdcProfile, Input, Machine, MonitorConfig, MonitorId};
use tandem_display::ddc::profiles::ProfileDb;
use tandem_proto::{
    Capabilities, LinkBasis, MonitorCandidate, MonitorCaps, MonitorScan, NewMonitor,
};

use crate::capabilities::{Seen, connected};

/// Detecta los monitores conectados ahora a esta máquina. Bloquea (abre la topología y el
/// bus DDC): se llama fuera del runtime.
pub trait MonitorScanner: Send + Sync + std::fmt::Debug {
    fn scan(&self) -> Result<Vec<Seen>, String>;
}

/// El detector del sistema ([`crate::autoconfig::platform_scan`]).
#[derive(Debug, Clone, Copy)]
pub struct PlatformScanner(pub Machine);

impl MonitorScanner for PlatformScanner {
    fn scan(&self) -> Result<Vec<Seen>, String> {
        crate::autoconfig::platform_scan(self.0).map(|d| crate::autoconfig::seen(&d))
    }
}

fn same(a: &str, b: &str) -> bool {
    !a.trim().is_empty() && a.trim().eq_ignore_ascii_case(b.trim())
}

/// Si `a` y `b` pueden ser el mismo monitor visto desde dos máquinas: el mismo número de
/// serie, o el mismo modelo o tamaño.
fn link(a: &MonitorCaps, b: &MonitorCaps) -> Option<LinkBasis> {
    if a.serial.is_some() && a.serial == b.serial {
        return Some(LinkBasis::Serial);
    }
    // Con dos números de serie distintos son dos monitores, sean del modelo que sean.
    if a.serial.is_some() && b.serial.is_some() {
        return None;
    }
    let same_id = [(&a.pnp_id, &b.pnp_id), (&a.model, &b.model)]
        .into_iter()
        .any(|(x, y)| matches!((x, y), (Some(x), Some(y)) if same(x, y)));
    let same_size = a.inches.is_some() && a.inches == b.inches;
    (same_id || same_size).then_some(LinkBasis::Model)
}

/// Los id PnP de un monitor visto por una o dos máquinas, sin repetir y con el de la PC
/// primero: así el id del monitor y su `edid_model` no dependen de qué máquina corre el
/// asistente.
fn pnp_ids(
    here: Option<(&MonitorCaps, Machine)>,
    peer: Option<(&MonitorCaps, Machine)>,
) -> Vec<String> {
    let mut seen: Vec<(Machine, String)> = [here, peer]
        .into_iter()
        .flatten()
        .filter_map(|(m, machine)| Some((machine, m.pnp_id.clone()?)))
        .collect();
    // `Machine::Mac < Machine::Pc`: de mayor a menor deja a la PC primero.
    seen.sort_by_key(|(machine, _)| std::cmp::Reverse(*machine));
    let mut out: Vec<String> = Vec::new();
    for (_, id) in seen {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&id)) {
            out.push(id);
        }
    }
    out
}

/// Los monitores físicos que se ven ahora, de `here` (esta máquina) y `peer` (la otra, si
/// se la conoce), con el vínculo propuesto entre las dos identidades.
pub fn propose(
    here: &Capabilities,
    peer: Option<&Capabilities>,
    desk: &Config,
    db: &ProfileDb,
) -> MonitorScan {
    let seen = |c: &Capabilities| -> Vec<MonitorCaps> {
        c.monitors.iter().filter(|m| m.seen).cloned().collect()
    };
    let mine = seen(here);
    let mut theirs: Vec<Option<MonitorCaps>> = peer
        .map(|p| seen(p).into_iter().map(Some).collect())
        .unwrap_or_default();
    // (la mía, la de la otra, con qué se vinculó)
    let mut pairs: Vec<(Option<MonitorCaps>, Option<MonitorCaps>, LinkBasis)> = Vec::new();
    let mut open: Vec<MonitorCaps> = Vec::new();
    // Primero los que coinciden por número de serie: lo más fiable.
    for m in mine {
        let at = theirs.iter().position(|t| {
            t.as_ref()
                .is_some_and(|t| link(&m, t) == Some(LinkBasis::Serial))
        });
        match at {
            Some(i) => {
                let t = theirs[i].take();
                pairs.push((Some(m), t, LinkBasis::Serial));
            }
            None => open.push(m),
        }
    }
    // Después por modelo o tamaño, en el orden en que cada máquina los lista.
    for m in open {
        let at = theirs
            .iter()
            .position(|t| t.as_ref().is_some_and(|t| link(&m, t).is_some()));
        match at {
            Some(i) => {
                let t = theirs[i].take();
                pairs.push((Some(m), t, LinkBasis::Model));
            }
            None => pairs.push((Some(m), None, LinkBasis::Single)),
        }
    }
    pairs.extend(
        theirs
            .into_iter()
            .flatten()
            .map(|t| (None, Some(t), LinkBasis::Single)),
    );

    let machine_of = |is_here: bool| {
        if is_here {
            here.machine
        } else {
            here.machine.other()
        }
    };
    let mut candidates: Vec<MonitorCandidate> = pairs
        .into_iter()
        .map(|(h, p, basis)| {
            let ids = pnp_ids(
                h.as_ref().map(|m| (m, machine_of(true))),
                p.as_ref().map(|m| (m, machine_of(false))),
            );
            let mut wiring: BTreeMap<Machine, Option<Input>> = BTreeMap::new();
            if let Some(h) = &h {
                wiring.insert(machine_of(true), h.output);
            }
            if let Some(p) = &p {
                wiring.insert(machine_of(false), p.output);
            }
            // Dos máquinas no pueden usar la misma entrada: se pregunta la de la otra.
            if let (Some(a), Some(b)) = (
                wiring.get(&machine_of(true)).copied().flatten(),
                wiring.get(&machine_of(false)).copied().flatten(),
            ) && a == b
            {
                wiring.insert(machine_of(false), None);
            }
            let label = h
                .as_ref()
                .or(p.as_ref())
                .map(|m| m.name.clone())
                .unwrap_or_default();
            let in_desk = desk
                .monitors
                .iter()
                .find(|m| ids.iter().any(|id| m.is_model(id)))
                .map(|m| m.id.clone());
            let profile = ids
                .iter()
                .find_map(|id| db.get(id))
                .map(|e| e.profile.name.clone());
            MonitorCandidate {
                label,
                pnp_ids: ids,
                here: h,
                peer: p,
                basis,
                wiring,
                in_desk,
                profile,
            }
        })
        .collect();
    // Dos con la misma etiqueta se distinguen por el modelo.
    let labels: Vec<String> = candidates.iter().map(|c| c.label.clone()).collect();
    for c in &mut candidates {
        if labels.iter().filter(|l| **l == c.label).count() > 1
            && let Some(model) = c.here.iter().chain(&c.peer).find_map(|m| m.model.clone())
            && model != c.label
        {
            c.label = format!("{} {model}", c.label);
        }
    }
    let on = connected(desk, here, peer);
    let disconnected = desk.monitors.iter().filter(|m| !on.contains(&m.id)).count();
    MonitorScan {
        candidates,
        peer_known: peer.is_some(),
        disconnected: u32::try_from(disconnected).unwrap_or(u32::MAX),
    }
}

/// Los monitores del puesto que salen de lo que el usuario eligió en el asistente.
///
/// Con el perfil DDC de la base si conoce alguno de sus id PnP (cualquiera de las dos
/// identidades). Rechaza lo que dejaría un puesto inválido o repetiría un monitor.
pub fn new_monitors(
    new: &[NewMonitor],
    desk: &Config,
    db: &ProfileDb,
) -> Result<Vec<MonitorConfig>, String> {
    let mut out: Vec<MonitorConfig> = Vec::new();
    for n in new {
        let ids: Vec<&String> = n.pnp_ids.iter().filter(|i| !i.trim().is_empty()).collect();
        let Some(first) = ids.first() else {
            return Err(format!(
                "{}: sin id PnP no hay cómo encontrarlo después",
                n.label
            ));
        };
        if n.wiring.is_empty() {
            return Err(format!(
                "{}: tiene que ir cableado a alguna máquina",
                n.label
            ));
        }
        let inputs: Vec<Input> = n.wiring.values().copied().collect();
        if inputs.len() == 2 && inputs[0] == inputs[1] {
            return Err(format!(
                "{}: las dos máquinas no pueden ir a la misma entrada",
                n.label
            ));
        }
        let probe = MonitorConfig {
            id: MonitorId(String::new()),
            label: n.label.clone(),
            edid_model: Some((*first).clone()),
            edid_aliases: ids.iter().skip(1).map(|s| (*s).clone()).collect(),
            wiring: n.wiring.clone(),
            ddc: None,
        };
        let taken = desk.monitors.iter().chain(&out);
        if let Some(other) = taken
            .into_iter()
            .find(|m| m.models().any(|x| probe.is_model(x)))
        {
            return Err(format!(
                "{} ya está en el puesto como {}",
                n.label, other.label
            ));
        }
        let mut aliases: Vec<String> = Vec::new();
        for id in ids.iter().skip(1) {
            if !id.eq_ignore_ascii_case(first)
                && !aliases.iter().any(|a| a.eq_ignore_ascii_case(id))
            {
                aliases.push((*id).clone());
            }
        }
        let ddc = ids.iter().find_map(|id| db.get(id)).map(|e| DdcProfile {
            hdmi: e.profile.codes.hdmi,
            display_port: e.profile.codes.display_port,
        });
        let base = first.to_lowercase();
        let mut id = base.clone();
        let mut k = 2;
        while desk.monitors.iter().chain(&out).any(|m| m.id.0 == id) {
            id = format!("{base}-{k}");
            k += 1;
        }
        out.push(MonitorConfig {
            id: MonitorId(id),
            label: n.label.clone(),
            edid_model: Some((*first).clone()),
            edid_aliases: aliases,
            wiring: n.wiring.clone(),
            ddc,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tandem_proto::{InputCaps, PowerCaps};

    use super::*;

    fn caps(machine: Machine, monitors: Vec<MonitorCaps>) -> Capabilities {
        Capabilities {
            machine,
            name: machine.to_string(),
            os: "test".into(),
            scanned: true,
            monitors,
            builtin: false,
            hub: None,
            input: InputCaps {
                capture: true,
                inject: true,
                missing: vec![],
                process: Default::default(),
            },
            power: PowerCaps {
                wake_peer: false,
                wakeable: true,
                power_off: false,
            },
            services: vec![],
        }
    }

    pub(crate) fn mon(
        pnp: &str,
        model: &str,
        serial: Option<u32>,
        inches: u32,
        output: Option<Input>,
    ) -> MonitorCaps {
        MonitorCaps {
            monitor: None,
            pnp_id: Some(pnp.into()),
            name: format!("{inches}\""),
            switch: false,
            ddc: false,
            seen: true,
            model: Some(model.into()),
            serial,
            inches: Some(inches),
            output,
        }
    }

    /// El puesto real: los ids de la PC (DisplayPort) y de la Mac (HDMI) son distintos.
    pub(crate) fn real_pc() -> Capabilities {
        caps(
            Machine::Pc,
            vec![
                mon(
                    "SAM711A",
                    "LC34G55T",
                    Some(11),
                    34,
                    Some(Input::DisplayPort),
                ),
                mon(
                    "SAM7489",
                    "Odyssey G5",
                    Some(32),
                    32,
                    Some(Input::DisplayPort),
                ),
            ],
        )
    }

    pub(crate) fn real_mac() -> Capabilities {
        caps(
            Machine::Mac,
            vec![
                mon("SAM7119", "LC34G55T", Some(11), 34, None),
                mon("SAM748A", "Odyssey G5", Some(32), 32, None),
            ],
        )
    }

    fn empty() -> Config {
        Config { monitors: vec![] }
    }

    #[test]
    fn the_real_desk_is_two_physical_monitors_linked_by_serial() {
        let scan = propose(
            &real_pc(),
            Some(&real_mac()),
            &empty(),
            ProfileDb::builtin(),
        );
        assert_eq!(scan.candidates.len(), 2, "{scan:#?}");
        assert!(scan.peer_known);
        for (c, ids) in scan
            .candidates
            .iter()
            .zip([("SAM711A", "SAM7119"), ("SAM7489", "SAM748A")])
        {
            assert_eq!(c.basis, LinkBasis::Serial);
            assert_eq!(c.here.as_ref().unwrap().pnp_id.as_deref(), Some(ids.0));
            assert_eq!(c.peer.as_ref().unwrap().pnp_id.as_deref(), Some(ids.1));
            // La PC lo ve por DisplayPort; de la Mac no se sabe (CoreGraphics no lo dice).
            assert_eq!(c.wiring[&Machine::Pc], Some(Input::DisplayPort));
            assert_eq!(c.wiring[&Machine::Mac], None);
            assert!(
                c.profile.is_some(),
                "perfil de la base por cualquiera de sus ids"
            );
        }
    }

    #[test]
    fn without_serials_the_link_is_by_model_or_size_and_needs_confirming() {
        let mut mac = real_mac();
        for m in &mut mac.monitors {
            m.serial = None;
        }
        let mut pc = real_pc();
        for m in &mut pc.monitors {
            m.serial = None;
        }
        let scan = propose(&pc, Some(&mac), &empty(), ProfileDb::builtin());
        assert_eq!(scan.candidates.len(), 2);
        assert!(scan.candidates.iter().all(|c| c.basis == LinkBasis::Model));
        // Mismo orden en las dos listas: el 34" con el 34".
        assert_eq!(scan.candidates[0].peer.as_ref().unwrap().inches, Some(34));
    }

    #[test]
    fn two_different_serials_are_never_linked() {
        let a = mon("DEL1", "U27", Some(1), 27, None);
        let b = mon("DEL1", "U27", Some(2), 27, None);
        let scan = propose(
            &caps(Machine::Pc, vec![a]),
            Some(&caps(Machine::Mac, vec![b])),
            &empty(),
            ProfileDb::builtin(),
        );
        assert_eq!(scan.candidates.len(), 2);
        assert!(scan.candidates.iter().all(|c| c.basis == LinkBasis::Single));
    }

    #[test]
    fn a_monitor_only_one_machine_sees_stays_single() {
        let scan = propose(&real_pc(), None, &empty(), ProfileDb::builtin());
        assert!(!scan.peer_known);
        assert_eq!(scan.candidates.len(), 2);
        assert!(scan.candidates.iter().all(|c| c.basis == LinkBasis::Single));
        assert!(scan.candidates.iter().all(|c| c.peer.is_none()));
    }

    #[test]
    fn the_same_cable_on_both_machines_is_asked_again() {
        let mut mac = real_mac();
        mac.monitors[0].output = Some(Input::DisplayPort);
        let scan = propose(&real_pc(), Some(&mac), &empty(), ProfileDb::builtin());
        assert_eq!(scan.candidates[0].wiring[&Machine::Mac], None);
    }

    #[test]
    fn what_is_already_in_the_desk_is_marked_and_disconnected_ones_counted() {
        let mut desk = empty();
        let new = new_monitors(
            &[NewMonitor {
                label: "34\"".into(),
                pnp_ids: vec!["SAM711A".into(), "SAM7119".into()],
                wiring: [
                    (Machine::Pc, Input::DisplayPort),
                    (Machine::Mac, Input::Hdmi),
                ]
                .into(),
            }],
            &desk,
            ProfileDb::builtin(),
        )
        .unwrap();
        desk.monitors = new;
        // Solo se ve el 32": el 34" del puesto no está conectado.
        let pc = caps(
            Machine::Pc,
            vec![mon(
                "SAM7489",
                "Odyssey G5",
                Some(32),
                32,
                Some(Input::DisplayPort),
            )],
        );
        let scan = propose(&pc, None, &desk, ProfileDb::builtin());
        assert_eq!(scan.disconnected, 1);
        assert_eq!(scan.candidates.len(), 1);
        assert_eq!(scan.candidates[0].in_desk, None);
        // Visto desde la Mac con su id: es el del puesto.
        let mut seen = mon("SAM7119", "LC34G55T", Some(11), 34, None);
        // El agente ata lo que ve al monitor del puesto por cualquiera de sus identidades.
        seen.monitor = Some("sam711a".into());
        let mac = caps(Machine::Mac, vec![seen]);
        let scan = propose(&mac, None, &desk, ProfileDb::builtin());
        assert_eq!(scan.candidates[0].in_desk, Some("sam711a".into()));
        assert_eq!(scan.disconnected, 0, "{scan:?}");
    }

    #[test]
    fn new_monitors_carry_both_identities_and_the_profile() {
        let got = new_monitors(
            &[NewMonitor {
                label: "34\"".into(),
                pnp_ids: vec!["SAM711A".into(), "SAM7119".into(), "sam7119".into()],
                wiring: [
                    (Machine::Pc, Input::DisplayPort),
                    (Machine::Mac, Input::Hdmi),
                ]
                .into(),
            }],
            &empty(),
            ProfileDb::builtin(),
        )
        .unwrap();
        let m = &got[0];
        assert_eq!(m.id, "sam711a".into());
        assert_eq!(m.edid_model.as_deref(), Some("SAM711A"));
        assert_eq!(m.edid_aliases, ["SAM7119"]);
        assert_eq!(
            m.ddc,
            Some(DdcProfile {
                hdmi: 0x06,
                display_port: 0x09
            })
        );
        // Una sola identidad conocida por la base alcanza: la de la Mac.
        let by_alias = new_monitors(
            &[NewMonitor {
                label: "32\"".into(),
                pnp_ids: vec!["SAM748A".into()],
                wiring: [(Machine::Mac, Input::Hdmi)].into(),
            }],
            &empty(),
            ProfileDb::builtin(),
        )
        .unwrap();
        assert_eq!(by_alias[0].ddc.as_ref().unwrap().hdmi, 0x05);
    }

    #[test]
    fn bad_new_monitors_are_rejected() {
        let ok = NewMonitor {
            label: "x".into(),
            pnp_ids: vec!["AAA0001".into()],
            wiring: [(Machine::Pc, Input::Hdmi)].into(),
        };
        let db = ProfileDb::builtin();
        let no_id = NewMonitor {
            pnp_ids: vec![],
            ..ok.clone()
        };
        assert!(new_monitors(&[no_id], &empty(), db).is_err());
        let unwired = NewMonitor {
            wiring: BTreeMap::new(),
            ..ok.clone()
        };
        assert!(new_monitors(&[unwired], &empty(), db).is_err());
        let same_input = NewMonitor {
            wiring: [(Machine::Pc, Input::Hdmi), (Machine::Mac, Input::Hdmi)].into(),
            ..ok.clone()
        };
        assert!(new_monitors(&[same_input], &empty(), db).is_err());
        // Repetido dentro del mismo pedido y contra el puesto.
        assert!(new_monitors(&[ok.clone(), ok.clone()], &empty(), db).is_err());
        let desk = Config {
            monitors: new_monitors(std::slice::from_ref(&ok), &empty(), db).unwrap(),
        };
        assert!(new_monitors(&[ok], &desk, db).is_err());
    }

    #[test]
    fn twins_get_distinct_ids() {
        let a = NewMonitor {
            label: "a".into(),
            pnp_ids: vec!["AAA0001".into()],
            wiring: [(Machine::Pc, Input::Hdmi)].into(),
        };
        // Mismo modelo pero otra identidad: otro monitor físico con el mismo id base.
        let b = NewMonitor {
            pnp_ids: vec!["AAA0001x".into()],
            ..a.clone()
        };
        let got = new_monitors(&[a, b], &empty(), ProfileDb::builtin());
        assert!(got.is_ok());
    }

    fn arb_mon() -> impl Strategy<Value = MonitorCaps> {
        (
            0u32..4,
            proptest::option::of(0u32..3),
            prop::sample::select(vec![24u32, 27, 32, 34]),
            proptest::option::of(prop::sample::select(vec![Input::Hdmi, Input::DisplayPort])),
        )
            .prop_map(|(p, serial, inches, output)| {
                mon(
                    &format!("AAA000{p}"),
                    &format!("M{p}"),
                    serial,
                    inches,
                    output,
                )
            })
    }

    proptest! {
        /// Cada monitor que ve alguna máquina aparece en exactamente un monitor físico, y
        /// nunca dos de la misma máquina en el mismo.
        #[test]
        fn every_seen_monitor_lands_in_exactly_one_candidate(
            a in proptest::collection::vec(arb_mon(), 0..5),
            b in proptest::collection::vec(arb_mon(), 0..5),
        ) {
            let pc = caps(Machine::Pc, a.clone());
            let mac = caps(Machine::Mac, b.clone());
            let scan = propose(&pc, Some(&mac), &empty(), ProfileDb::builtin());
            let mine = scan.candidates.iter().filter(|c| c.here.is_some()).count();
            let theirs = scan.candidates.iter().filter(|c| c.peer.is_some()).count();
            prop_assert_eq!(mine, a.len());
            prop_assert_eq!(theirs, b.len());
            prop_assert!(scan.candidates.len() >= a.len().max(b.len()));
            prop_assert!(scan.candidates.len() <= a.len() + b.len());
            for c in &scan.candidates {
                if c.basis == LinkBasis::Single {
                    prop_assert!(c.here.is_none() || c.peer.is_none());
                } else {
                    prop_assert!(c.here.is_some() && c.peer.is_some());
                }
                // Nunca la misma entrada en las dos máquinas.
                let ins: Vec<Input> = c.wiring.values().flatten().copied().collect();
                if ins.len() == 2 {
                    prop_assert_ne!(ins[0], ins[1]);
                }
            }
        }
    }
}
