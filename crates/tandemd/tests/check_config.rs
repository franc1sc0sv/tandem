//! `tandemd --check-config`: code 0 if the configuration is valid, != 0 if not.

use std::path::Path;
use std::process::{Command, Output};

const VALID: &str = r#"
machine = "b"

[[screens]]
machine = "b"
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
    assert!(stdout.contains("valid configuration"), "{stdout}");
    assert!(stdout.contains("1 screens declared"), "{stdout}");
}

#[test]
fn invalid_toml_exits_nonzero() {
    let out = check_text("machine = ");
    assert!(!out.status.success());
    assert!(!out.stderr.is_empty());
}

#[test]
fn invalid_desk_exits_nonzero() {
    // A screen without area.
    let out = check_text(
        "machine = \"a\"\n[[screens]]\nmachine = \"a\"\nid = \"a\"\nx = 0\ny = 0\nwidth = 0\nheight = 10\n",
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("no tiene área"), "{stderr}");
}

/// A v0.1 configuration is valid: the `[[monitors]]` are ignored with a warning.
#[test]
fn an_old_config_with_monitors_is_valid_with_a_warning() {
    let out = check_text(
        "machine = \"b\"\n[[monitors]]\nid = \"m34\"\nlabel = '34\"'\nwiring = { pc = \"displayport\" }\n",
    );
    assert!(out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("[[monitors]]"), "{stderr}");
}

#[test]
fn missing_file_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let out = check(&dir.path().join("missing.toml"));
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

/// The example configs in docs/examples are valid, and the v0.2 ones (`mac`/`pc` sides)
/// too: the agent translates them on its own.
#[test]
fn example_configs_are_valid() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples");
    for (file, side) in [("tandem.mac.toml", "a"), ("tandem.pc.toml", "b")] {
        let out = check(&examples.join(file));
        assert!(out.status.success(), "{file}: {out:?}");
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(
            stdout.contains(&format!(
                "side {}, 3 screens declared)",
                side.to_uppercase()
            )),
            "{stdout}"
        );
        assert!(out.stderr.is_empty(), "{file}: no warnings");
    }
    for file in ["v0.2/tandem.mac.toml", "v0.2/tandem.pc.toml"] {
        let out = check(&examples.join(file));
        assert!(out.status.success(), "{file}: {out:?}");
        let stdout = String::from_utf8(out.stdout).unwrap();
        // Until the keys are known, this machine is A.
        assert!(stdout.contains("side A, 3 screens declared;"), "{stdout}");
        assert!(stdout.contains("v0.2"), "{stdout}");
        assert!(out.stderr.is_empty(), "{file}: no warnings");
    }
}
