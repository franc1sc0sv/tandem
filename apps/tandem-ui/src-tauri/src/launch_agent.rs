//! The macOS LaunchAgent that starts `tandemd` at login.
//!
//! The app installs or refreshes it every time it starts: the plist points at the `tandemd`
//! that ships inside `Tandem.app` (`Contents/MacOS/tandemd`) and carries the version, so a
//! new app changes the plist and that restarts the agent with the new binary.
//!
//! Everything it decides (the plist, whether the app is in a stable place, what to do with
//! what is already there) is portable and tested on any system; `launchctl` sits behind
//! [`Launchd`].

use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// Service label in launchd and plist name.
pub const LABEL: &str = "com.franc1sc0sv.tandem.tandemd";
/// The app's bundle identifier (`identifier` in `tauri.conf.json`). The LaunchAgent names it in
/// `AssociatedBundleIdentifiers` so macOS takes the app as the code responsible for `tandemd`
/// (T-344, Apple TN3179): the Local Network alert and its System Settings entry are the app's.
pub const APP_BUNDLE_ID: &str = "com.franc1sc0sv.tandem";
/// Name of the sidecar inside `Contents/MacOS` (Tauri strips the target suffix).
pub const SIDECAR: &str = "tandemd";
/// If `tandemd` exits with an error (for example, an invalid `tandem.toml`), launchd waits
/// this long before relaunching it: without a wait, a persistent error would flood the log.
pub const THROTTLE_SECS: u32 = 30;

/// What goes in the plist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    /// `tandemd` inside the app.
    pub program: PathBuf,
    /// Output and errors of `tandemd` before it opens its own log.
    pub log: PathBuf,
    /// App version: if it changes, the plist changes and the agent restarts.
    pub version: String,
}

/// `~/Library/LaunchAgents/<LABEL>.plist`.
pub fn plist_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// `~/Library/Logs/Tandem/tandemd.log`: `~/Library/Logs` always exists and launchd does not
/// create directories.
pub fn log_path(home: &Path) -> PathBuf {
    home.join("Library/Logs/Tandem/tandemd.log")
}

/// Why the app does not install the agent from where it is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Placement {
    #[error("the app is not running from a Tandem.app (development build)")]
    NotBundled,
    #[error(
        "macOS opened the app from a temporary copy (App Translocation); \
         move it to Applications and open it again"
    )]
    Translocated,
    #[error(
        "the app is running from the disk image; copy it to Applications and open it from there"
    )]
    DiskImage,
}

/// The `tandemd` that sits next to the app's executable, if the app is somewhere the agent
/// can depend on: inside a `.app`, not in a temporary copy or on the `.dmg`.
pub fn sidecar_for(exe: &Path) -> Result<PathBuf, Placement> {
    let dir = exe.parent().ok_or(Placement::NotBundled)?;
    let names: Vec<&str> = dir
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    let bundled = matches!(
        names.as_slice(),
        [.., app, "Contents", "MacOS"] if app.ends_with(".app")
    );
    if !bundled {
        return Err(Placement::NotBundled);
    }
    if names.contains(&"AppTranslocation") {
        return Err(Placement::Translocated);
    }
    if names.first() == Some(&"Volumes") {
        return Err(Placement::DiskImage);
    }
    Ok(dir.join(SIDECAR))
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The LaunchAgent plist.
///
/// - `AssociatedBundleIdentifiers`: the app is the responsible code of `tandemd`. Without it a
///   LaunchAgent that is not installed with `SMAppService` is on its own, and macOS did not show
///   the Local Network alert for it (T-344).
/// - `RunAtLoad`: starts at login (and when installed).
/// - `KeepAlive.SuccessfulExit = false`: if it dies with an error, launchd brings it back (after
///   [`THROTTLE_SECS`]); if it exits cleanly (for example, on shutdown) it does not.
/// - `LimitLoadToSessionType = Aqua` and `ProcessType = Interactive`: only in a graphical
///   session (keyboard and mouse capture needs it) and without macOS throttling it like a
///   background process.
pub fn plist(spec: &AgentSpec) -> String {
    let program = escape(&spec.program.to_string_lossy());
    let log = escape(&spec.log.to_string_lossy());
    let version = escape(&spec.version);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{program}</string>
	</array>
	<key>AssociatedBundleIdentifiers</key>
	<array>
		<string>{APP_BUNDLE_ID}</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>TANDEM_APP_VERSION</key>
		<string>{version}</string>
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ThrottleInterval</key>
	<integer>{THROTTLE_SECS}</integer>
	<key>LimitLoadToSessionType</key>
	<string>Aqua</string>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#
    )
}

