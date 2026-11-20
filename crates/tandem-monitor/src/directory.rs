//! What a machine knows about the desk's monitors: the channels it sees itself, the ones
//! the other announced, who owns each one and the last thing that owner read.
//!
//! It is pure logic: the agent hands it the announcements and the reports and asks it for the
//! views for the UI and which monitors it has to read.

use std::collections::{BTreeMap, BTreeSet};

use tandem_core::{Machine, MonitorInput};
use tandem_proto::{MonitorChannel, MonitorCompat, MonitorReport, MonitorView};

use crate::election::elect;

#[derive(Debug, Clone)]
pub struct Directory {
    me: Machine,
    local: Vec<MonitorChannel>,
    /// What the other announced, while it is on and connected; `None` otherwise.
    peer: Option<Vec<MonitorChannel>>,
    /// The last report of each monitor from each machine. Kept per machine (T-280): a reading
    /// of the non-owner, made while it did not know the other's channels yet, must not replace
    /// the owner's, which the owner does not send again while nothing changes on its side.
    reports: BTreeMap<(String, Machine), MonitorReport>,
    /// The owners as of the last change of channels, to forget what a machine read once it
    /// stops owning a monitor.
    elected: BTreeMap<String, Machine>,
}

impl Directory {
    pub fn new(me: Machine) -> Self {
        Directory {
            me,
            local: Vec::new(),
            peer: None,
            reports: BTreeMap::new(),
            elected: BTreeMap::new(),
        }
    }

    pub fn me(&self) -> Machine {
        self.me
    }

