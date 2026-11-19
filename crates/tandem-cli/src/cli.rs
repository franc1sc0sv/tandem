//! Command line of `tandem`: `tandem [--json] [--data-dir D] <tool> [--param value]...`.
//! The subcommands and flags come from the [`crate::spec`] table, they are not repeated here.

use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::spec::{self, Ty};

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Help,
    Version,
    /// `tandem mcp`: MCP server over stdio.
    Mcp,
    Tool {
        name: &'static str,
        args: Value,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub command: Command,
    /// Stable JSON output instead of readable text.
    pub json: bool,
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CliError {
    #[error("missing tool; see `tandem --help`")]
    NoCommand,
    #[error("unknown tool: `{0}`; see `tandem --help`")]
    UnknownTool(String),
    #[error("`{tool}` does not have the option `{flag}`")]
    UnknownFlag { tool: &'static str, flag: String },
    #[error("missing value for `--{0}`")]
    MissingValue(String),
    #[error("`--{0}` expects a number")]
    NotANumber(String),
    #[error("`--{0}` expects true or false")]
    NotABool(String),
    #[error("`--{0}` appears twice")]
    Repeated(String),
    #[error("unexpected argument `{0}`")]
    Extra(String),
}

pub fn usage() -> String {
    let mut s = String::from(
        "Usage: tandem [--json] [--data-dir <dir>] <tool> [--parameter value]...\n\
         \n\
         Tools (the same ones `tandem mcp` exposes):\n",
    );
    for t in spec::TOOLS {
        let es = t.desc.split(" / ").next().unwrap_or(t.desc);
        s.push_str(&format!("  {:<13} {es}\n", t.name.replace('_', "-")));
        for p in t.params {
            let name = p.name.replace('_', "-");
            let value = match p.ty {
                Ty::Bool => String::new(),
                Ty::Int => " <n>".into(),
                Ty::Str => " <text>".into(),
                Ty::List => " <text>...".into(),
            };
            let req = if p.required { " (required)" } else { "" };
            s.push_str(&format!("      --{name}{value}{req}\n"));
        }
    }
    s.push_str(
        "\n  mcp           MCP server over stdio (`claude mcp add tandem -- tandem mcp`)\n\
         \n\
         Options:\n  --json                 JSON output (the same the MCP gives)\n  \
         --data-dir <dir>       the agent's data directory\n  \
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
        // The lists already bring all their elements; the rest cannot be repeated.
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
    fn tool_and_flags_come_from_the_table() {
        let r = p(&[
            "--json", "exec", "--tunnel", "ab", "--cmd", "cargo", "--arg", "x",
        ]);
        // `--arg` does not exist: it is `--args`.
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
    fn hyphens_and_underscores_are_equivalent() {
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
    fn clear_errors() {
        assert_eq!(p(&[]), Err(CliError::NoCommand));
        assert_eq!(
            p(&["nothing"]),
            Err(CliError::UnknownTool("nothing".into()))
        );
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
    fn mcp_help_and_data_dir() {
        let r = p(&["--data-dir", "/d", "mcp"]).unwrap();
        assert_eq!(r.command, Command::Mcp);
        assert_eq!(r.data_dir, Some(PathBuf::from("/d")));
        assert_eq!(p(&["--help"]).unwrap().command, Command::Help);
        assert_eq!(p(&["-V"]).unwrap().command, Command::Version);
    }

    #[test]
    fn help_lists_all_tools() {
        let u = usage();
        for t in spec::TOOLS {
            assert!(u.contains(&t.name.replace('_', "-")), "{}", t.name);
        }
    }
}
