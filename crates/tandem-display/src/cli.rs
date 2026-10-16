//! Lógica de `tandem-ddc`: parseo de argumentos y ejecución contra cualquier [`DdcBus`].
//! El binario solo pone el bus real y la espera real.

use std::io::Write;
use std::time::Duration;

use tandem_core::model::Input;

use crate::ddc::switch::{
    self, PlanError, SwitchError, SwitchEvent, SwitchPlan, Waiter, input_name, known_profile,
    mccs_standard, other_input,
};
use crate::ddc::{DdcBus, DdcError, MonitorInfo, VCP_INPUT_SOURCE, VCP_POWER_MODE, caps};

pub const USAGE: &str = "\
tandem-ddc — DDC/CI de los monitores conectados a esta PC

USO:
  tandem-ddc list
  tandem-ddc caps <monitor>
  tandem-ddc get <monitor> <vcp>
  tandem-ddc set <monitor> <vcp> <valor>
  tandem-ddc switch <monitor> hdmi|dp --return-after <s> [opciones]

<monitor>  índice de `list`, o texto que coincida con un solo monitor (modelo, id PnP, serie)
<vcp>      código en hexadecimal: 60 o 0x60
<valor>    decimal (50) o hexadecimal (0x32)

`switch` siempre vuelve solo: tras <s> segundos (1–300) manda la entrada de vuelta,
con reintentos. Ctrl+C adelanta la vuelta, no la cancela.
  --return-after <s>    obligatorio
  --code <hex>          código de ida (repetible); por omisión, el del perfil del monitor
  --return-to hdmi|dp   entrada de vuelta (por omisión, la otra: hdmi→dp, dp→hdmi);
                        no puede ser la misma de la ida
  --return-code <hex>   código de vuelta (repetible); por omisión, el del perfil

`set` no acepta 0x60 (usa `switch`) ni 0xD6 (energía): podrían dejarte sin imagen.
";

/// Códigos que `set` no escribe: sin vuelta automática podrían dejar al usuario sin imagen.
const UNSAFE_SET: [u8; 2] = [VCP_INPUT_SOURCE, VCP_POWER_MODE];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorSelector {
    Index(usize),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchArgs {
    pub monitor: MonitorSelector,
    pub target: Input,
    pub return_after: Duration,
    pub codes: Vec<u8>,
    pub return_to: Input,
    pub return_codes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    List,
    Caps(MonitorSelector),
    Get(MonitorSelector, u8),
    Set(MonitorSelector, u8, u32),
    Switch(SwitchArgs),
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{0}\n\nUsa `tandem-ddc help` para ver la ayuda.")]
    Usage(String),
    #[error("ningún monitor coincide con `{0}`")]
    NoMatch(String),
    #[error("`{query}` coincide con varios monitores: {matches}")]
    Ambiguous { query: String, matches: String },
    #[error("no se encontró ningún monitor con DDC/CI")]
    NoMonitors,
    #[error(
        "`set` no escribe el VCP {0:#04X}: puede dejarte sin imagen. Usa `switch`, que vuelve solo."
    )]
    Unsafe(u8),
    #[error(transparent)]
    Ddc(#[from] DdcError),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Switch(#[from] SwitchError),
    #[error("no se pudo escribir la salida: {0}")]
    Io(#[from] std::io::Error),
}

impl CliError {
    /// 2 para errores de uso (como las herramientas de Unix), 1 para el resto.
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Usage(_) | CliError::Unsafe(_) | CliError::Plan(_) => 2,
            _ => 1,
        }
    }
}

fn usage(msg: impl Into<String>) -> CliError {
    CliError::Usage(msg.into())
}

pub fn parse_args<I, S>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
    let Some((cmd, rest)) = args.split_first() else {
        return Ok(Command::Help);
    };
    let positional = |n: usize| -> Result<&[String], CliError> {
        if rest.len() == n {
            Ok(rest)
        } else {
            Err(usage(format!(
                "`{cmd}` espera {n} argumento(s), recibió {}",
                rest.len()
            )))
        }
    };
    match cmd.as_str() {
        "help" | "-h" | "--help" => Ok(Command::Help),
        "list" => positional(0).map(|_| Command::List),
        "caps" => {
            let a = positional(1)?;
            Ok(Command::Caps(parse_monitor(&a[0])))
        }
        "get" => {
            let a = positional(2)?;
            Ok(Command::Get(parse_monitor(&a[0]), parse_vcp(&a[1])?))
        }
        "set" => {
            let a = positional(3)?;
            let code = parse_vcp(&a[1])?;
            // Antes de abrir el bus: estos códigos pueden dejar al usuario sin imagen.
            if UNSAFE_SET.contains(&code) {
                return Err(CliError::Unsafe(code));
            }
            Ok(Command::Set(
                parse_monitor(&a[0]),
                code,
                parse_value(&a[2])?,
            ))
        }
        "switch" => parse_switch(rest).map(Command::Switch),
        other => Err(usage(format!("comando desconocido `{other}`"))),
    }
}