/// `launchctl` in the user's session domain (`gui/<uid>`).
pub trait Launchd {
    fn is_loaded(&self, label: &str) -> bool;
    /// Loaded and with a live process. A loaded job can have none: launchd does not
    /// relaunch a process that exited in a way `KeepAlive` accepts.
    fn is_running(&self, label: &str) -> bool;
    /// Removes it from launchd (and stops it). Not being loaded is not an error.
    fn bootout(&self, label: &str) -> Result<(), String>;
    /// Loads it from the plist; with `RunAtLoad` it starts right away.
    fn bootstrap(&self, plist: &Path) -> Result<(), String>;
    /// Kills the service and launches it again (`launchctl kickstart -k`).
    fn kickstart(&self, label: &str) -> Result<(), String>;
}

/// Whether `launchctl print gui/<uid>/<label>` describes a job with a live process.
///
/// Only the job's own lines count (one tab deep): nested blocks (endpoints, event
/// triggers) have their own `state` and `pid`-like keys.
pub fn print_says_running(print: &str) -> bool {
    print.lines().any(|line| {
        let Some(own) = line.strip_prefix('\t') else {
            return false;
        };
        if own.starts_with('\t') {
            return false;
        }
        own == "state = running"
            || own
                .strip_prefix("pid = ")
                .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Restarts `tandemd` so newly granted permissions take effect: macOS evaluates several of
/// them (Input Monitoring, above all) only once per process. It does not start an agent
/// that is not loaded: that is [`ensure`]'s job, when the app opens.
pub fn restart(launchd: &dyn Launchd) -> Result<(), String> {
    if !launchd.is_loaded(LABEL) {
        return Err(
            "tandemd is not loaded in launchd; open Tandem.app from Applications".to_owned(),
        );
    }
    launchd.kickstart(LABEL)
}

/// What [`revive`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revived {
    /// No agent in launchd: there is nothing to start (a development build, or the app has
    /// not installed it yet; [`ensure`] does that when the app opens).
    NotLoaded,
    /// It has a process: launchd is in charge.
    Running,
    /// Loaded without a process: it was started.
    Started,
}

/// Starts `tandemd` if launchd has it loaded but without a process. For when the window keeps
/// finding nobody on the IPC while the app is open; [`ReviveGate`] decides when to call it.
pub fn revive(launchd: &dyn Launchd) -> Result<Revived, String> {
    if !launchd.is_loaded(LABEL) {
        return Ok(Revived::NotLoaded);
    }
    if launchd.is_running(LABEL) {
        return Ok(Revived::Running);
    }
    launchd.kickstart(LABEL)?;
    Ok(Revived::Started)
}

/// Failed connections in a row (one every `session::RETRY_EVERY`) before looking at launchd:
/// a restart in progress (an update, "Restart agent") leaves nobody listening for a moment.
pub const REVIVE_AFTER_MISSES: u32 = 3;

/// Decides when the window, finding nobody on the IPC, asks launchd: after
/// [`REVIVE_AFTER_MISSES`] misses in a row and never more often than [`THROTTLE_SECS`], the
/// same pace launchd keeps for its own relaunches, so a `tandemd` that dies on start is never
/// kickstarted in a loop.
#[derive(Debug, Default)]
pub struct ReviveGate {
    misses: u32,
    last: Option<Instant>,
}

impl ReviveGate {
    /// Something answered on the IPC: the count starts again (the throttle does not).
    pub fn reached(&mut self) {
        self.misses = 0;
    }

    /// One more attempt found nobody listening; `true` if launchd should be asked now.
    pub fn missed(&mut self, now: Instant) -> bool {
        self.misses = self.misses.saturating_add(1);
        if self.misses < REVIVE_AFTER_MISSES {
            return false;
        }
        let throttle = Duration::from_secs(THROTTLE_SECS.into());
        if self
            .last
            .is_some_and(|t| now.saturating_duration_since(t) < throttle)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// What [`ensure`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The plist was already this one and launchd had it loaded.
    Unchanged,
    /// The plist was already this one but it was not loaded (it had been removed by hand or failed before).
    Loaded,
    /// There was no plist.
    Created,
    /// There was a different one (another version or another app location): it was replaced and restarted.
    Updated,
    /// The plist was already this one and loaded, but `tandemd` was not running: it was started.
    Restarted,
}

impl Outcome {
    pub fn describe(self) -> &'static str {
        match self {
            Outcome::Unchanged => "tandemd was already installed as a login agent",
            Outcome::Loaded => "tandemd was loaded into launchd again",
            Outcome::Created => "tandemd is installed: it starts by itself at login",
            Outcome::Updated => "tandemd was brought up to date and restarted",
            Outcome::Restarted => "tandemd was loaded but not running: it was started again",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("{0} is missing; the app looks incomplete")]
    MissingSidecar(PathBuf),
    #[error("could not write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("launchctl did not load the agent: {0}")]
    Launchctl(String),
}

/// Installs and loads the LaunchAgent for `spec` in `plist_file`.
///
/// It only touches launchd when needed: opening the app with the agent already up to date does not restart it.
pub fn ensure(
    plist_file: &Path,
    spec: &AgentSpec,
    launchd: &dyn Launchd,
) -> Result<Outcome, InstallError> {
    if !spec.program.is_file() {
        return Err(InstallError::MissingSidecar(spec.program.clone()));
    }
    let want = plist(spec);
    let have = std::fs::read_to_string(plist_file).ok();
    if have.as_deref() == Some(want.as_str()) {
        if launchd.is_loaded(LABEL) {
            if launchd.is_running(LABEL) {
                return Ok(Outcome::Unchanged);
            }
            launchd.kickstart(LABEL).map_err(InstallError::Launchctl)?;
            return Ok(Outcome::Restarted);
        }
        launchd
            .bootstrap(plist_file)
            .map_err(InstallError::Launchctl)?;
        return Ok(Outcome::Loaded);
    }

    if let Some(dir) = spec.log.parent() {
        create_dir(dir)?;
    }
    write_atomic(plist_file, &want)?;
    // The old one keeps running with the previous binary until it is removed.
    let outcome = if have.is_some() {
        Outcome::Updated
    } else {
        Outcome::Created
    };
    if launchd.is_loaded(LABEL) {
        launchd.bootout(LABEL).map_err(InstallError::Launchctl)?;
    }
    launchd
        .bootstrap(plist_file)
        .map_err(InstallError::Launchctl)?;
    Ok(outcome)
}

/// How the login agent ended up, for the first start.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    /// `tandemd` is installed and loaded.
    pub ok: bool,
    pub text: String,
}

/// Installs or updates the agent of the app running at `exe`, for the user of `home`.
pub fn install_from(exe: &Path, home: &Path, version: &str, launchd: &dyn Launchd) -> Report {
    let program = match sidecar_for(exe) {
        Ok(p) => p,
        Err(why) => {
            return Report {
                ok: false,
                text: format!("tandemd was not installed: {why}"),
            };
        }
    };
    let spec = AgentSpec {
        program,
        log: log_path(home),
        version: version.to_owned(),
    };
    match ensure(&plist_path(home), &spec, launchd) {
        Ok(outcome) => Report {
            ok: true,
            text: outcome.describe().to_owned(),
        },
        Err(e) => Report {
            ok: false,
            text: format!("tandemd was not installed: {e}"),
        },
    }
}

fn create_dir(dir: &Path) -> Result<(), InstallError> {
    std::fs::create_dir_all(dir).map_err(|source| InstallError::Write {
        path: dir.to_owned(),
        source,
    })
}

fn write_atomic(path: &Path, text: &str) -> Result<(), InstallError> {
    let err = |source| InstallError::Write {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent() {
        create_dir(dir)?;
    }
    // Write aside and rename: launchd never sees a half-written plist.
    let tmp = path.with_extension("plist.tmp");
    std::fs::write(&tmp, text).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)
}

/// The real `launchctl`, in the user's graphical session.
#[cfg(target_os = "macos")]
pub mod system {
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;

    use super::Launchd;

    const LAUNCHCTL: &str = "/bin/launchctl";

    pub struct SystemLaunchd {
        domain: String,
    }

    impl SystemLaunchd {
        /// The user's `gui/<uid>` domain, who runs the app.
        pub fn current_user() -> Result<Self, String> {
            let out = Command::new("/usr/bin/id")
                .arg("-u")
                .output()
                .map_err(|e| format!("could not run id -u: {e}"))?;
            let uid = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if !out.status.success() || uid.is_empty() || !uid.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("id -u returned \"{uid}\""));
            }
            Ok(SystemLaunchd {
                domain: format!("gui/{uid}"),
            })
        }

        fn run(&self, args: &[&str]) -> Result<(), String> {
            let out = Command::new(LAUNCHCTL)
                .args(args)
                .output()
                .map_err(|e| format!("could not run launchctl: {e}"))?;
            if out.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "launchctl {}: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            }
        }
    }

    impl Launchd for SystemLaunchd {
        fn is_loaded(&self, label: &str) -> bool {
            Command::new(LAUNCHCTL)
                .args(["print", format!("{}/{label}", self.domain).as_str()])
                .output()
                .is_ok_and(|o| o.status.success())
        }

        fn is_running(&self, label: &str) -> bool {
            Command::new(LAUNCHCTL)
                .args(["print", format!("{}/{label}", self.domain).as_str()])
                .output()
                .is_ok_and(|o| {
                    o.status.success()
                        && super::print_says_running(&String::from_utf8_lossy(&o.stdout))
                })
        }

        fn bootout(&self, label: &str) -> Result<(), String> {
            let target = format!("{}/{label}", self.domain);
            if self.run(&["bootout", target.as_str()]).is_err() && self.is_loaded(label) {
                return Err(format!("could not stop {target}"));
            }
            Ok(())
        }

        fn bootstrap(&self, plist: &Path) -> Result<(), String> {
            let plist = plist.to_string_lossy();
            // Right after a `bootout`, launchd may take a while to release the service
            // ("Bootstrap failed: 5"): retry a couple of times.
            let mut last = String::new();
            for attempt in 0..4 {
                if attempt > 0 {
                    std::thread::sleep(Duration::from_millis(500));
                }
                match self.run(&["bootstrap", self.domain.as_str(), &*plist]) {
                    Ok(()) => return Ok(()),
                    Err(e) => last = e,
                }
            }
            Err(last)
        }

        fn kickstart(&self, label: &str) -> Result<(), String> {
            let target = format!("{}/{label}", self.domain);
            self.run(&["kickstart", "-k", target.as_str()])
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn spec(dir: &Path) -> AgentSpec {
        let program = dir.join("Tandem.app/Contents/MacOS/tandemd");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, b"").unwrap();
        AgentSpec {
            program,
            log: dir.join("Logs/Tandem/tandemd.log"),
            version: "0.1.0".into(),
        }
    }

    #[derive(Default)]
    struct FakeLaunchd {
        loaded: RefCell<bool>,
        running: RefCell<bool>,
        calls: RefCell<Vec<String>>,
        fail_bootstrap: bool,
    }

    impl Launchd for FakeLaunchd {
        fn is_loaded(&self, _: &str) -> bool {
            *self.loaded.borrow()
        }
        fn is_running(&self, _: &str) -> bool {
            *self.running.borrow()
        }
        fn bootout(&self, label: &str) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("bootout {label}"));
            *self.loaded.borrow_mut() = false;
            *self.running.borrow_mut() = false;
            Ok(())
        }
        fn bootstrap(&self, plist: &Path) -> Result<(), String> {
            self.calls.borrow_mut().push(format!(
                "bootstrap {}",
                plist.file_name().unwrap().display()
            ));
            if self.fail_bootstrap {
                return Err("Bootstrap failed: 5".into());
            }
            *self.loaded.borrow_mut() = true;
            *self.running.borrow_mut() = true;
            Ok(())
        }
        fn kickstart(&self, label: &str) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("kickstart {label}"));
            *self.running.borrow_mut() = *self.loaded.borrow();
            Ok(())
        }
    }

    impl FakeLaunchd {
        fn calls(&self) -> Vec<String> {
            self.calls.borrow_mut().drain(..).collect()
        }
    }

    #[test]
    fn plist_content() {
        let s = AgentSpec {
            program: "/Applications/Tandem.app/Contents/MacOS/tandemd".into(),
            log: "/Users/user/Library/Logs/Tandem/tandemd.log".into(),
            version: "0.1.0".into(),
        };
        let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>com.franc1sc0sv.tandem.tandemd</string>
	<key>ProgramArguments</key>
	<array>
		<string>/Applications/Tandem.app/Contents/MacOS/tandemd</string>
	</array>
	<key>AssociatedBundleIdentifiers</key>
	<array>
		<string>com.franc1sc0sv.tandem</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>TANDEM_APP_VERSION</key>
		<string>0.1.0</string>
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ThrottleInterval</key>
	<integer>30</integer>
	<key>LimitLoadToSessionType</key>
	<string>Aqua</string>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>StandardOutPath</key>
	<string>/Users/user/Library/Logs/Tandem/tandemd.log</string>
	<key>StandardErrorPath</key>
	<string>/Users/user/Library/Logs/Tandem/tandemd.log</string>
</dict>
</plist>
"#;
        assert_eq!(plist(&s), expected);
    }

    /// T-344: the bundle identifier the plist names is the app's real one; if they drift apart,
    /// macOS no longer takes the app as responsible for `tandemd`.
    #[test]
    fn the_associated_bundle_is_the_app() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], APP_BUNDLE_ID);
    }

    /// The agents installed before T-344 lack `AssociatedBundleIdentifiers`: the new plist
    /// differs, so opening the new app reloads the job with it.
    #[test]
    fn plist_without_the_associated_bundle_is_replaced_and_reloaded() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let file = dir.path().join("a.plist");
        let old = plist(&s).replace(
            &format!(
                "\t<key>AssociatedBundleIdentifiers</key>\n\t<array>\n\t\t<string>{APP_BUNDLE_ID}</string>\n\t</array>\n"
            ),
            "",
        );
        assert_ne!(old, plist(&s));
        std::fs::write(&file, old).unwrap();
        let ld = FakeLaunchd::default();
        *ld.loaded.borrow_mut() = true;
        assert_eq!(ensure(&file, &s, &ld).unwrap(), Outcome::Updated);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), plist(&s));
        assert_eq!(
            ld.calls(),
            [
                "bootout com.franc1sc0sv.tandem.tandemd",
                "bootstrap a.plist"
            ]
        );
    }

    #[test]
    fn plist_escapes_paths() {
        let s = AgentSpec {
            program: "/Users/a&b/Apps/<T>.app/Contents/MacOS/tandemd".into(),
            log: "/l".into(),
            version: "1\"2".into(),
        };
        let p = plist(&s);
        assert!(
            p.contains("<string>/Users/a&amp;b/Apps/&lt;T&gt;.app/Contents/MacOS/tandemd</string>")
        );
        assert!(p.contains("<string>1&quot;2</string>"));
        assert!(!p.contains("a&b"));
    }

    #[test]
    fn standard_locations() {
        let home = Path::new("/Users/user");
        assert_eq!(
            plist_path(home),
            Path::new("/Users/user/Library/LaunchAgents/com.franc1sc0sv.tandem.tandemd.plist")
        );
        assert_eq!(
            log_path(home),
            Path::new("/Users/user/Library/Logs/Tandem/tandemd.log")
        );
    }

    #[test]
    fn sidecar_next_to_the_app_binary() {
        assert_eq!(
            sidecar_for(Path::new(
                "/Applications/Tandem.app/Contents/MacOS/tandem-ui"
            )),
            Ok("/Applications/Tandem.app/Contents/MacOS/tandemd".into())
        );
        assert_eq!(
            sidecar_for(Path::new(
                "/Users/f/Apps/Tandem.app/Contents/MacOS/tandem-ui"
            )),
            Ok("/Users/f/Apps/Tandem.app/Contents/MacOS/tandemd".into())
        );
    }

    #[test]
    fn no_agent_from_unstable_places() {
        assert_eq!(
            sidecar_for(Path::new("/repo/target/debug/tandem-ui")),
            Err(Placement::NotBundled)
        );
        assert_eq!(
            sidecar_for(Path::new("tandem-ui")),
            Err(Placement::NotBundled)
        );
        assert_eq!(
            sidecar_for(Path::new("/x/Tandem/Contents/MacOS/tandem-ui")),
            Err(Placement::NotBundled)
        );
        assert_eq!(
            sidecar_for(Path::new(
                "/private/var/folders/xy/T/AppTranslocation/1234/d/Tandem.app/Contents/MacOS/tandem-ui"
            )),
            Err(Placement::Translocated)
        );
        assert_eq!(
            sidecar_for(Path::new(
                "/Volumes/Tandem/Tandem.app/Contents/MacOS/tandem-ui"
            )),
            Err(Placement::DiskImage)
        );
    }

    #[test]
    fn first_install_writes_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let file = plist_path(&dir.path().join("home"));
        let ld = FakeLaunchd::default();
        assert_eq!(ensure(&file, &s, &ld).unwrap(), Outcome::Created);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), plist(&s));
        assert!(s.log.parent().unwrap().is_dir());
        assert!(!file.with_extension("plist.tmp").exists());
        assert_eq!(
            ld.calls(),
            ["bootstrap com.franc1sc0sv.tandem.tandemd.plist"]
        );
    }

    #[test]
    fn up_to_date_and_loaded_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let file = dir.path().join("a.plist");
        let ld = FakeLaunchd::default();
        ensure(&file, &s, &ld).unwrap();
        ld.calls();
        assert_eq!(ensure(&file, &s, &ld).unwrap(), Outcome::Unchanged);
        assert!(ld.calls().is_empty());
    }

    #[test]
    fn up_to_date_but_unloaded_is_loaded_again() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let file = dir.path().join("a.plist");
        let ld = FakeLaunchd::default();
        ensure(&file, &s, &ld).unwrap();
        ld.calls();
        *ld.loaded.borrow_mut() = false;
        assert_eq!(ensure(&file, &s, &ld).unwrap(), Outcome::Loaded);
        assert_eq!(ld.calls(), ["bootstrap a.plist"]);
    }

    /// Seen on a real Mac (v0.3.0): `tandemd` exited with 0 after a Quit AppleEvent, launchd
    /// kept the job loaded without a process, and opening the app did nothing.
    #[test]
    fn up_to_date_and_loaded_but_not_running_is_kickstarted() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let file = dir.path().join("a.plist");
        let ld = FakeLaunchd::default();
        ensure(&file, &s, &ld).unwrap();
        ld.calls();
        *ld.running.borrow_mut() = false;
        assert_eq!(ensure(&file, &s, &ld).unwrap(), Outcome::Restarted);
        assert_eq!(ld.calls(), ["kickstart com.franc1sc0sv.tandem.tandemd"]);
        assert!(*ld.running.borrow());
    }

    /// `launchctl print` of the agent running (trimmed from a real Mac, v0.3.0).
    const PRINT_RUNNING: &str = "gui/501/com.franc1sc0sv.tandem.tandemd = {
