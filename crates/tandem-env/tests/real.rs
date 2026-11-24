//! The scan and the sampling for real, on the machine running the tests (in CI, the runner
//! of each system): they must finish without error and find at least the runner's shell.

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
        // Windows runners ship PowerShell 5 and cmd; macOS ones, zsh and bash.
        Platform::Windows => &["powershell", "cmd"],
        Platform::MacOs => &["zsh", "bash"],
        Platform::Linux => &["bash"],
    };
    assert!(
        expected.iter().any(|s| shells.contains(s)),
        "no expected shell ({expected:?}) in {shells:?}"
    );
    assert_eq!(
        scan.environments
            .iter()
            .filter(|e| e.kind == EnvKind::Shell && e.default)
            .count(),
        1,
        "a single default shell"
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
    let disk = s.disk.expect("the system disk");
    assert!(disk.total > 0 && disk.free <= disk.total);
}
