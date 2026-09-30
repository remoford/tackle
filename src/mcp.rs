// tackle's MCP server, given to hr. Claude starts `tackle.exe --mcp` as a stdio server;
// that process only pipes JSON-RPC lines to the running app, which answers them here.
//
// Looking is open to everyone. Acting on a session is allowed only to the human, their
// delegates, the session itself and the sessions above it in the chain that started it;
// fleet-wide changes only to the human and delegates. Every act is logged with who asked.

use crate::fleet::{tk, Fleet, Shared, Who};
use crate::session::{clip, Activity, Role};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

pub fn serve(reader: BufReader<TcpStream>, mut writer: TcpStream, fleet: Shared) {
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(resp) = handle(&req, &fleet) {
            if writeln!(writer, "{}", resp).is_err() {
                break;
            }
        }
    }
}

fn handle(req: &Value, fleet: &Shared) -> Option<Value> {
    // Notifications carry no id and get no response.
    let id = req.get("id")?.clone();
    let result = match req["method"].as_str().unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "tackle", "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let mut f = fleet.lock().unwrap();
            let (text, error) = match call(req["params"]["name"].as_str().unwrap_or_default(), &req["params"]["arguments"], &mut f) {
                Ok(t) => (t, false),
                Err(t) => (t, true),
            };
            json!({ "content": [{ "type": "text", "text": text }], "isError": error })
        }
        _ => return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "method not found" } })),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn tool(name: &str, description: &str, mut props: Value, required: &[&str], acting: bool) -> Value {
    let mut required: Vec<&str> = required.to_vec();
    if acting {
        props["requested_by"] = json!({ "type": "string", "description": "the exact `from` of the cross-session message asking for this, or \"human\" if the human typed it to you directly" });
        props["requester_name"] = json!({ "type": "string", "description": "the exact `from-name` of that message (empty for the human)" });
        required.push("requested_by");
    }
    json!({ "name": name, "description": description, "inputSchema": { "type": "object", "properties": props, "required": required } })
}

/// A call from the `tk` command line. `caller` is the tackle session the calling process
/// runs under, if any (found from the process tree); otherwise it is the human at the
/// machine. Either way it goes through the same checks as a call from hr.
pub fn cli(tool: &str, args: &Value, f: &mut Fleet, caller: Option<String>) -> Value {
    let mut args = if args.is_object() { args.clone() } else { json!({}) };
    match caller {
        Some(name) => {
            args["requested_by"] = json!("cli");
            args["requester_name"] = json!(name);
        }
        None => {
            args["requested_by"] = json!("human");
            args["requester_name"] = json!("");
        }
    }
    match call(tool, &args, f) {
        Ok(text) => json!({ "ok": true, "text": text }),
        Err(text) => json!({ "ok": false, "text": text }),
    }
}

