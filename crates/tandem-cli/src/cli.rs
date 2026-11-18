//! Línea de órdenes de `tandem`: `tandem [--json] [--data-dir D] <herramienta> [--param valor]…`.
//! Los subcomandos y banderas salen de la tabla de [`crate::spec`], no se repiten aquí.

use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::spec::{self, Ty};

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Help,
    Version,
    /// `tandem mcp`: servidor MCP por stdio.
    Mcp,
    Tool {
        name: &'static str,
        args: Value,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub command: Command,
    /// Salida JSON estable en vez de texto legible.
    pub json: bool,
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CliError {
    #[error("falta la herramienta; mira `tandem --help`")]
    NoCommand,
    #[error("herramienta desconocida: `{0}`; mira `tandem --help`")]
    UnknownTool(String),
    #[error("`{tool}` no tiene la opción `{flag}`")]
    UnknownFlag { tool: &'static str, flag: String },
    #[error("falta el valor de `--{0}`")]
    MissingValue(String),
    #[error("`--{0}` espera un número")]
    NotANumber(String),
    #[error("`--{0}` espera true o false")]
    NotABool(String),
    #[error("`--{0}` aparece dos veces")]
    Repeated(String),
    #[error("sobra el argumento `{0}`")]
    Extra(String),
}

pub fn usage() -> String {
    let mut s = String::from(
        "Uso: tandem [--json] [--data-dir <dir>] <herramienta> [--parámetro valor]…\n\
         \n\
         Herramientas (las mismas que expone `tandem mcp`):\n",
    );
    for t in spec::TOOLS {
        let es = t.desc.split(" / ").next().unwrap_or(t.desc);
        s.push_str(&format!("  {:<13} {es}\n", t.name.replace('_', "-")));
        for p in t.params {
            let name = p.name.replace('_', "-");
            let value = match p.ty {
                Ty::Bool => String::new(),
                Ty::Int => " <n>".into(),
                Ty::Str => " <texto>".into(),
                Ty::List => " <texto>…".into(),
            };
            let req = if p.required { " (obligatorio)" } else { "" };
            s.push_str(&format!("      --{name}{value}{req}\n"));
        }
    }
    s.push_str(
        "\n  mcp           Servidor MCP por stdio (`claude mcp add tandem -- tandem mcp`)\n\
         \n\
         Opciones:\n  --json                 salida JSON (la misma que da el MCP)\n  \
         --data-dir <dir>       directorio de datos del agente\n  \
         -h, --help | -V, --version\n",
    );
    s
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Parsed, CliError> {
    let mut json = false;
    let mut data_dir = None;
    let mut rest: Vec<String> = Vec::new();
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--json" => json = true,
            "--data-dir" => {
                data_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| CliError::MissingValue("data-dir".into()))?,
                ));
            }
            _ => rest.push(a),
        }
    }
    let mut rest = rest.into_iter();
    let parsed = |command| Parsed {
        command,
        json,
        data_dir: data_dir.clone(),
    };
    let Some(first) = rest.next() else {
        return Err(CliError::NoCommand);
    };
    match first.as_str() {
        "-h" | "--help" | "help" => return Ok(parsed(Command::Help)),
        "-V" | "--version" => return Ok(parsed(Command::Version)),
        "mcp" => {
            return match rest.next() {
                Some(extra) => Err(CliError::Extra(extra)),
                None => Ok(parsed(Command::Mcp)),
            };
        }
        _ => {}
    }
    let tool = spec::find(&first.replace('-', "_")).ok_or(CliError::UnknownTool(first))?;
    let mut out = Map::new();
    while let Some(a) = rest.next() {
        let Some(flag) = a.strip_prefix("--") else {
            return Err(CliError::Extra(a));
        };
        let (name, inline) = match flag.split_once('=') {
            Some((n, v)) => (n, Some(v.to_owned())),
            None => (flag, None),
        };
        let key = name.replace('-', "_");
        let param =
            tool.params
                .iter()
                .find(|p| p.name == key)
                .ok_or_else(|| CliError::UnknownFlag {
                    tool: tool.name,
                    flag: a.clone(),
                })?;
        let value = |inline: Option<String>, rest: &mut dyn Iterator<Item = String>| {
            inline
                .or_else(|| rest.next())
                .ok_or_else(|| CliError::MissingValue(name.into()))
        };
        let v: Value = match param.ty {
            Ty::Bool => match inline.as_deref() {
                None | Some("true") => true.into(),
                Some("false") => false.into(),
                Some(_) => return Err(CliError::NotABool(name.into())),
            },
            Ty::Str => value(inline, &mut rest)?.into(),
            Ty::Int => value(inline, &mut rest)?
                .parse::<u64>()
                .map_err(|_| CliError::NotANumber(name.into()))?
                .into(),
            Ty::List => {
                let item = Value::from(value(inline, &mut rest)?);
                match out.remove(&key) {
                    Some(Value::Array(mut a)) => {
                        a.push(item);
                        a.into()
                    }
                    _ => vec![item].into(),
                }
            }
        };
        // Las listas ya traen todos sus elementos; lo demás no se puede repetir.
        if out.insert(key, v).is_some() && param.ty != Ty::List {
            return Err(CliError::Repeated(name.into()));
        }
    }
    Ok(parsed(Command::Tool {
        name: tool.name,
        args: Value::Object(out),
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn p(args: &[&str]) -> Result<Parsed, CliError> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn la_herramienta_y_sus_banderas_salen_de_la_tabla() {
        let r = p(&[
            "--json", "exec", "--tunnel", "ab", "--cmd", "cargo", "--arg", "x",
        ]);
        // `--arg` no existe: es `--args`.
        assert!(matches!(r, Err(CliError::UnknownFlag { .. })));
        let r = p(&[
            "exec",
            "--tunnel",
            "ab",
            "--cmd",
            "cargo",
            "--args",
            "test",
            "--args=-q",
            "--env",
            "A=1",
            "--json",
        ])
        .unwrap();
        assert!(r.json);
        assert_eq!(
            r.command,
            Command::Tool {
                name: "exec",
                args: json!({"tunnel": "ab", "cmd": "cargo", "args": ["test", "-q"], "env": ["A=1"]})
            }
        );
    }

    #[test]
    fn guiones_y_guiones_bajos_valen_igual() {
        let r = p(&["tunnel-open", "--project-root", "/x", "--workspace"]).unwrap();
        assert_eq!(
            r.command,
            Command::Tool {
                name: "tunnel_open",
                args: json!({"project_root": "/x", "workspace": true})
            }
        );
        assert!(matches!(
            p(&["tunnel_open", "--keep=false"]).unwrap().command,
            Command::Tool { .. }
        ));
    }

    #[test]
    fn errores_claros() {
        assert_eq!(p(&[]), Err(CliError::NoCommand));
        assert_eq!(p(&["nada"]), Err(CliError::UnknownTool("nada".into())));
        assert_eq!(
            p(&["kill", "--process", "x"]),
            Err(CliError::NotANumber("process".into()))
        );
        assert_eq!(
            p(&["kill", "--process"]),
            Err(CliError::MissingValue("process".into()))
        );
        assert_eq!(
            p(&["kill", "--process", "1", "--process", "2"]),
            Err(CliError::Repeated("process".into()))
        );
        assert_eq!(p(&["kill", "3"]), Err(CliError::Extra("3".into())));
        assert_eq!(p(&["mcp", "x"]), Err(CliError::Extra("x".into())));
    }

    #[test]
    fn mcp_ayuda_y_data_dir() {
        let r = p(&["--data-dir", "/d", "mcp"]).unwrap();
        assert_eq!(r.command, Command::Mcp);
        assert_eq!(r.data_dir, Some(PathBuf::from("/d")));
        assert_eq!(p(&["--help"]).unwrap().command, Command::Help);
        assert_eq!(p(&["-V"]).unwrap().command, Command::Version);
    }

    #[test]
    fn la_ayuda_lista_todas_las_herramientas() {
        let u = usage();
        for t in spec::TOOLS {
            assert!(u.contains(&t.name.replace('_', "-")), "{}", t.name);
        }
    }
}
