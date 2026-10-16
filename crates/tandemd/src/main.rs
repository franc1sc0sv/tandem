//! Punto de entrada de `tandemd`.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use tandemd::cli::{self, Command, RunArgs};
use tandemd::paths::{self, Paths};
use tandemd::{Agent, AgentConfig, StateStore, daemon, ipc, logging};

fn main() -> ExitCode {
    let cmd = match cli::parse(std::env::args_os().skip(1)) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("tandemd: {e}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    match cmd {
        Command::Help => println!("{}", cli::USAGE),
        Command::Version => println!("tandemd {}", env!("CARGO_PKG_VERSION")),
        Command::CheckConfig(path) => match AgentConfig::load(&path) {
            Ok(cfg) => println!(
                "{}: configuración válida (máquina {}, {} monitores)",
                path.display(),
                cfg.machine,
                cfg.desk.monitors.len()
            ),
            Err(e) => {
                eprintln!("tandemd: {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        },
        Command::Run(args) => {
            if let Err(e) = run(args) {
                tracing::error!("{e:#}");
                eprintln!("tandemd: {e:#}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

fn run(args: RunArgs) -> anyhow::Result<()> {
    let paths = Paths::resolve(args.config, args.data_dir)?;
    paths::create_private_dir(&paths.data_dir)
        .with_context(|| format!("no se pudo crear {}", paths.data_dir.display()))?;
    let log = logging::init(&paths.log_dir()).context("no se pudo iniciar el log")?;
    tracing::info!(version = env!("CARGO_PKG_VERSION"), log = %log.display(), "tandemd arranca");

    let cfg = AgentConfig::load(&paths.config)?;
    tracing::info!(
        "configuración {}: máquina {}, {} monitores",
        paths.config.display(),
        cfg.machine,
        cfg.desk.monitors.len()
    );
    let agent = Arc::new(Agent::with_platform(
        cfg,
        StateStore::new(paths.state_file()),
    ));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("no se pudo crear el runtime")?;
    rt.block_on(async {
        let endpoint = ipc::default_endpoint(&paths.data_dir)?;
        let listener = ipc::bind(&endpoint).await?;
        tracing::info!("IPC escuchando en {}", endpoint.display());
        daemon::run(agent, listener, daemon::shutdown_signal()).await?;
        Ok(())
    })
}