fn parse_switch(rest: &[String]) -> Result<SwitchArgs, CliError> {
    let mut positional = Vec::new();
    let mut return_after = None;
    let mut codes = Vec::new();
    let mut return_to = None;
    let mut return_codes = Vec::new();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .ok_or_else(|| usage(format!("falta el valor de `{flag}`")))
        };
        match arg.as_str() {
            "--return-after" => {
                let v = value(arg)?;
                let secs: u64 = v
                    .parse()
                    .map_err(|_| usage(format!("`--return-after` espera segundos, no `{v}`")))?;
                return_after = Some(Duration::from_secs(secs));
            }
            "--code" => codes.push(parse_vcp(value(arg)?)?),
            "--return-code" => return_codes.push(parse_vcp(value(arg)?)?),
            "--return-to" => return_to = Some(parse_input(value(arg)?)?),
            flag if flag.starts_with("--") => {
                return Err(usage(format!("opción desconocida `{flag}`")));
            }
            _ => positional.push(arg),
        }
    }
    let [monitor, target] = positional.as_slice() else {
        return Err(usage("`switch` espera <monitor> y hdmi|dp"));
    };
    let return_after = return_after.ok_or_else(|| {
        usage("`switch` exige `--return-after <s>`: siempre hay vuelta automática")
    })?;
    let target = parse_input(target)?;
    Ok(SwitchArgs {
        monitor: parse_monitor(monitor),
        target,
        return_after,
        codes,
        // La misma entrada no se corrige aquí: la rechaza `SwitchPlan::validate`.
        return_to: return_to.unwrap_or_else(|| other_input(target)),
        return_codes,
    })
}

fn parse_monitor(s: &str) -> MonitorSelector {
    s.parse()
        .map(MonitorSelector::Index)
        .unwrap_or_else(|_| MonitorSelector::Text(s.to_owned()))
}

fn parse_input(s: &str) -> Result<Input, CliError> {
    match s.to_ascii_lowercase().as_str() {
        "hdmi" => Ok(Input::Hdmi),
        "dp" | "displayport" => Ok(Input::DisplayPort),
        _ => Err(usage(format!("entrada desconocida `{s}` (usa hdmi o dp)"))),
    }
}

/// Los códigos VCP se escriben siempre en hexadecimal, con o sin `0x`.
fn parse_vcp(s: &str) -> Result<u8, CliError> {
    let digits = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u8::from_str_radix(digits, 16).map_err(|_| usage(format!("código hexadecimal inválido `{s}`")))
}

/// Los valores VCP son de 16 bits.
fn parse_value(s: &str) -> Result<u32, CliError> {
    let parsed = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u16::from_str_radix(hex, 16),
        None => s.parse::<u16>(),
    };
    parsed
        .map(u32::from)
        .map_err(|_| usage(format!("valor inválido `{s}` (0–65535 o 0x0–0xFFFF)")))
}

pub fn select(monitors: &[MonitorInfo], sel: &MonitorSelector) -> Result<usize, CliError> {
    if monitors.is_empty() {
        return Err(CliError::NoMonitors);
    }
    match sel {
        MonitorSelector::Index(i) if *i < monitors.len() => Ok(*i),
        MonitorSelector::Index(i) => Err(CliError::NoMatch(i.to_string())),
        MonitorSelector::Text(q) => {
            let found: Vec<&MonitorInfo> = monitors.iter().filter(|m| m.matches(q)).collect();
            match found.as_slice() {
                [] => Err(CliError::NoMatch(q.clone())),
                [one] => Ok(one.index),
                many => Err(CliError::Ambiguous {
                    query: q.clone(),
                    matches: many
                        .iter()
                        .map(|m| m.to_string())
                        .collect::<Vec<_>>()
                        .join("; "),
                }),
            }
        }
    }
}

