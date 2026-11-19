//! Updating both machines (T-122, DESIGN §13, DECISIONS D19).
//!
//! Tauri's updater downloads and installs the package of **this** machine (UI and `tandemd`,
//! which is inside). What lives here decides the rest and is portable:
//!
//! - [`Version`]: comparing versions (`x.y.z` and `x.y.z-rc.n`, in SemVer order).
//! - [`plan`] and [`run`]: "Update all". The other machine first and, when it comes back with
//!   the new version, this one. The two are never restarted at once: while one updates, the
//!   other keeps the keyboard and mouse, and this one (the one that drives) is not touched until
//!   the other has come back. If the other does not come back, rollback: this one stays on its
//!   version and nothing else changes, so the desk stays usable with the machine that works.
//! - [`drives_auto`] and [`requested_install`]: who updates automatically and when the other
//!   machine asked to install.
//! - [`Settings`]: the "Update automatically" switch in `update.toml`.

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

/// How long to wait for the other machine to come back with the new version: download the
/// package, install it and start again. Past this it is given up for lost and this one is not updated.
pub const PEER_RETURN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How often to look for a new version with "Update automatically".
pub const AUTO_CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// The switch's file, next to the agent's configuration.
pub const SETTINGS_FILE: &str = "update.toml";

// ---------------------------------------------------------------------------------------
// Versions

/// A Tandem version: `major.minor.patch` with an optional prerelease suffix
/// (`0.3.0-rc.2`). Ordered like SemVer: a prerelease comes before its final version and
/// its numeric parts compare as numbers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// The suffix parts, separated by `.`; empty in a final version.
    pub pre: Vec<Ident>,
}

/// One part of the prerelease suffix.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ident {
    Num(u64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("\"{0}\" is not a version (expected x.y.z or x.y.z-rc.n)")]
pub struct BadVersion(pub String);

impl FromStr for Version {
    type Err = BadVersion;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || BadVersion(s.to_owned());
        // Tags carry a `v`; build metadata (`+...`) does not count for ordering.
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
                    // Only digits but not a number: leading zeros, which SemVer does not allow.
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

/// A number without leading zeros (SemVer); `None` if it is not one.
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
            // Numeric ones come before text ones.
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
                // Part by part; with all equal, the longer one is greater.
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
// The plan for the two machines

/// The other machine, as seen for updating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Peer {
    /// No peer: this machine updates by itself.
    Alone,
    /// Paired but without a link.
    Unreachable,
    /// Connected, but it has not said its version yet.
    Unknown,
    /// It connected speaking another protocol version: it runs another version of Tandem.
    OtherProtocol,
    At(Version),
}

/// One step of "Update all", in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Ask the other machine to install the new version.
    AskPeer,
    /// Wait for it to come back with it.
    WaitPeer,
    /// Install here; the app restarts and `tandemd` with it.
    InstallHere,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("both machines already have {0}")]
    UpToDate(Version),
    #[error("the other machine is not connected: both must be updated together")]
    PeerUnreachable,
    #[error("it is not yet known which version the other machine has")]
    PeerUnknown,
    #[error(
        "the other machine runs a different Tandem version that cannot talk to this one: install {0} by hand on the one that is behind"
    )]
    PeerOtherProtocol(Version),
}

/// The steps to leave both machines on `target`: the other first, then this one, and
/// only those with an older version.
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

/// The other machine according to the session with `tandemd`: `paired` and `link` come from the peer
/// ([`crate::session::PeerView`]) and `status` from the latest the agent said about versions.
pub fn peer_from(paired: bool, link: bool, status: Option<&UpdateStatus>) -> Peer {
    if !paired {
        return Peer::Alone;
    }
    // Protocol first: if the other speaks another, the link never comes up.
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

/// The other machine came back with `target` (or a newer one). If it no longer understands this one, also:
/// that only happens if its version changed, and that is exactly what it was asked.
pub fn came_back(peer: &Peer, target: &Version) -> bool {
    match peer {
        Peer::At(v) => v >= target,
        Peer::OtherProtocol => true,
        Peer::Alone | Peer::Unreachable | Peer::Unknown => false,
    }
}

/// What "Update all" needs from outside: the other machine (through `tandemd`) and this
/// one's updater. In tests, a double.
pub trait Fleet: Send {
    /// Ask the other machine to install `target`.
    fn ask_peer(&mut self, target: &Version) -> impl Future<Output = Result<(), String>> + Send;
    /// Waits for the other machine's next change (its version, or that it was lost); `None` if no more
    /// will come (the session with `tandemd` ended).
    fn peer_changed(&mut self) -> impl Future<Output = Option<Peer>> + Send;
    /// Downloads and installs `target` here. In the app it does not return on success: it restarts.
    fn install_here(&mut self, target: &Version)
    -> impl Future<Output = Result<(), String>> + Send;
}

/// Why "Update all" did not finish.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    #[error("the other machine did not agree to update: {0}")]
    PeerRefused(String),
    /// Rollback: this one was not installed and the desk stays with the machine that works.
    #[error(
        "the other machine did not come back with {target} in {} min: this one stays on its version",
        .waited.as_secs() / 60
    )]
    PeerDidNotReturn { target: Version, waited: Duration },
    #[error("the connection with tandemd was lost while the other machine was updating")]
    SessionLost,
    #[error("the other machine already has {target}, but this one could not be updated: {reason}")]
    InstallHere { target: Version, reason: String },
}

