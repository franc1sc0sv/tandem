//! The scan and the sampling with the real outputs of each tool (`tests/fixtures/`),
//! on any system: the [`Platform`] is chosen by hand.

use std::sync::Arc;

use tandem_env::fake::{Answer, FakeMeter, FakeRunner};
use tandem_env::meter::DiskReading;
use tandem_env::scan::PROBE_SCRIPT;
use tandem_env::{Host, MeterInfo, Output, Platform, Reading};
use tandem_proto::{EnvKind, Environment, Gpu, GpuLoad, Tool};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

const CIM_COMMAND: &str = "Get-CimInstance Win32_VideoController | Select-Object Name,AdapterRAM,DriverVersion | ConvertTo-Json";

fn tool(name: &str, version: &str) -> Tool {
    Tool {
        name: name.into(),
        version: version.into(),
    }
}

fn find<'a>(envs: &'a [Environment], id: &str) -> &'a Environment {
    envs.iter()
        .find(|e| e.id == id)
        .unwrap_or_else(|| panic!("{id} missing in {envs:#?}"))
}

fn meter() -> FakeMeter {
    let m = FakeMeter::new(MeterInfo {
        cpu: Some("AMD Ryzen 7 9800X3D 8-Core Processor".into()),
        cores: 16,
        memory: 64 * GIB,
    });
    m.push(Reading {
        cpu: 64.04,
        cores: vec![100.0, 28.0],
        temperatures: vec![("Tctl".into(), 61.6)],
        memory_used: 17 * GIB,
        memory_total: 64 * GIB,
        disks: vec![
            DiskReading {
                mount: "C:\\".into(),
                free: 412 * GIB,
                total: 1800 * GIB,
            },
            DiskReading {
                mount: "D:\\".into(),
                free: 1,
                total: 4000 * GIB,
            },
        ],
    });
    m
}

/// The PC: PowerShell 5 (without PowerShell 7), cmd, the System32 bash (which is WSL), a WSL distro,
/// Docker Desktop with the NVIDIA runtime and two GPUs.
fn windows() -> FakeRunner {
    let r = FakeRunner::new();
    let tools = tandem_env::parse::decode_text(include_bytes!("fixtures/windows-tools.txt"));
    for (name, body) in tandem_env::parse::sections(&tools) {
        r.on(&name, &["--version"], body);
    }
    r.answer(
        "python",
        &["--version"],
        Answer::Output(Output::failed(
            "",
            "Python was not found; run without arguments to install from the Microsoft Store, or disable this shortcut from Settings > Manage App Execution Aliases.",
        )),
    );
    r.on(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$PSVersionTable.PSVersion.ToString()",
        ],
        include_bytes!("fixtures/windows-powershell5-version.txt").as_slice(),
    );
    r.on(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", CIM_COMMAND],
        include_bytes!("fixtures/windows-cim-gpu.json").as_slice(),
    );
    r.on(
        "cmd",
        &["/d", "/c", "ver"],
        include_bytes!("fixtures/windows-cmd-ver.txt").as_slice(),
    );
    r.install("bash", r"C:\Windows\System32\bash.exe");
    r.on(
        "wsl",
        &["-l", "-v"],
        include_bytes!("fixtures/windows-wsl-list.utf16").as_slice(),
    );
    r.on(
        "wsl",
        &["-d", "Ubuntu-24.04", "-e", "bash", "-lic", PROBE_SCRIPT],
        include_bytes!("fixtures/windows-wsl-ubuntu-probe.txt").as_slice(),
    );
    r.on(
        "docker",
        &["version"],
        include_bytes!("fixtures/windows-docker-version.json").as_slice(),
    );
    r.on(
        "docker",
        &["info"],
        include_bytes!("fixtures/windows-docker-runtimes.txt").as_slice(),
    );
    r.on(
        "nvidia-smi",
        &[],
        include_bytes!("fixtures/windows-nvidia-smi.csv").as_slice(),
    );
    r.set_var("SystemDrive", "C:");
    r
}

