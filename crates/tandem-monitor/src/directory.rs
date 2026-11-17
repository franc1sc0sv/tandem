//! Lo que una máquina sabe de los monitores del puesto: los canales que ve ella, los que
//! anunció la otra, quién es el dueño de cada uno y lo último que leyó ese dueño.
//!
//! Es lógica pura: el agente le pasa los anuncios y los informes y le pide las vistas para la
//! UI y qué monitores le toca leer.

use std::collections::{BTreeMap, BTreeSet};

use tandem_core::{Machine, MonitorInput};
use tandem_proto::{MonitorChannel, MonitorReport, MonitorView};

use crate::election::elect;

#[derive(Debug, Clone)]
pub struct Directory {
    me: Machine,
    local: Vec<MonitorChannel>,
    /// Lo que anunció la otra, mientras está prendida y conectada; `None` si no.
    peer: Option<Vec<MonitorChannel>>,
    /// El último informe de cada monitor y quién lo mandó.
    reports: BTreeMap<String, (Machine, MonitorReport)>,
}

impl Directory {
    pub fn new(me: Machine) -> Self {
        Directory {
            me,
            local: Vec::new(),
            peer: None,
            reports: BTreeMap::new(),
        }
    }

    pub fn me(&self) -> Machine {
        self.me
    }

    /// Esta máquina resultó ser el otro lado: los informes guardados cambian de lado con ella.
    pub fn set_me(&mut self, me: Machine) {
        if me != self.me {
            for (from, _) in self.reports.values_mut() {
                *from = from.other();
            }
            self.me = me;
        }
    }

    pub fn local(&self) -> &[MonitorChannel] {
        &self.local
    }

    /// `true` si cambió.
    pub fn set_local(&mut self, channels: Vec<MonitorChannel>) -> bool {
        let changed = self.local != channels;
        self.local = channels;
        changed
    }

    /// `true` si cambió.
    pub fn set_peer(&mut self, channels: Option<Vec<MonitorChannel>>) -> bool {
        let changed = self.peer != channels;
        self.peer = channels;
        changed
    }

    /// Un informe de `from` (esta máquina o la otra). Solo se muestra mientras `from` sea el
    /// dueño: uno de un dueño anterior ya no dice la verdad.
    pub fn set_report(&mut self, from: Machine, report: MonitorReport) {
        self.reports.insert(report.id.clone(), (from, report));
    }

    fn channel_of(&self, machine: Machine, id: &str) -> Option<&MonitorChannel> {
        let list = if machine == self.me {
            Some(&self.local)
        } else {
            self.peer.as_ref()
        };
        list?.iter().find(|c| c.id == id)
    }

    fn ids(&self) -> BTreeSet<&str> {
        self.local
            .iter()
            .chain(self.peer.iter().flatten())
            .map(|c| c.id.as_str())
            .collect()
    }

    pub fn owner(&self, id: &str) -> Option<Machine> {
        elect(
            self.channel_of(Machine::A, id),
            self.channel_of(Machine::B, id),
        )
    }

    /// El dueño de cada monitor que tiene uno.
    pub fn owners(&self) -> BTreeMap<String, Machine> {
        self.ids()
            .into_iter()
            .filter_map(|id| self.owner(id).map(|o| (id.to_owned(), o)))
            .collect()
    }

    /// Los monitores que le toca leer a esta máquina.
    pub fn owned_here(&self) -> BTreeSet<String> {
        self.owners()
            .into_iter()
            .filter(|(_, owner)| *owner == self.me)
            .map(|(id, _)| id)
            .collect()
    }

    /// Lo último que leyó el dueño actual de `id`.
    pub fn report(&self, id: &str) -> Option<&MonitorReport> {
        let owner = self.owner(id)?;
        self.reports
            .get(id)
            .filter(|(from, _)| *from == owner)
            .map(|(_, r)| r)
    }

    /// La entrada en pantalla según el dueño.
    pub fn input(&self, id: &str) -> Option<MonitorInput> {
        self.report(id)?.input
    }

    /// Un monitor que alguna de las dos ve, compatible o no.
    pub fn channel(&self, id: &str) -> Option<&MonitorChannel> {
        self.channel_of(self.me, id)
            .or_else(|| self.channel_of(self.me.other(), id))
    }

