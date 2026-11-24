//! Check results and the Markdown report.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Pass,
    Fail,
    /// Could not be checked (something is missing on this machine); it does not count as a failure.
    Skip,
}

/// One check on one machine. One per JSON line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub check: String,
    /// `pc` or `mac`: set by the orchestrator.
    pub machine: String,
    pub outcome: Outcome,
    pub detail: String,
    pub millis: u64,
    /// Tasks and milestones it covers ("T-104", "H2").
    pub covers: Vec<String>,
}

impl CheckResult {
    pub fn new(check: &str, machine: &str, covers: &[&str]) -> Self {
        CheckResult {
            check: check.into(),
            machine: machine.into(),
            outcome: Outcome::Skip,
            detail: String::new(),
            millis: 0,
            covers: covers.iter().map(|c| (*c).into()).collect(),
        }
    }

    pub fn pass(mut self, detail: impl Into<String>) -> Self {
        self.outcome = Outcome::Pass;
        self.detail = detail.into();
        self
    }

    pub fn fail(mut self, detail: impl Into<String>) -> Self {
        self.outcome = Outcome::Fail;
        self.detail = detail.into();
        self
    }

    pub fn skip(mut self, detail: impl Into<String>) -> Self {
        self.outcome = Outcome::Skip;
        self.detail = detail.into();
        self
    }

