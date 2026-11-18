//! Ajustes → Avanzado: dónde están el archivo de configuración y los logs, y cómo mostrarlos
//! en el explorador de cada sistema. La orden se arma aquí (pura) para probarla en Linux.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Rutas que muestra Ajustes → Avanzado (texto, tal como las ve el usuario).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FilesInfo {
    pub config: String,
    pub logs: String,
}

/// Qué abrir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Config,
    Logs,
}

/// El archivo de configuración y la carpeta de logs de esta máquina.
pub fn locate() -> Result<(PathBuf, PathBuf), String> {
    let p = tandemd::paths::Paths::resolve(None, None).map_err(|e| e.to_string())?;
    let logs = p.log_dir();
    Ok((p.config, logs))
}

/// Orden que muestra `path` en el explorador: el archivo seleccionado o la carpeta abierta.
pub fn reveal_command(os: &str, path: &Path, is_file: bool) -> (String, Vec<String>) {
    let p = path.display().to_string();
    match os {
        "macos" if is_file => ("open".into(), vec!["-R".into(), p]),
        "macos" => ("open".into(), vec![p]),
        "windows" if is_file => ("explorer".into(), vec![format!("/select,{p}")]),
        "windows" => ("explorer".into(), vec![p]),
        _ => {
            let dir = if is_file {
                path.parent().unwrap_or(path).display().to_string()
            } else {
                p
            };
            ("xdg-open".into(), vec![dir])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_os_reveals_a_file_or_opens_a_folder() {
        let f = Path::new("/a/b/tandem.toml");
        assert_eq!(
            reveal_command("macos", f, true),
            ("open".into(), vec!["-R".into(), "/a/b/tandem.toml".into()])
        );
        assert_eq!(
            reveal_command("macos", Path::new("/a/logs"), false),
            ("open".into(), vec!["/a/logs".into()])
        );
        assert_eq!(
            reveal_command("windows", f, true).1,
            vec!["/select,/a/b/tandem.toml".to_string()]
        );
        assert_eq!(
            reveal_command("linux", f, true),
            ("xdg-open".into(), vec!["/a/b".into()])
        );
    }
}
