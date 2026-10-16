//! `tandem-ddc`: diagnóstico DDC/CI desde la PC. Ver `tandem_display::cli::USAGE`.

use std::io::Write;
use std::process::ExitCode;

use tandem_display::cli::{self, CliError, Command};

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
    run_platform(&cmd, &mut out)
}

#[cfg(windows)]
fn run_platform(cmd: &Command, out: &mut dyn Write) -> Result<(), CliError> {
    use tandem_display::ddc::switch::SleepWaiter;
    use tandem_display::windows::{console, ddc::WindowsDdc};

    let interrupt = console::install_interrupt_handler()?;
    let mut bus = WindowsDdc::open()?;
    let mut waiter = SleepWaiter::new(Some(interrupt));
    let result = cli::run(cmd, &mut bus, &mut waiter, out);
    console::mark_finished();
    result
}

#[cfg(not(windows))]
fn run_platform(_cmd: &Command, _out: &mut dyn Write) -> Result<(), CliError> {
    Err(tandem_display::ddc::DdcError::Unsupported.into())
}