#[test]
fn windows_finds_shells_wsl_docker_and_gpus() {
    let host = Host::new(Platform::Windows, Arc::new(windows()), Box::new(meter()));
    let scan = host.scan();
    let envs = &scan.environments;
    let ids: Vec<&str> = envs.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["powershell", "cmd", "wsl:Ubuntu-24.04", "docker"]);

    let ps = find(envs, "powershell");
    assert_eq!(ps.name, "Windows PowerShell 5.1");
    assert_eq!(ps.version.as_deref(), Some("5.1.26100.9818"));
    assert!(ps.default && ps.ready);
    assert_eq!(
        ps.tools,
        [
            tool("git", "2.55.0"),
            tool("node", "24.21.0"),
            tool("npm", "12.0.2"),
            tool("uv", "0.12.12"),
            tool("winget", "1.29.380"),
            tool("CUDA", "13.4"),
        ]
    );
    assert_eq!(ps.detail, "git · node · npm · uv · winget · CUDA 13.4");
    let cmd = find(envs, "cmd");
    assert_eq!(cmd.version.as_deref(), Some("10.0.26200.9974"));
    assert!(!cmd.default);

    let ubuntu = find(envs, "wsl:Ubuntu-24.04");
    assert_eq!(ubuntu.kind, EnvKind::Wsl);
    assert_eq!(ubuntu.name, "Ubuntu 24.04");
    assert_eq!(ubuntu.version.as_deref(), Some("2"));
    assert!(ubuntu.default && ubuntu.gpu);
    assert_eq!(
        ubuntu.detail,
        "git · cargo · rustc · node · pnpm · npm · uv · python"
    );
    assert_eq!(ubuntu.tools[0], tool("git", "2.43.0"));

    let docker = find(envs, "docker");
    assert_eq!(docker.name, "Docker Desktop");
    assert_eq!(docker.version.as_deref(), Some("29.8.0"));
    assert!(docker.ready && docker.gpu);
    assert_eq!(docker.detail, "GPU: --gpus all available");

    assert_eq!(scan.hardware.cores, 16);
    assert_eq!(scan.hardware.memory, 64 * GIB);
    assert_eq!(
        scan.hardware.gpus,
        [
            Gpu {
                name: "NVIDIA GeForce RTX 5080".into(),
                vram: Some(16303 * MIB),
                driver: Some("621.01".into()),
            },
            Gpu {
                name: "AMD Radeon(TM) Graphics".into(),
                vram: Some(512 * MIB),
                driver: Some("32.0.21045.16010".into()),
            },
        ]
    );

    let s = host.sample();
    assert_eq!(s.cpu.load, 640);
    assert_eq!(s.cpu.cores, [1000, 280]);
    assert_eq!(s.cpu.temperature, Some(62));
    assert_eq!(s.memory.used, 17 * GIB);
    assert_eq!(s.disk.map(|d| d.free), Some(412 * GIB));
    assert_eq!(
        s.gpus,
        [GpuLoad {
            name: "NVIDIA GeForce RTX 5080".into(),
            load: Some(0),
            vram_used: Some(1260 * MIB),
            vram_total: Some(16303 * MIB),
            temperature: Some(38),
        }]
    );
}

#[test]
fn powershell_7_is_the_default_when_installed() {
    let r = windows();
    r.on("pwsh", &["--version"], "PowerShell 7.5.2\r\n");
    let scan = Host::new(Platform::Windows, Arc::new(r), Box::new(meter())).scan();
    let pwsh = find(&scan.environments, "pwsh");
    assert_eq!(pwsh.name, "PowerShell 7");
    assert!(pwsh.default);
    assert!(!find(&scan.environments, "powershell").default);
}

#[test]
fn git_bash_counts_as_a_shell_but_the_wsl_launcher_does_not() {
    let r = windows();
    // The WSL launcher comes earlier in the PATH; Git Bash is found anyway.
    r.install_also("bash", r"C:\Program Files\Git\bin\bash.exe");
    r.on(
        "bash",
        &["--version"],
        "GNU bash, version 5.2.37(1)-release (x86_64-pc-msys)\r\n",
    );
    let scan = Host::new(Platform::Windows, Arc::new(r), Box::new(meter())).scan();
    let bash = find(&scan.environments, "bash");
    assert_eq!(bash.name, "Git Bash");
    assert_eq!(bash.version.as_deref(), Some("5.2.37"));
}