    pub fn to_line(&self) -> String {
        // One line: line breaks in the detail are escaped by JSON.
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Reads the result lines; ignores those that are not JSON (program noise).
pub fn parse_lines(text: &str) -> Vec<CheckResult> {
    text.lines()
        .filter_map(|l| serde_json::from_str(l.trim()).ok())
        .collect()
}

/// What no automatic suite can check: stated in the report so nobody
/// thinks it was covered.
pub const MANUAL: &[&str] = &[
    "An hour of normal use with no stuck keys (T-093).",
    // The "ñ" names the key under test on a Spanish layout; it is data, not prose.
    "ES/LatAm layout: ñ, dead keys, AltGr (T-093).",
    "Different scales between screens: physical cursor speed (T-093).",
    "Real hook loss on Windows and `kCGEventTapDisabledBy*` on macOS (T-093).",
    "Dragging a file between Finder and Explorer (T-121).",
    "Elevated Windows installer and scheduled task (T-104).",
    "USB4/Thunderbolt cable (H5, T-094): waiting on the cable.",
    "Real Claude Code session with `tandem mcp` (T-108).",
];

/// Tail of a log: the last `lines` lines and at most `max_bytes`.
pub fn tail(text: &str, lines: usize, max_bytes: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let from = all.len().saturating_sub(lines);
    let mut out = all[from..].join("\n");
    if out.len() > max_bytes {
        let mut cut = out.len() - max_bytes;
        while !out.is_char_boundary(cut) {
            cut += 1;
        }
        out = format!("…{}", &out[cut..]);
    }
    out
}

pub struct Log {
    pub name: String,
    pub text: String,
}

/// The report in `docs/reports/hw-<date>.md`.
pub fn render(date: &str, results: &[CheckResult], logs: &[Log]) -> String {
    let count = |o: Outcome| results.iter().filter(|r| r.outcome == o).count();
    let mut out = format!(
        "# Hardware report {date}\n\n\
         Passed {}, failed {}, skipped {}.\n\n\
         | Check | Machine | Result | Covers | Detail |\n|---|---|---|---|---|\n",
        count(Outcome::Pass),
        count(Outcome::Fail),
        count(Outcome::Skip),
    );
    for r in results {
        let mark = match r.outcome {
            Outcome::Pass => "ok",
            Outcome::Fail => "FAILED",
            Outcome::Skip => "skipped",
        };
        out.push_str(&format!(
            "| {} | {} | {mark} | {} | {} |\n",
            r.check,
            r.machine,
            r.covers.join(", "),
            r.detail.replace('|', "/").replace('\n', " "),
        ));
    }
    out.push_str("\n## Items this suite does not cover\n\n");
    for m in MANUAL {
        out.push_str(&format!("- {m}\n"));
    }
    if !logs.is_empty() {
        out.push_str("\n## Trimmed logs\n");
        for l in logs {
            out.push_str(&format!(
                "\n### {}\n\n```\n{}\n```\n",
                l.name,
                tail(&redact(&l.text), 40, 4096)
            ));
        }
    }
    out
}

/// `true` if there are monitor checks and all of them came out skipped.
pub fn monitors_all_skipped(results: &[CheckResult]) -> bool {
    let mut it = results
        .iter()
        .filter(|r| r.check.starts_with("monitor-"))
        .peekable();
    it.peek().is_some() && it.all(|r| r.outcome == Outcome::Skip)
}

/// Strips IPv4 addresses and monitor ids (`id=...`, `monitor=...`) from a log before it
/// reaches the versioned report.
pub fn redact(text: &str) -> String {
    text.lines().map(redact_line).collect::<Vec<_>>().join("\n")
}

fn redact_line(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while !rest.is_empty() {
        if let Some(len) = ipv4_len(rest) {
            out.push_str("<ip>");
            rest = &rest[len..];
            continue;
        }
        if let Some((key, len)) = id_key(rest) {
            out.push_str(key);
            out.push_str("<id>");
            rest = &rest[len..];
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, ',' | '"' | '}' | ')' | ';'))
                .unwrap_or(rest.len());
            rest = &rest[end..];
            continue;
        }
        let Some(c) = rest.chars().next() else { break };
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Length of an IPv4 at the start of `s`, if it is not glued to another digit behind it.
fn ipv4_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    for part in 0..4 {
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() && i - start < 3 {
            i += 1;
        }
        if i == start {
            return None;
        }
        if part < 3 {
            if b.get(i) != Some(&b'.') {
                return None;
            }
            i += 1;
        }
    }
    (!b.get(i).is_some_and(|c| c.is_ascii_digit())).then_some(i)
}

fn id_key(s: &str) -> Option<(&'static str, usize)> {
    ["monitor_id=", "monitor=", "id="]
        .into_iter()
        .find(|k| s.starts_with(k))
        .map(|k| (k, k.len()))
}

/// `true` if there is no failure (skipped ones do not count).
pub fn all_green(results: &[CheckResult]) -> bool {
    results.iter().all(|r| r.outcome != Outcome::Fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<CheckResult> {
        vec![
            CheckResult::new("pair", "pc", &["H1"]).pass("connected over Wi-Fi"),
            CheckResult::new("tunnel-files", "mac", &["T-106"])
                .fail("missing | a byte\nsecond line"),
            CheckResult::new("monitor-roundtrip", "pc", &["H3"]).skip("no --with-monitors"),
        ]
    }

    #[test]
    fn a_result_survives_the_json_line() {
        for r in sample() {
            let line = r.to_line();
            assert!(!line.contains('\n'));
            assert_eq!(parse_lines(&line), vec![r]);
        }
    }

    #[test]
    fn noise_between_results_is_ignored() {
        let r = sample();
        let text = format!(
            "hello\n{}\n\n{{no json\n{}\n",
            r[0].to_line(),
            r[1].to_line()
        );
        assert_eq!(parse_lines(&text), r[..2].to_vec());
    }

    #[test]
    fn the_report_counts_and_escapes_table_cells() {
        let md = render("2026-01-01", &sample(), &[]);
        assert!(md.contains("Passed 1, failed 1, skipped 1."));
        assert!(md.contains("missing / a byte second line"));
        assert!(md.contains("does not cover"));
        assert!(!all_green(&sample()));
        assert!(all_green(&sample()[..1]));
    }

    #[test]
    fn all_skipped_monitors_are_detected() {
        let base = || CheckResult::new("monitor-kill", "pc", &["H3"]);
        assert!(monitors_all_skipped(&[base().skip("x")]));
        assert!(!monitors_all_skipped(&[base().pass("x")]));
        assert!(!monitors_all_skipped(&sample()[..1]));
    }

    #[test]
    fn redact_removes_lan_ips_and_monitor_ids() {
        let t = "connected to 192.168.1.20:52000 monitor=DEL4321-XYZ ok id=abc123, end 1.2.3";
        assert_eq!(
            redact(t),
            "connected to <ip>:52000 monitor=<id> ok id=<id>, end 1.2.3"
        );
        assert!(!redact("10.0.0.5\n172.16.0.1").contains("0.5"));
    }

    #[test]
    fn tail_keeps_the_end_within_the_limits() {
        let text = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let t = tail(&text, 3, 1000);
        assert_eq!(t, "line 97\nline 98\nline 99");
        let t = tail(&text, 100, 20);
        assert!(t.len() <= 20 + '…'.len_utf8());
        assert!(t.ends_with("line 99"));
    }

    #[test]
    fn logs_are_trimmed_in_the_report() {
        let md = render(
            "d",
            &[],
            &[Log {
                name: "pc".into(),
                text: "x\n".repeat(500),
            }],
        );
        assert!(md.matches("x\n").count() <= 41);
    }
}
