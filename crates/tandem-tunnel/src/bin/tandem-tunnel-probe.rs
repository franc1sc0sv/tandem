//! Proceso de prueba para el lanzador real (`tests/system.rs`): hace lo mismo en los tres
//! sistemas, sin depender de `echo` o `sleep` de cada shell. No es parte del producto.
//!
//! - `tandem-tunnel-probe echo <texto…>`: imprime los argumentos y un salto de línea.
//! - `tandem-tunnel-probe err <texto…>`: igual, por stderr.
//! - `tandem-tunnel-probe exit <código>`.
//! - `tandem-tunnel-probe sleep <ms>`.
//! - `tandem-tunnel-probe flood <bytes>`: bytes deterministas por stdout (ver `pattern`).
//! - `tandem-tunnel-probe env <nombre>` y `tandem-tunnel-probe cwd`: imprimen la variable o la carpeta de trabajo.
//! - `tandem-tunnel-probe tree`: lanza un nieto que duerme y duerme él también; ambos tienen la misma
//!   salida abierta.
//! - `tandem-tunnel-probe orphan`: lanza ese nieto y termina enseguida.

use std::io::Write;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

/// El byte `i` de `flood`: no se repite en ciclos cortos.
fn pattern(i: u64) -> u8 {
    u8::try_from((i.wrapping_mul(2_654_435_761) >> 13) & 0xff).unwrap_or(0)
}

/// Uso incorrecto o fallo de E/S: el código 2, sin pánico.
const BAD: u8 = 2;

// A propósito no se espera al nieto: tiene que sobrevivir al proceso que lo lanzó.
#[allow(clippy::zombie_processes)]
fn grandchild() -> Option<()> {
    let exe = std::env::current_exe().ok()?;
    Command::new(exe)
        .args(["sleep", "120000"])
        .stdin(Stdio::null())
        .spawn()
        .ok()?;
    Some(())
}

fn run(args: &[String]) -> Option<ExitCode> {
    let out = &mut std::io::stdout();
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(0) {
        Some("echo") => writeln!(out, "{}", args[1..].join(" ")).ok()?,
        Some("err") => eprintln!("{}", args[1..].join(" ")),
        Some("exit") => {
            return Some(ExitCode::from(
                u8::try_from(arg(1)?.parse::<i32>().ok()?).ok()?,
            ));
        }
        Some("sleep") => std::thread::sleep(Duration::from_millis(arg(1)?.parse().ok()?)),
        Some("flood") => {
            let total: u64 = arg(1)?.parse().ok()?;
            let mut at = 0;
            while at < total {
                let n = (total - at).min(64 * 1024);
                let chunk: Vec<u8> = (at..at + n).map(pattern).collect();
                out.write_all(&chunk).ok()?;
                at += n;
            }
        }
        Some("env") => writeln!(out, "{}", std::env::var(arg(1)?).unwrap_or_default()).ok()?,
        Some("cwd") => writeln!(out, "{}", std::env::current_dir().ok()?.display()).ok()?,
        Some("tree") => {
            grandchild()?;
            writeln!(out, "listo").ok()?;
            out.flush().ok()?;
            std::thread::sleep(Duration::from_secs(600));
        }
        Some("orphan") => {
            grandchild()?;
            writeln!(out, "listo").ok()?;
        }
        other => {
            eprintln!("orden desconocida: {other:?}");
            return Some(ExitCode::from(64));
        }
    }
    Some(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(&args).unwrap_or(ExitCode::from(BAD))
}
