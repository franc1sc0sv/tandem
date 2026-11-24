//! The checks as hardware tests, to run by hand against a test agent that is already
//! running:
//!
//! ```text
//! TANDEM_HW_DATA_DIR=<data directory of the test agent> \
//!   cargo test -p tandem-hw --test hardware -- --ignored --test-threads=1
//! ```
//!
//! The monitor ones switch the input **with automatic rollback** (guard + watchdog) and
//! need `TANDEM_HW_MONITORS=1` and `tandem-monitor-guard` next to the test executable
//! (`cargo build -p tandem-monitor` leaves it in `target/<profile>/`). Never in CI.

use std::path::PathBuf;
use std::time::Duration;

use tandem_hw::checks::{self, Ctx};
use tandem_hw::report::{CheckResult, Outcome};

fn ctx() -> Ctx {
    let data_dir = std::env::var_os("TANDEM_HW_DATA_DIR")
        .map(PathBuf::from)
        .expect("missing TANDEM_HW_DATA_DIR (data directory of the test agent)");
    let scratch = data_dir.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    Ctx {
        data_dir,
        label: "local".into(),
        peer_exe: std::env::var("TANDEM_HW_PEER_EXE").unwrap_or_default(),
        remote_dir: std::env::var("TANDEM_HW_REMOTE_DIR").unwrap_or_default(),
        remote_sep: std::env::var("TANDEM_HW_REMOTE_SEP")
            .ok()
            .and_then(|s| s.chars().next())
            .unwrap_or('/'),
    }
}

fn expect(r: CheckResult) {
    eprintln!("{}", r.to_line());
    assert_ne!(r.outcome, Outcome::Fail, "{}: {}", r.check, r.detail);
}

#[tokio::test]
#[ignore = "hardware"]
async fn pair_over_the_real_link() {
    expect(checks::pair(&ctx(), Duration::from_secs(60)).await);
}

#[tokio::test]
#[ignore = "hardware"]
async fn control_ping() {
    expect(checks::ping(&ctx(), 50).await);
}

#[tokio::test]
#[ignore = "hardware"]
async fn synthetic_input_crosses() {
    expect(checks::input_cross(&ctx(), Duration::from_secs(6)).await);
}

#[tokio::test]
#[ignore = "hardware"]
async fn tunnels_end_to_end() {
    let ctx = ctx();
    for name in checks::TUNNEL_CHECKS {
        expect(checks::run_tunnel(&ctx, name).await.unwrap());
    }
}

/// Only with `TANDEM_HW_MONITORS=1`: switches the 34" to the other input and back.
#[test]
#[ignore = "hardware"]
fn monitor_switch_returns_automatically() {
    if std::env::var_os("TANDEM_HW_MONITORS").is_none() {
        eprintln!("skipped: TANDEM_HW_MONITORS=1 is missing");
        return;
    }
    expect(tandem_hw::monitor::roundtrip("local"));
}
