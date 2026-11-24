//! Reading the output of each tool. Pure functions: tested with the real outputs
//! saved in `tests/fixtures/`.

use serde_json::Value;
use tandem_proto::{Gpu, GpuLoad};

/// Text of an output: UTF-16LE if it looks like it (`wsl.exe` without `WSL_UTF8`), otherwise UTF-8 with
/// invalid bytes replaced. Without BOM and without `\r`.
pub fn decode_text(bytes: &[u8]) -> String {
    let utf16 = bytes.starts_with(&[0xFF, 0xFE])
        // ASCII in UTF-16LE: a zero in every odd byte.
        || (bytes.len() >= 4 && bytes.len().is_multiple_of(2) && bytes[1] == 0 && bytes[3] == 0);
    let text = if utf16 {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.trim_start_matches('\u{feff}').replace('\r', "")
}

/// The first version (`1.2`, `5.2.21`, `v24.1.0`) that appears in the text: only the numbers
/// and dots at the start of the word (`2.51.0.windows.1` → `2.51.0`). `None` if there is
/// none, as in the "not installed" notices of macOS and the Windows store.
pub fn version_token(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == ',' || c == '[' || c == ']')
        .map(|w| {
            w.strip_prefix('v')
                .or_else(|| w.strip_prefix('V'))
                .unwrap_or(w)
        })
        .filter_map(|w| {
            let end = w
                .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                .unwrap_or(w.len());
            let v = w[..end].trim_end_matches('.');
            let mut parts = v.split('.');
            let ok = v.contains('.')
                && parts.all(|p| !p.is_empty())
                && v.starts_with(|c: char| c.is_ascii_digit());
            ok.then(|| v.to_owned())
        })
        .next()
}

/// A distro from `wsl.exe -l -v`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslDistro {
    pub name: String,
    pub running: bool,
    /// 1 or 2.
    pub wsl: u8,
    pub default: bool,
}

/// `wsl.exe -l -v`: the first line is the header (translated according to the Windows
/// language); each distro is `[*] NAME STATE VERSION`. Docker Desktop's internal distros
/// are not user environments.
pub fn wsl_list(text: &str) -> Vec<WslDistro> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .skip(1)
        .filter_map(|line| {
            let line = line.trim();
            let (default, rest) = match line.strip_prefix('*') {
                Some(rest) => (true, rest.trim_start()),
                None => (false, line),
            };
            let cols: Vec<&str> = rest.split_whitespace().collect();
            let [name, state, version] = cols.as_slice() else {
                return None;
            };
            let wsl = version.parse().ok()?;
            Some(WslDistro {
                name: (*name).to_owned(),
                running: state.eq_ignore_ascii_case("running"),
                wsl,
                default,
            })
        })
        .filter(|d| !d.name.starts_with("docker-desktop"))
        .collect()
}

/// "Ubuntu 24.04" from `/etc/os-release`: `NAME` and `VERSION_ID`, or `PRETTY_NAME`.
pub fn os_release_name(text: &str) -> Option<String> {
    let field = |key: &str| {
        text.lines().find_map(|l| {
            let v = l.strip_prefix(key)?.strip_prefix('=')?;
            let v = v.trim().trim_matches('"').trim_matches('\'').trim();
            (!v.is_empty()).then(|| v.to_owned())
        })
    };
    match (field("NAME"), field("VERSION_ID")) {
        (Some(name), Some(version)) => Some(format!("{name} {version}")),
        (Some(name), None) => field("PRETTY_NAME").or(Some(name)),
        (None, _) => field("PRETTY_NAME"),
    }
}

/// What `docker version --format json` said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerVersion {
    pub client: Option<String>,
    /// `None` if the daemon did not respond.
    pub server: Option<DockerServer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerServer {
    /// "Docker Desktop", "Docker Engine - Community"...
    pub platform: Option<String>,
    pub version: Option<String>,
    pub arch: Option<String>,
}

/// `docker version --format json`. With the daemon down the CLI still writes the
/// client part (and `"Server": null`) and exits with an error.
pub fn docker_version(text: &str) -> Option<DockerVersion> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    let s = |v: &Value| v.as_str().map(str::to_owned).filter(|s| !s.is_empty());
    let server = v.get("Server").filter(|s| s.is_object()).map(|srv| {
        let platform = srv
            .pointer("/Platform/Name")
            .and_then(s)
            .map(|p| strip_build(&p));
        DockerServer {
            platform,
            version: srv.get("Version").and_then(s),
            arch: srv.get("Arch").and_then(s),
        }
    });
    Some(DockerVersion {
        client: v.pointer("/Client/Version").and_then(s),
        server,
    })
}

