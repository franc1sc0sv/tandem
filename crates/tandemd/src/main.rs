//! Punto de entrada de `tandemd`: lee la línea de órdenes y le pasa todo a
//! [`tandemd::daemon`].

use std::process::ExitCode;

use anyhow::Context;
use tandemd::cli::{self, Command, RunArgs};
use tandemd::daemon::{self, Backends};
use tandemd::network::NetOptions;
use tandemd::paths::{self, Paths};
use tandemd::{AgentConfig, autoconfig, ipc, logging};

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
            Ok(cfg) => {
                println!(
                    "{}: configuración válida (lado {}, {} pantallas declaradas{})",
                    path.display(),
                    cfg.machine,
                    cfg.desk.screens.len(),
                    if cfg.rewrite_sides {
                        "; con los lados mac/pc de v0.2, que el agente traduce solo al arrancar"
                    } else {
                        ""
                    }
                );
                for w in cfg.warnings() {
                    eprintln!("tandemd: {}: aviso: {w}", path.display());
                }
            }
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

    // Sin `tandem.toml`, se genera con el puesto vacío (T-087).
    let loaded = autoconfig::load_or_generate(&paths.config, autoconfig::initial_side())?;
    let (cfg, generated) = (loaded.config, loaded.generated);
    tracing::info!(
        "configuración {}: lado {}, {} pantallas declaradas{}",
        paths.config.display(),
        cfg.machine,
        cfg.desk.screens.len(),
        if generated { " (generada ahora)" } else { "" }
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
        daemon.agent.set_generated(generated);
        daemon.serve(listener, daemon::shutdown_signal()).await?;
        Ok(())
    })
}

/// Los de la plataforma, salvo con `--sim-backends`: pasos simulados y sin mDNS, para
/// que dos agentes de prueba no se mezclen con nada de la LAN.
fn backends(args: &RunArgs, paths: &Paths) -> (Backends, NetOptions) {
    #[cfg(feature = "sim-backends")]
    if args.sim_backends {
        tracing::warn!("backends simulados: esta máquina no toca la entrada ni la energía");
        let net = NetOptions {
            mdns: false,
            // Las pruebas de dos procesos fijan los puertos de los dos: un puerto tomado
            // tiene que fallar enseguida, no pasarse a uno que el otro no conoce.
            fallback_ports: Vec::new(),
            ..NetOptions::default()
        };
        return (tandemd::sim::backends(&paths.data_dir), net);
    }
    let _ = (args, paths);
    (Backends::Platform, NetOptions::default())
}

#[cfg(windows)]
fn install(args: RunArgs) -> anyhow::Result<()> {
    use tandemd::install::{self, AgentInstall, HighestTask, NotElevated, RunKey, WinAcl};

    let paths = Paths::resolve(args.config.clone(), args.data_dir.clone())?;
    let exe = std::env::current_exe().context("no se encontró el ejecutable de tandemd")?;
    let launcher = install::launcher_for(&exe, |p| p.is_file());
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(name)) => Some(format!("{domain}\\{name}")),
        _ => None,
    };
    let installed = install::install_agent(
        &mut HighestTask,
        &WinAcl,
        &mut RunKey,
        &launcher,
        &args,
        user.as_deref(),
    )?;
    if paths.config.exists() {
        println!("se conserva la configuración de {}", paths.config.display());
    } else {
        println!(
            "la configuración se genera al arrancar el agente, en {}",
            paths.config.display()
        );
    }
    match installed {
        AgentInstall::Task { .. } => println!(
            "arranque con la sesión: tarea programada «{}» con los privilegios más altos",
            install::TASK_NAME
        ),
        AgentInstall::RunKey(report, reason) => {
            let why = match reason {
                NotElevated::NoAdmin => "sin administrador".to_owned(),
                NotElevated::NoUser => "no se conoce el usuario".to_owned(),
                NotElevated::Writable(why) => {
                    format!("{why}: instala en %ProgramFiles% para elevarlo")
                }
            };
            println!(
                "arranque con la sesión: {} ({why}: los túneles correrán sin elevar)",
                report.command
            );
        }
    }
    Ok(())
}

#[cfg(windows)]
fn uninstall() -> anyhow::Result<()> {
    use tandemd::install::{HighestTask, RunKey, uninstall_agent};

    let (task, run) = uninstall_agent(&mut HighestTask, &mut RunKey)?;
    if task || run {
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
