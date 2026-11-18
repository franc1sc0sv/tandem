//! `tandem-hw`: lo que el orquestador (`scripts/hw/run.sh`) corre en cada máquina.
//!
//! ```text
//! tandem-hw provision --role pc|mac --root DIR --port N --peer IP:PUERTO
//! tandem-hw run --label pc|mac --data-dir DIR --checks a,b,c [--peer-exe P --remote-dir D
//!               --remote-sep S --keep-secs N --window-secs N --pair-wait-secs N]
//! tandem-hw report --date AAAA-MM-DD --out ARCHIVO [--log nombre=ruta]... resultados.jsonl...
//! tandem-hw emit TEXTO | sleep SEGUNDOS | echo-server PUERTO | monitor-hold | monitor-input | monitor-restore dp|hdmi
//! ```

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, bail};
use tandem_hw::checks::{self, Ctx};
use tandem_hw::monitor;
use tandem_hw::provision::{self, Role};
use tandem_hw::report::{self, CheckResult, Log};

struct Opts(BTreeMap<String, Vec<String>>);

impl Opts {
    fn parse(args: &[String]) -> anyhow::Result<(Self, Vec<String>)> {
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut rest = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if let Some(k) = a.strip_prefix("--") {
                let v = it
                    .next()
                    .with_context(|| format!("falta el valor de --{k}"))?;
                map.entry(k.into()).or_default().push(v.clone());
            } else {
                rest.push(a.clone());
            }
        }
        Ok((Opts(map), rest))
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.0.get(k).and_then(|v| v.last()).map(String::as_str)
    }

    fn need(&self, k: &str) -> anyhow::Result<&str> {
        self.get(k).with_context(|| format!("falta --{k}"))
    }

    fn secs(&self, k: &str, default: u64) -> anyhow::Result<Duration> {
        match self.get(k) {
            Some(v) => Ok(Duration::from_secs(
                v.parse()
                    .with_context(|| format!("--{k} no es un número"))?,
            )),
            None => Ok(Duration::from_secs(default)),
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("tandem-hw: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn dispatch(args: &[String]) -> anyhow::Result<ExitCode> {
    let Some((cmd, rest)) = args.split_first() else {
        bail!("falta el subcomando (ver la cabecera de tandem-hw)");
    };
    let (opts, pos) = Opts::parse(rest)?;
    match cmd.as_str() {
        "provision" => {
            let role = Role::parse(opts.need("role")?).context("--role es pc o mac")?;
            let p = provision::provision(
                &PathBuf::from(opts.need("root")?),
                role,
                opts.get("port")
                    .map_or(Ok(provision::TEST_PORT), str::parse)?,
                opts.need("peer")?,
            )?;
            println!(
                "{}",
                serde_json::json!({
                    "machine": p.machine.to_string(),
                    "config": p.config,
                    "data_dir": p.data_dir,
                })
            );
        }
        "run" => return run(&opts),
        "report" => return write_report(&opts, &pos),
        "emit" => println!("{}", pos.join(" ")),
        "sleep" => std::thread::sleep(Duration::from_secs(
            pos.first().context("falta SEGUNDOS")?.parse()?,
        )),
        "echo-server" => echo_server(pos.first().context("falta PUERTO")?.parse()?)?,
        "monitor-hold" => monitor::hold()?,
        "monitor-input" => println!(
            "{}",
            monitor::current_input_name().map_err(anyhow::Error::msg)?
        ),
        "monitor-restore" => monitor::restore_with_guard(pos.first().context("falta dp|hdmi")?)
            .map_err(anyhow::Error::msg)?,
        other => bail!("subcomando desconocido: {other}"),
    }
    Ok(ExitCode::SUCCESS)
}

/// Un solo cliente y a dormir: el túnel de prueba lo mata de todos modos. Se rinde a los
/// 60 s para no dejar el puerto abierto si algo falla antes.
fn echo_server(port: u16) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    listener.set_nonblocking(true)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        match listener.accept() {
            Ok((mut s, _)) => {
                s.set_nonblocking(false)?;
                let mut buf = [0u8; 14];
                s.read_exact(&mut buf)?;
                s.write_all(&buf)?;
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn run(opts: &Opts) -> anyhow::Result<ExitCode> {
    let label = opts.need("label")?.to_string();
    let ctx = Ctx {
        data_dir: PathBuf::from(opts.need("data-dir")?),
        label: label.clone(),
        peer_exe: opts.get("peer-exe").unwrap_or_default().into(),
        remote_dir: opts.get("remote-dir").unwrap_or_default().into(),
        remote_sep: opts
            .get("remote-sep")
            .and_then(|s| s.chars().next())
            .unwrap_or('/'),
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let mut failed = false;
    for name in opts.need("checks")?.split(',').filter(|s| !s.is_empty()) {
        let result = match name {
            "pair" => rt.block_on(checks::pair(&ctx, opts.secs("pair-wait-secs", 60)?)),
            "ping" => rt.block_on(checks::ping(&ctx, 50)),
            "input-cross" => rt.block_on(checks::input_cross(&ctx, opts.secs("keep-secs", 6)?)),
            "input-latency" => {
                rt.block_on(checks::latency_watch(&ctx, opts.secs("window-secs", 25)?))
            }
            monitor::ROUNDTRIP => monitor::roundtrip(&label),
            monitor::KILL_MIDWAY => monitor::kill_midway(&label),
            other => match rt.block_on(checks::run_tunnel(&ctx, other)) {
                Some(r) => r,
                None => CheckResult::new(other, &label, &[]).fail("comprobación desconocida"),
            },
        };
        failed |= result.outcome == report::Outcome::Fail;
        println!("{}", result.to_line());
        std::io::stdout().flush()?;
    }
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn write_report(opts: &Opts, files: &[String]) -> anyhow::Result<ExitCode> {
    let mut results = Vec::new();
    for f in files {
        results.extend(report::parse_lines(
            &std::fs::read_to_string(f).with_context(|| f.clone())?,
        ));
    }
    let logs: Vec<Log> = opts
        .0
        .get("log")
        .into_iter()
        .flatten()
        .filter_map(|spec| {
            let (name, path) = spec.split_once('=')?;
            Some(Log {
                name: name.into(),
                text: std::fs::read_to_string(path).unwrap_or_default(),
            })
        })
        .collect();
    let mut md = report::render(opts.need("date")?, &results, &logs);
    let monitors_skipped =
        opts.get("require-monitors") == Some("1") && report::monitors_all_skipped(&results);
    if monitors_skipped {
        md.push_str(
            "\n## AVISO\n\nSe pidieron monitores (`--with-monitors`) pero todas las comprobaciones \
             de monitor salieron omitidas: H3 no quedó probado.\n",
        );
        eprintln!("AVISO: se pidieron monitores y todas las comprobaciones salieron omitidas");
    }
    let out = PathBuf::from(opts.need("out")?);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out, md)?;
    println!("{}", out.display());
    Ok(if report::all_green(&results) && !monitors_skipped {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}