/// Runs the steps of [`plan`] in order, reporting each one to `on_step` before starting it.
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

/// With "Update automatically", who drives: with no peer, this one; with a peer, only `A`. That way both
/// machines know without talking and never both drive at once (DECISIONS A1).
pub fn drives_auto(me: Machine, peer: &Peer) -> bool {
    matches!(peer, Peer::Alone) || me == Machine::A
}

/// The version the other machine asked to install here, if it is newer than this one's.
pub fn requested_install(here: &Version, request: Option<&str>) -> Option<Version> {
    let v: Version = request?.parse().ok()?;
    (v > *here).then_some(v)
}

// ---------------------------------------------------------------------------------------
// "Update automatically"

/// What is saved in `update.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Search and install automatically (by default, yes: that is how it comes in `F-Settings`).
    pub auto: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto: true }
    }
}

/// `update.toml` in the same directory as the agent's configuration.
pub fn settings_file_for(agent_config: &Path) -> PathBuf {
    agent_config
        .parent()
        .map_or_else(|| PathBuf::from(SETTINGS_FILE), |d| d.join(SETTINGS_FILE))
}

/// A file that is missing or not understood means the usual: it is no reason not to
/// update.
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
    // Write aside and rename: a cut halfway does not leave the file empty.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

// ---------------------------------------------------------------------------------------
// What the window sees

/// The Updates tab of Settings (`F-Settings`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateView {
    /// Grows with every change. The command replies and the events reach the window on
    /// different channels: it keeps the view with the highest one, not the last to arrive.
    pub seq: u64,
    /// This machine's version.
    pub here: String,
    /// The other machine's; `None` with no peer or if it is not known yet.
    pub peer: Option<String>,
    /// There is a peer: "Update all" includes the other machine.
    pub paired: bool,
    /// The new version that was found, if it is newer than either machine's.
    pub available: Option<String>,
    /// Checking now.
    pub checking: bool,
    /// The "Update all" step in progress.
    pub step: Option<Step>,
    /// The latest thing that went wrong (when checking or updating), to show as is.
    pub error: Option<String>,
    /// "Update automatically".
    pub auto: bool,
}

/// Numbers a view that just changed: above every earlier one.
pub fn renumber(view: &mut UpdateView) {
    view.seq = view.seq.wrapping_add(1);
}

/// Takes the install for "Update all" if nothing else holds it (`busy`), and already shows
/// its first step: the command reply then says what is happening, instead of an idle view
/// that the window could take as newer than the step events. `false` if it was taken.
pub fn take_install(view: &mut UpdateView, busy: &mut bool, first: Option<Step>) -> bool {
    if *busy {
        return false;
    }
    *busy = true;
    view.step = first;
    view.error = None;
    true
}

/// The updater's comparator when **checking**: the release is useful if either machine
/// is behind, not just this one (the default comparator only looks at "newer than this app", and
/// with this one up to date and the other behind it would never offer anything). When installing here the usual one applies.
pub fn fetch_release(current: &Version, release: &Version, peer: &Peer) -> bool {
    worth_offering(release, current, peer)
}

