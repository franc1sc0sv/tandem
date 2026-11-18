//! The administrator service that the setup installs (T-131, `F-Setup1`).
//!
//! - macOS: a LaunchDaemon (`/Library/LaunchDaemons`) with the `.app`'s `tandemd`. Writing
//!   there asks for credentials: they are requested with the system dialog (`osascript ... with
//!   administrator privileges`). The daemon uses its own state directory, so as not to
//!   fight with the session agent.
//! - Windows: the elevated task of T-104 (`tandemd --install` with UAC). Before asking for UAC the
//!   same ACL checks as T-104 are made: elevating an executable that a
//!   normal user writes would be handing out a UAC jump. The elevated `--install` repeats them.
//!
//! Everything it decides is portable. What elevates privileges goes behind [`Elevator`] and the
//! state behind [`Probe`]; the tests use doubles and never elevate.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tandemd::install::PathAcl;

/// Label of the LaunchDaemon (different from the session LaunchAgent's).
pub const DAEMON_LABEL: &str = "com.franc1sc0sv.tandem.admin";
pub const DAEMON_PLIST: &str = "/Library/LaunchDaemons/com.franc1sc0sv.tandem.admin.plist";
/// Root folder where the copy that runs as root lives: the `.app`'s `tandemd` can be
/// replaced by whoever writes to the app, and that would be root for that user.
pub const DAEMON_BIN_DIR: &str = "/Library/PrivilegedHelperTools";
pub const DAEMON_BIN: &str = "/Library/PrivilegedHelperTools/com.franc1sc0sv.tandem.admin";
pub const DAEMON_DATA: &str = "/Library/Application Support/Tandem";

/// What the setup row reports. `pending`: it has to be installed (or was left half-done).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminState {
    Off,
    Pending,
    Ok,
}

/// What the UI draws: state, whether there is an "Install" button and, if not, why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminView {
    pub state: AdminState,
    pub can_install: bool,
    pub note: String,
}

/// Where the app runs and with which `tandemd` it would be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Platform {
    Mac {
        daemon: PathBuf,
    },
    Windows {
        /// `tandemd.exe`: what is launched elevated with `--install`.
        daemon: PathBuf,
        /// What the task registers to start: `tandemw.exe` if it exists, otherwise `tandemd.exe`.
        /// It is the same one the elevated `--install` chooses (`launcher_for`).
        launcher: PathBuf,
    },
    /// Another system, or a build without `.app` (development): there is nothing to install.
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdminError {
    #[error("{0}")]
    NotApplicable(String),
    #[error("the administrator request was canceled")]
    Cancelled,
    #[error("could not elevate: {0}")]
    Failed(String),
}

/// A command with privileges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Elevation {
    /// A shell script as administrator (macOS: system dialog).
    MacShell { script: String, prompt: String },
    /// `<program> <arg>` with UAC (Windows).
    WinUac { program: PathBuf, arg: String },
}

pub trait Elevator {
    fn run(&self, what: &Elevation) -> Result<(), AdminError>;
}

