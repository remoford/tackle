// tk: tackle from the command line. Every tool tk-hr has is a command here, with the same
// checks: tackle works out who is calling from the process tree, so a command run from
// inside a tackle session has that session's rights, and anything else is the human.
//
//   tk help                          list commands
//   tk help screen                   one command's arguments
//   tk list-sessions
//   tk screen tk-hr --lines-back 40
//   tk send-keys tk-scratch-1 down enter
//   tk start-session scratch --model haiku --brief "..."
//
// Required arguments may be given in order without their names; the rest as --name value
// (dashes or underscores), --flag / --no-flag for yes/no, and JSON for objects.

use serde_json::{json, Map, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

const HIDDEN: [&str; 2] = ["requested_by", "requester_name"];

fn request(port: u16, msg: &Value) -> Result<Value, String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(|_| "tackle isn't running (no answer on its port)".to_string())?;
    writeln!(s, "{}", msg).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|_| "tackle sent something tk doesn't understand".to_string())
}

fn port() -> Result<u16, String> {
    let dir = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_default().join("tackle");
    std::fs::read_to_string(dir.join("port")).ok().and_then(|p| p.trim().parse().ok()).ok_or_else(|| "tackle isn't running (no port file)".to_string())
}

fn cmd_name(tool: &str) -> String {
    tool.replace('_', "-")
}

fn usage_line(t: &Value) -> String {
    let required: Vec<&str> = t["inputSchema"]["required"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).filter(|r| !HIDDEN.contains(r)).collect()).unwrap_or_default();
    let props = t["inputSchema"]["properties"].as_object().cloned().unwrap_or_default();
    let mut parts = vec![cmd_name(t["name"].as_str().unwrap_or_default())];
    for r in &required {
        parts.push(format!("<{}>", r));
    }
    for (k, v) in &props {
        if HIDDEN.contains(&k.as_str()) || required.contains(&k.as_str()) {
            continue;
        }
        parts.push(match v["type"].as_str() {
            Some("boolean") => format!("[--{}]", cmd_name(k)),
            _ => format!("[--{} <{}>]", cmd_name(k), v["type"].as_str().unwrap_or("value")),
        });
    }
    parts.join(" ")
}

fn help(tools: &[Value], which: Option<&str>) {
    match which.and_then(|w| tools.iter().find(|t| t["name"] == w.replace('-', "_"))) {
        Some(t) => {
            println!("tk {}\n\n{}\n", usage_line(t), t["description"].as_str().unwrap_or_default());
            for (k, v) in t["inputSchema"]["properties"].as_object().cloned().unwrap_or_default() {
                if !HIDDEN.contains(&k.as_str()) {
                    println!("  {:<18} {}", cmd_name(&k), v["description"].as_str().unwrap_or(v["type"].as_str().unwrap_or_default()));
                }
            }
        }
        None => {
            println!("tk: tackle from the command line. Commands:\n");
            for t in tools {
                let d = t["description"].as_str().unwrap_or_default();
                let first = d.split(". ").next().unwrap_or(d);
                println!("  {:<16} {}", cmd_name(t["name"].as_str().unwrap_or_default()), first.trim_end_matches('.'));
            }
            println!("\n`tk help <command>` for its arguments.");
        }
    }
}

fn parse_args(tool: &Value, argv: &[String]) -> Result<Value, String> {
    let schema = &tool["inputSchema"];
    let props = schema["properties"].as_object().cloned().unwrap_or_default();
    let required: Vec<String> = schema["required"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).filter(|r| !HIDDEN.contains(r)).map(str::to_string).collect()).unwrap_or_default();
    let convert = |key: &str, raw: &str| -> Result<Value, String> {
        match props.get(key).and_then(|p| p["type"].as_str()) {
            Some("integer") => raw.parse::<i64>().map(Value::from).map_err(|_| format!("--{} wants a number", cmd_name(key))),
            Some("boolean") => Ok(Value::Bool(matches!(raw, "true" | "yes" | "1"))),
            Some("object") | Some("array") if raw.starts_with('{') || raw.starts_with('[') => serde_json::from_str(raw).map_err(|e| format!("--{}: {}", cmd_name(key), e)),
            Some("array") => Ok(json!(raw.split(',').map(str::trim).collect::<Vec<_>>())),
            _ => Ok(Value::String(raw.to_string())),
        }
    };
    let mut out = Map::new();
    let mut positional = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        if let Some(flag) = a.strip_prefix("--") {
            let (flag, inline) = match flag.split_once('=') {
                Some((f, v)) => (f, Some(v.to_string())),
                None => (flag, None),
            };
            let (key, negated) = match flag.strip_prefix("no-") {
                Some(k) if props.contains_key(&k.replace('-', "_")) => (k.replace('-', "_"), true),
                _ => (flag.replace('-', "_"), false),
            };
            if !props.contains_key(&key) || HIDDEN.contains(&key.as_str()) {
                return Err(format!("unknown option --{}", flag));
            }
            if props[&key]["type"] == "boolean" && inline.is_none() {
                out.insert(key, Value::Bool(!negated));
            } else {
                let raw = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        argv.get(i).cloned().ok_or_else(|| format!("--{} needs a value", flag))?
                    }
                };
                out.insert(key.clone(), convert(&key, &raw)?);
            }
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }
    // Positional arguments fill the required ones in order; an array takes all that remain.
    let mut rest = positional.into_iter();
    let unset: Vec<&String> = required.iter().filter(|k| !out.contains_key(*k)).collect();
    for key in unset {
        if props.get(key).map(|p| p["type"] == "array").unwrap_or(false) {
            let all: Vec<String> = rest.by_ref().collect();
            if !all.is_empty() {
                out.insert(key.clone(), json!(all));
            }
        } else if let Some(v) = rest.next() {
            out.insert(key.clone(), convert(key, &v)?);
        }
    }
    let extra: Vec<String> = rest.collect();
    if !extra.is_empty() {
        return Err(format!("unexpected argument(s): {}", extra.join(" ")));
    }
    if let Some(missing) = required.iter().find(|k| !out.contains_key(*k)) {
        return Err(format!("missing <{}>; usage: tk {}", missing, usage_line(tool)));
    }
    Ok(Value::Object(out))
}

fn run() -> Result<bool, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let port = port()?;
    let tools = request(port, &json!({ "cli_tools": true }))?["tools"].as_array().cloned().unwrap_or_default();
    let first = argv.first().map(String::as_str).unwrap_or("help");
    if matches!(first, "help" | "-h" | "--help") {
        help(&tools, argv.get(1).map(String::as_str));
        return Ok(true);
    }
    let name = first.replace('-', "_");
    let tool = tools.iter().find(|t| t["name"] == name.as_str()).ok_or_else(|| format!("unknown command {:?}; try `tk help`", first))?;
    let args = parse_args(tool, &argv[1..])?;
    let reply = request(port, &json!({ "cli": { "tool": name, "args": args } }))?;
    println!("{}", reply["text"].as_str().unwrap_or_default());
    Ok(reply["ok"] == true)
}

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("tk: {}", e);
            std::process::exit(2);
        }
    }
}
