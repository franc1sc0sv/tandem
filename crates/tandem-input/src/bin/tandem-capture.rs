//! `tandem-capture`: manual test of Windows capture.
//!
//! Prints each captured event for a few seconds. With `--grab-for`, it swallows local
//! input for that time and gives it back on its own (30 s at most): even if the
//! keyboard and mouse do not respond, the program releases them when the deadline expires.

use std::time::Duration;

const USAGE: &str = "usage: tandem-capture [--seconds N] [--grab-after S] [--grab-for G]
  --seconds N     total duration, 1-300 s (default 15)
  --grab-after S  seconds before swallowing input (default 3)
  --grab-for G    swallow input for G seconds, 1-30; it is given back on its own";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Options {
    total: Duration,
    grab: Option<(Duration, Duration)>,
}

fn parse_secs(name: &str, value: Option<String>, range: (u64, u64)) -> anyhow::Result<Duration> {
    let value = value.ok_or_else(|| anyhow::anyhow!("missing value for {name}\n{USAGE}"))?;
    let secs: u64 = value
        .parse()
        .map_err(|_| anyhow::anyhow!("{name}: «{value}» is not a number of seconds"))?;
    anyhow::ensure!(
        (range.0..=range.1).contains(&secs),
        "{name} has to be between {} and {} s",
        range.0,
        range.1
    );
    Ok(Duration::from_secs(secs))
}

fn parse_args(args: impl IntoIterator<Item = String>) -> anyhow::Result<Options> {
    let mut total = Duration::from_secs(15);
    let mut grab_after = Duration::from_secs(3);
    let mut grab_for = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seconds" => total = parse_secs("--seconds", args.next(), (1, 300))?,
            "--grab-after" => grab_after = parse_secs("--grab-after", args.next(), (0, 60))?,
            "--grab-for" => grab_for = Some(parse_secs("--grab-for", args.next(), (1, 30))?),
            other => anyhow::bail!("unknown argument: {other}\n{USAGE}"),
        }
    }
    let grab = grab_for.map(|g| (grab_after, g));
    if let Some((after, g)) = grab {
        // The program has to stay alive to give the input back.
        total = total.max(after + g + Duration::from_secs(1));
    }
    Ok(Options { total, grab })
}

#[cfg(windows)]
fn run(opts: Options) -> anyhow::Result<()> {
    use std::time::Instant;

    use tandem_input::windows::capture::{CaptureConfig, WindowsCapture};

    let t0 = Instant::now();
    let capture = WindowsCapture::start(CaptureConfig::default(), move |ev| {
        println!("{:>8.3}  {ev:?}", t0.elapsed().as_secs_f64());
    })?;
    println!("capturing {} s (Ctrl+C to exit)", opts.total.as_secs());
    if let Some((after, grab_for)) = opts.grab {
        std::thread::sleep(after);
        println!("--- input swallowed for {} s ---", grab_for.as_secs());
        capture.set_grab(true);
        std::thread::sleep(grab_for);
        capture.set_grab(false);
        println!("--- input returned ---");
    }
    let elapsed = t0.elapsed();
    std::thread::sleep(opts.total.saturating_sub(elapsed));
    println!(
        "hooks reinstalled by the watchdog: {}",
        capture.reinstalls()
    );
    Ok(())
}

#[cfg(not(windows))]
fn run(_opts: Options) -> anyhow::Result<()> {
    anyhow::bail!("tandem-capture only works on Windows")
}

fn main() -> anyhow::Result<()> {
    run(parse_args(std::env::args().skip(1))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<Options> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn by_default_it_only_listens() {
        assert_eq!(
            parse(&[]).unwrap(),
            Options {
                total: Duration::from_secs(15),
                grab: None
            }
        );
    }

    #[test]
    fn the_grab_has_a_cap_and_lengthens_the_duration() {
        let o = parse(&["--seconds", "5", "--grab-after", "4", "--grab-for", "10"]).unwrap();
        assert_eq!(
            o.grab,
            Some((Duration::from_secs(4), Duration::from_secs(10)))
        );
        assert_eq!(o.total, Duration::from_secs(15));
        assert!(parse(&["--grab-for", "31"]).is_err());
        assert!(parse(&["--grab-for", "0"]).is_err());
        assert!(parse(&["--grab-for"]).is_err());
        assert!(parse(&["--seconds", "x"]).is_err());
        assert!(parse(&["--other"]).is_err());
    }
}
