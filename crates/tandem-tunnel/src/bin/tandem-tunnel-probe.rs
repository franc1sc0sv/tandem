//! Test process for the real launcher (`tests/system.rs`): does the same on the three
//! systems, without depending on each shell's `echo` or `sleep`. Not part of the product.
//!
//! - `tandem-tunnel-probe echo <text…>`: prints the arguments and a newline.
//! - `tandem-tunnel-probe err <text…>`: same, over stderr.
//! - `tandem-tunnel-probe exit <code>`.
//! - `tandem-tunnel-probe sleep <ms>`.
//! - `tandem-tunnel-probe flood <bytes>`: deterministic bytes over stdout (see `pattern`).
//! - `tandem-tunnel-probe env <name>` and `tandem-tunnel-probe cwd`: print the variable or the working folder.
//! - `tandem-tunnel-probe tree`: launches a grandchild that sleeps and sleeps itself too; both have the same
//!   output open.
//! - `tandem-tunnel-probe orphan`: launches that grandchild and ends right away.

use std::io::Write;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

/// Byte `i` of `flood`: does not repeat in short cycles.
fn pattern(i: u64) -> u8 {
    u8::try_from((i.wrapping_mul(2_654_435_761) >> 13) & 0xff).unwrap_or(0)
}

/// Incorrect use or I/O failure: code 2, without panic.
const BAD: u8 = 2;

// The grandchild is deliberately not waited for: it has to outlive the process that launched it.
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
            writeln!(out, "ready").ok()?;
            out.flush().ok()?;
            std::thread::sleep(Duration::from_secs(600));
        }
        Some("orphan") => {
            grandchild()?;
            writeln!(out, "ready").ok()?;
        }
        other => {
            eprintln!("unknown command: {other:?}");
            return Some(ExitCode::from(64));
        }
    }
    Some(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(&args).unwrap_or(ExitCode::from(BAD))
}
