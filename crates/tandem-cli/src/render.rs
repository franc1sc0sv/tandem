//! Salida legible de la CLI (sin `--json`). El JSON es el contrato; esto es para personas.

use serde_json::Value;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub out: String,
    pub err: String,
}

pub fn render(tool: &str, v: &Value) -> Rendered {
    if tool == "output" {
        return output(v);
    }
    let mut out = String::new();
    tree(v, 0, &mut out);
    // El aviso de lo omitido va por stderr para que no ensucie lo que se canaliza.
    let err = v["warning"]
        .as_str()
        .map(|w| format!("[aviso] {w}\n"))
        .unwrap_or_default();
    Rendered { out, err }
}

/// stdout y stderr del proceso en los suyos, y el estado al final por stderr.
fn output(v: &Value) -> Rendered {
    let text = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
    let state = if v["more"].as_bool().unwrap_or(false) {
        "[hay más]".to_owned()
    } else if v["done"].as_bool().unwrap_or(false) {
        format!("[terminó: {}]", v["process"]["state"])
    } else {
        "[sigue corriendo]".to_owned()
    };
    let next = format!(
        "--from-stdout {} --from-stderr {}",
        v["next"]["stdout"], v["next"]["stderr"]
    );
    Rendered {
        out: text("stdout"),
        err: format!("{}{state} {next}\n", text("stderr")),
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".into(),
        other => other.to_string(),
    }
}

fn tree(v: &Value, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    match v {
        Value::Object(o) => {
            for (k, v) in o {
                if matches!(v, Value::Object(_) | Value::Array(_)) && !is_empty(v) {
                    out.push_str(&format!("{pad}{k}:\n"));
                    tree(v, depth + 1, out);
                } else if is_empty(v) {
                    out.push_str(&format!("{pad}{k}: (vacío)\n"));
                } else {
                    out.push_str(&format!("{pad}{k}: {}\n", scalar(v)));
                }
            }
        }
        Value::Array(a) => {
            for item in a {
                if matches!(item, Value::Object(_) | Value::Array(_)) {
                    out.push_str(&format!("{pad}-\n"));
                    tree(item, depth + 1, out);
                } else {
                    out.push_str(&format!("{pad}- {}\n", scalar(item)));
                }
            }
        }
        other => out.push_str(&format!("{pad}{}\n", scalar(other))),
    }
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Object(o) => o.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn arbol_con_sangria() {
        let r = render(
            "tunnels",
            &json!({"a": 1, "b": {"c": "x", "d": []}, "e": ["p", {"f": 2}]}),
        );
        assert_eq!(
            r.out,
            "a: 1\nb:\n  c: x\n  d: (vacío)\ne:\n  - p\n  -\n    f: 2\n"
        );
    }

    #[test]
    fn la_salida_va_a_su_flujo_y_dice_desde_donde_seguir() {
        let r = render(
            "output",
            &json!({"stdout": "hola\n", "stderr": "mal\n", "done": false,
                    "next": {"stdout": 5, "stderr": 4}, "process": {"state": "Running"}}),
        );
        assert_eq!(r.out, "hola\n");
        assert_eq!(
            r.err,
            "mal\n[sigue corriendo] --from-stdout 5 --from-stderr 4\n"
        );
    }
}
