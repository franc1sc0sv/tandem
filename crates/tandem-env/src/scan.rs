//! El escaneo de entornos (DESIGN §8): shells, WSL, Docker, herramientas y GPU.
//!
//! Solo lee la salida de programas por [`Runner`]: la misma lógica corre con los programas
//! reales y con las salidas guardadas de las pruebas. Nada supone un equipo (DECISIONS D10):
//! lo que no aparece en el escaneo no se anuncia.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tandem_proto::{EnvKind, Environment, Gpu, Tool};

use crate::Platform;
use crate::parse;
use crate::run::Runner;

/// Cuánto se espera a un `--version`.
const QUICK: Duration = Duration::from_secs(10);
/// Cuánto se espera al guion de un shell de inicio de sesión o de una distro de WSL, que
/// puede tener que arrancarla.
const SLOW: Duration = Duration::from_secs(45);

/// Las herramientas que se buscan dentro de un shell Unix o de una distro de WSL: el nombre
/// del programa y cómo se muestra.
const UNIX_TOOLS: [(&str, &str); 10] = [
    ("git", "git"),
    ("cargo", "cargo"),
    ("rustc", "rustc"),
    ("node", "node"),
    ("pnpm", "pnpm"),
    ("npm", "npm"),
    ("bun", "bun"),
    ("uv", "uv"),
    ("python3", "python"),
    ("nvcc", "CUDA"),
];

/// Las de Windows: `python` (no `python3`) y `winget`.
const WINDOWS_TOOLS: [(&str, &str); 11] = [
    ("git", "git"),
    ("cargo", "cargo"),
    ("rustc", "rustc"),
    ("node", "node"),
    ("pnpm", "pnpm"),
    ("npm", "npm"),
    ("bun", "bun"),
    ("uv", "uv"),
    ("python", "python"),
    ("winget", "winget"),
    ("nvcc", "CUDA"),
];

/// Guion POSIX que corre dentro de un shell de inicio de sesión (o de una distro de WSL):
/// así se ve el `PATH` del usuario (Homebrew, `~/.cargo/bin`, fnm), no el del agente. Sin
/// comillas dobles ni barras invertidas, para que pase intacto por la línea de comandos de
/// Windows hasta `wsl.exe`.
pub const PROBE_SCRIPT: &str = "echo @@os; cat /etc/os-release 2>/dev/null; \
echo @@path; echo $PATH; \
for t in git cargo rustc node pnpm npm bun uv python3 nvcc; do \
if command -v $t >/dev/null 2>&1; then echo @@tool $t; $t --version 2>&1 | head -n 6; fi; done; \
if [ -x /usr/local/cuda/bin/nvcc ]; then echo @@tool nvcc; /usr/local/cuda/bin/nvcc --version 2>&1 | head -n 6; fi; \
if command -v nvidia-smi >/dev/null 2>&1 || [ -e /usr/lib/wsl/lib/nvidia-smi ]; then echo @@gpu; fi";

/// Lo que el guion encontró en un shell o en una distro.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// «Ubuntu 24.04» (de `/etc/os-release`).
    pub os: Option<String>,
    pub path: Option<String>,
    pub tools: Vec<Tool>,
    /// Hay `nvidia-smi` (la GPU llega).
    pub gpu: bool,
}

/// Lee la salida de [`PROBE_SCRIPT`]. `None` si no es suya (el shell no corrió el guion).
pub fn read_probe(text: &str) -> Option<Probe> {
    let sections = parse::sections(text);
    if !sections.iter().any(|(k, _)| k == "path") {
        return None;
    }
    let mut probe = Probe::default();
    for (key, body) in sections {
        match key.split_once(' ') {
            Some(("tool", program)) => {
                let Some(label) = UNIX_TOOLS
                    .iter()
                    .find(|(p, _)| *p == program)
                    .map(|(_, l)| *l)
                else {
                    continue;
                };
                add_tool(&mut probe.tools, label, &body);
            }
            _ => match key.as_str() {
                "os" => probe.os = parse::os_release_name(&body),
                "path" => probe.path = Some(body.trim().to_owned()).filter(|p| !p.is_empty()),
                "gpu" => probe.gpu = true,
                _ => {}
            },
        }
    }
    Some(probe)
}

/// Agrega la herramienta si la salida trae una versión y no estaba ya.
fn add_tool(tools: &mut Vec<Tool>, label: &str, output: &str) {
    if tools.iter().any(|t| t.name == label) {
        return;
    }
    if let Some(version) = parse::version_token(output) {
        tools.push(Tool {
            name: label.to_owned(),
            version,
        });
    }
}