    /// This machine turned out to be the other side: the stored reports switch sides with it.
    pub fn set_me(&mut self, me: Machine) {
        if me != self.me {
            self.reports = std::mem::take(&mut self.reports)
                .into_iter()
                .map(|((id, from), r)| ((id, from.other()), r))
                .collect();
            for owner in self.elected.values_mut() {
                *owner = owner.other();
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
        self.reelect();
        changed
    }

    /// `true` if it changed.
    pub fn set_peer(&mut self, channels: Option<Vec<MonitorChannel>>) -> bool {
        let changed = self.peer != channels;
        self.peer = channels;
        self.reelect();
        changed
    }

    /// A machine that stops owning a monitor no longer reads it: what it read last stops
    /// telling the truth and is forgotten, so it is not shown if it becomes the owner again
    /// before it reads anew. The new owner's report stays, even if it arrived before this
    /// machine knew the channels that made it the owner (T-280).
    fn reelect(&mut self) {
        let owners = self.owners();
        for (id, before) in &self.elected {
            if owners.get(id) != Some(before) {
                self.reports.remove(&(id.clone(), *before));
            }
        }
        self.elected = owners;
    }

    /// A report from `from` (this machine or the other). It is only shown while `from` is the
    /// owner: one from a previous owner no longer tells the truth.
    pub fn set_report(&mut self, from: Machine, report: MonitorReport) {
        self.reports.insert((report.id.clone(), from), report);
    }

    /// Everything `from` reads now (each `ControlMsg::Monitors` carries the sender's whole
    /// list): what it no longer reads is forgotten. A reading from a machine that is not the
    /// owner here is kept only while that machine still reads it, so it is never shown later as
    /// current if that machine becomes the owner.
    pub fn set_reports(&mut self, from: Machine, reports: Vec<MonitorReport>) {
        self.reports.retain(|(_, m), _| *m != from);
        for r in reports {
            self.set_report(from, r);
        }
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
        self.reports.get(&(id.to_owned(), owner))
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
                // One seen only by its EDID tells the least (T-340).
                let (_, shown) = seen
                    .iter()
                    .find(|(_, c)| c.compat.is_compatible())
                    .or_else(|| {
                        seen.iter()
                            .find(|(_, c)| c.compat != MonitorCompat::NoChannel)
                    })
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
    use proptest::prelude::*;

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
        assert_eq!(a.input("std"), Some(MonitorInput::Hdmi), "A's own reading");
        a.set_peer(Some(vec![ch("std", true, 5)]));
        assert_eq!(
            a.input("std"),
            None,
            "the report was from before B stopped owning it"
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

    /// T-340: the PC has the channel (DP); the Mac only the EDID (HDMI). One monitor with both
    /// connections, owned by the PC, and nothing changes when the Mac's comes second.
    #[test]
    fn a_machine_without_a_channel_adds_its_connection_but_never_owns() {
        let pc = MonitorChannel {
            via: Some(MonitorInput::DisplayPort),
            ..ch("m34", true, 40)
        };
        let mac = MonitorChannel {
            compat: MonitorCompat::NoChannel,
            via: Some(MonitorInput::Hdmi),
            ..ch("m34", true, 0)
        };
        for (a, b) in [(pc.clone(), mac.clone()), (mac.clone(), pc.clone())] {
            let pc_side = if a == pc { Machine::A } else { Machine::B };
            let (da, db) = pair(vec![a], vec![b]);
            assert_eq!(da.owners(), db.owners());
            assert_eq!(
                da.owner("m34"),
                Some(pc_side),
                "a 0 ms latency does not win"
            );
            let views = da.views();
            assert_eq!(views.len(), 1);
            assert!(views[0].compat.is_compatible());
            assert_eq!(views[0].seen_by.len(), 2);
            let mut want = vec![
                (pc_side, MonitorInput::DisplayPort),
                (pc_side.other(), MonitorInput::Hdmi),
            ];
            want.sort();
            assert_eq!(views[0].via, want);
        }
        // Only the Mac, without a channel: shown, without owner.
        let (da, _) = pair(vec![mac], vec![]);
        assert_eq!(da.owner("m34"), None);
        assert_eq!(da.views()[0].compat, MonitorCompat::NoChannel);
    }

    /// T-280: `A` reports, then `B` reads the monitor while it still believes it owns it (it does
    /// not know `A`'s channel yet), then the channels cross and `A` is elected. `A` has nothing
    /// new to send, so `B` must still have `A`'s report.
    #[test]
    fn an_early_reading_of_the_non_owner_does_not_replace_the_owners_report() {
        let std = ch("std", true, 5);
        let mut b = Directory::new(Machine::B);
        b.set_local(vec![std.clone()]);
        b.set_report(Machine::A, report("std", MonitorInput::DisplayPort));
        assert_eq!(b.owned_here(), BTreeSet::from(["std".to_owned()]));
        b.set_report(Machine::B, report("std", MonitorInput::Hdmi));
        assert_eq!(b.input("std"), Some(MonitorInput::Hdmi));
        b.set_peer(Some(vec![std]));
        assert_eq!(b.owner("std"), Some(Machine::A), "a tie goes to A");
        assert_eq!(b.input("std"), Some(MonitorInput::DisplayPort));
    }

    /// Review of #183: `B`'s early claim reaches `A`, then `A` is elected. If `A` later loses
    /// its channel, `B`'s old reading must not show as current. `B` stopped reading once `A`
    /// was elected, and its next list (empty) clears it.
    #[test]
    fn a_reading_the_other_no_longer_sends_is_not_shown_when_it_becomes_the_owner() {
        let std = ch("std", true, 5);
        let mut a = Directory::new(Machine::A);
        a.set_local(vec![std.clone()]);
        a.set_reports(Machine::A, vec![report("std", MonitorInput::DisplayPort)]);
        a.set_reports(Machine::B, vec![report("std", MonitorInput::Hdmi)]);
        a.set_peer(Some(vec![std.clone()]));
        assert_eq!(a.input("std"), Some(MonitorInput::DisplayPort));
        a.set_reports(Machine::B, vec![]);
        a.set_local(vec![]);
        assert_eq!(a.owner("std"), Some(Machine::B));
        assert_eq!(
            a.input("std"),
            None,
            "B's reading is from before A was elected"
        );
        a.set_reports(Machine::B, vec![report("std", MonitorInput::DisplayPort)]);
        assert_eq!(a.input("std"), Some(MonitorInput::DisplayPort));
    }

    #[test]
    fn a_new_list_replaces_the_senders_previous_one() {
        let mut d = Directory::new(Machine::A);
        d.set_peer(Some(vec![ch("std", true, 5), ch("two", true, 5)]));
        d.set_reports(
            Machine::B,
            vec![
                report("std", MonitorInput::Hdmi),
                report("two", MonitorInput::Hdmi),
            ],
        );
        d.set_reports(Machine::B, vec![report("two", MonitorInput::DisplayPort)]);
        assert_eq!(d.input("std"), None);
        assert_eq!(d.input("two"), Some(MonitorInput::DisplayPort));
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

    proptest! {
        /// T-280: both machines see the monitor with the same latency (a tie: `A` owns it).
        /// Whatever the order in which `A`'s report, `B`'s early reading and the other's
        /// channels arrive, both end up showing `A`'s report.
        #[test]
        fn both_show_the_owners_report_in_any_order(
            order in Just(vec![0u8, 1, 2]).prop_shuffle(),
        ) {
            let std = ch("std", true, 5);
            for me in Machine::ALL {
                let mut d = Directory::new(me);
                d.set_local(vec![std.clone()]);
                for step in &order {
                    match step {
                        0 => d.set_report(Machine::A, report("std", MonitorInput::DisplayPort)),
                        1 => d.set_report(Machine::B, report("std", MonitorInput::Hdmi)),
                        _ => {
                            d.set_peer(Some(vec![std.clone()]));
                        }
                    }
                }
                prop_assert_eq!(d.owner("std"), Some(Machine::A));
                prop_assert_eq!(d.input("std"), Some(MonitorInput::DisplayPort), "{:?} on {}", order, me);
            }
        }
    }
}
