//! `tandemw` arranca el `tandemd` de al lado con los mismos argumentos y devuelve su código.

use std::process::Command;

fn launch(args: &[&std::ffi::OsStr]) -> std::process::ExitStatus {
    Command::new(env!("CARGO_BIN_EXE_tandemw"))
        .args(args)
        .status()
        .unwrap()
}

#[test]
fn forwards_arguments_and_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let valid = dir.path().join("valida.toml");
    std::fs::write(&valid, "machine = \"pc\"\nmonitors = []\n").unwrap();
    let invalid = dir.path().join("invalida.toml");
    std::fs::write(&invalid, "machine = ").unwrap();

    assert!(launch(&["--check-config".as_ref(), valid.as_os_str()]).success());
    let status = launch(&["--check-config".as_ref(), invalid.as_os_str()]);
    assert_eq!(status.code(), Some(1));
    // Argumentos mal formados: tandemd sale con 2.
    assert_eq!(launch(&["--nada".as_ref()]).code(), Some(2));
}