/// "Docker Desktop 4.92.0 (240144)" → "Docker Desktop".
fn strip_build(name: &str) -> String {
    let words: Vec<&str> = name
        .split_whitespace()
        .take_while(|w| !w.starts_with(|c: char| c.is_ascii_digit() || c == '('))
        .collect();
    if words.is_empty() {
        name.trim().to_owned()
    } else {
        words.join(" ")
    }
}

/// `docker info --format '{{range $k, $v := .Runtimes}}{{$k}} {{end}}'`: the NVIDIA GPU
/// reaches the containers if the `nvidia` runtime is there.
pub fn docker_has_gpu(runtimes: &str) -> bool {
    runtimes.split_whitespace().any(|r| r == "nvidia")
}

/// The columns requested from `nvidia-smi`, in this order.
pub const NVIDIA_QUERY: &str =
    "--query-gpu=name,memory.total,driver_version,memory.used,utilization.gpu,temperature.gpu";

/// A GPU according to `nvidia-smi` ([`NVIDIA_QUERY`], `--format=csv,noheader,nounits`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvidiaGpu {
    pub gpu: Gpu,
    pub load: GpuLoad,
}

const MIB: u64 = 1 << 20;

/// One line per GPU; values the card does not give come as `[N/A]`.
pub fn nvidia_smi(text: &str) -> Vec<NvidiaGpu> {
    text.lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split(',').map(str::trim).collect();
            let [name, total, driver, used, util, temp] = cols.as_slice() else {
                return None;
            };
            if name.is_empty() {
                return None;
            }
            let number = |s: &str| s.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0);
            // `as` saturates on absurd values; the product has to saturate too.
            let total = number(total).map(|m| (m as u64).saturating_mul(MIB));
            Some(NvidiaGpu {
                gpu: Gpu {
                    name: (*name).to_owned(),
                    vram: total,
                    driver: (!driver.starts_with('[') && !driver.is_empty())
                        .then(|| (*driver).to_owned()),
                },
                load: GpuLoad {
                    name: (*name).to_owned(),
                    load: number(util).map(|p| tenths(p as f32)),
                    vram_used: number(used).map(|m| (m as u64).saturating_mul(MIB)),
                    vram_total: total,
                    temperature: number(temp).map(|t| t.round() as i16),
                },
            })
        })
        .collect()
}

/// Percentage (0–100) to tenths, clamped.
pub fn tenths(percent: f32) -> u16 {
    if percent.is_finite() {
        (percent.clamp(0.0, 100.0) * 10.0).round() as u16
    } else {
        0
    }
}

/// `Get-CimInstance Win32_VideoController | Select Name,AdapterRAM,DriverVersion |
/// ConvertTo-Json`: one object if there is a single one, an array if there are several. `AdapterRAM` is
/// 32 bits: what reaches ~4 GiB is clipped and not used.
pub fn cim_gpus(text: &str) -> Vec<Gpu> {
    let Ok(v) = serde_json::from_str::<Value>(text.trim()) else {
        return Vec::new();
    };
    let items = match v {
        Value::Array(items) => items,
        Value::Object(_) => vec![v],
        _ => Vec::new(),
    };
    items
        .iter()
        .filter_map(|g| {
            let name = g.get("Name")?.as_str()?.trim().to_owned();
            // Adapters that are not a card (remote desktop, the basic Windows one).
            if name.is_empty() || name.starts_with("Microsoft ") {
                return None;
            }
            let vram = g
                .get("AdapterRAM")
                .and_then(Value::as_u64)
                .filter(|&b| b > 0 && b < 0xFFF0_0000);
            let driver = g
                .get("DriverVersion")
                .and_then(Value::as_str)
                .map(str::to_owned);
            Some(Gpu { name, vram, driver })
        })
        .collect()
}

