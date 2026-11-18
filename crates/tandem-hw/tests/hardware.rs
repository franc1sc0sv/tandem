//! Las comprobaciones como pruebas de hardware, para correrlas a mano contra un agente de
//! prueba que ya esté en marcha:
//!
//! ```text
//! TANDEM_HW_DATA_DIR=<carpeta de datos del agente de prueba> \
//!   cargo test -p tandem-hw --test hardware -- --ignored --test-threads=1
//! ```
//!
//! Las de monitor cambian la entrada **con vuelta automática** (guardia + vigilante) y
//! necesitan `TANDEM_HW_MONITORS=1` y `tandem-monitor-guard` junto al ejecutable de la
//! prueba (`cargo build -p tandem-monitor` lo deja en `target/<perfil>/`). Nunca en CI.

use std::path::PathBuf;
use std::time::Duration;

use tandem_hw::checks::{self, Ctx};
use tandem_hw::report::{CheckResult, Outcome};

fn ctx() -> Ctx {
    let data_dir = std::env::var_os("TANDEM_HW_DATA_DIR")
        .map(PathBuf::from)
        .expect("falta TANDEM_HW_DATA_DIR (carpeta de datos del agente de prueba)");
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

/// Solo con `TANDEM_HW_MONITORS=1`: cambia el 34" a la otra entrada y vuelve.
#[test]
#[ignore = "hardware"]
fn monitor_switch_returns_automatically() {
    if std::env::var_os("TANDEM_HW_MONITORS").is_none() {
        eprintln!("omitida: falta TANDEM_HW_MONITORS=1");
        return;
    }
    expect(tandem_hw::monitor::roundtrip("local"));
}