pub fn tools() -> Vec<Value> {
    let name = json!({ "name": { "type": "string", "description": "session name, as in list_sessions (the tk- prefix may be left off)" } });
    let project = json!({ "project": { "type": "string", "description": "a project name from list_projects, or a directory" } });
    vec![
        tool("list_sessions", "Every session tackle knows, one line each: role, model, state, context, staleness, the tool it is running. Stopped and dormant sessions too.", json!({}), &[], false),
        tool(
            "activity",
            "What one session is doing, in a few lines: time in the turn, the tool running now, the prompt, recent tool calls, the last thing it said, its state-file line, read commit and staleness, and a cold-cache warning. Much cheaper than screen; prefer it.",
            name.clone(),
            &["name"],
            false,
        ),
        tool("get_context", "Token usage of one session, in detail, from its transcript.", name.clone(), &["name"], false),
        tool(
            "screen",
            "Exactly what is on one session's terminal screen right now, as plain text. With lines_back, scrolled up that many lines into its history.",
            json!({ "name": name["name"], "lines_back": { "type": "integer", "description": "scroll up this many lines first (default 0)" } }),
            &["name"],
            false,
        ),
        tool("list_projects", "The projects tackle knows: name and directory.", json!({}), &[], false),
        tool("get_settings", "tackle's settings: clear thresholds, context window, cache lifetime, build-lock words.", json!({}), &[], false),
        tool("locks", "Writer tokens (who may edit and commit in which project) and the build lock.", json!({}), &[], false),
        tool(
            "usage",
            "The plan's usage limits (session window, weekly), their resets, when each runs out at the last hour's rate, and which sessions used the tokens. Read from /usage every few minutes.",
            json!({}),
            &[],
            false,
        ),
        tool(
            "traffic",
            "Messages tackle's sessions have sent each other (by SendMessage): when, from, to, and the summary line. Optionally only those to or from one session.",
            json!({ "n": { "type": "integer", "description": "how many (default 20)" }, "name": { "type": "string", "description": "only messages to or from this session" } }),
            &[],
            false,
        ),
        tool("units", "Who holds which unit of work in a project, from its orchestration/assignments.md.", project.clone(), &["project"], false),
        tool("list_presets", "Saved ways to start a session (project, model, role, brief), by name.", json!({}), &[], false),
        tool("log", "tackle's most recent actions, refusals and wake measurements.", json!({ "n": { "type": "integer", "description": "how many lines (default 20)" } }), &[], false),
        tool("clear", "Queue /clear for a session, typed the next time it is idle between turns.", name.clone(), &["name"], true),
        tool("show_context", "Queue /context for a session, typed when it is next idle. Read the result with screen.", name.clone(), &["name"], true),
        tool(
            "type_text",
            "Type text into a session's input box as if at its keyboard, and press Enter if submit is true (default). A busy session queues a submitted prompt until its turn ends.",
            json!({ "name": name["name"], "text": { "type": "string" }, "submit": { "type": "boolean" } }),
            &["name", "text"],
            true,
        ),
        tool(
            "send_keys",
            "Press keys in a session, in order: enter, esc, tab, shift+tab, backspace, up, down, left, right, home, end, pageup, pagedown, delete, space, ctrl+<letter>, or single characters.",
            json!({ "name": name["name"], "keys": { "type": "array", "items": { "type": "string" } } }),
            &["name", "keys"],
            true,
        ),
        tool(
            "start_session",
            "Launch a new session in a project. The requester becomes its manager. brief, if given, is typed in as its first prompt.",
            json!({
                "project": project["project"],
                "name": { "type": "string", "description": "one word; tackle adds tk-; default is the project name plus a number, e.g. tk-ic-1" },
                "model": { "type": "string", "description": "opus, sonnet or haiku (default opus)" },
                "role": { "type": "string", "enum": ["worker", "orchestrator"] },
                "brief": { "type": "string" },
            }),
            &["project"],
            true,
        ),
        tool("stop_session", "Kill a session. It stays listed as stopped and can be resumed or started fresh.", name.clone(), &["name"], true),
        tool("resume_session", "Start a stopped or dormant session again by its Claude session id (not after a compaction).", name.clone(), &["name"], true),
        tool(
            "fresh_session",
            "Start a stopped, dormant or compacted session again as a new Claude session with the same name, role, project and manager, told to resume from its state file.",
            json!({ "name": name["name"], "brief": { "type": "string", "description": "optional instead of the default 'resume from your state file'" } }),
            &["name"],
            true,
        ),
        tool(
            "grant_writer",
            "Give a session the writer token for a project, so it alone may edit and commit there. It should pull before editing.",
            json!({ "project": project["project"], "name": name["name"] }),
            &["project", "name"],
            true,
        ),
        tool("release_writer", "Take back the writer token for a project (after the holder has committed and pushed).", project.clone(), &["project"], true),
        tool("release_lock", "Free the build lock, e.g. if its build died without tackle noticing.", json!({}), &[], true),
        tool("forget_session", "Remove a stopped session from tackle's list. Its transcript and state file stay.", name.clone(), &["name"], true),
        tool(
            "refresh_stale",
            "For a session whose read files changed since it read them: queue /clear for its next idle moment, then tell it which files changed so it re-reads what its task needs. Without a name, every stale session the requester may act on.",
            json!({ "name": { "type": "string", "description": "one session; leave out for all you may act on" } }),
            &[],
            true,
        ),
        tool(
            "assign_unit",
            "Record in a project's orchestration/assignments.md that a session holds a unit of work, or change its state or note. The unit name comes from the book (e.g. part2-step3).",
            json!({
                "project": project["project"],
                "unit": { "type": "string" },
                "name": { "type": "string", "description": "the session holding it" },
                "state": { "type": "string", "description": "working (default), review, blocked or done" },
                "note": { "type": "string" },
            }),
            &["project", "unit", "name"],
            true,
        ),
        tool("remove_unit", "Remove a unit's line from a project's assignments file.", json!({ "project": project["project"], "unit": { "type": "string" } }), &["project", "unit"], true),
        tool(
            "start_preset",
            "Start a session from a saved preset. The requester becomes its manager.",
            json!({ "preset": { "type": "string" }, "name": { "type": "string", "description": "optional session name" }, "brief": { "type": "string", "description": "optional, instead of the preset's brief" } }),
            &["preset"],
            true,
        ),
        tool("delete_preset", "Delete a preset. Human and delegates only.", json!({ "preset": { "type": "string" } }), &["preset"], true),
        tool(
            "save_preset",
            "Save (or replace) a preset. Human and delegates only.",
            json!({
                "preset": { "type": "string" },
                "project": project["project"],
                "model": { "type": "string" },
                "role": { "type": "string", "enum": ["worker", "orchestrator"] },
                "brief": { "type": "string" },
            }),
            &["preset", "project"],
            true,
        ),
        tool("add_project", "Add a project (or change its directory). Human and delegates only.", json!({ "name": { "type": "string" }, "directory": { "type": "string" } }), &["name", "directory"], true),
        tool("remove_project", "Remove a project from tackle's list. Human and delegates only.", json!({ "name": { "type": "string" } }), &["name"], true),
        tool(
            "set_settings",
            "Change tackle's settings; leave out what stays. Human and delegates only.",
            json!({
                "hr_clear_at": { "type": "integer", "description": "clear hr at this context size (tokens; 0 never)" },
                "others_clear_at": { "type": "integer", "description": "clear a session that has written BETWEEN UNITS once its context reaches this (0 never)" },
                "context_window": { "type": "integer" },
                "cache_ttl_min": { "type": "integer", "description": "minutes after which a session's cache is assumed cold" },
                "usage_every_min": { "type": "integer", "description": "minutes between /usage readings (0 never)" },
                "plan_usd_month": { "type": "number", "description": "what the plan costs a month, for plan-share dollars" },
                "one_writer": { "type": "object", "description": "{\"project\": true/false}: require the writer token there" },
                "lock_words": { "type": "array", "items": { "type": "string" }, "description": "words in a Bash command that take the build lock" },
            }),
            &[],
            true,
        ),
    ]
}