/// La línea bajo el nombre de un entorno con herramientas («uv · cargo · CUDA 12.8 · git»).
pub fn tools_line(tools: &[Tool]) -> String {
    if tools.is_empty() {
        return "Sin herramientas de desarrollo detectadas".into();
    }
    tools
        .iter()
        .map(|t| {
            if t.name == "CUDA" {
                format!("CUDA {}", t.version)
            } else {
                t.name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Por dónde se leen los recursos de la GPU en vivo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuSource {
    NvidiaSmi(PathBuf),
    Ioreg(PathBuf),
    None,
}

/// Lo que encontró un escaneo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub environments: Vec<Environment>,
    pub gpus: Vec<Gpu>,
    pub gpu_source: GpuSource,
}

/// Escanea esta máquina con `runner`. No falla: lo que no responde no aparece (o aparece
/// como no listo, con la razón).
pub fn scan(runner: &dyn Runner, platform: Platform) -> Found {
    match platform {
        Platform::Windows => scan_windows(runner),
        Platform::MacOs | Platform::Linux => scan_unix(runner, platform),
    }
}

fn run_text(
    runner: &dyn Runner,
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Option<String> {
    match runner.run(program, args, timeout) {
        Ok(out) if out.success => Some(out.text()),
        Ok(out) => {
            tracing::debug!(
                programa = %program.display(),
                error = %parse::decode_text(&out.stderr).trim(),
                "salió con error"
            );
            None
        }
        Err(e) => {
            tracing::debug!("{e}");
            None
        }
    }
}

/// El nombre de un shell para la UI.
fn shell_name(id: &str, version: Option<&str>) -> String {
    match id {
        "pwsh" => match version.and_then(|v| v.split('.').next()) {
            Some(major) => format!("PowerShell {major}"),
            None => "PowerShell".into(),
        },
        "powershell" => match version {
            Some(v) => format!(
                "Windows PowerShell {}",
                v.split('.').take(2).collect::<Vec<_>>().join(".")
            ),
            None => "Windows PowerShell".into(),
        },
        other => other.to_owned(),
    }
}

fn shell(id: &str, version: Option<String>, tools: &[Tool]) -> Environment {
    Environment {
        id: id.to_owned(),
        kind: EnvKind::Shell,
        name: shell_name(id, version.as_deref()),
        version,
        default: false,
        ready: true,
        detail: tools_line(tools),
        tools: tools.to_vec(),
        gpu: false,
    }
}

/// El shell de inicio de sesión del usuario: el de `$SHELL` o el primero que haya.
fn login_shell(runner: &dyn Runner) -> Option<PathBuf> {
    if let Some(sh) = runner.var("SHELL") {
        let path = PathBuf::from(sh);
        // fish no es POSIX: el guion va por sh, que también lee el perfil.
        if path.file_stem().is_some_and(|s| s != "fish") {
            return Some(path);
        }
    }
    ["zsh", "bash", "sh"]
        .iter()
        .find_map(|s| runner.find(s, None))
}

/// Corre [`PROBE_SCRIPT`] en `program` primero como shell interactivo (fnm y nvm se cargan
/// desde el rc) y, si no salió, solo de inicio de sesión.
fn probe_in(runner: &dyn Runner, program: &Path, prefix: &[&str]) -> Option<Probe> {
    ["-lic", "-lc"].iter().find_map(|flags| {
        let mut args = prefix.to_vec();
        args.extend([*flags, PROBE_SCRIPT]);
        let out = runner.run(program, &args, SLOW).ok()?;
        read_probe(&out.text())
    })
}

fn scan_unix(runner: &dyn Runner, platform: Platform) -> Found {
    let probe = login_shell(runner)
        .and_then(|sh| probe_in(runner, &sh, &[]))
        .unwrap_or_default();
    let path = probe.path.as_deref();
    let mut environments = Vec::new();
    for id in ["zsh", "bash", "fish", "pwsh"] {
        let Some(program) = runner.find(id, path) else {
            continue;
        };
        let version = run_text(runner, &program, &["--version"], QUICK)
            .and_then(|t| parse::version_token(&t));
        environments.push(shell(id, version, &probe.tools));
    }
    let default = runner.var("SHELL").and_then(|s| {
        Path::new(&s)
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
    });
    mark_default(&mut environments, default.as_deref());
    environments.extend(docker(runner, path));
    let (gpus, gpu_source) = match platform {
        Platform::MacOs => mac_gpus(runner),
        _ => nvidia(runner, path),
    };
    Found {
        environments,
        gpus,
        gpu_source,
    }
}

/// El shell `id` (o, si no está, el primero) queda como predeterminado.
fn mark_default(shells: &mut [Environment], id: Option<&str>) {
    let i = id
        .and_then(|id| shells.iter().position(|e| e.id == id))
        .unwrap_or(0);
    if let Some(e) = shells.get_mut(i) {
        e.default = true;
    }
}

fn scan_windows(runner: &dyn Runner) -> Found {
    let mut tools = Vec::new();
    for (program, label) in WINDOWS_TOOLS {
        if let Some(path) = runner.find(program, None)
            && let Some(text) = run_text(runner, &path, &["--version"], QUICK)
        {
            add_tool(&mut tools, label, &text);
        }
    }
    let mut environments = Vec::new();
    let powershell = runner.find("powershell", None);
    let shells: [(&str, &[&str]); 4] = [
        ("pwsh", &["--version"]),
        (
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$PSVersionTable.PSVersion.ToString()",
            ],
        ),
        ("cmd", &["/d", "/c", "ver"]),
        ("bash", &["--version"]),
    ];
    for (id, args) in shells {
        let found = if id == "bash" {
            // El bash de System32 (o de WindowsApps) es el lanzador de WSL, no un shell de
            // Windows: vale el primero que no sea ese, aunque esté después en el PATH.
            runner.find_all(id).into_iter().find(|p| {
                let lower = p.to_string_lossy().to_ascii_lowercase();
                !lower.contains("system32") && !lower.contains("windowsapps")
            })
        } else {
            runner.find(id, None)
        };
        let Some(program) = found else {
            continue;
        };
        let version =
            run_text(runner, &program, args, QUICK).and_then(|t| parse::version_token(&t));
        let mut env = shell(id, version, &tools);
        if id == "bash" {
            env.name = "Git Bash".into();
        }
        environments.push(env);
    }
    // PowerShell 7 si está; si no, el 5 que trae Windows.
    let default = ["pwsh", "powershell"]
        .into_iter()
        .find(|id| environments.iter().any(|e| e.id == *id));
    mark_default(&mut environments, default);
    environments.extend(wsl(runner));
    environments.extend(docker(runner, None));
    let (mut gpus, gpu_source) = nvidia(runner, None);
    if let Some(ps) = powershell {
        let cim = run_text(
            runner,
            &ps,
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-CimInstance Win32_VideoController | Select-Object Name,AdapterRAM,DriverVersion | ConvertTo-Json",
            ],
            QUICK,
        );
        // Las que nvidia-smi ya describió (con su VRAM real) no se repiten.
        for g in cim.map(|t| parse::cim_gpus(&t)).unwrap_or_default() {
            if !gpus.iter().any(|n| n.name.eq_ignore_ascii_case(&g.name)) {
                gpus.push(g);
            }
        }
    }
    Found {
        environments,
        gpus,
        gpu_source,
    }
}

/// Las distros de WSL, cada una con lo que tiene adentro. Correr el guion arranca la distro
/// si estaba detenida; WSL la vuelve a apagar sola al rato.
fn wsl(runner: &dyn Runner) -> Vec<Environment> {
    let Some(wsl) = runner.find("wsl", None) else {
        return Vec::new();
    };
    let Some(list) = run_text(runner, &wsl, &["-l", "-v"], QUICK) else {
        return Vec::new();
    };
    parse::wsl_list(&list)
        .into_iter()
        .map(|d| {
            let probe = probe_in(runner, &wsl, &["-d", &d.name, "-e", "bash"])
                .or_else(|| probe_in(runner, &wsl, &["-d", &d.name, "-e", "sh"]));
            let (name, detail, tools, gpu) = match probe {
                Some(p) => (
                    p.os.unwrap_or_else(|| d.name.clone()),
                    tools_line(&p.tools),
                    p.tools,
                    p.gpu,
                ),
                None => (
                    d.name.clone(),
                    "No respondió al escaneo".to_owned(),
                    Vec::new(),
                    false,
                ),
            };
            Environment {
                id: format!("wsl:{}", d.name),
                kind: EnvKind::Wsl,
                name,
                version: Some(d.wsl.to_string()),
                default: d.default,
                ready: true,
                detail,
                tools,
                gpu,
            }
        })
        .collect()
}

fn docker(runner: &dyn Runner, path: Option<&str>) -> Option<Environment> {
    let docker = runner.find("docker", path)?;
    // Con el daemon caído sale con error pero igual escribe la parte del cliente.
    let version = runner
        .run(&docker, &["version", "--format", "json"], QUICK)
        .ok()
        .and_then(|o| parse::docker_version(&o.text()));
    let Some(version) = version else {
        return Some(Environment {
            id: "docker".into(),
            kind: EnvKind::Docker,
            name: "Docker".into(),
            version: None,
            default: false,
            ready: false,
            detail: "No respondió a «docker version»".into(),
            tools: Vec::new(),
            gpu: false,
        });
    };
    let Some(server) = version.server else {
        return Some(Environment {
            id: "docker".into(),
            kind: EnvKind::Docker,
            name: "Docker".into(),
            version: version.client,
            default: false,
            ready: false,
            detail: "El daemon no responde: ¿Docker está abierto?".into(),
            tools: Vec::new(),
            gpu: false,
        });
    };
    let gpu = run_text(
        runner,
        &docker,
        &[
            "info",
            "--format",
            "{{range $k, $v := .Runtimes}}{{$k}} {{end}}",
        ],
        QUICK,
    )
    .is_some_and(|r| parse::docker_has_gpu(&r));
    let detail = if gpu {
        "GPU: --gpus all disponible".to_owned()
    } else {
        match &server.arch {
            Some(arch) => format!("{arch} · sin GPU"),
            None => "sin GPU".to_owned(),
        }
    };
    Some(Environment {
        id: "docker".into(),
        kind: EnvKind::Docker,
        name: server.platform.unwrap_or_else(|| "Docker".into()),
        version: server.version.or(version.client),
        default: false,
        ready: true,
        detail,
        tools: Vec::new(),
        gpu,
    })
}

/// Los argumentos de `nvidia-smi` para el escaneo y para cada muestra.
pub fn nvidia_args() -> [&'static str; 2] {
    [parse::NVIDIA_QUERY, "--format=csv,noheader,nounits"]
}

fn nvidia(runner: &dyn Runner, path: Option<&str>) -> (Vec<Gpu>, GpuSource) {
    let Some(smi) = runner.find("nvidia-smi", path) else {
        return (Vec::new(), GpuSource::None);
    };
    match run_text(runner, &smi, &nvidia_args(), QUICK) {
        Some(text) => {
            let gpus: Vec<Gpu> = parse::nvidia_smi(&text)
                .into_iter()
                .map(|g| g.gpu)
                .collect();
            let source = if gpus.is_empty() {
                GpuSource::None
            } else {
                GpuSource::NvidiaSmi(smi)
            };
            (gpus, source)
        }
        None => (Vec::new(), GpuSource::None),
    }
}

fn mac_gpus(runner: &dyn Runner) -> (Vec<Gpu>, GpuSource) {
    let gpus = runner
        .find("system_profiler", Some("/usr/sbin"))
        .and_then(|sp| run_text(runner, &sp, &["SPDisplaysDataType", "-json"], SLOW))
        .map(|t| parse::system_profiler_gpus(&t))
        .unwrap_or_default();
    let source = match runner.find("ioreg", Some("/usr/sbin")) {
        Some(ioreg) => GpuSource::Ioreg(ioreg),
        None => GpuSource::None,
    };
    (gpus, source)
}

/// Los argumentos de `ioreg` para el uso de la GPU en macOS.
pub const IOREG_ARGS: [&str; 5] = ["-r", "-d", "1", "-c", "IOAccelerator"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_script_survives_the_windows_command_line() {
        assert!(!PROBE_SCRIPT.contains('"'));
        assert!(!PROBE_SCRIPT.contains('\\'));
        for (program, _) in UNIX_TOOLS {
            assert!(PROBE_SCRIPT.contains(program), "{program}");
        }
    }

    #[test]
    fn output_without_the_markers_is_not_a_probe() {
        assert_eq!(read_probe("zsh: command not found"), None);
        assert_eq!(
            read_probe("@@os\n@@path\n\n"),
            Some(Probe {
                os: None,
                path: None,
                tools: vec![],
                gpu: false
            })
        );
    }

    #[test]
    fn a_tool_without_a_version_is_skipped_and_duplicates_too() {
        let probe = read_probe(
            "@@path\n/bin\n@@tool python3\nxcode-select: note: No developer tools were found\n\
             @@tool nvcc\nCuda compilation tools, release 12.8, V12.8.93\n\
             @@tool nvcc\nCuda compilation tools, release 12.4, V12.4.1\n@@tool raro\n1.0\n",
        )
        .unwrap();
        assert_eq!(
            probe.tools,
            [Tool {
                name: "CUDA".into(),
                version: "12.8".into()
            }]
        );
        assert_eq!(tools_line(&probe.tools), "CUDA 12.8");
        assert_eq!(tools_line(&[]), "Sin herramientas de desarrollo detectadas");
    }

    #[test]
    fn shell_names_come_from_the_version() {
        assert_eq!(shell_name("pwsh", Some("7.5.2")), "PowerShell 7");
        assert_eq!(
            shell_name("powershell", Some("5.1.26100.9818")),
            "Windows PowerShell 5.1"
        );
        assert_eq!(shell_name("zsh", Some("5.9")), "zsh");
        assert_eq!(shell_name("pwsh", None), "PowerShell");
    }
}