pub trait Probe {
    /// macOS: whether the plist exists and whether launchd has it loaded. Windows: whether the task exists.
    fn state(&self, platform: &Platform) -> AdminState;
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The LaunchDaemon plist: starts with the system and brings it back up if it fails with an error.
pub fn daemon_plist(program: &Path) -> String {
    let program = escape_xml(&program.to_string_lossy());
    let data = escape_xml(DAEMON_DATA);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{DAEMON_LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{program}</string>
		<string>--data-dir</string>
		<string>{data}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ThrottleInterval</key>
	<integer>30</integer>
</dict>
</plist>
"#
    )
}

/// The script that runs as administrator: copies `source` (the `.app`'s `tandemd`) to
/// [`DAEMON_BIN`] with root as owner and the plist points to that copy. The plist already comes escaped as
/// XML (without `'`), so it fits between shell single quotes with no more. The path of `source` goes
/// shell-escaped: a `'` would close it.
pub fn mac_install_script(source: &Path) -> String {
    let plist = daemon_plist(Path::new(DAEMON_BIN));
    let source = source.to_string_lossy().replace('\'', r"'\''");
    // `-L` rejects a link in the last piece right before copying; those of earlier stretches
    // are discarded by `mac_platform` when resolving the path.
    format!(
        "[ ! -L '{source}' ] && /usr/bin/install -d -o root -g wheel -m 755 '{DAEMON_BIN_DIR}' \
         && /usr/bin/install -o root -g wheel -m 755 '{source}' '{DAEMON_BIN}' \
         && /bin/mkdir -p '{DAEMON_DATA}' && /usr/bin/printf '%s' '{plist}' > '{DAEMON_PLIST}' \
         && /usr/sbin/chown root:wheel '{DAEMON_PLIST}' && /bin/chmod 644 '{DAEMON_PLIST}' \
         && {{ /bin/launchctl bootout system/{DAEMON_LABEL} 2>/dev/null; \
         /bin/launchctl bootstrap system '{DAEMON_PLIST}'; }}"
    )
}

pub fn mac_uninstall_script() -> String {
    format!(
        "{{ /bin/launchctl bootout system/{DAEMON_LABEL} 2>/dev/null; \
         /bin/rm -f '{DAEMON_PLIST}' '{DAEMON_BIN}'; }}"
    )
}

/// The `osascript` arguments that show the system's administrator dialog.
pub fn osascript_args(script: &str, prompt: &str) -> Vec<String> {
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    vec![
        "-e".to_owned(),
        format!(
            "do shell script {} with prompt {} with administrator privileges",
            q(script),
            q(prompt)
        ),
    ]
}

/// The PowerShell arguments that launch `<program> <arg>` with UAC and wait for it to finish.
pub fn powershell_args(program: &Path, arg: &str) -> Vec<String> {
    let lit = |s: &str| format!("'{}'", s.replace('\'', "''"));
    vec![
        "-NoProfile".to_owned(),
        "-NonInteractive".to_owned(),
        "-Command".to_owned(),
        format!(
            "Start-Process -FilePath {} -ArgumentList {} -Verb RunAs -Wait -WindowStyle Hidden",
            lit(&program.to_string_lossy()),
            lit(arg)
        ),
    ]
}

/// The macOS state from what is seen: plist in place and loaded in launchd.
pub fn mac_state(plist_exists: bool, loaded: bool) -> AdminState {
    match (plist_exists, loaded) {
        (_, true) => AdminState::Ok,
        (true, false) => AdminState::Pending,
        (false, false) => AdminState::Off,
    }
}

/// What the row shows right now, with the possibility of installing.
pub fn view(platform: &Platform, probe: &dyn Probe, acl: &dyn PathAcl) -> AdminView {
    let state = probe.state(platform);
    let blocked = check(platform, acl).err().map(|e| e.to_string());
    match (state, blocked) {
        (AdminState::Ok, _) => AdminView {
            state,
            can_install: false,
            note: String::new(),
        },
        (_, Some(note)) => AdminView {
            state,
            can_install: false,
            note,
        },
        (_, None) => AdminView {
            state,
            can_install: true,
            note: String::new(),
        },
    }
}

/// The prior checks: if they do not pass, no elevation is asked for.
pub fn check(platform: &Platform, acl: &dyn PathAcl) -> Result<(), AdminError> {
    match platform {
        Platform::Unsupported(why) => Err(AdminError::NotApplicable(why.clone())),
        // Root copies `source` and runs it: neither it nor its ancestors can be written by a
        // user (otherwise, whoever writes to the `.app` gets root). Same as on Windows. Neither
        // the copy's destination. The release is unsigned (D19): there is no `codesign` that applies.
        Platform::Mac { daemon } => {
            if let Some(why) = tandemd::install::not_admin_only(acl, daemon) {
                return Err(AdminError::NotApplicable(format!(
                    "user-level installation does not support the administrator service: {why}. \
                     Move Tandem to Applications (owned by root) to install it"
                )));
            }
            match tandemd::install::not_admin_only(acl, Path::new(DAEMON_BIN)) {
                None => Ok(()),
                Some(why) => Err(AdminError::NotApplicable(format!(
                    "the service is not installed: {why}"
                ))),
            }
        }
        // What is elevated (`tandemd.exe --install`) and what the task registers are checked.
        Platform::Windows { daemon, launcher } => [daemon, launcher]
            .into_iter()
            .find_map(|p| tandemd::install::not_admin_only(acl, p))
            .map_or(Ok(()), |why| {
                Err(AdminError::NotApplicable(format!(
                    "user-level installation: {why}. Install Tandem in Program Files for the service"
                )))
            }),
    }
}

pub fn install(
    platform: &Platform,
    elevator: &dyn Elevator,
    probe: &dyn Probe,
    acl: &dyn PathAcl,
) -> Result<(), AdminError> {
    check(platform, acl)?;
    let what = match platform {
        Platform::Mac { daemon } => Elevation::MacShell {
            script: mac_install_script(daemon),
            prompt: "Tandem is installing the administrator service.".to_owned(),
        },
        Platform::Windows { daemon, .. } => Elevation::WinUac {
            program: daemon.clone(),
            arg: "--install".to_owned(),
        },
        Platform::Unsupported(why) => return Err(AdminError::NotApplicable(why.clone())),
    };
    elevator.run(&what)?;
    // The elevated `--install` falls back to the `Run` key if its checks fail: that is not success.
    match probe.state(platform) {
        AdminState::Ok => Ok(()),
        _ => Err(AdminError::Failed("the service did not become active".to_owned())),
    }
}

pub fn uninstall(platform: &Platform, elevator: &dyn Elevator) -> Result<(), AdminError> {
    let what = match platform {
        Platform::Mac { .. } => Elevation::MacShell {
            script: mac_uninstall_script(),
            prompt: "Tandem is removing the administrator service.".to_owned(),
        },
        Platform::Windows { daemon, .. } => Elevation::WinUac {
            program: daemon.clone(),
            arg: "--uninstall".to_owned(),
        },
        Platform::Unsupported(why) => return Err(AdminError::NotApplicable(why.clone())),
    };
    elevator.run(&what)
}

/// The platform according to the app's executable.
pub fn platform_for(exe: &Path) -> Platform {
    if cfg!(target_os = "macos") {
        mac_platform(exe, |p| std::fs::canonicalize(p))
    } else if cfg!(windows) {
        let daemon = exe.with_file_name("tandemd.exe");
        let launcher = tandemd::install::launcher_for(&daemon, |p| p.is_file());
        Platform::Windows { daemon, launcher }
    } else {
        Platform::Unsupported(
            "the administrator service only exists on macOS and Windows".to_owned(),
        )
    }
}

/// The `.app`'s `tandemd` as root will see it: the path already resolved (`resolve` = `realpath`) and
/// without links. If resolving it changes anything there is a link on the way: it is discarded.
pub fn mac_platform(exe: &Path, resolve: impl Fn(&Path) -> std::io::Result<PathBuf>) -> Platform {
    let daemon = match crate::launch_agent::sidecar_for(exe) {
        Ok(d) => d,
        Err(why) => return Platform::Unsupported(why.to_string()),
    };
    match resolve(&daemon) {
        Ok(real) if real == daemon => Platform::Mac { daemon: real },
        Ok(_) => Platform::Unsupported(
            "the tandemd path goes through a symbolic link: the administrator service does not support it"
                .to_owned(),
        ),
        Err(e) => Platform::Unsupported(format!("could not resolve {}: {e}", daemon.display())),
    }
}

/// Unix permissions: administrator-only is what has root as owner and nobody else writes, and is not
/// a link. What does not exist yet (the script creates the folder as root) does not count.
#[cfg(unix)]
pub struct UnixAcl;

/// The macOS `admin` group.
#[cfg(unix)]
const ADMIN_GID: u32 = 80;

#[cfg(unix)]
impl PathAcl for UnixAcl {
    fn admin_only(&self, path: &Path, _: tandemd::install::acl::Rigor) -> std::io::Result<bool> {
        use std::os::unix::fs::MetadataExt;
        match std::fs::symlink_metadata(path) {
            // Root owner, no write by "others" and by the group only if it is `admin` (80, like
            // `/Applications`: `root:admin 775`).
            Ok(m) => {
                let group_ok = m.mode() & 0o020 == 0 || m.gid() == ADMIN_GID;
                Ok(
                    !m.file_type().is_symlink()
                        && m.uid() == 0
                        && m.mode() & 0o002 == 0
                        && group_ok,
                )
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e),
        }
    }
}

/// The real system: `osascript` / PowerShell and `launchctl` / the Task Scheduler.
pub struct System;

impl Elevator for System {
    fn run(&self, what: &Elevation) -> Result<(), AdminError> {
        let (bin, args) = match what {
            Elevation::MacShell { script, prompt } => ("osascript", osascript_args(script, prompt)),
            Elevation::WinUac { program, arg } => ("powershell.exe", powershell_args(program, arg)),
        };
        let out = std::process::Command::new(bin)
            .args(args)
            .output()
            .map_err(|e| AdminError::Failed(format!("{bin}: {e}")))?;
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr);
        // osascript: -128 is "Cancel"; PowerShell: UAC rejected.
        if err.contains("-128") || err.contains("canceled by the user") {
            return Err(AdminError::Cancelled);
        }
        Err(AdminError::Failed(err.trim().to_owned()))
    }
}

