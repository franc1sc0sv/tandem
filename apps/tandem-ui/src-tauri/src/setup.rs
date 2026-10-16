//! El primer arranque en macOS: la guía de permisos (Accesibilidad, Input Monitoring, red
//! local) y cómo quedó el agente de inicio de sesión.
//!
//! La guía se muestra hasta que el usuario la termina; eso queda marcado en `setup.done`,
//! junto a `tandem.toml`. Se puede volver a abrir desde la ventana.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::launch_agent::Report;

const SETUP_FILE: &str = "setup.done";

/// Un permiso de macOS que `tandemd` necesita.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Inyectar teclado y mouse, y mover ventanas («Traer ventanas»).
    Accessibility,
    /// Capturar teclado y mouse para mandarlos a la PC.
    InputMonitoring,
    /// Hablar con la PC por la LAN (mDNS y QUIC).
    LocalNetwork,
}

impl Permission {
    pub const ALL: [Permission; 3] = [
        Permission::Accessibility,
        Permission::InputMonitoring,
        Permission::LocalNetwork,
    ];

    /// El panel de Ajustes del Sistema que lo concede.
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

/// `setup.done` en el mismo directorio que la configuración del agente.
pub fn setup_file_for(agent_config: &Path) -> PathBuf {
    agent_config
        .parent()
        .map_or_else(|| PathBuf::from(SETUP_FILE), |d| d.join(SETUP_FILE))
}

pub fn is_done(file: &Path) -> bool {
    file.is_file()
}

/// Marca la guía como terminada. Guarda la versión: sirve para saber, si una versión futura
/// pide un permiso nuevo, quién vio la guía anterior.
pub fn mark_done(file: &Path, version: &str) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, format!("{version}\n"))
}

/// Lo que recibe la ventana.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetupView {
    /// Hay guía en este sistema (solo macOS).
    pub available: bool,
    /// Mostrarla al abrir: hay guía y todavía no se terminó.
    pub show: bool,
    pub permissions: Vec<Permission>,
    /// Cómo quedó el agente de inicio de sesión, si la app intentó instalarlo.
    pub agent: Option<Report>,
}

impl SetupView {
    pub fn new(available: bool, done: bool, agent: Option<Report>) -> Self {
        SetupView {
            available,
            show: available && !done,
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
    fn no_guide_outside_macos() {
        let v = SetupView::new(false, false, None);
        assert!(!v.available && !v.show);
        assert!(v.permissions.is_empty());
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
