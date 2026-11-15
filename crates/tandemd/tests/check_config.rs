//! `tandemd --check-config`: código 0 si la configuración es válida, ≠ 0 si no.

use std::path::Path;
use std::process::{Command, Output};

const VALID: &str = r#"
machine = "pc"

[[screens]]
machine = "pc"
id = "34"
x = 0
y = 0
width = 3440
height = 1440
"#;

fn check(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tandemd"))
        .arg("--check-config")
        .arg(path)
        .output()
        .unwrap()
}

fn check_text(text: &str) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tandem.toml");
    std::fs::write(&path, text).unwrap();
    check(&path)
}

#[test]
fn valid_config_exits_zero() {
    let out = check_text(VALID);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("configuración válida"), "{stdout}");
    assert!(stdout.contains("1 pantallas declaradas"), "{stdout}");
}

#[test]
fn invalid_toml_exits_nonzero() {
    let out = check_text("machine = ");
    assert!(!out.status.success());
    assert!(!out.stderr.is_empty());
}

#[test]
fn invalid_desk_exits_nonzero() {
    // Una pantalla sin área.
    let out = check_text(
        "machine = \"mac\"\n[[screens]]\nmachine = \"mac\"\nid = \"a\"\nx = 0\ny = 0\nwidth = 0\nheight = 10\n",
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("no tiene área"), "{stderr}");
}

/// Una configuración de v0.1 es válida: los `[[monitors]]` se ignoran con un aviso.
#[test]
fn an_old_config_with_monitors_is_valid_with_a_warning() {
    let out = check_text(
        "machine = \"pc\"\n[[monitors]]\nid = \"m34\"\nlabel = '34\"'\nwiring = { pc = \"displayport\" }\n",
    );
    assert!(out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("[[monitors]]"), "{stderr}");
}

#[test]
fn missing_file_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let out = check(&dir.path().join("no-existe.toml"));
    assert!(!out.status.success());
}

#[test]
fn bad_arguments_exit_nonzero() {
    let out = Command::new(env!("CARGO_BIN_EXE_tandemd"))
        .arg("--check-config")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// Las configs de ejemplo de docs/examples son válidas.
#[test]
fn example_configs_are_valid() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples");
    for (file, machine) in [("tandem.mac.toml", "mac"), ("tandem.pc.toml", "pc")] {
        let out = check(&examples.join(file));
        assert!(out.status.success(), "{file}: {out:?}");
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(
            stdout.contains(&format!("máquina {machine}, 3 pantallas declaradas")),
            "{stdout}"
        );
        assert!(out.stderr.is_empty(), "{file}: sin avisos");
    }
}