fn hex_list(codes: &[u8]) -> String {
    codes
        .iter()
        .map(|c| format!("{c:#04X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Construye el plan de `switch`: códigos explícitos o los del perfil del monitor.
pub fn plan_switch(
    info: &MonitorInfo,
    args: &SwitchArgs,
    out: &mut dyn Write,
) -> Result<SwitchPlan, CliError> {
    let profile = known_profile(info);
    let codes = match &profile {
        Some((name, codes)) => {
            writeln!(out, "perfil {name}")?;
            codes.clone()
        }
        None => {
            if args.codes.is_empty() || args.return_codes.is_empty() {
                writeln!(
                    out,
                    "sin perfil para este monitor: uso los códigos MCCS estándar (puede no obedecerlos)"
                )?;
            }
            mccs_standard()
        }
    };
    let pick = |explicit: &[u8], input| {
        if explicit.is_empty() {
            codes.for_input(input).to_vec()
        } else {
            explicit.to_vec()
        }
    };
    let plan = SwitchPlan {
        monitor: info.index,
        target: args.target,
        target_codes: pick(&args.codes, args.target),
        return_to: args.return_to,
        return_codes: pick(&args.return_codes, args.return_to),
        return_after: args.return_after,
    };
    plan.validate()?;
    Ok(plan)
}

/// Ejecuta un comando ya parseado. `Help` no necesita bus, pero se acepta igual.
pub fn run(
    cmd: &Command,
    bus: &mut dyn DdcBus,
    waiter: &mut dyn Waiter,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    match cmd {
        Command::Help => write!(out, "{USAGE}")?,
        Command::List => {
            if bus.monitors().is_empty() {
                return Err(CliError::NoMonitors);
            }
            for m in bus.monitors() {
                writeln!(out, "{m}")?;
            }
        }
        Command::Caps(sel) => {
            let i = select(bus.monitors(), sel)?;
            let raw = bus.capabilities(i)?;
            writeln!(out, "{raw}")?;
            match caps::parse(&raw) {
                Ok(c) => {
                    if let Some(v) = &c.mccs_ver {
                        writeln!(out, "MCCS {v}")?;
                    }
                    for (code, values) in &c.vcp {
                        if values.is_empty() {
                            writeln!(out, "  VCP {code:#04X}")?;
                        } else {
                            writeln!(out, "  VCP {code:#04X}: {}", hex_list(values))?;
                        }
                    }
                }
                Err(e) => writeln!(out, "(no se pudo interpretar: {e})")?,
            }
        }
        Command::Get(sel, code) => {
            let i = select(bus.monitors(), sel)?;
            let v = bus.get_vcp(i, *code)?;
            writeln!(
                out,
                "VCP {code:#04X}: actual {} ({:#06X}), máximo {} ({:#06X})",
                v.current, v.current, v.maximum, v.maximum
            )?;
            if *code == VCP_INPUT_SOURCE {
                writeln!(
                    out,
                    "ojo: muchos monitores mienten en 0x60; Tandem no confía en esta lectura"
                )?;
            }
        }
        Command::Set(sel, code, value) => {
            if UNSAFE_SET.contains(code) {
                return Err(CliError::Unsafe(*code));
            }
            let i = select(bus.monitors(), sel)?;
            bus.set_vcp(i, *code, *value)?;
            writeln!(out, "VCP {code:#04X} = {value} ({value:#06X})")?;
        }
        Command::Switch(args) => {
            let i = select(bus.monitors(), &args.monitor)?;
            let info = bus.monitors()[i].clone();
            let plan = plan_switch(&info, args, out)?;
            writeln!(
                out,
                "{info}: {} ({}), vuelta a {} ({}) en {} s",
                input_name(plan.target),
                hex_list(&plan.target_codes),
                input_name(plan.return_to),
                hex_list(&plan.return_codes),
                plan.return_after.as_secs()
            )?;
            let mut io_err = None;
            let report = switch::switch_with_return(bus, &plan, waiter, &mut |e| {
                if let Err(err) = print_event(out, e) {
                    io_err.get_or_insert(err);
                }
            })?;
            if let Some(err) = io_err {
                return Err(err.into());
            }
            report.outcome()?;
            writeln!(out, "listo")?;
        }
    }
    Ok(())
}

fn print_event(out: &mut dyn Write, e: &SwitchEvent) -> std::io::Result<()> {
    match e {
        SwitchEvent::Sent {
            phase,
            code,
            attempt,
            result,
        } => {
            let what = match phase {
                switch::Phase::Target => "ida",
                switch::Phase::Return => "vuelta",
            };
            let retry = if *attempt > 1 {
                format!(" (intento {attempt})")
            } else {
                String::new()
            };
            match result {
                Ok(()) => writeln!(out, "{what}: 0x60 = {code:#04X}{retry}: ok"),
                Err(err) => writeln!(out, "{what}: 0x60 = {code:#04X}{retry}: {err}"),
            }
        }
        SwitchEvent::Waiting(d) => writeln!(
            out,
            "esperando {} s (Ctrl+C adelanta la vuelta)",
            d.as_secs()
        ),
        SwitchEvent::Interrupted => writeln!(out, "interrumpido: vuelvo ya"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddc::fake::{FakeDdc, FakeMonitor};
    use crate::ddc::{VcpValue, edid};

    struct NoWait(Vec<Duration>);
    impl Waiter for NoWait {
        fn wait(&mut self, d: Duration, _: bool) -> bool {
            self.0.push(d);
            false
        }
    }

    const CAPS: &str = "(prot(monitor)type(lcd)SAMSUNGcmds(01 02 03 07 0C E3 F3)vcp(02 04 05 08 10 12 14(05 08 0B 0C) 16 18 1A 52 60( 12 0F 10) AA(01 02 03 FF) AC AE B2 B6 C6 C8 C9 D6(01 04 05) DC(00 02 03 05 ) DF FD)mccs_ver(2.1)mswhql(1))";

    fn monitor(name: &str, serial: &str) -> FakeMonitor {
        FakeMonitor {
            info: Some(MonitorInfo {
                index: 0,
                description: "Monitor PnP genérico".into(),
                device: r"\\.\DISPLAY1".into(),
                edid: Some(edid::parse(&edid::tests::sample(name, serial)).unwrap()),
            }),
            capabilities: CAPS.into(),
            vcp: [(
                0x10,
                VcpValue {
                    current: 50,
                    maximum: 100,
                },
            )]
            .into(),
        }
    }

    fn bus() -> FakeDdc {
        FakeDdc::new(vec![monitor("LC34G55T", "AAA"), monitor("U2720Q", "BBB")])
    }

    fn exec(bus: &mut FakeDdc, args: &[&str]) -> (Result<(), CliError>, String, Vec<Duration>) {
        let mut out = Vec::new();
        let mut waiter = NoWait(Vec::new());
        let result = parse_args(args).and_then(|cmd| run(&cmd, bus, &mut waiter, &mut out));
        (result, String::from_utf8(out).unwrap(), waiter.0)
    }

    #[test]
    fn parses_every_command() {
        assert_eq!(parse_args::<_, &str>([]).unwrap(), Command::Help);
        assert_eq!(parse_args(["--help"]).unwrap(), Command::Help);
        assert_eq!(parse_args(["list"]).unwrap(), Command::List);
        assert_eq!(
            parse_args(["caps", "1"]).unwrap(),
            Command::Caps(MonitorSelector::Index(1))
        );
        assert_eq!(
            parse_args(["get", "LC34", "0x60"]).unwrap(),
            Command::Get(MonitorSelector::Text("LC34".into()), 0x60)
        );
        assert_eq!(
            parse_args(["set", "0", "10", "0x32"]).unwrap(),
            Command::Set(MonitorSelector::Index(0), 0x10, 50)
        );
        assert_eq!(
            parse_args(["set", "0", "10", "65535"]).unwrap(),
            Command::Set(MonitorSelector::Index(0), 0x10, 0xFFFF)
        );
        assert_eq!(
            parse_args([
                "switch",
                "0",
                "hdmi",
                "--return-after",
                "10",
                "--code",
                "06",
                "--return-code",
                "0x09",
                "--return-code",
                "0F",
                "--return-to",
                "DP"
            ])
            .unwrap(),
            Command::Switch(SwitchArgs {
                monitor: MonitorSelector::Index(0),
                target: Input::Hdmi,
                return_after: Duration::from_secs(10),
                codes: vec![0x06],
                return_to: Input::DisplayPort,
                return_codes: vec![0x09, 0x0F],
            })
        );
    }

    #[test]
    fn rejects_bad_arguments() {
        let bad: &[&[&str]] = &[
            &["bogus"],
            &["list", "x"],
            &["caps"],
            &["get", "0", "zz"],
            &["get", "0", "100"],
            &["set", "0", "10", "70000"],
            &["set", "0", "10", "0xG"],
            &["switch", "0", "hdmi"],
            &["switch", "0", "vga", "--return-after", "5"],
            &["switch", "0", "--return-after", "5"],
            &["switch", "0", "hdmi", "--return-after"],
            &["switch", "0", "hdmi", "--return-after", "x"],
            &["switch", "0", "hdmi", "--return-after", "5", "--nope"],
        ];
        for args in bad {
            let err = parse_args(*args).unwrap_err();
            assert!(matches!(err, CliError::Usage(_)), "{args:?}: {err}");
            assert_eq!(err.exit_code(), 2);
        }
    }

    #[test]
    fn list_and_help() {
        let (r, out, _) = exec(&mut bus(), &["list"]);
        r.unwrap();
        assert!(out.contains(r"[0] LC34G55T (SAM7140, serie AAA) en \\.\DISPLAY1"));
        assert!(out.contains("[1] U2720Q"));
        let (r, out, _) = exec(&mut bus(), &["help"]);
        r.unwrap();
        assert!(out.contains("--return-after"));
        let (r, _, _) = exec(&mut FakeDdc::new(vec![]), &["list"]);
        assert!(matches!(r, Err(CliError::NoMonitors)));
    }

    #[test]
    fn caps_prints_raw_and_parsed() {
        let (r, out, _) = exec(&mut bus(), &["caps", "lc34g55t"]);
        r.unwrap();
        assert!(out.starts_with(CAPS));
        assert!(out.contains("MCCS 2.1"));
        assert!(out.contains("VCP 0x60: 0x12 0x0F 0x10"));
        assert!(out.contains("  VCP 0x10\n"));

        let mut b = FakeDdc::new(vec![FakeMonitor {
            capabilities: "(vcp(1".into(),
            ..Default::default()
        }]);
        let (r, out, _) = exec(&mut b, &["caps", "0"]);
        r.unwrap();
        assert!(out.contains("no se pudo interpretar"));
    }

    #[test]
    fn get_and_set() {
        let mut b = bus();
        let (r, out, _) = exec(&mut b, &["get", "0", "10"]);
        r.unwrap();
        assert!(out.contains("actual 50 (0x0032), máximo 100"));
        let (r, _, _) = exec(&mut b, &["set", "BBB", "10", "70"]);
        r.unwrap();
        assert_eq!(b.value(1, 0x10), Some(70));
        let (r, out, _) = exec(&mut b, &["get", "0", "60"]);
        assert!(r.is_err(), "{out}");
    }

    #[test]
    fn get_input_warns_that_monitors_lie() {
        let mut b = bus();
        exec(&mut b, &["set", "0", "62", "1"]).0.unwrap();
        b.set_vcp(0, 0x60, 0x09).unwrap();
        let (r, out, _) = exec(&mut b, &["get", "0", "60"]);
        r.unwrap();
        assert!(out.contains("mienten"));
    }

    #[test]
    fn set_refuses_input_and_power() {
        for code in ["60", "0xD6"] {
            let mut b = bus();
            let (r, _, _) = exec(&mut b, &["set", "0", code, "6"]);
            let err = r.unwrap_err();
            assert!(matches!(err, CliError::Unsafe(_)));
            assert_eq!(err.exit_code(), 2);
            assert!(b.writes.is_empty());
        }
    }

    #[test]
    fn run_also_refuses_unsafe_set() {
        let mut b = bus();
        let cmd = Command::Set(MonitorSelector::Index(0), 0x60, 6);
        let err = run(&cmd, &mut b, &mut NoWait(Vec::new()), &mut Vec::new()).unwrap_err();
        assert!(matches!(err, CliError::Unsafe(0x60)));
        assert!(b.writes.is_empty());
        assert!(matches!(
            parse_args(["set", "0", "d6", "4"]),
            Err(CliError::Unsafe(0xD6))
        ));
    }

    #[test]
    fn selection_errors() {
        let (r, _, _) = exec(&mut bus(), &["caps", "7"]);
        assert!(matches!(r, Err(CliError::NoMatch(_))));
        let (r, _, _) = exec(&mut bus(), &["caps", "dell"]);
        assert!(matches!(r, Err(CliError::NoMatch(_))));
        let (r, _, _) = exec(&mut bus(), &["caps", "SAM"]);
        let err = r.unwrap_err();
        assert!(matches!(err, CliError::Ambiguous { .. }));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn switch_uses_known_profile_and_returns() {
        let mut b = bus();
        let (r, out, waits) = exec(&mut b, &["switch", "0", "hdmi", "--return-after", "8"]);
        r.unwrap();
        let writes: Vec<_> = b.writes.iter().map(|w| (w.1, w.2)).collect();
        assert_eq!(writes, vec![(0x60, 0x06), (0x60, 0x0F), (0x60, 0x09)]);
        assert_eq!(waits[0], Duration::from_secs(8));
        assert!(out.contains("perfil LC34G55T"));
        assert!(out.contains("vuelta: 0x60 = 0x0F: ok"));
        assert!(out.ends_with("listo\n"));
    }

    #[test]
    fn switch_with_explicit_codes_for_unknown_monitor() {
        let mut b = bus();
        let (r, out, _) = exec(
            &mut b,
            &[
                "switch",
                "1",
                "hdmi",
                "--return-after",
                "3",
                "--code",
                "11",
                "--return-code",
                "0F",
            ],
        );
        r.unwrap();
        let writes: Vec<_> = b.writes.iter().map(|w| w.2).collect();
        assert_eq!(writes, vec![0x11, 0x0F]);
        assert!(!out.contains("MCCS estándar"));
    }

    #[test]
    fn switch_unknown_monitor_falls_back_to_mccs() {
        let mut b = bus();
        let (r, out, _) = exec(&mut b, &["switch", "1", "hdmi", "--return-after", "3"]);
        r.unwrap();
        assert!(out.contains("MCCS estándar"));
        let writes: Vec<_> = b.writes.iter().map(|w| w.2).collect();
        assert_eq!(writes, vec![0x11, 0x0F]);
    }

    #[test]
    fn switch_rejects_out_of_range_wait_before_writing() {
        let mut b = bus();
        let (r, _, _) = exec(&mut b, &["switch", "0", "hdmi", "--return-after", "0"]);
        let err = r.unwrap_err();
        assert!(matches!(err, CliError::Plan(_)));
        assert_eq!(err.exit_code(), 2);
        let (r, _, _) = exec(&mut b, &["switch", "0", "hdmi", "--return-after", "301"]);
        assert!(r.is_err());
        assert!(b.writes.is_empty());
    }

    #[test]
    fn switch_rejects_same_input_before_writing() {
        for args in [
            &[
                "switch",
                "0",
                "hdmi",
                "--return-after",
                "5",
                "--return-to",
                "hdmi",
            ][..],
            &[
                "switch",
                "0",
                "dp",
                "--return-after",
                "5",
                "--return-to",
                "dp",
            ][..],
        ] {
            let mut b = bus();
            let (r, _, _) = exec(&mut b, args);
            let err = r.unwrap_err();
            assert!(
                matches!(err, CliError::Plan(PlanError::SameInput(_))),
                "{args:?}: {err}"
            );
            assert_eq!(err.exit_code(), 2);
            assert!(b.writes.is_empty(), "{args:?}");
        }
    }

    #[test]
    fn switch_dp_defaults_to_return_hdmi() {
        assert_eq!(
            parse_args(["switch", "0", "hdmi", "--return-after", "5"])
                .map(|c| match c {
                    Command::Switch(a) => a.return_to,
                    _ => unreachable!(),
                })
                .unwrap(),
            Input::DisplayPort
        );
        let mut b = bus();
        let (r, out, _) = exec(&mut b, &["switch", "0", "dp", "--return-after", "5"]);
        r.unwrap();
        let writes: Vec<_> = b.writes.iter().map(|w| w.2).collect();
        assert_eq!(writes, vec![0x0F, 0x09, 0x06]);
        assert!(out.contains("vuelta a hdmi (0x06)"), "{out}");
    }

    #[test]
    fn switch_reports_failed_return() {
        let mut b = bus();
        b.write_script = std::iter::once(true)
            .chain(std::iter::repeat_n(false, 10))
            .collect();
        let (r, out, _) = exec(&mut b, &["switch", "0", "hdmi", "--return-after", "2"]);
        assert!(matches!(
            r,
            Err(CliError::Switch(SwitchError::ReturnFailed))
        ));
        assert!(out.contains("(intento 3)"));
        assert!(out.contains("0xC0262589"));
    }
}