fn session_line(f: &Fleet, s: &crate::session::Session) -> String {
    let mut line = format!("{} ({} {}): {}, {}k, ${:.2}", s.rec.name, s.rec.role.label(), s.rec.model, s.state_phrase(), s.usage.context / 1000, s.rec.cost_usd);
    if !s.doing.stale.is_empty() {
        line += &format!(", {} of {} read files changed", s.doing.stale.len(), s.rec.reads.len());
    }
    if let Some(m) = s.cold(f.s.cache_ttl_min) {
        line += &format!(", cold {}m", m);
    }
    if let Some((tool, _)) = &s.doing.tool {
        line += &format!(", running {}", clip(tool, 60));
    }
    if !s.doing.notice.is_empty() {
        line += &format!(", waiting on: {}", clip(&s.doing.notice, 60));
    }
    if s.doing.between_units.is_some() {
        line += ", between units";
    }
    if !s.pending.is_empty() {
        line += &format!(", queued {}", s.pending.join(" "));
    }
    if s.contaminated {
        line += ", COMPACTED";
    }
    line
}

fn call(tool: &str, args: &Value, f: &mut Fleet) -> Result<String, String> {
    let arg = |k: &str| args[k].as_str().unwrap_or_default().trim().to_string();
    let name = arg("name");
    let who = f.who(&arg("requested_by"), &arg("requester_name"));
    let fleet_wide = |f: &mut Fleet, what: &str| -> Result<(), String> {
        match who {
            Who::Human | Who::Delegate(_) => Ok(()),
            _ => Err(f.deny(&who, what)),
        }
    };
    let on_session = |f: &mut Fleet, what: &str| -> Result<(), String> {
        if f.may_act(&who, &name) {
            Ok(())
        } else {
            Err(f.deny(&who, &format!("{} {}", what, tk(&name))))
        }
    };
    let done = |f: &mut Fleet, what: String| -> Result<String, String> {
        f.add_log(format!("{} -> {}", who.label(), what));
        Ok(what)
    };
    match tool {
        // ---- looking ----
        "list_sessions" => {
            let mut lines: Vec<String> = f.sessions.iter().map(|s| session_line(f, s)).collect();
            for r in &f.dormant {
                lines.push(format!("{} ({} {}): not running{}", r.name, r.role.label(), r.model, if r.compacted { ", COMPACTED" } else { "" }));
            }
            Ok(if lines.is_empty() { "no sessions".into() } else { lines.join("\n") })
        }
        "list_projects" => {
            let list: Vec<String> = crate::projects::load(&f.data).iter().map(|p| format!("{}: {}", p.name, p.path.display())).collect();
            Ok(if list.is_empty() { "no projects".into() } else { list.join("\n") })
        }
        "get_settings" => Ok(format!(
            "hr_clear_at {}, others_clear_at {} (at a handover only), context_window {}, cache_ttl_min {}, build-lock words {}, one-writer projects {}",
            f.s.hr_clear_at,
            f.s.others_clear_at,
            f.s.window,
            f.s.cache_ttl_min,
            f.s.lock_words.join("/"),
            f.s.one_writer.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        )),
        "locks" => {
            let mut out: Vec<String> = f.s.writers.iter().map(|(p, h)| format!("writer token for {}: {}", p, h)).collect();
            for p in &f.s.one_writer {
                if !f.s.writers.contains_key(&crate::fleet::key(p)) {
                    out.push(format!("writer token for {}: nobody", p.display()));
                }
            }
            out.push(match &f.lock {
                Some(l) => format!("build lock: {} ({}, {})", l.session, clip(&l.command, 60), crate::session::ago(l.since)),
                None => "build lock: free".into(),
            });
            Ok(out.join("\n"))
        }
        "usage" => Ok(crate::usage::report(&f.usage, f.s.plan_usd_month)),
        "traffic" => {
            let n = args["n"].as_u64().unwrap_or(20) as usize;
            let only = Some(tk(&name)).filter(|_| !name.is_empty());
            let lines: Vec<String> = f
                .traffic
                .iter()
                .rev()
                .filter(|m| only.as_ref().map(|o| m.from == *o || tk(&m.to) == *o).unwrap_or(true))
                .take(n)
                .map(|m| format!("{} {} -> {}: {}", m.at, m.from, m.to, m.summary))
                .collect();
            Ok(if lines.is_empty() { "no messages".into() } else { lines.into_iter().rev().collect::<Vec<_>>().join("\n") })
        }
        "units" => {
            let dir = crate::projects::resolve(&f.data, &arg("project"))?.path;
            Ok(crate::units::render(&dir))
        }
        "list_presets" => Ok(if f.s.presets.is_empty() {
            "no presets".into()
        } else {
            f.s.presets.iter().map(|p| format!("{}: {} {} in {}{}", p.name, p.model, p.role.label(), p.project, if p.brief.is_empty() { String::new() } else { format!("; brief: {}", clip(&p.brief, 80)) })).collect::<Vec<_>>().join("\n")
        }),
        "save_preset" => {
            fleet_wide(f, "save a preset")?;
            crate::projects::resolve(&f.data, &arg("project"))?;
            let preset = crate::fleet::Preset {
                name: arg("preset"),
                project: arg("project"),
                model: Some(arg("model")).filter(|m| !m.is_empty()).unwrap_or_else(|| "opus".into()),
                role: if arg("role") == "orchestrator" { Role::Orchestrator } else { Role::Worker },
                brief: arg("brief"),
            };
            f.s.presets.retain(|p| p.name != preset.name);
            let label = preset.name.clone();
            f.s.presets.push(preset);
            done(f, format!("preset {} saved", label))
        }
        "delete_preset" => {
            fleet_wide(f, "delete a preset")?;
            let before = f.s.presets.len();
            f.s.presets.retain(|p| p.name != arg("preset"));
            if f.s.presets.len() == before {
                return Err(format!("no preset {:?}", arg("preset")));
            }
            done(f, format!("preset {} deleted", arg("preset")))
        }
        "start_preset" => {
            let manager = match &who {
                Who::Human | Who::Delegate(_) => None,
                Who::Session(n) => Some(n.clone()),
                Who::Unknown { .. } => return Err(f.deny(&who, "start a session")),
            };
            let p = f.s.presets.iter().find(|p| p.name == arg("preset")).cloned().ok_or_else(|| format!("no preset {:?}; list_presets shows them", arg("preset")))?;
            let brief = Some(arg("brief")).filter(|b| !b.is_empty()).or_else(|| Some(p.brief.clone()).filter(|b| !b.is_empty()));
            let started = f.start_in(&p.project, &name, &p.model, p.role, manager, brief)?;
            done(f, format!("started {} from preset {}", started, p.name))
        }
        "refresh_stale" => {
            let targets: Vec<String> = if name.is_empty() {
                f.sessions.iter().filter(|s| !s.doing.stale.is_empty() && s.activity != Activity::Exited).map(|s| s.rec.name.clone()).filter(|n| f.may_act(&who, n)).collect()
            } else {
                on_session(f, "refresh")?;
                vec![tk(&name)]
            };
            if targets.is_empty() {
                return Ok("no stale sessions you may act on".into());
            }
            let mut out = Vec::new();
            for t in targets {
                out.push(f.refresh(&t)?);
            }
            done(f, out.join("; "))
        }
        "assign_unit" | "remove_unit" => {
            let dir = crate::projects::resolve(&f.data, &arg("project"))?.path;
            let unit = arg("unit");
            if tool == "assign_unit" {
                on_session(f, "assign a unit to")?;
                // Whatever isn't given keeps its value; a new holder starts a new "since".
                let old = crate::units::load(&dir).into_iter().find(|u| u.unit == unit);
                let same_holder = old.as_ref().map(|u| u.holder == tk(&name)).unwrap_or(false);
                let state = Some(arg("state")).filter(|s| !s.is_empty()).or_else(|| old.as_ref().map(|u| u.state.clone())).unwrap_or_else(|| "working".into());
                let note = Some(arg("note")).filter(|n| !n.is_empty()).or_else(|| old.as_ref().map(|u| u.note.clone())).unwrap_or_default();
                let since = old.filter(|_| same_holder).map(|u| u.since).unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d %H:%M").to_string());
                crate::units::upsert(&dir, crate::units::Unit { unit: unit.clone(), holder: tk(&name), state: state.clone(), since, note })?;
                done(f, format!("unit {} in {}: {} ({})", unit, dir.display(), tk(&name), state))
            } else {
                let holder = crate::units::load(&dir).into_iter().find(|u| u.unit == unit).map(|u| u.holder);
                let allowed = matches!(who, Who::Human | Who::Delegate(_)) || holder.as_deref().map(|h| f.may_act(&who, h)).unwrap_or(false);
                if !allowed {
                    return Err(f.deny(&who, &format!("remove unit {}", unit)));
                }
                let removed = crate::units::remove(&dir, &unit)?;
                done(f, if removed { format!("unit {} removed", unit) } else { format!("no unit {}", unit) })
            }
        }
        "forget_session" => {
            on_session(f, "forget")?;
            f.forget(&name)?;
            done(f, format!("forgot {}", tk(&name)))
        }
        "log" => {
            let n = args["n"].as_u64().unwrap_or(20) as usize;
            let lines: Vec<&String> = f.log.iter().rev().take(n).collect();
            Ok(lines.into_iter().rev().cloned().collect::<Vec<_>>().join("\n"))
        }
        "activity" | "get_context" | "screen" => {
            let ttl = f.s.cache_ttl_min;
            let s = f.find(&name).ok_or_else(|| format!("no running session named {:?}", name))?;
            match tool {
                "activity" => Ok(s.report(ttl)),
                "get_context" => {
                    let u = &s.usage;
                    Ok(format!(
                        "{}: cost so far ${:.2} at API prices; context {}; last message in {} / cache read {} / cache write {} / out {}; totals over {} messages: in {} / cache read {} / cache write {} / out {}; transcript {}",
                        s.rec.name,
                        s.rec.cost_usd,
                        u.context,
                        u.input,
                        u.cache_read,
                        u.cache_write,
                        u.output,
                        u.messages,
                        u.total_input,
                        u.total_cache_read,
                        u.total_cache_write,
                        u.total_output,
                        s.transcript.as_ref().map(|t| t.path.display().to_string()).unwrap_or_default()
                    ))
                }
                _ => {
                    let mut parser = s.parser.lock().unwrap();
                    let back = parser.screen().scrollback();
                    parser.set_scrollback(args["lines_back"].as_u64().unwrap_or(0) as usize);
                    let screen = parser.screen();
                    let (rows, cols) = screen.size();
                    let shown = screen.scrollback();
                    let text: Vec<String> = screen.rows(0, cols).map(|r| r.trim_end().to_string()).collect();
                    let text = text.join("\n").trim_end().to_string();
                    parser.set_scrollback(back);
                    let at = if shown > 0 { format!(", scrolled up {} lines", shown) } else { String::new() };
                    Ok(format!("screen of {} ({}x{}, {}{}):\n{}", s.rec.name, cols, rows, s.activity.label(), at, text))
                }
            }
        }

        // ---- fleet-wide: human and delegates ----
        "add_project" => {
            fleet_wide(f, "add a project")?;
            crate::projects::add(&f.data, &name, &arg("directory"))?;
            done(f, format!("project {} added", name))
        }
        "remove_project" => {
            fleet_wide(f, "remove a project")?;
            crate::projects::remove(&f.data, &name)?;
            done(f, format!("project {} removed", name))
        }
        "set_settings" => {
            fleet_wide(f, "change settings")?;
            let num = |k: &str| args[k].as_u64();
            if let Some(v) = num("hr_clear_at") {
                f.s.hr_clear_at = v;
            }
            if let Some(v) = num("others_clear_at") {
                f.s.others_clear_at = v;
            }
            if let Some(v) = num("context_window") {
                f.s.window = v.max(1);
            }
            if let Some(v) = num("cache_ttl_min") {
                f.s.cache_ttl_min = v;
            }
            if let Some(v) = num("usage_every_min") {
                f.s.usage_every_min = v;
            }
            if let Some(v) = args["plan_usd_month"].as_f64() {
                f.s.plan_usd_month = v;
            }
            if let Some(words) = args["lock_words"].as_array() {
                f.s.lock_words = words.iter().filter_map(|w| w.as_str()).map(str::to_string).collect();
            }
            if let Some(map) = args["one_writer"].as_object() {
                for (p, on) in map {
                    let dir = crate::projects::resolve(&f.data, p)?.path;
                    f.s.one_writer.retain(|x| x.display().to_string().to_lowercase() != dir.display().to_string().to_lowercase());
                    if on.as_bool() == Some(true) {
                        f.s.one_writer.push(dir);
                    }
                }
            }
            done(f, format!("settings changed: {}", clip(&args.to_string(), 200)))
        }

        // ---- starting: human, delegates, and tackle's own sessions ----
        "start_session" => {
            let manager = match &who {
                Who::Human | Who::Delegate(_) => None,
                Who::Session(n) => Some(n.clone()),
                Who::Unknown { .. } => return Err(f.deny(&who, "start a session")),
            };
            let role = if arg("role") == "orchestrator" { Role::Orchestrator } else { Role::Worker };
            let brief = Some(arg("brief")).filter(|b| !b.is_empty());
            let started = f.start_in(&arg("project"), &name, &arg("model"), role, manager, brief)?;
            let dir = f.find_ref(&started).map(|s| s.rec.cwd.display().to_string()).unwrap_or_default();
            done(f, format!("started {} in {}", started, dir))
        }

        // ---- the writer token and the build lock ----
        "grant_writer" | "release_writer" => {
            let dir = crate::projects::resolve(&f.data, &arg("project"))?.path;
            if tool == "grant_writer" {
                on_session(f, "grant the writer token to")?;
                f.grant_writer(&dir, &name, &who.label());
                Ok(format!("{} holds the writer token for {}; it should pull before editing, and release after commit and push", tk(&name), dir.display()))
            } else {
                let holder = f.s.writers.get(&crate::fleet::key(&dir)).cloned();
                let allowed = matches!(who, Who::Human | Who::Delegate(_)) || holder.as_deref().map(|h| f.may_act(&who, h)).unwrap_or(true);
                if !allowed {
                    return Err(f.deny(&who, &format!("release the writer token for {}", dir.display())));
                }
                f.release_writer(&dir, &who.label());
                Ok(format!("writer token for {} released", dir.display()))
            }
        }
        "release_lock" => {
            let holder = f.lock.as_ref().map(|l| l.session.clone());
            let allowed = matches!(who, Who::Human | Who::Delegate(_)) || holder.as_deref().map(|h| f.may_act(&who, h)).unwrap_or(true);
            if !allowed {
                return Err(f.deny(&who, "release the build lock"));
            }
            f.release_lock(&who.label());
            Ok("build lock released".into())
        }

        // ---- acting on one session ----
        "resume_session" => {
            on_session(f, "resume")?;
            f.resume(&name)?;
            done(f, format!("resumed {}", tk(&name)))
        }
        "fresh_session" => {
            on_session(f, "restart")?;
            f.fresh(&name, Some(arg("brief")))?;
            done(f, format!("started {} fresh from its state file", tk(&name)))
        }
        "clear" | "show_context" | "type_text" | "send_keys" | "stop_session" => {
            on_session(f, tool)?;
            let ttl = f.s.cache_ttl_min;
            let s = f.find(&name).ok_or_else(|| format!("no running session named {:?}", name))?;
            if s.activity == Activity::Exited {
                return Err(format!("{} is not running", s.rec.name));
            }
            let target = s.rec.name.clone();
            let cold = s.cold(ttl).map(|m| format!(" (its cache was likely cold after {} min idle: this wake re-reads ~{}k tokens)", m, s.usage.context / 1000)).unwrap_or_default();
            let what = match tool {
                "clear" | "show_context" => {
                    let cmd = if tool == "clear" { "/clear" } else { "/context" };
                    s.pending.push(cmd.to_string());
                    format!("{} queued for {}; it runs when {} is next idle (now {})", cmd, target, target, s.activity.label())
                }
                "type_text" => {
                    let submit = args["submit"].as_bool().unwrap_or(true);
                    let text = args["text"].as_str().unwrap_or_default();
                    s.type_text(text, submit);
                    format!("typed into {}{}: {}{}", target, if submit { " and pressed Enter" } else { "" }, clip(text, 80), if submit { cold } else { String::new() })
                }
                "send_keys" => {
                    let keys: Vec<&str> = args["keys"].as_array().map(|a| a.iter().filter_map(|k| k.as_str()).collect()).unwrap_or_default();
                    let mut bytes = Vec::new();
                    for k in &keys {
                        bytes.extend(crate::term::named_key(k).ok_or_else(|| format!("unknown key {:?}", k))?);
                    }
                    s.write(&bytes);
                    format!("pressed {} in {}", keys.join(" "), target)
                }
                _ => {
                    if s.rec.role == Role::Hr {
                        return Err("hr can't stop itself; tackle would only restart it".into());
                    }
                    s.stop();
                    format!("stopping {} (asked it to exit; killed in 3 s if it hasn't)", target)
                }
            };
            done(f, what)
        }
        _ => Err(format!("unknown tool {}", tool)),
    }
}