impl Probe for System {
    fn state(&self, platform: &Platform) -> AdminState {
        match platform {
            Platform::Mac { .. } => {
                let exists = Path::new(DAEMON_PLIST).exists();
                let loaded = std::process::Command::new("/bin/launchctl")
                    .args(["print", &format!("system/{DAEMON_LABEL}")])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                mac_state(exists, loaded)
            }
            #[cfg(windows)]
            Platform::Windows { .. } => {
                use tandemd::install::{HighestTask, Scheduler};
                if HighestTask.exists().unwrap_or(false) {
                    AdminState::Ok
                } else {
                    AdminState::Off
                }
            }
            _ => AdminState::Off,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io;

    use proptest::prelude::*;
    use tandemd::install::acl::Rigor;

    use super::*;

    struct Acl(bool);
    impl PathAcl for Acl {
        fn admin_only(&self, _: &Path, _: Rigor) -> io::Result<bool> {
            Ok(self.0)
        }
    }

    /// Records what it is asked to elevate; never elevates. Before elevating the service is `off`.
    struct Fake {
        asked: RefCell<Vec<Elevation>>,
        result: Result<(), AdminError>,
        after: AdminState,
    }
    impl Fake {
        fn new(result: Result<(), AdminError>, after: AdminState) -> Self {
            Self {
                asked: RefCell::new(vec![]),
                result,
                after,
            }
        }
        fn elevate(&self) {
            self.asked.borrow_mut().push(Elevation::MacShell {
                script: String::new(),
                prompt: String::new(),
            });
        }
    }
    impl Elevator for Fake {
        fn run(&self, what: &Elevation) -> Result<(), AdminError> {
            self.asked.borrow_mut().push(what.clone());
            self.result.clone()
        }
    }
    impl Probe for Fake {
        fn state(&self, _: &Platform) -> AdminState {
            if self.asked.borrow().is_empty() {
                AdminState::Off
            } else {
                self.after
            }
        }
    }

    fn mac() -> Platform {
        Platform::Mac {
            daemon: "/Applications/Tandem.app/Contents/MacOS/tandemd".into(),
        }
    }
    fn win() -> Platform {
        Platform::Windows {
            daemon: r"C:\Program Files\Tandem\tandemd.exe".into(),
            launcher: r"C:\Program Files\Tandem\tandemw.exe".into(),
        }
    }

    /// It only says "writable by the user" for the paths it is told.
    struct AclExcept(&'static str);
    impl PathAcl for AclExcept {
        fn admin_only(&self, p: &Path, _: Rigor) -> io::Result<bool> {
            Ok(!p.to_string_lossy().contains(self.0))
        }
    }

    #[test]
    fn mac_copies_the_binary_as_root_and_the_plist_points_at_the_copy() {
        let src = "/Users/ana/Applications/Tandem.app/Contents/MacOS/tandemd";
        let script = mac_install_script(Path::new(src));
        // The binary that runs as root is a root copy, not the `.app`'s.
        assert!(script.contains(&format!(
            "/usr/bin/install -o root -g wheel -m 755 '{src}' '{DAEMON_BIN}'"
        )));
        assert!(script.contains(
            "/usr/bin/install -d -o root -g wheel -m 755 '/Library/PrivilegedHelperTools'"
        ));
        let plist = daemon_plist(Path::new(DAEMON_BIN));
        assert!(plist.contains(&format!("<string>{DAEMON_BIN}</string>")));
        assert!(!plist.contains("Tandem.app"));
        assert!(!script.contains(&format!("<string>{src}</string>")));
        // The copy goes before loading the daemon.
        assert!(
            script.find("install -o root").unwrap() < script.find("launchctl bootstrap").unwrap()
        );
    }

    /// Like a Mac: `/Applications` is root's; `/Users/<u>/...` is written by the user.
    #[test]
    fn mac_source_in_a_user_writable_app_is_rejected_and_one_owned_by_root_is_accepted() {
        let acl = AclExcept("/Users/");
        let user_app = Platform::Mac {
            daemon: "/Users/ana/Applications/Tandem.app/Contents/MacOS/tandemd".into(),
        };
        let f = Fake::new(Ok(()), AdminState::Ok);
        let err = install(&user_app, &f, &f, &acl).unwrap_err();
        let AdminError::NotApplicable(why) = err else {
            panic!("NotApplicable was expected")
        };
        assert!(why.contains("user-level installation does not support the administrator service"));
        // The password was not asked for and the row does not offer "Install".
        assert!(f.asked.borrow().is_empty());
        let v = view(&user_app, &f, &acl);
        assert!(!v.can_install);
        assert!(v.note.contains("user-level installation"));
        // `/Applications/Tandem.app` with root as owner does.
        install(&mac(), &f, &f, &acl).unwrap();
        assert_eq!(f.asked.borrow().len(), 1);
    }

    #[test]
    fn mac_script_refuses_symlinks_before_copying() {
        let script =
            mac_install_script(Path::new("/Applications/Tandem.app/Contents/MacOS/tandemd"));
        let guard = script
            .find("! -L '/Applications/Tandem.app/Contents/MacOS/tandemd'")
            .unwrap();
        assert!(guard < script.find("install -o root").unwrap());
    }

    #[test]
    fn mac_platform_resolves_the_source_and_rejects_symlinks() {
        let exe = Path::new("/Applications/Tandem.app/Contents/MacOS/tandem-ui");
        let same = |p: &Path| Ok(p.to_path_buf());
        assert!(matches!(mac_platform(exe, same), Platform::Mac { .. }));
        // The real path is another (a link somewhere along the way): not valid.
        let linked = |_: &Path| Ok(PathBuf::from("/Users/ana/evil/tandemd"));
        let Platform::Unsupported(why) = mac_platform(exe, linked) else {
            panic!("it should have been rejected")
        };
        assert!(why.contains("enlace"));
        let missing = |_: &Path| Err(io::Error::from(io::ErrorKind::NotFound));
        assert!(matches!(
            mac_platform(exe, missing),
            Platform::Unsupported(_)
        ));
    }

    #[test]
    fn mac_with_a_user_writable_destination_never_elevates() {
        let f = Fake::new(Ok(()), AdminState::Ok);
        let acl = AclExcept("PrivilegedHelperTools");
        let err = install(&mac(), &f, &f, &acl).unwrap_err();
        assert!(matches!(err, AdminError::NotApplicable(_)));
        assert!(f.asked.borrow().is_empty());
        assert!(!view(&mac(), &f, &acl).can_install);
        // A writable ancestor (for example /Library changed) also discards it.
        assert!(check(&mac(), &AclExcept("/Library")).is_err());
    }

    #[test]
    fn windows_validates_the_executables_it_elevates_and_registers() {
        // `tandemd.exe --install` is elevated and the task points to the launcher: both are checked.
        assert!(check(&win(), &AclExcept("tandemw.exe")).is_err());
        assert!(check(&win(), &AclExcept("tandemd.exe")).is_err());
        assert!(check(&win(), &Acl(true)).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn unix_acl_accepts_only_root_owned_paths_without_user_write() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        // If the tests run as root, their folder is root's.
        if std::fs::metadata(dir.path()).unwrap().uid() == 0 {
            return;
        }
        // A test folder belongs to the user (not root): it is not administrator-only.
        assert!(!UnixAcl.admin_only(dir.path(), Rigor::Full).unwrap());
        // What does not exist yet is created by root with root as owner: it does not count against the path.
        assert!(
            UnixAcl
                .admin_only(&dir.path().join("new"), Rigor::Full)
                .unwrap()
        );
    }

    #[test]
    fn mac_install_asks_admin_with_the_daemon_script() {
        let f = Fake::new(Ok(()), AdminState::Ok);
        install(&mac(), &f, &f, &Acl(true)).unwrap();
        let asked = f.asked.borrow();
        let Elevation::MacShell { script, .. } = &asked[0] else {
            panic!("not a shell")
        };
        assert!(script.contains("/Applications/Tandem.app/Contents/MacOS/tandemd"));
        assert!(script.contains("launchctl bootstrap system"));
        assert!(script.contains("--data-dir"));
    }

    #[test]
    fn windows_install_runs_dash_install_with_uac_after_the_acl_check() {
        let f = Fake::new(Ok(()), AdminState::Ok);
        install(&win(), &f, &f, &Acl(true)).unwrap();
        assert_eq!(
            f.asked.borrow()[0],
            Elevation::WinUac {
                program: r"C:\Program Files\Tandem\tandemd.exe".into(),
                arg: "--install".into()
            }
        );
    }

    #[test]
    fn windows_with_a_writable_launcher_never_elevates() {
        let f = Fake::new(Ok(()), AdminState::Ok);
        let err = install(&win(), &f, &f, &Acl(false)).unwrap_err();
        assert!(matches!(err, AdminError::NotApplicable(_)));
        assert!(f.asked.borrow().is_empty());
        let v = view(&win(), &f, &Acl(false));
        assert!(!v.can_install);
        assert!(v.note.contains("user-level installation"));
    }

    #[test]
    fn unsupported_platform_explains_and_offers_no_button() {
        let p = Platform::Unsupported("the app does not run from a Tandem.app".into());
        let f = Fake::new(Ok(()), AdminState::Ok);
        assert!(install(&p, &f, &f, &Acl(true)).is_err());
        let v = view(&p, &f, &Acl(true));
        assert!(!v.can_install);
        assert_eq!(v.note, "the app does not run from a Tandem.app");
    }

    #[test]
    fn cancelled_elevation_propagates() {
        let f = Fake::new(Err(AdminError::Cancelled), AdminState::Ok);
        assert_eq!(
            install(&mac(), &f, &f, &Acl(true)),
            Err(AdminError::Cancelled)
        );
    }

    #[test]
    fn a_fallback_to_the_run_key_is_not_success() {
        // The elevated `--install` went fine but left no task: the row cannot end up "ok".
        let f = Fake::new(Ok(()), AdminState::Off);
        assert!(matches!(
            install(&win(), &f, &f, &Acl(true)),
            Err(AdminError::Failed(_))
        ));
    }

    #[test]
    fn view_reports_the_three_states() {
        let f = Fake::new(Ok(()), AdminState::Pending);
        let v = view(&mac(), &f, &Acl(true));
        assert_eq!((v.state, v.can_install), (AdminState::Off, true));
        f.elevate();
        assert_eq!(view(&mac(), &f, &Acl(true)).state, AdminState::Pending);
        let ok = Fake::new(Ok(()), AdminState::Ok);
        ok.elevate();
        let v = view(&mac(), &ok, &Acl(true));
        assert_eq!((v.state, v.can_install), (AdminState::Ok, false));
    }

    #[test]
    fn mac_state_table() {
        assert_eq!(mac_state(false, false), AdminState::Off);
        assert_eq!(mac_state(true, false), AdminState::Pending);
        assert_eq!(mac_state(true, true), AdminState::Ok);
    }

    #[test]
    fn uninstall_asks_for_the_matching_command() {
        let f = Fake::new(Ok(()), AdminState::Off);
        uninstall(&mac(), &f).unwrap();
        uninstall(&win(), &f).unwrap();
        let asked = f.asked.borrow();
        assert!(
            matches!(&asked[0], Elevation::MacShell { script, .. } if script.contains("rm -f"))
        );
        assert!(matches!(&asked[1], Elevation::WinUac { arg, .. } if arg == "--uninstall"));
    }

    #[test]
    fn osascript_shows_the_system_dialog() {
        let a = osascript_args("echo \"x\"", "Pide");
        assert_eq!(a[0], "-e");
        assert!(a[1].ends_with("with administrator privileges"));
        assert!(a[1].contains(r#"\"x\""#));
    }

    #[test]
    fn powershell_runs_as_with_the_path_quoted() {
        let a = powershell_args(Path::new(r"C:\Users\O'Neil\tandemd.exe"), "--install");
        assert!(a[3].contains("-Verb RunAs"));
        assert!(a[3].contains(r"'C:\Users\O''Neil\tandemd.exe'"));
    }

    proptest! {
        /// Whatever the path, the plist leaves no single quotes (it fits in the shell script).
        #[test]
        fn plist_is_safe_for_the_shell_script(path in "/[ -~]{0,40}") {
            let plist = daemon_plist(Path::new(&path));
            prop_assert!(!plist.contains('\''));
            prop_assert!(mac_install_script(Path::new(&path)).contains(DAEMON_PLIST));
        }
    }
}