#[test]
fn docker_without_its_daemon_and_a_silent_distro_still_show_up() {
    let r = windows();
    r.answer(
        "docker",
        &["version"],
        Answer::Output(Output::failed(
            r#"{"Client":{"Version":"29.8.0"},"Server":null}"#,
            "Cannot connect to the Docker daemon at npipe:////./pipe/docker_engine.",
        )),
    );
    // The newest answer for the same arguments replaces the previous one.
    r.answer(
        "wsl",
        &["-d", "Ubuntu-24.04", "-e", "bash", "-lic", PROBE_SCRIPT],
        Answer::Timeout,
    );
    let scan = Host::new(Platform::Windows, Arc::new(r), Box::new(meter())).scan();
    let docker = find(&scan.environments, "docker");
    assert!(!docker.ready && !docker.gpu);
    assert_eq!(docker.version.as_deref(), Some("29.8.0"));
    assert!(docker.detail.contains("daemon"), "{}", docker.detail);
    let ubuntu = find(&scan.environments, "wsl:Ubuntu-24.04");
    assert_eq!(ubuntu.name, "Ubuntu-24.04");
    assert_eq!(ubuntu.detail, "No response to the scan");
    assert!(ubuntu.tools.is_empty());
}

/// The Mac: login zsh, system bash, Docker Desktop without GPU and the Apple GPU
/// (without its own VRAM).
fn macos() -> FakeRunner {
    let r = FakeRunner::new();
    r.set_var("SHELL", "/bin/zsh");
    r.on(
        "zsh",
        &["-lic", PROBE_SCRIPT],
        include_bytes!("fixtures/macos-zsh-probe.txt").as_slice(),
    );
    r.on("zsh", &["--version"], "zsh 5.9 (arm64-apple-darwin24.0)\n");
    r.on(
        "bash",
        &["--version"],
        "GNU bash, version 3.2.57(1)-release (arm64-apple-darwin24)\nCopyright (C) 2007 Free Software Foundation, Inc.\n",
    );
    r.on(
        "docker",
        &["version"],
        include_bytes!("fixtures/macos-docker-version.json").as_slice(),
    );
    r.on("docker", &["info"], "io.containerd.runc.v2 runc \n");
    r.on(
        "system_profiler",
        &[],
        include_bytes!("fixtures/macos-system-profiler-displays.json").as_slice(),
    );
    r.on(
        "ioreg",
        &[],
        include_bytes!("fixtures/macos-ioreg-accelerator.txt").as_slice(),
    );
    r
}

#[test]
fn macos_finds_zsh_docker_and_the_apple_gpu() {
    let host = Host::new(Platform::MacOs, Arc::new(macos()), Box::new(meter()));
    let scan = host.scan();
    let ids: Vec<&str> = scan.environments.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["zsh", "bash", "docker"]);
    let zsh = find(&scan.environments, "zsh");
    assert!(zsh.default);
    assert_eq!(zsh.version.as_deref(), Some("5.9"));
    assert_eq!(
        zsh.detail,
        "git · cargo · rustc · node · pnpm · npm · uv · python"
    );
    assert_eq!(zsh.tools[0], tool("git", "2.50.1"));
    assert!(!find(&scan.environments, "bash").default);
    let docker = find(&scan.environments, "docker");
    assert_eq!(docker.detail, "arm64 · no GPU");
    assert!(docker.ready && !docker.gpu);
    assert_eq!(
        scan.hardware.gpus,
        [Gpu {
            name: "Apple M4 Pro".into(),
            vram: None,
            driver: None,
        }]
    );
    let s = host.sample();
    assert_eq!(
        s.gpus,
        [GpuLoad {
            name: "Apple M4 Pro".into(),
            load: Some(60),
            vram_used: Some(1_203_765_248),
            vram_total: None,
            temperature: None,
        }]
    );
}

#[test]
fn without_shell_variable_the_first_shell_is_the_default_and_tools_come_from_a_plain_login() {
    let r = FakeRunner::new();
    // Interactive mode did not return the script (for example, an rc that fails): the
    // login-only one wins.
    r.on("bash", &["-lic"], "bash: no job control in this shell\n");
    r.on(
        "bash",
        &["-lc", PROBE_SCRIPT],
        "@@os\nNAME=\"Debian GNU/Linux\"\nVERSION_ID=\"13\"\n@@path\n/usr/bin:/bin\n@@tool git\ngit version 2.47.3\n",
    );
    r.on(
        "bash",
        &["--version"],
        "GNU bash, version 5.2.37(1)-release\n",
    );
    let scan = Host::new(Platform::Linux, Arc::new(r), Box::new(meter())).scan();
    let [bash] = scan.environments.as_slice() else {
        panic!("{:#?}", scan.environments);
    };
    assert!(bash.default);
    assert_eq!(bash.tools, [tool("git", "2.47.3")]);
    assert!(scan.hardware.gpus.is_empty());
}