/// `system_profiler SPDisplaysDataType -json`. On Apple Silicon the GPU shares RAM (no
/// `vram`); on an Intel Mac the discrete one says `spdisplays_vram` ("4 GB").
pub fn system_profiler_gpus(text: &str) -> Vec<Gpu> {
    let Ok(v) = serde_json::from_str::<Value>(text.trim()) else {
        return Vec::new();
    };
    let Some(items) = v.get("SPDisplaysDataType").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|g| {
            let name = g
                .get("sppci_model")
                .or_else(|| g.get("_name"))?
                .as_str()?
                .trim()
                .to_owned();
            let vram = ["spdisplays_vram", "_spdisplays_vram"]
                .iter()
                .find_map(|k| g.get(*k).and_then(Value::as_str))
                .and_then(size_text);
            Some(Gpu {
                name,
                vram,
                driver: None,
            })
        })
        .collect()
}

/// "4 GB", "1536 MB" → bytes.
fn size_text(s: &str) -> Option<u64> {
    let mut words = s.split_whitespace();
    let n: f64 = words.next()?.parse().ok()?;
    let unit = match words.next()?.to_ascii_uppercase().as_str() {
        "GB" => 1u64 << 30,
        "MB" => 1 << 20,
        _ => return None,
    };
    Some((n * unit as f64) as u64)
}

/// `ioreg -r -d 1 -c IOAccelerator`: each accelerator with its model and its
/// `PerformanceStatistics` (device utilization and memory in use). macOS does not give
/// the temperature without private APIs.
pub fn ioreg_gpus(text: &str) -> Vec<GpuLoad> {
    text.split("+-o ")
        .filter_map(|block| {
            let name = quoted_after(block, "\"model\" = ")?;
            let stat = |key: &str| -> Option<u64> {
                let i = block.find(&format!("\"{key}\"="))? + key.len() + 3;
                let digits: String = block[i..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                digits.parse().ok()
            };
            Some(GpuLoad {
                name,
                load: stat("Device Utilization %").map(|p| tenths(p as f32)),
                vram_used: stat("In use system memory"),
                vram_total: None,
                temperature: None,
            })
        })
        .collect()
}

fn quoted_after(text: &str, key: &str) -> Option<String> {
    let rest = &text[text.find(key)? + key.len()..];
    // As text (`"Apple M4 Pro"`) or as data (`<"AMD Radeon Pro 5500M">`).
    let rest = rest.strip_prefix('<').unwrap_or(rest).strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_owned())
}