    /// Todos los monitores, ordenados por id.
    pub fn views(&self) -> Vec<MonitorView> {
        self.ids()
            .into_iter()
            .filter_map(|id| {
                let seen: Vec<(Machine, &MonitorChannel)> = Machine::ALL
                    .into_iter()
                    .filter_map(|m| self.channel_of(m, id).map(|c| (m, c)))
                    .collect();
                // Si una lo ve compatible y la otra no, vale lo que permite controlarlo (el
                // dueño lo ve compatible); si no, lo de cualquiera.
                let (_, shown) = seen
                    .iter()
                    .find(|(_, c)| c.compat.is_compatible())
                    .or_else(|| seen.first())?;
                Some(MonitorView {
                    id: id.to_owned(),
                    name: shown.name.clone(),
                    compat: shown.compat.clone(),
                    seen_by: seen.iter().map(|(m, _)| *m).collect(),
                    owner: self.owner(id),
                    report: self.report(id).cloned(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use tandem_proto::MonitorCompat;

    use super::*;

    fn ch(id: &str, compatible: bool, latency_ms: u32) -> MonitorChannel {
        MonitorChannel {
            id: id.into(),
            name: id.to_uppercase(),
            compat: if compatible {
                MonitorCompat::Compatible { protocol: 2 }
            } else {
                MonitorCompat::NotCompatible {
                    reason: "fábrica".into(),
                }
            },
            latency_ms,
        }
    }

    fn report(id: &str, input: MonitorInput) -> MonitorReport {
        MonitorReport {
            id: id.into(),
            input: Some(input),
            power_on: Some(true),
            signal: Some(true),
            events: vec![],
        }
    }

    /// Las dos máquinas, cada una con su vista de los mismos anuncios.
    fn pair(a: Vec<MonitorChannel>, b: Vec<MonitorChannel>) -> (Directory, Directory) {
        let mut da = Directory::new(Machine::A);
        da.set_local(a.clone());
        da.set_peer(Some(b.clone()));
        let mut db = Directory::new(Machine::B);
        db.set_local(b);
        db.set_peer(Some(a));
        (da, db)
    }

    #[test]
    fn both_machines_agree_on_the_owners() {
        let (a, b) = pair(
            vec![ch("std", true, 40), ch("fab", false, 3)],
            vec![ch("std", true, 5)],
        );
        assert_eq!(a.owners(), b.owners());
        assert_eq!(a.owner("std"), Some(Machine::B));
        assert_eq!(a.owner("fab"), None);
        assert!(a.owned_here().is_empty());
        assert_eq!(b.owned_here(), BTreeSet::from(["std".to_owned()]));
    }

    #[test]
    fn losing_the_peer_hands_its_monitors_over() {
        let (mut a, _) = pair(vec![ch("std", true, 40)], vec![ch("std", true, 5)]);
        assert_eq!(a.owner("std"), Some(Machine::B));
        assert!(a.set_peer(None));
        assert_eq!(a.owner("std"), Some(Machine::A));
        assert!(!a.set_peer(None), "sin cambio");
    }

    #[test]
    fn only_the_current_owners_report_is_shown() {
        let (mut a, _) = pair(vec![ch("std", true, 40)], vec![ch("std", true, 5)]);
        a.set_report(Machine::A, report("std", MonitorInput::Hdmi));
        assert_eq!(a.input("std"), None, "A no es el dueño");
        a.set_report(Machine::B, report("std", MonitorInput::DisplayPort));
        assert_eq!(a.input("std"), Some(MonitorInput::DisplayPort));
        a.set_peer(None);
        assert_eq!(a.input("std"), None, "el informe era del dueño anterior");
    }

    #[test]
    fn views_list_every_monitor_with_who_sees_it() {
        let (a, _) = pair(
            vec![ch("std", true, 40), ch("fab", false, 3)],
            vec![ch("std", true, 5)],
        );
        let views = a.views();
        assert_eq!(views.len(), 2);
        let fab = &views[0];
        assert_eq!(
            (fab.id.as_str(), fab.owner, fab.seen_by.as_slice()),
            ("fab", None, &[Machine::A][..])
        );
        assert!(!fab.compat.is_compatible());
        let std = &views[1];
        assert_eq!(std.seen_by, [Machine::A, Machine::B]);
        assert_eq!(std.owner, Some(Machine::B));
    }

    #[test]
    fn changing_sides_keeps_the_reports_with_their_machine() {
        let mut d = Directory::new(Machine::A);
        d.set_local(vec![ch("std", true, 5)]);
        d.set_report(Machine::A, report("std", MonitorInput::Hdmi));
        d.set_me(Machine::B);
        assert_eq!(d.owner("std"), Some(Machine::B));
        assert_eq!(d.input("std"), Some(MonitorInput::Hdmi));
    }
}
