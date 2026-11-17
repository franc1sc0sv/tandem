//! El escaneo y el muestreo de verdad, en la máquina que corre las pruebas (en CI, el runner
//! de cada sistema): tienen que terminar sin error y encontrar al menos el shell del runner.

use tandem_env::{Host, Platform};
use tandem_proto::EnvKind;

#[test]
fn the_real_scan_finds_the_runner_shell() {
    let host = Host::system();
    let scan = host.scan();
    let shells: Vec<&str> = scan
        .environments
        .iter()
        .filter(|e| e.kind == EnvKind::Shell)
        .map(|e| e.id.as_str())
        .collect();
    eprintln!("{scan:#?}");
    let expected: &[&str] = match Platform::current() {
        // Los runners de Windows traen PowerShell 5 y cmd; los de macOS, zsh y bash.
        Platform::Windows => &["powershell", "cmd"],
        Platform::MacOs => &["zsh", "bash"],
        Platform::Linux => &["bash"],
    };
    assert!(
        expected.iter().any(|s| shells.contains(s)),
        "ningún shell esperado ({expected:?}) en {shells:?}"
    );
    assert_eq!(
        scan.environments
            .iter()
            .filter(|e| e.kind == EnvKind::Shell && e.default)
            .count(),
        1,
        "un solo shell predeterminado"
    );
    assert!(scan.hardware.cores > 0);
    assert!(scan.hardware.memory > 0);
}

#[test]
fn the_real_sample_reads_cpu_memory_and_disk() {
    let host = Host::system();
    let first = host.sample();
    let s = host.sample();
    eprintln!("{s:#?}");
    assert!(!s.cpu.cores.is_empty());
    assert!(s.cpu.load <= 1000 && first.cpu.load <= 1000);
    assert!(s.memory.total > 0 && s.memory.used <= s.memory.total);
    let disk = s.disk.expect("el disco del sistema");
    assert!(disk.total > 0 && disk.free <= disk.total);
}
