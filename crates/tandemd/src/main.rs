//! Punto de entrada de `tandemd`: lee la línea de órdenes y le pasa todo a
//! [`tandemd::daemon`].

use std::process::ExitCode;

use anyhow::Context;
use tandemd::cli::{self, Command, RunArgs};
use tandemd::daemon::{self, Backends};
use tandemd::network::NetOptions;
use tandemd::paths::{self, Paths};
use tandemd::{AgentConfig, ipc, logging};

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
        Command::Install(args) => {
            if let Err(e) = install(args) {
                eprintln!("tandemd: {e:#}");
                return ExitCode::FAILURE;
            }
        }
        Command::Uninstall => {
            if let Err(e) = uninstall() {
                eprintln!("tandemd: {e:#}");
                return ExitCode::FAILURE;
            }
        }
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
    let paths = Paths::resolve(args.config.clone(), args.data_dir.clone())?;
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
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("no se pudo crear el runtime")?;
    rt.block_on(async {
        // La IPC primero: si ya hay un tandemd corriendo, el error lo dice claro.
        let endpoint = ipc::default_endpoint(&paths.data_dir)?;
        let listener = ipc::bind(&endpoint).await?;
        tracing::info!("IPC escuchando en {}", endpoint.display());
        let (backends, net) = backends(&args, &paths);
        let daemon = daemon::start(cfg, &paths, backends, net)?;
        daemon.serve(listener, daemon::shutdown_signal()).await?;
        Ok(())
    })
}

/// Los de la plataforma, salvo con `--sim-backends`: pasos simulados y sin mDNS, para
/// que dos agentes de prueba no se mezclen con nada de la LAN.
fn backends(args: &RunArgs, paths: &Paths) -> (Backends, NetOptions) {
    #[cfg(feature = "sim-backends")]
    if args.sim_backends {
        tracing::warn!("backends simulados: esta máquina no toca monitores, entrada ni energía");
        let net = NetOptions {
            mdns: false,
            ..NetOptions::default()
        };
        return (tandemd::sim::backends(&paths.data_dir), net);
    }
    let _ = (args, paths);
    (Backends::Platform, NetOptions::default())
}

#[cfg(windows)]
fn install(args: RunArgs) -> anyhow::Result<()> {
    use tandemd::install::{self, RunKey};

    let paths = Paths::resolve(args.config.clone(), args.data_dir.clone())?;
    let exe = std::env::current_exe().context("no se encontró el ejecutable de tandemd")?;
    let launcher = install::launcher_for(&exe, |p| p.is_file());
    let report = install::install(
        &mut RunKey,
        &paths.config,
        &install::command_line(&launcher, &args),
    )?;
    if report.config_created {
        println!("configuración inicial creada en {}", paths.config.display());
    } else {
        println!("se conserva la configuración de {}", paths.config.display());
    }
    println!("arranque con la sesión: {}", report.command);
    Ok(())
}

#[cfg(windows)]
fn uninstall() -> anyhow::Result<()> {
    if tandemd::install::uninstall(&mut tandemd::install::RunKey)? {
        println!("arranque con la sesión quitado");
    } else {
        println!("el arranque con la sesión no estaba registrado");
    }
    Ok(())
}

// En macOS el arranque es un LaunchAgent que instala la app (T-070).
#[cfg(not(windows))]
fn install(_args: RunArgs) -> anyhow::Result<()> {
    anyhow::bail!("--install solo existe en Windows")
}

#[cfg(not(windows))]
fn uninstall() -> anyhow::Result<()> {
    anyhow::bail!("--uninstall solo existe en Windows")
}