/// The found version is useful if it is newer than either machine.
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
            "0.3.0-\u{f1}",
        ] {
            assert!(s.parse::<Version>().is_err(), "\"{s}\" should not be valid");
        }
    }

    #[test]
    fn semver_order() {
        // The example from the SemVer specification, from lowest to highest.
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

        /// The plan never restarts this one before the other, and only asks for what is missing.
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
        // The other already updated (for example, last time this one failed): only this one.
        assert_eq!(plan(&old, &at("0.3.1"), &new), Ok(vec![InstallHere]));
        // This one is already done: only the other.
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
        assert_eq!(requested_install(&here, Some("garbage")), None);
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
            peer_from(true, true, Some(&status(Some("weird"), None))),
            Peer::Unknown
        );
        // With another protocol there is no link, but it is known that it runs another version.
        assert_eq!(
            peer_from(true, false, Some(&status(Some("0.3.0"), Some(99)))),
            Peer::OtherProtocol
        );
    }

    /// Review of #77: with this one up to date and the other behind, the release still has to be downloaded
    /// to offer "Update all" (just the other).
    #[test]
    fn this_up_to_date_and_the_other_behind_still_offers() {
        let (here, release) = (v("0.3.1"), v("0.3.1"));
        let peer = Peer::At(v("0.3.0"));
        assert!(fetch_release(&here, &release, &peer));
        assert_eq!(
            plan(&here, &peer, &release),
            Ok(vec![Step::AskPeer, Step::WaitPeer])
        );
        // Both up to date, or a release older than both: nothing to offer.
        let same = Peer::At(v("0.3.1"));
        assert!(!fetch_release(&here, &release, &same));
        assert!(!fetch_release(&here, &v("0.3.0"), &same));
        // This one behind, like the usual comparator.
        assert!(fetch_release(&v("0.3.0"), &release, &Peer::Alone));
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

    /// The other machine and the updater, simulated. `peer` are the changes it will go on
    /// announcing; empty, it stays silent forever.
    #[derive(Default)]
    struct FakeFleet {
        log: Vec<String>,
        refuse: Option<String>,
        peer: VecDeque<Option<Peer>>,
        install_fails: Option<String>,
    }

    impl Fleet for FakeFleet {
        async fn ask_peer(&mut self, target: &Version) -> Result<(), String> {
            self.log.push(format!("ask {target}"));
            match &self.refuse {
                Some(why) => Err(why.clone()),
                None => Ok(()),
            }
        }

        async fn peer_changed(&mut self) -> Option<Peer> {
            match self.peer.pop_front() {
                Some(p) => {
                    self.log.push(format!("peer: {p:?}"));
                    p
                }
                None => std::future::pending().await,
            }
        }

        async fn install_here(&mut self, target: &Version) -> Result<(), String> {
            self.log.push(format!("install {target}"));
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
                // It drops while installing and comes back with the new one.
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
                "ask 0.3.1",
                "peer: Some(Unreachable)",
                "peer: Some(At(Version { major: 0, minor: 3, patch: 1, pre: [] }))",
                "install 0.3.1",
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_old_version_coming_back_is_not_enough() {
        let mut f = FakeFleet {
            // It came back, but with the old one (the installer failed there): keep waiting.
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
            !f.log.iter().any(|l| l.starts_with("install")),
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
            "the other machine did not come back with 0.3.1 in 1 min: this one stays on its version"
        );
        // This one was not touched.
        assert_eq!(seen, [Step::AskPeer, Step::WaitPeer]);
        assert_eq!(f.log, ["ask 0.3.1", "peer: Some(Unreachable)"]);
    }

    #[tokio::test(start_paused = true)]
    async fn another_protocol_counts_as_back() {
        let mut f = FakeFleet {
            peer: VecDeque::from([Some(Peer::OtherProtocol)]),
            ..FakeFleet::default()
        };
        assert_eq!(go(&mut f).await.0, Ok(()));
        assert_eq!(f.log.last().map(String::as_str), Some("install 0.3.1"));
    }

    #[tokio::test(start_paused = true)]
    async fn refused_or_lost_stops_before_touching_this() {
        let mut f = FakeFleet {
            refuse: Some("the other machine is not connected".into()),
            ..FakeFleet::default()
        };
        let (r, seen) = go(&mut f).await;
        assert_eq!(
            r,
            Err(Failure::PeerRefused(
                "the other machine is not connected".into()
            ))
        );
        assert_eq!(seen, [Step::AskPeer]);

        let mut f = FakeFleet {
            peer: VecDeque::from([None]),
            ..FakeFleet::default()
        };
        assert_eq!(go(&mut f).await.0, Err(Failure::SessionLost));
        assert!(!f.log.iter().any(|l| l.starts_with("install")));
    }

    #[tokio::test(start_paused = true)]
    async fn install_here_failing_is_reported_with_the_other_updated() {
        let mut f = FakeFleet {
            peer: VecDeque::from([Some(Peer::At(v("0.3.1")))]),
            install_fails: Some("no network".into()),
            ..FakeFleet::default()
        };
        let err = go(&mut f).await.0.unwrap_err();
        assert_eq!(
            err.to_string(),
            "the other machine already has 0.3.1, but this one could not be updated: no network"
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
        std::fs::write(&path, "auto = \"maybe\"").unwrap();
        assert_eq!(load_settings(&path), Settings::default());
    }

    fn idle_view() -> UpdateView {
        UpdateView {
            seq: 0,
            here: "0.3.0".into(),
            peer: Some("0.3.0".into()),
            paired: true,
            available: Some("0.3.1".into()),
            checking: false,
            step: None,
            error: Some("the other machine did not come back".into()),
            auto: true,
        }
    }

    // T-251: the window orders the reply and the events by `seq`.
    #[test]
    fn renumber_grows() {
        let mut v = idle_view();
        renumber(&mut v);
        let first = v.seq;
        renumber(&mut v);
        assert!(v.seq > first);
    }

    // T-251: the reply of "Update all" already says its first step, not the idle view.
    #[test]
    fn taking_the_install_shows_the_first_step() {
        let (mut v, mut busy) = (idle_view(), false);
        assert!(take_install(&mut v, &mut busy, Some(Step::AskPeer)));
        assert!(busy);
        assert_eq!(v.step, Some(Step::AskPeer));
        assert_eq!(v.error, None);
        let before = v.clone();
        assert!(
            !take_install(&mut v, &mut busy, Some(Step::InstallHere)),
            "already in progress"
        );
        assert_eq!(v, before);
    }

    #[test]
    fn view_json_shape() {
        let view = UpdateView {
            seq: 4,
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
                "seq": 4,
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