/// The `@@key rest` sections of a Unix shell's scan script ([`crate::scan`]).
pub fn sections(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("@@") {
            out.push((head.trim().to_owned(), String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn versions_are_found_in_each_tool_output() {
        let cases = [
            ("git version 2.43.0", Some("2.43.0")),
            ("git version 2.51.0.windows.1", Some("2.51.0")),
            ("cargo 1.98.1 (797e8a9bc 2026-08-05)", Some("1.98.1")),
            ("v24.21.0", Some("24.21.0")),
            ("uv 0.12.20 (x86_64-unknown-linux-gnu)", Some("0.12.20")),
            ("Python 3.12.3", Some("3.12.3")),
            (
                "GNU bash, version 5.2.21(1)-release (x86_64-pc-linux-gnu)",
                Some("5.2.21"),
            ),
            ("zsh 5.9 (arm64-apple-darwin24.0)", Some("5.9")),
            ("fish, version 3.7.1", Some("3.7.1")),
            ("PowerShell 7.5.2", Some("7.5.2")),
            (
                "Microsoft Windows [Version 10.0.26200.9974]",
                Some("10.0.26200.9974"),
            ),
            (
                "nvcc: NVIDIA (R) Cuda compiler driver\nCopyright (c) 2005-2025 NVIDIA Corporation\nCuda compilation tools, release 12.8, V12.8.93",
                Some("12.8"),
            ),
            (
                "Python was not found; run without arguments to install from the Microsoft Store, or disable this shortcut from Settings > Manage App Execution Aliases.",
                None,
            ),
            (
                "xcode-select: note: No developer tools were found, requesting install.",
                None,
            ),
            ("", None),
            ("12", None),
        ];
        for (text, want) in cases {
            assert_eq!(version_token(text).as_deref(), want, "{text}");
        }
    }

    #[test]
    fn utf16_and_crlf_are_decoded() {
        let utf16: Vec<u8> = "  NAME\r\n* Ubuntu\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(decode_text(&utf16), "  NAME\n* Ubuntu\n");
        assert_eq!(decode_text(b"\xEF\xBB\xBFhello\r\n"), "hello\n");
        assert_eq!(decode_text(b""), "");
    }

    #[test]
    fn the_wsl_list_skips_the_translated_header_and_docker() {
        // A Spanish-localized `wsl -l -v` header: the Spanish words are the parser's input.
        let text = "  NOMBRE            ESTADO          VERSIÓN\n\
                    * Ubuntu-24.04      Running         2\n  \
                      Debian            Stopped         1\n  \
                      docker-desktop    Running         2\n";
        assert_eq!(
            wsl_list(text),
            [
                WslDistro {
                    name: "Ubuntu-24.04".into(),
                    running: true,
                    wsl: 2,
                    default: true,
                },
                WslDistro {
                    name: "Debian".into(),
                    running: false,
                    wsl: 1,
                    default: false,
                },
            ]
        );
        let none = "Windows Subsystem for Linux has no installed distributions.\nUse 'wsl.exe --list --online' to list available distributions.";
        assert!(wsl_list(none).is_empty());
    }

    #[test]
    fn os_release_gives_name_and_version() {
        let ubuntu = "PRETTY_NAME=\"Ubuntu 24.04.3 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\n";
        assert_eq!(os_release_name(ubuntu).as_deref(), Some("Ubuntu 24.04"));
        let arch = "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\n";
        assert_eq!(os_release_name(arch).as_deref(), Some("Arch Linux"));
        assert_eq!(os_release_name("").as_deref(), None);
    }

    #[test]
    fn docker_without_daemon_keeps_the_client() {
        let text = r#"{"Client":{"Version":"29.8.0","Os":"darwin","Arch":"arm64"},"Server":null}"#;
        assert_eq!(
            docker_version(text),
            Some(DockerVersion {
                client: Some("29.8.0".into()),
                server: None,
            })
        );
        assert_eq!(docker_version("Cannot connect to the Docker daemon"), None);
        assert!(docker_has_gpu("io.containerd.runc.v2 nvidia runc "));
        assert!(!docker_has_gpu("io.containerd.runc.v2 runc "));
    }

    #[test]
    fn nvidia_smi_handles_missing_values() {
        let text = "NVIDIA GeForce GTX 1650, 4096, 537.13, [N/A], [N/A], 41\n\n";
        let [g] = nvidia_smi(text).try_into().unwrap();
        assert_eq!(g.gpu.vram, Some(4096 * MIB));
        assert_eq!(g.load.vram_used, None);
        assert_eq!(g.load.load, None);
        assert_eq!(g.load.temperature, Some(41));
        assert!(nvidia_smi("No devices were found").is_empty());
    }

    proptest! {
        /// Absurd numbers in the columns do not overflow.
        #[test]
        fn nvidia_smi_never_overflows(total in any::<u64>(), used in any::<f64>(), temp in any::<f64>()) {
            let text = format!("GPU, {total}, 1.0, {used}, 50, {temp}");
            let [g] = nvidia_smi(&text).try_into().unwrap();
            prop_assert!(g.gpu.vram.is_some());
        }
    }

    #[test]
    fn sections_split_the_scan_script_output() {
        let text = "garbage\n@@os\nNAME=x\n@@tool git\ngit version 2.43.0\n@@gpu\n";
        assert_eq!(
            sections(text),
            [
                ("os".into(), "NAME=x\n".into()),
                ("tool git".into(), "git version 2.43.0\n".into()),
                ("gpu".into(), String::new()),
            ]
        );
    }

    proptest! {
        #[test]
        fn tenths_never_leave_the_range(p in proptest::num::f32::ANY) {
            prop_assert!(tenths(p) <= 1000);
        }

        #[test]
        fn parsers_never_panic(s in ".{0,200}") {
            let _ = version_token(&s);
            let _ = wsl_list(&s);
            let _ = os_release_name(&s);
            let _ = docker_version(&s);
            let _ = nvidia_smi(&s);
            let _ = cim_gpus(&s);
            let _ = system_profiler_gpus(&s);
            let _ = ioreg_gpus(&s);
            let _ = sections(&s);
        }

        #[test]
        fn decoding_never_panics(b in proptest::collection::vec(any::<u8>(), 0..200)) {
            let _ = decode_text(&b);
        }
    }
}
