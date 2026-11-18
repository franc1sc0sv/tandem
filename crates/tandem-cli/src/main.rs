//! `tandem`: CLI y `tandem mcp`. Conecta con la IPC del agente local como cliente.

use std::process::ExitCode;

use anyhow::Context as _;
use tandem_cli::api::{Api, Context};
use tandem_cli::cli::{self, Command, Parsed};
use tandem_cli::{mcp, render};
use tandem_proto::{ClientKind, Peer};
use tandemd::ipc;
use tandemd::paths::Paths;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};

fn main() -> ExitCode {
    let parsed = match cli::parse(std::env::args().skip(1)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("tandem: {e}");
            return ExitCode::from(2);
        }
    };
    match parsed.command {
        Command::Help => {
            print!("{}", cli::usage());
            return ExitCode::SUCCESS;
        }
        Command::Version => {
            println!("tandem {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tandem: no se pudo crear el runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match rt.block_on(run(parsed)) {
        Ok(code) => code,
        Err(e) => {
            // En `tandem mcp` stdout es el protocolo: los errores van siempre por stderr.
            eprintln!("tandem: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(parsed: Parsed) -> anyhow::Result<ExitCode> {
    let kind = match parsed.command {
        Command::Mcp => ClientKind::Mcp,
        _ => ClientKind::Cli,
    };
    let paths = Paths::resolve(None, parsed.data_dir.clone())?;
    let endpoint = ipc::default_endpoint(&paths.data_dir)
        .context("no se pudo calcular el socket del agente")?;
    let client = ipc::connect_as(&endpoint, Peer::Client(kind))
        .await
        .with_context(|| agent_down(&endpoint))?;
    let session = std::env::var("CLAUDE_SESSION_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| match kind {
            ClientKind::Mcp => "mcp".into(),
            ClientKind::Cli => "cli".into(),
        });
    let ctx = Context {
        cwd: std::env::current_dir().ok(),
        session,
    };
    let mut api = Api::new(client, ctx);
    match parsed.command {
        Command::Mcp => {
            let stdin = BufReader::new(tokio::io::stdin());
            mcp::serve(&mut api, stdin, tokio::io::stdout()).await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Tool { name, args } => tool(&mut api, name, &args, parsed.json).await,
        Command::Help | Command::Version => Ok(ExitCode::SUCCESS),
    }
}

async fn tool<S: AsyncRead + AsyncWrite + Unpin>(
    api: &mut Api<S>,
    name: &str,
    args: &serde_json::Value,
    json: bool,
) -> anyhow::Result<ExitCode> {
    match api.call(name, args).await {
        Ok(v) if json => println!("{}", serde_json::to_string_pretty(&v)?),
        Ok(v) => {
            let r = render::render(name, &v);
            print!("{}", r.out);
            eprint!("{}", r.err);
        }
        Err(e) if json => {
            println!("{}", serde_json::json!({"error": e.to_string()}));
            return Ok(ExitCode::FAILURE);
        }
        Err(e) => {
            eprintln!("tandem: {e}");
            return Ok(ExitCode::FAILURE);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn agent_down(endpoint: &std::path::Path) -> String {
    format!(
        "no se pudo conectar con el agente en {}: ¿está corriendo `tandemd`?",
        endpoint.display()
    )
}