\tactive count = 3
\tpath = /Users/user/Library/LaunchAgents/com.franc1sc0sv.tandem.tandemd.plist
\ttype = LaunchAgent
\tstate = running

\tprogram = /Applications/Tandem.app/Contents/MacOS/tandemd
\targuments = {
\t\t/Applications/Tandem.app/Contents/MacOS/tandemd
\t}

\tdomain = gui/501 [100020]
\tminimum runtime = 30
\texit timeout = 5
\truns = 4
\tpid = 92836
\timmediate reason = non-ipc demand
\tlast exit code = 0

\tsemaphores = {
\t\tsuccessful exit => 0
\t}
}
";

    /// The same job after `tandemd` exited with 0: loaded, no process. The nested block
    /// checks that only the job's own lines count.
    const PRINT_EXITED: &str = "gui/501/com.franc1sc0sv.tandem.tandemd = {
\tactive count = 0
\tpath = /Users/user/Library/LaunchAgents/com.franc1sc0sv.tandem.tandemd.plist
\ttype = LaunchAgent
\tstate = not running

\tprogram = /Applications/Tandem.app/Contents/MacOS/tandemd
\targuments = {
\t\t/Applications/Tandem.app/Contents/MacOS/tandemd
\t}

\tdomain = gui/501 [100020]
\tminimum runtime = 30
\texit timeout = 5
\truns = 4
\tlast exit code = 0

