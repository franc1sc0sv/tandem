//! `tandemd --check-config`: código 0 si la configuración es válida, ≠ 0 si no.

use std::path::Path;
use std::process::{Command, Output};

const VALID: &str = r#"
machine = "pc"

[[monitors]]
id = "m34"
label = '34"'
wiring = { pc = "displayport", mac = "hdmi" }
ddc = { hdmi = 0x06, display_port = 0x0F }
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
}

#[test]
fn invalid_toml_exits_nonzero() {
    let out = check_text("machine = ");
    assert!(!out.status.success());
    assert!(!out.stderr.is_empty());
}

#[test]
fn invalid_desk_exits_nonzero() {
    // Monitor sin cablear a ninguna máquina.
    let out =
        check_text("machine = \"mac\"\n[[monitors]]\nid = \"a\"\nlabel = \"a\"\nwiring = {}\n");
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("no está cableado"), "{stderr}");
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
