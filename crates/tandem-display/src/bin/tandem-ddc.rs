//! `tandem-ddc`: diagnóstico DDC/CI desde la PC. Ver `tandem_display::cli::USAGE`.

use std::io::Write;
use std::process::ExitCode;

use tandem_display::cli::{self, CliError, Command};
use tandem_display::ddc::profiles::{self, ProfileDb};

/// El mismo directorio de datos que `tandemd` (`%APPDATA%\\Tandem` en Windows).
const DATA_DIR: &str = "Tandem";

fn main() -> ExitCode {
    let result = cli::parse_args(std::env::args().skip(1)).and_then(execute);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tandem-ddc: {e}");
            ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(1))
        }
    }
}

fn execute(cmd: Command) -> Result<(), CliError> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if cmd == Command::Help {
        write!(out, "{}", cli::USAGE)?;
        return Ok(());
    }
    let profiles = load_profiles(&cmd);
    run_platform(&cmd, &profiles, &mut out)
}

fn user_dir() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|d| profiles::user_dir(&d.join(DATA_DIR)))
}

/// Los del repo más los de `<datos>/profiles/ddc/`. Un archivo inválido no impide usar
/// los demás: se avisa y se sigue.
fn load_profiles(cmd: &Command) -> ProfileDb {
    let db = ProfileDb::load(user_dir().as_deref());
    if cmd.uses_profiles() {
        for e in db.errors() {
            eprintln!("tandem-ddc: aviso: perfil ignorado: {e}");
        }
    }
    db
}

#[cfg(windows)]
fn run_platform(cmd: &Command, profiles: &ProfileDb, out: &mut dyn Write) -> Result<(), CliError> {
    use tandem_display::ddc::switch::SleepWaiter;
    use tandem_display::windows::{console, ddc::WindowsDdc};

    let interrupt = console::install_interrupt_handler()?;
    let mut bus = WindowsDdc::open()?;
    let result = match cmd {
        Command::Calibrate(args) => calibrate(args, &mut bus, profiles, interrupt),
        _ => {
            let mut waiter = SleepWaiter::new(Some(interrupt));
            cli::run(cmd, &mut bus, &mut waiter, profiles, out)
        }
    };
    console::mark_finished();
    result
}

/// Corre la calibración en un runtime propio; Ctrl+C la cancela (el motor vuelve solo).
#[cfg(windows)]
fn calibrate(
    args: &cli::CalibrateArgs,
    bus: &mut tandem_display::windows::ddc::WindowsDdc,
    profiles: &ProfileDb,
    interrupt: &'static std::sync::atomic::AtomicBool,
) -> Result<(), CliError> {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use tandem_display::calibrate::console::LineConfirmer;
    use tandem_display::calibrate::{CalibrationOptions, CancelToken, profile};

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;
    let cancel = CancelToken::new();
    let confirmer = |wiring| {
        Box::new(LineConfirmer::stdin(Box::new(std::io::stdout()), wiring))
            as Box<dyn tandem_display::calibrate::Confirmer>
    };
    let dir = user_dir();
    let desk = load_desk();
    let env = cli::CalibrateEnv {
        confirmer: &confirmer,
        cancel: &cancel,
        options: CalibrationOptions::default(),
        user_dir: dir.as_deref(),
        desk: desk.as_ref(),
        today: profile::today(),
    };
    let mut out = std::io::stdout();
    let watch = cancel.clone();
    runtime.spawn(async move {
        while !interrupt.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        watch.cancel();
    });
    runtime
        .block_on(cli::run_calibrate(args, bus, profiles, env, &mut out))
        .map(|_| ())
}

/// El puesto de `tandem.toml` (el mismo de `tandemd`), para saber qué máquina va en
/// cada entrada. Sin él, o inválido, se avisa y se usa el cableado por omisión.
#[cfg(windows)]
fn load_desk() -> Option<tandem_core::Config> {
    let path = dirs::config_dir()?.join(DATA_DIR).join("tandem.toml");
    let text = std::fs::read_to_string(&path).ok()?;
    match tandem_core::Config::from_toml(&text) {
        Ok(desk) => Some(desk),
        Err(e) => {
            eprintln!("tandem-ddc: aviso: {} ignorado: {e}", path.display());
            None
        }
    }
}

#[cfg(not(windows))]
fn run_platform(
    _cmd: &Command,
    _profiles: &ProfileDb,
    _out: &mut dyn Write,
) -> Result<(), CliError> {
    Err(tandem_display::ddc::DdcError::Unsupported.into())
}