\tsemaphores = {
\t\tsuccessful exit => 0
\t}

\tendpoints = {
\t\t\"com.example\" = {
\t\t\tstate = running
\t\t\tpid = 1
\t\t}
\t}
}
";

    #[test]
    fn print_of_a_running_job_says_running() {
        assert!(print_says_running(PRINT_RUNNING));
        let state_only = PRINT_RUNNING.replace("\tpid = 92836\n", "");
        assert!(print_says_running(&state_only));
    }

    #[test]
    fn print_of_an_exited_job_says_not_running() {
        assert!(!print_says_running(PRINT_EXITED));
        assert!(!print_says_running(""));
        assert!(!print_says_running("\tpid = \n\tstate = spawn scheduled\n"));
    }

    #[test]
    fn revive_starts_only_a_loaded_job_without_a_process() {
        let ld = FakeLaunchd::default();
        assert_eq!(revive(&ld), Ok(Revived::NotLoaded));
        assert!(ld.calls().is_empty());

        *ld.loaded.borrow_mut() = true;
        *ld.running.borrow_mut() = true;
        assert_eq!(revive(&ld), Ok(Revived::Running));
        assert!(ld.calls().is_empty());

        *ld.running.borrow_mut() = false;
        assert_eq!(revive(&ld), Ok(Revived::Started));
        assert_eq!(ld.calls(), ["kickstart com.franc1sc0sv.tandem.tandemd"]);
    }

    #[test]
    fn revive_gate_waits_for_misses_in_a_row() {
        let t0 = Instant::now();
        let mut gate = ReviveGate::default();
        for _ in 1..REVIVE_AFTER_MISSES {
            assert!(!gate.missed(t0));
        }
        gate.reached();
        for _ in 1..REVIVE_AFTER_MISSES {
            assert!(!gate.missed(t0));
        }
        assert!(gate.missed(t0));
    }

    #[test]
    fn revive_gate_never_asks_more_often_than_the_throttle() {
        let t0 = Instant::now();
        let throttle = Duration::from_secs(THROTTLE_SECS.into());
        let mut gate = ReviveGate::default();
        let mut asked = Vec::new();
        // A `tandemd` that dies on start: a miss every 2 s for 2 minutes.
        for i in 0..60u64 {
            let now = t0 + Duration::from_secs(2 * i);
            if gate.missed(now) {
                asked.push(now);
            }
        }
        assert!(asked.len() >= 2, "{asked:?}");
        assert!(
            asked.windows(2).all(|w| w[1] - w[0] >= throttle),
            "{asked:?}"
        );
        // Reaching it resets the count, not the throttle.
        gate.reached();
        let after = *asked.last().unwrap() + Duration::from_secs(1);
        for _ in 0..REVIVE_AFTER_MISSES {
            assert!(!gate.missed(after));
        }
    }

    #[test]
    fn new_version_replaces_and_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = spec(dir.path());
        let file = dir.path().join("a.plist");
        let ld = FakeLaunchd::default();
        ensure(&file, &s, &ld).unwrap();
        ld.calls();
        s.version = "0.2.0".into();
        assert_eq!(ensure(&file, &s, &ld).unwrap(), Outcome::Updated);
        assert!(std::fs::read_to_string(&file).unwrap().contains("0.2.0"));
        assert_eq!(
            ld.calls(),
            [
                "bootout com.franc1sc0sv.tandem.tandemd",
                "bootstrap a.plist"
            ]
        );
    }

    #[test]
    fn foreign_plist_not_loaded_is_replaced_without_bootout() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let file = dir.path().join("a.plist");
        std::fs::write(&file, "old").unwrap();
        let ld = FakeLaunchd::default();
        assert_eq!(ensure(&file, &s, &ld).unwrap(), Outcome::Updated);
        assert_eq!(ld.calls(), ["bootstrap a.plist"]);
    }

    #[test]
    fn missing_sidecar_touches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let s = AgentSpec {
            program: dir.path().join("missing"),
            log: dir.path().join("l/t.log"),
            version: "0.1.0".into(),
        };
        let file = dir.path().join("a.plist");
        let ld = FakeLaunchd::default();
        assert!(matches!(
            ensure(&file, &s, &ld),
            Err(InstallError::MissingSidecar(_))
        ));
        assert!(!file.exists());
        assert!(ld.calls().is_empty());
    }

    #[test]
    fn launchctl_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let file = dir.path().join("a.plist");
        let ld = FakeLaunchd {
            fail_bootstrap: true,
            ..FakeLaunchd::default()
        };
        let e = ensure(&file, &s, &ld).unwrap_err();
        assert!(e.to_string().contains("Bootstrap failed: 5"), "{e}");
        // The plist stays written: the app's next start only tries to load it.
        assert_eq!(std::fs::read_to_string(&file).unwrap(), plist(&s));
    }
    #[test]
    fn restart_kicks_the_loaded_agent() {
        let ld = FakeLaunchd::default();
        *ld.loaded.borrow_mut() = true;
        restart(&ld).unwrap();
        assert_eq!(ld.calls(), ["kickstart com.franc1sc0sv.tandem.tandemd"]);
    }

    #[test]
    fn restart_without_a_loaded_agent_says_so_and_starts_nothing() {
        let ld = FakeLaunchd::default();
        let e = restart(&ld).unwrap_err();
        assert!(e.contains("not loaded"), "{e}");
        assert!(ld.calls().is_empty());
    }

    #[test]
    fn install_from_a_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(dir.path());
        let exe = s.program.with_file_name("tandem-ui");
        let home = dir.path().join("home");
        let ld = FakeLaunchd::default();
        let r = install_from(&exe, &home, "0.1.0", &ld);
        assert!(r.ok, "{r:?}");
        assert_eq!(r.text, Outcome::Created.describe());
        let text = std::fs::read_to_string(plist_path(&home)).unwrap();
        // On Windows, `s.program` mixes separators: compare with the one the app computes.
        let program = sidecar_for(&exe).unwrap();
        assert!(program.is_file());
        assert!(text.contains(&format!(
            "<string>{}</string>",
            escape(&program.to_string_lossy())
        )));
        assert!(log_path(&home).parent().unwrap().is_dir());
        // The second time there is nothing to do.
        let r = install_from(&exe, &home, "0.1.0", &ld);
        assert_eq!(r.text, Outcome::Unchanged.describe());
    }

    #[test]
    fn install_from_a_dev_build_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let ld = FakeLaunchd::default();
        let r = install_from(
            Path::new("/repo/target/debug/tandem-ui"),
            dir.path(),
            "0.1.0",
            &ld,
        );
        assert!(!r.ok);
        assert!(r.text.contains("development"), "{}", r.text);
        assert!(!plist_path(dir.path()).exists());
        assert!(ld.calls().is_empty());
    }
}
