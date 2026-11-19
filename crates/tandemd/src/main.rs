//! Entry point of `tandemd`: reads the command line and hands everything to
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
                    "{}: valid configuration (side {}, {} screens declared{})",
                    path.display(),
                    cfg.machine,
                    cfg.desk.screens.len(),
                    if cfg.rewrite_sides {
                        "; with the v0.2 mac/pc sides, which the agent translates on its own at startup"
                    } else {
                        ""
                    }
                );
                for w in cfg.warnings() {
                    eprintln!("tandemd: {}: warning: {w}", path.display());
                }
            }
            Err(e) => {
                eprintln!("tandemd: {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        },
        Command::Install { args, with_service } => {
            if let Err(e) = install(args, with_service) {
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
        Command::InstallService => {
            if let Err(e) = service_cmd::install() {
                eprintln!("tandemd: {e:#}");
                return ExitCode::FAILURE;
            }
        }
        Command::UninstallService => {
            if let Err(e) = service_cmd::uninstall() {
                eprintln!("tandemd: {e:#}");
                return ExitCode::FAILURE;
            }
        }
        Command::Service => {
            if let Err(e) = service_cmd::run_service() {
                eprintln!("tandemd: {e:#}");
                return ExitCode::FAILURE;
            }
        }
        Command::InputHelper => {
            if let Err(e) = service_cmd::run_helper() {
                tracing::error!("{e:#}");
                eprintln!("tandemd: {e:#}");
                return ExitCode::FAILURE;
            }
        }
        Command::Links(args) => {
            if let Err(e) = links(args) {
                eprintln!("tandemd: {e:#}");
                return ExitCode::FAILURE;
            }
        }
        Command::Run(args) => {
            if let Err(e) = run_hosted(args) {
                tracing::error!("{e:#}");
                eprintln!("tandemd: {e:#}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

/// `tandemd links`: interfaces, detected type, link address and state of the paths.
fn links(args: RunArgs) -> anyhow::Result<()> {
    use tandem_link::iface::{Interfaces, SystemInterfaces};

    let paths = Paths::resolve(args.config, args.data_dir)?;
    let (side, port) = AgentConfig::load(&paths.config)
        .map(|c| (c.machine, c.network.port))
        .unwrap_or((tandem_core::Machine::A, tandemd::config::DEFAULT_PORT));
    let os = if cfg!(windows) {
        tandem_core::Os::Windows
    } else {
        tandem_core::Os::MacOs
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("could not create the runtime")?;
    rt.block_on(async {
        let interfaces = SystemInterfaces::spawn().list();
        // The paths' state is only held by the running agent: without it it is omitted.
        let status = network_status(&paths.data_dir).await;
        print!(
            "{}",
            tandemd::links_cmd::render(&interfaces, side, os, port, status.as_ref())
        );
        Ok(())
    })
}

async fn network_status(data_dir: &std::path::Path) -> Option<tandem_proto::NetworkStatus> {
    use tandem_proto::{AgentToUi, UiCommand};

    let endpoint = ipc::default_endpoint(data_dir).ok()?;
    let mut client = ipc::connect(&endpoint).await.ok()?;
    client
        .request(UiCommand::WatchNetwork { on: true })
        .await
        .ok()?;
    let wait = async {
        while let Ok(Some(event)) = client.next_event().await {
            if let AgentToUi::Network(status) = event {
                return Some(*status);
            }
        }
        None
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), wait)
        .await
        .ok()
        .flatten()
}

/// On macOS, AppKit requires the main thread for the synthetic file drag (T-121):
/// the agent runs on another thread and this one serves the run loop. When the agent ends the
/// process exits, because that run loop does not return on its own.
#[cfg(target_os = "macos")]
fn run_hosted(args: RunArgs) -> anyhow::Result<()> {
    let agent = std::thread::Builder::new()
        .name("tandemd".into())
        .spawn(move || {
            // If `run` panics the thread dies, but the main thread's run loop
            // does not return: without this the process would stay alive without an agent.
            let _guard = ExitOnPanic::new(|code| std::process::exit(code));
            let code = match run(args) {
                Ok(()) => 0,
                Err(e) => {
                    tracing::error!("{e:#}");
                    eprintln!("tandemd: {e:#}");
                    1
                }
            };
            std::process::exit(code)
        })
        .context("could not create the agent thread")?;
    tandem_files::drag_macos::run_app_loop();
    // It only gets here if the run loop did not start: the agent continues on its way.
    let _ = agent.join();
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn run_hosted(args: RunArgs) -> anyhow::Result<()> {
    run(args)
}

fn run(args: RunArgs) -> anyhow::Result<()> {
    let paths = Paths::resolve(args.config.clone(), args.data_dir.clone())?;
    paths::create_private_dir(&paths.data_dir)
        .with_context(|| format!("could not create {}", paths.data_dir.display()))?;
    let log = logging::init(&paths.log_dir()).context("could not initialize the log")?;
    tracing::info!(version = env!("CARGO_PKG_VERSION"), log = %log.display(), "tandemd starting");

    // Without `tandem.toml`, it is generated with an empty desk (T-087).
    let loaded = autoconfig::load_or_generate(&paths.config, autoconfig::initial_side())?;
    let (cfg, generated) = (loaded.config, loaded.generated);
    tracing::info!(
        "configuration {}: side {}, {} screens declared{}",
        paths.config.display(),
        cfg.machine,
        cfg.desk.screens.len(),
        if generated { " (generated now)" } else { "" }
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not create the runtime")?;
    rt.block_on(async {
        // The IPC first: if a tandemd is already running, the error says so clearly.
        let endpoint = ipc::default_endpoint(&paths.data_dir)?;
        let listener = ipc::bind(&endpoint).await?;
        tracing::info!("IPC listening on {}", endpoint.display());
        let (backends, net) = backends(&args, &paths);
        let daemon = daemon::start(cfg, &paths, backends, net)?;
        daemon.agent.set_generated(generated);
        daemon.serve(listener, daemon::shutdown_signal()).await?;
        Ok(())
    })
}

/// The platform's, except with `--sim-backends`: simulated steps and no mDNS, so
/// two test agents do not mix with anything on the LAN.
fn backends(args: &RunArgs, paths: &Paths) -> (Backends, NetOptions) {
    #[cfg(feature = "sim-backends")]
    if args.sim_backends {
        tracing::warn!("simulated backends: this machine does not touch input or power");
        let net = NetOptions {
            mdns: false,
            // The two-process tests fix both ports: a taken port
            // has to fail right away, not move to one the other does not know.
            fallback_ports: Vec::new(),
            ..NetOptions::default()
        };
        return (tandemd::sim::backends(&paths.data_dir), net);
    }
    let _ = (args, paths);
    (Backends::Platform, NetOptions::default())
}

#[cfg(windows)]
fn install(args: RunArgs, with_service: bool) -> anyhow::Result<()> {
    use tandemd::install::{
        self, AgentInstall, HighestTask, NotElevated, RunKey, Scheduler, WinAcl,
    };

    let paths = Paths::resolve(args.config.clone(), args.data_dir.clone())?;
    let exe = std::env::current_exe().context("could not find tandemd's executable")?;
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
        println!("the configuration of {} is kept", paths.config.display());
    } else {
        println!(
            "the configuration is generated when the agent starts, in {}",
            paths.config.display()
        );
    }
    match installed {
        AgentInstall::Task { .. } => println!(
            "startup with the session: scheduled task \"{}\" with the highest privileges",
            install::TASK_NAME
        ),
        AgentInstall::RunKey(report, reason) => {
            let why = match reason {
                NotElevated::NoAdmin => "without an administrator".to_owned(),
                NotElevated::NoUser => "the user is not known".to_owned(),
                NotElevated::Writable(why) => {
                    format!("{why}: install in %ProgramFiles% to elevate it")
                }
            };
            println!(
                "startup with the session: {} ({why}: tunnels will run without elevation)",
                report.command
            );
        }
    }
    // The installer runs `--install` on every update: an installed service is refreshed with
    // this version's binary (only possible with administrator).
    let installed = tandemd::secure::windows::service_state()
        .ok()
        .flatten()
        .is_some();
    if with_service || installed {
        if with_service || HighestTask.is_elevated() {
            service_cmd::install()?;
        } else {
            println!(
                "the system service was not refreshed (requires an administrator): run tandemd --install-service as administrator"
            );
        }
    }
    Ok(())
}

#[cfg(windows)]
fn uninstall() -> anyhow::Result<()> {
    use tandemd::install::{HighestTask, RunKey, uninstall_agent};

    // The service goes first: removing the task requires the same administrator, and an
    // uninstall without one must not leave the service half removed.
    let service = tandemd::secure::windows::service_state()
        .ok()
        .flatten()
        .is_some();
    if service {
        use tandemd::install::Scheduler;
        if HighestTask.is_elevated() {
            service_cmd::uninstall()?;
        } else {
            println!(
                "the system service stays installed (removing it requires an administrator): run tandemd --uninstall-service as administrator"
            );
        }
    }
    let (task, run) = uninstall_agent(&mut HighestTask, &mut RunKey)?;
    if task || run {
        println!("startup with the session removed");
    } else {
        println!("startup with the session was not registered");
    }
    Ok(())
}

/// The secure desktop service (T-027): `--install-service`, `--uninstall-service`,
/// `--service` and `--input-helper`.
#[cfg(windows)]
mod service_cmd {
    use anyhow::Context;
    use tandemd::secure::install::Step;
    use tandemd::secure::windows;

    pub fn install() -> anyhow::Result<()> {
        let exe = std::env::current_exe().context("could not find tandemd's executable")?;
        let steps =
            windows::install_service(&exe).context("could not install the system service")?;
        for step in &steps {
            if let Step::SetSasPolicy { value, .. } = step {
                println!(
                    "policy SoftwareSASGeneration = {value} (lets services send Ctrl+Alt+Del)"
                );
            }
        }
        println!(
            "system service \"{}\" installed and running (secure desktop: UAC, sign-in screen, Ctrl+Alt+Del)",
            tandemd::secure::SERVICE_NAME
        );
        Ok(())
    }

    pub fn uninstall() -> anyhow::Result<()> {
        let exe = std::env::current_exe().context("could not find tandemd's executable")?;
        let steps =
            windows::uninstall_service(&exe).context("could not remove the system service")?;
        if steps.contains(&Step::Delete) {
            println!("system service removed");
        } else {
            println!("the system service was not installed");
        }
        Ok(())
    }

    pub fn run_service() -> anyhow::Result<()> {
        windows::run_service().context(
            "the service dispatcher did not start: --service is only for the Windows service manager",
        )
    }

    pub fn run_helper() -> anyhow::Result<()> {
        if let Ok(dir) = tandemd::secure::windows::service_log_dir("helper") {
            let _ = tandemd::logging::init(&dir);
        }
        windows::run_helper().context("the input helper failed")
    }
}

#[cfg(not(windows))]
mod service_cmd {
    pub fn install() -> anyhow::Result<()> {
        anyhow::bail!("--install-service only exists on Windows")
    }

    pub fn uninstall() -> anyhow::Result<()> {
        anyhow::bail!("--uninstall-service only exists on Windows")
    }

    pub fn run_service() -> anyhow::Result<()> {
        anyhow::bail!("--service only exists on Windows")
    }

    pub fn run_helper() -> anyhow::Result<()> {
        anyhow::bail!("--input-helper only exists on Windows")
    }
}

// On macOS startup is a LaunchAgent installed by the app (T-070).
#[cfg(not(windows))]
fn install(_args: RunArgs, _with_service: bool) -> anyhow::Result<()> {
    anyhow::bail!("--install only exists on Windows")
}

#[cfg(not(windows))]
fn uninstall() -> anyhow::Result<()> {
    anyhow::bail!("--uninstall only exists on Windows")
}

/// When dropped during a panic it calls `exit(1)`; on a normal exit it does nothing.
/// `exit` is injected so it can be tested without killing the test process.
#[cfg(any(test, target_os = "macos"))]
struct ExitOnPanic<F: FnMut(i32)> {
    exit: F,
}

#[cfg(any(test, target_os = "macos"))]
impl<F: FnMut(i32)> ExitOnPanic<F> {
    fn new(exit: F) -> Self {
        Self { exit }
    }
}

#[cfg(any(test, target_os = "macos"))]
impl<F: FnMut(i32)> Drop for ExitOnPanic<F> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            (self.exit)(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ExitOnPanic;
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    #[test]
    fn the_guard_exits_with_1_on_panic() {
        let code = Cell::new(None);
        let r = catch_unwind(AssertUnwindSafe(|| {
            let _g = ExitOnPanic::new(|c| code.set(Some(c)));
            panic!("test");
        }));
        assert!(r.is_err());
        assert_eq!(code.get(), Some(1));
    }

    #[test]
    fn the_guard_does_not_exit_without_panic() {
        let code = Cell::new(None);
        {
            let _g = ExitOnPanic::new(|c| code.set(Some(c)));
        }
        assert_eq!(code.get(), None);
    }
}
