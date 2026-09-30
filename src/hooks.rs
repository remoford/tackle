// Everything that reaches tackle from its sessions comes in on one localhost port:
//
// - hooks: each session's settings file has hooks that run `tackle.exe --hook`, which sends
//   one line {"session", "hook"} and waits for one line back; {"block": why} makes it exit
//   2 with `why` on stderr, which blocks the tool call or compaction;
// - MCP: hr's `tackle` MCP server is `tackle.exe --mcp`, which sends {"mcp": session} and
//   then pipes JSON-RPC lines both ways for as long as hr runs.

use crate::fleet::Shared;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

const EVENTS: [&str; 12] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Notification",
    "Stop",
    "StopFailure",
    "PreCompact",
    "PostCompact",
    "SessionEnd",
];

pub fn serve(listener: TcpListener, fleet: Shared) {
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let fleet = fleet.clone();
            std::thread::spawn(move || {
                let Ok(mut writer) = stream.try_clone() else { return };
                let peer = stream.peer_addr().map(|a| a.port()).unwrap_or(0);
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    return;
                }
                let Ok(v) = serde_json::from_str::<Value>(&line) else { return };
                if v.get("mcp").is_some() {
                    crate::mcp::serve(reader, writer, fleet);
                } else if v.get("cli_tools").is_some() {
                    let _ = writeln!(writer, "{}", json!({ "tools": crate::mcp::tools() }));
                } else if let Some(cli) = v.get("cli") {
                    // Who is calling: the tackle session whose process tree the caller is in, if any.
                    let caller_pid = crate::procs::tcp_owner(peer);
                    let mut f = fleet.lock().unwrap();
                    let caller = caller_pid.and_then(|pid| {
                        let all = crate::procs::snapshot();
                        let chain = crate::procs::ancestors(&all, pid);
                        f.sessions.iter().find(|s| s.pid.map(|p| chain.contains(&p)).unwrap_or(false)).map(|s| s.rec.name.clone())
                    });
                    let reply = crate::mcp::cli(cli["tool"].as_str().unwrap_or_default(), &cli["args"], &mut f, caller);
                    let _ = writeln!(writer, "{}", reply);
                } else if let Some(session) = v["session"].as_str() {
                    let reply = fleet.lock().unwrap().on_hook(session, &v["hook"]);
                    let _ = writeln!(writer, "{}", reply);
                }
            });
        }
    });
}

fn connect() -> Option<(String, TcpStream)> {
    let session = std::env::var("TACKLE_SESSION").ok()?;
    let port = std::env::var("TACKLE_PORT").ok()?.parse::<u16>().ok()?;
    Some((session, TcpStream::connect(("127.0.0.1", port)).ok()?))
}

/// Body of `tackle.exe --hook`. If tackle isn't running it lets everything through;
/// sessions still can't compact, because they run with DISABLE_AUTO_COMPACT.
pub fn run_hook() {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let hook: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let Some((session, mut s)) = connect() else { return };
    if writeln!(s, "{}", json!({ "session": session, "hook": hook })).is_err() {
        return;
    }
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(4)));
    let mut reply = String::new();
    let _ = BufReader::new(s).read_line(&mut reply);
    let reply: Value = serde_json::from_str(&reply).unwrap_or(Value::Null);
    if let Some(why) = reply["block"].as_str() {
        eprintln!("{}", why);
        std::process::exit(2);
    }
}

/// Body of `tackle.exe --mcp`: a stdio MCP server that is only a pipe to the app.
pub fn run_mcp_bridge() {
    let Some((session, stream)) = connect() else { return };
    let Ok(mut up) = stream.try_clone() else { return };
    if writeln!(up, "{}", json!({ "mcp": session })).is_err() {
        return;
    }
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if writeln!(up, "{}", line).is_err() {
                break;
            }
        }
        let _ = up.shutdown(std::net::Shutdown::Both);
    });
    let mut out = std::io::stdout().lock();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        if writeln!(out, "{}", line).and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

fn exe() -> std::io::Result<String> {
    Ok(std::env::current_exe()?.display().to_string().replace('\\', "/"))
}

pub fn write_settings(dir: &Path, file: &str, allow: &[&str]) -> std::io::Result<PathBuf> {
    let command = format!("\"{}\" --hook", exe()?);
    let mut hooks = serde_json::Map::new();
    for e in EVENTS {
        hooks.insert(e.to_string(), json!([{ "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }]));
    }
    let mut settings = json!({ "hooks": hooks });
    if !allow.is_empty() {
        settings["permissions"] = json!({ "allow": allow });
    }
    let path = dir.join(file);
    std::fs::write(&path, serde_json::to_string_pretty(&settings)?)?;
    Ok(path)
}

pub fn write_mcp_config(dir: &Path, port: u16, session: &str) -> std::io::Result<PathBuf> {
    let config = json!({ "mcpServers": { "tackle": {
        "type": "stdio",
        "command": exe()?,
        "args": ["--mcp"],
        "env": { "TACKLE_PORT": port.to_string(), "TACKLE_SESSION": session },
    }}});
    let path = dir.join("mcp.json");
    std::fs::write(&path, serde_json::to_string_pretty(&config)?)?;
    Ok(path)
}
