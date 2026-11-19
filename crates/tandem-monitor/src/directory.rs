//! What a machine knows about the desk's monitors: the channels it sees itself, the ones
//! the other announced, who owns each one and the last thing that owner read.
//!
//! It is pure logic: the agent hands it the announcements and the reports and asks it for the
//! views for the UI and which monitors it has to read.

use std::collections::{BTreeMap, BTreeSet};

use tandem_core::{Machine, MonitorInput};
use tandem_proto::{MonitorChannel, MonitorReport, MonitorView};

use crate::election::elect;

#[derive(Debug, Clone)]
pub struct Directory {
    me: Machine,
    local: Vec<MonitorChannel>,
    /// What the other announced, while it is on and connected; `None` otherwise.
    peer: Option<Vec<MonitorChannel>>,
    /// The last report of each monitor and who sent it.
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

    /// This machine turned out to be the other side: the stored reports switch sides with it.
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

    /// `true` if it changed.
    pub fn set_local(&mut self, channels: Vec<MonitorChannel>) -> bool {
        let changed = self.local != channels;
        self.local = channels;
        changed
    }

    /// `true` if it changed.
    pub fn set_peer(&mut self, channels: Option<Vec<MonitorChannel>>) -> bool {
        let changed = self.peer != channels;
        self.peer = channels;
        changed
    }

    /// A report from `from` (this machine or the other). It is only shown while `from` is the
    /// owner: one from a previous owner no longer tells the truth.
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

    /// The owner of each monitor that has one.
    pub fn owners(&self) -> BTreeMap<String, Machine> {
        self.ids()
            .into_iter()
            .filter_map(|id| self.owner(id).map(|o| (id.to_owned(), o)))
            .collect()
    }

    /// The monitors this machine has to read.
    pub fn owned_here(&self) -> BTreeSet<String> {
        self.owners()
            .into_iter()
            .filter(|(_, owner)| *owner == self.me)
            .map(|(id, _)| id)
            .collect()
    }

    /// The last thing the current owner of `id` read.
    pub fn report(&self, id: &str) -> Option<&MonitorReport> {
        let owner = self.owner(id)?;
        self.reports
            .get(id)
            .filter(|(from, _)| *from == owner)
            .map(|(_, r)| r)
    }

    /// The input on screen according to the owner.
    pub fn input(&self, id: &str) -> Option<MonitorInput> {
        self.report(id)?.input
    }

    /// A monitor that either machine sees, compatible or not.
    pub fn channel(&self, id: &str) -> Option<&MonitorChannel> {
        self.channel_of(self.me, id)
            .or_else(|| self.channel_of(self.me.other(), id))
    }

    /// All the monitors, sorted by id.
    pub fn views(&self) -> Vec<MonitorView> {
        self.ids()
            .into_iter()
            .filter_map(|id| {
                let seen: Vec<(Machine, &MonitorChannel)> = Machine::ALL
                    .into_iter()
                    .filter_map(|m| self.channel_of(m, id).map(|c| (m, c)))
                    .collect();
                // If one sees it as compatible and the other does not, what lets it be controlled wins (the
                // owner sees it as compatible); otherwise, whichever one's.
                let (_, shown) = seen
                    .iter()
                    .find(|(_, c)| c.compat.is_compatible())
                    .or_else(|| seen.first())?;
                Some(MonitorView {
                    id: id.to_owned(),
                    name: shown.name.clone(),
                    compat: shown.compat.clone(),
                    seen_by: seen.iter().map(|(m, _)| *m).collect(),
                    via: seen
                        .iter()
                        .filter_map(|(m, c)| c.via.map(|i| (*m, i)))
                        .collect(),
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
                    reason: "factory".into(),
                }
            },
            latency_ms,
            via: None,
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

    /// The two machines, each with its view of the same announcements.
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
        assert!(!a.set_peer(None), "no change");
    }

    #[test]
    fn only_the_current_owners_report_is_shown() {
        let (mut a, _) = pair(vec![ch("std", true, 40)], vec![ch("std", true, 5)]);
        a.set_report(Machine::A, report("std", MonitorInput::Hdmi));
        assert_eq!(a.input("std"), None, "A is not the owner");
        a.set_report(Machine::B, report("std", MonitorInput::DisplayPort));
        assert_eq!(a.input("std"), Some(MonitorInput::DisplayPort));
        a.set_peer(None);
        assert_eq!(
            a.input("std"),
            None,
            "the report was from the previous owner"
        );
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
