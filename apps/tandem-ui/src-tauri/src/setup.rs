//! The first launch: the setup guide on all systems and, on macOS, its permissions
//! (Accessibility, Input Monitoring, local network) and how the login agent ended up.
//!
//! The guide is shown until the user finishes it; that is marked in `setup.done`,
//! next to `tandem.toml`. It can be reopened from the window.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::launch_agent::Report;

const SETUP_FILE: &str = "setup.done";

/// A macOS permission that `tandemd` needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Inject keyboard and mouse, and move windows ("Bring windows").
    Accessibility,
    /// Capture keyboard and mouse to send them to the PC.
    InputMonitoring,
    /// Talk to the PC over the LAN (mDNS and QUIC).
    LocalNetwork,
}

impl Permission {
    pub const ALL: [Permission; 3] = [
        Permission::Accessibility,
        Permission::InputMonitoring,
        Permission::LocalNetwork,
    ];

    /// The System Settings panel that grants it.
    pub fn settings_url(self) -> &'static str {
        match self {
            Permission::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            Permission::InputMonitoring => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
            }
            Permission::LocalNetwork => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork"
            }
        }
    }
}

/// `setup.done` in the same directory as the agent's configuration.
pub fn setup_file_for(agent_config: &Path) -> PathBuf {
    agent_config
        .parent()
        .map_or_else(|| PathBuf::from(SETUP_FILE), |d| d.join(SETUP_FILE))
}

pub fn is_done(file: &Path) -> bool {
    file.is_file()
}

/// Marks the guide as finished. Saves the version: useful to know, if a future version
/// asks for a new permission, who saw the previous guide.
pub fn mark_done(file: &Path, version: &str) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, format!("{version}\n"))
}

/// What the window receives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetupView {
    /// There are permissions to grant on this system (macOS only).
    pub available: bool,
    /// Open the setup on launch: it was not finished yet (on any system).
    pub show: bool,
    pub permissions: Vec<Permission>,
    /// How the login agent ended up, if the app tried to install it.
    pub agent: Option<Report>,
}

impl SetupView {
    pub fn new(available: bool, done: bool, agent: Option<Report>) -> Self {
        SetupView {
            available,
            show: !done,
            permissions: if available {
                Permission::ALL.to_vec()
            } else {
                Vec::new()
            },
            agent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_open_the_privacy_panes() {
        let urls: Vec<&str> = Permission::ALL.iter().map(|p| p.settings_url()).collect();
        assert!(urls.iter().all(|u| {
            u.starts_with("x-apple.systempreferences:com.apple.preference.security?Privacy_")
        }));
        assert!(urls[0].ends_with("Privacy_Accessibility"));
        assert!(urls[1].ends_with("Privacy_ListenEvent"));
        assert!(urls[2].ends_with("Privacy_LocalNetwork"));
    }

    #[test]
    fn permission_names_match_the_frontend() {
        let names = serde_json::to_string(&Permission::ALL).unwrap();
        assert_eq!(
            names,
            r#"["accessibility","input_monitoring","local_network"]"#
        );
        let p: Permission = serde_json::from_str(r#""input_monitoring""#).unwrap();
        assert_eq!(p, Permission::InputMonitoring);
    }

    #[test]
    fn marker_lives_next_to_the_config() {
        assert_eq!(
            setup_file_for(Path::new("/c/Tandem/tandem.toml")),
            Path::new("/c/Tandem/setup.done")
        );
        assert_eq!(setup_file_for(Path::new("")), Path::new("setup.done"));
    }

    #[test]
    fn finishing_hides_the_guide() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("setup.done");
        assert!(!is_done(&file));
        assert!(SetupView::new(true, is_done(&file), None).show);
        mark_done(&file, "0.1.0").unwrap();
        assert!(is_done(&file));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "0.1.0\n");
        let v = SetupView::new(true, is_done(&file), None);
        assert!(v.available && !v.show);
        assert_eq!(v.permissions, Permission::ALL);
    }

    #[test]
    fn first_run_opens_on_every_system() {
        // Outside macOS there are no permissions to grant, but the setup opens anyway.
        let v = SetupView::new(false, false, None);
        assert!(!v.available && v.show);
        assert!(v.permissions.is_empty());
        assert!(!SetupView::new(false, true, None).show);
    }

    #[test]
    fn view_json_shape() {
        let v = SetupView::new(
            true,
            false,
            Some(Report {
                ok: true,
                text: "listo".into(),
            }),
        );
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            serde_json::json!({
                "available": true,
                "show": true,
                "permissions": ["accessibility", "input_monitoring", "local_network"],
                "agent": { "ok": true, "text": "listo" }
            })
        );
    }
}
