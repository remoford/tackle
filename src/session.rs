use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Hr,
    Orchestrator,
    Worker,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Hr => "hr",
            Role::Orchestrator => "orchestrator",
            Role::Worker => "worker",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Activity {
    Starting,
    Idle,
    Busy,
    Waiting,
    Exited,
}

impl Activity {
    pub fn label(self) -> &'static str {
        match self {
            Activity::Starting => "starting",
            Activity::Idle => "idle",
            Activity::Busy => "busy",
            Activity::Waiting => "needs input",
            Activity::Exited => "exited",
        }
    }
}

/// What tackle remembers about a session, on disk across restarts of tackle. `tid` is
/// tackle's own id: it survives /clear (which gives Claude a new session id) and restarts.
#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub tid: String,
    pub name: String,
    pub role: Role,
    pub cwd: PathBuf,
    pub model: String,
    /// The session that started this one; None means the human.
    pub manager: Option<String>,
    /// Claude's current session id, for `claude --resume`.
    #[serde(default)]
    pub claude_id: String,
    #[serde(default)]
    pub rc_url: String,
    /// The commit its CLAUDE.md imports were loaded at (its read).
    #[serde(default)]
    pub read_commit: Option<String>,
    /// A compacted session is dead: it may only be started fresh.
    #[serde(default)]
    pub compacted: bool,
    /// What it has cost so far at API list prices, across clears and restarts.
    #[serde(default)]
    pub cost_usd: f64,
}

impl Record {
    pub fn new(name: String, role: Role, cwd: PathBuf, model: String, manager: Option<String>) -> Record {
        Record { tid: uuid::Uuid::new_v4().to_string(), name, role, cwd, model, manager, claude_id: String::new(), rc_url: String::new(), read_commit: None, compacted: false, cost_usd: 0.0 }
    }

    /// Where the session keeps its state file (see docs/orchestration.md).
    pub fn state_file(&self) -> PathBuf {
        self.cwd.join("orchestration").join("state").join(format!("{}.md", self.name))
    }
}

/// How to launch: a new Claude session, or `--resume` of the recorded one.
pub enum Launch {
    Fresh,
    Resume,
}

/// Token counts from the transcript. `context` is the prompt size of the latest
/// assistant message, which is what the session is carrying right now.
#[derive(Default, Clone)]
pub struct Usage {
    pub context: u64,
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub total_input: u64,
    pub total_cache_read: u64,
    pub total_cache_write: u64,
    pub total_output: u64,
    pub messages: u64,
    last_id: String,
}

impl Usage {
    pub fn hit_rate(&self) -> Option<f64> {
        let all = self.input + self.cache_read + self.cache_write;
        (all > 0).then(|| self.cache_read as f64 / all as f64)
    }
}

pub struct Transcript {
    pub path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    /// Messages before this offset were already costed (a resumed session's history).
    costed_to: u64,
}

impl Transcript {
    fn new(path: PathBuf, resumed: bool) -> Self {
        let costed_to = if resumed { std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) } else { 0 };
        Transcript { path, offset: 0, partial: Vec::new(), costed_to }
    }

    /// Reads whatever was appended since the last poll, keeping the latest assistant text
    /// in `said`. Returns true if a compaction boundary went by.
    fn poll(&mut self, usage: &mut Usage, said: &mut String, cost: &mut f64) -> bool {
        let Ok(mut f) = std::fs::File::open(&self.path) else { return false };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return false;
        }
        let mut buf = Vec::new();
        let Ok(n) = f.read_to_end(&mut buf) else { return false };
        self.offset += n as u64;
        self.partial.extend_from_slice(&buf);
        let mut compacted = false;
        let base = self.offset - self.partial.len() as u64;
        let mut consumed = 0u64;
        while let Some(i) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=i).collect();
            consumed += line.len() as u64;
            let before_costed = base + consumed <= self.costed_to;
            let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue };
            if v["type"] == "system" && v["subtype"] == "compact_boundary" {
                compacted = true;
            }
            if v["type"] != "assistant" {
                continue;
            }
            if let Some(blocks) = v["message"]["content"].as_array() {
                let text: Vec<&str> = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
                if !text.is_empty() {
                    *said = clip(&text.join(" "), 400);
                }
            }
            let u = &v["message"]["usage"];
            if !u.is_object() {
                continue;
            }
            let get = |k: &str| u[k].as_u64().unwrap_or(0);
            usage.input = get("input_tokens");
            usage.cache_read = get("cache_read_input_tokens");
            usage.cache_write = get("cache_creation_input_tokens");
            usage.output = get("output_tokens");
            usage.context = usage.input + usage.cache_read + usage.cache_write;
            // One API message is split over several transcript lines that repeat its usage.
            let id = v["message"]["id"].as_str().unwrap_or_default();
            if id != usage.last_id {
                usage.last_id = id.to_string();
                usage.messages += 1;
                usage.total_input += usage.input;
                usage.total_cache_read += usage.cache_read;
                usage.total_cache_write += usage.cache_write;
                usage.total_output += usage.output;
                if !before_costed {
                    *cost += crate::pricing::cost(v["message"]["model"].as_str().unwrap_or_default(), u).unwrap_or(0.0);
                }
            }
        }
        compacted
    }
}

/// A one-line, length-capped rendering of text for hr to read cheaply.
pub fn clip(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    format!("{}…", flat.chars().take(max).collect::<String>())
}

/// "Bash: cargo test", "Read: src/main.rs": a tool call as one short line.
pub fn tool_line(v: &Value) -> String {
    let name = v["tool_name"].as_str().unwrap_or("tool");
    let input = &v["tool_input"];
    let arg = ["command", "file_path", "pattern", "url", "query", "description", "to", "name", "prompt"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .or_else(|| input.as_object().and_then(|o| o.values().find_map(|x| x.as_str())))
        .unwrap_or_default();
    clip(&format!("{}: {}", name, arg), 120)
}

pub fn ago(t: Instant) -> String {
    let s = t.elapsed().as_secs();
    if s >= 3600 {
        format!("{}h{}m", s / 3600, s / 60 % 60)
    } else if s >= 60 {
        format!("{}m{}s", s / 60, s % 60)
    } else {
        format!("{}s", s)
    }
}

/// A background command and the processes it started. Claude's background shell detaches
/// (its bash exits and the job is orphaned), so the job can't be found under the session's
/// process; tackle finds it instead by diffing the process list around its launch.
#[derive(Clone)]
pub struct Job {
    pub command: String,
    pub since: Instant,
    pub pids: Vec<(u32, String)>,
}

/// What a session is doing, from its hooks and transcript, so hr can answer "what is X
/// doing?" without reading a whole screen.
#[derive(Default)]
pub struct Doing {
    /// The prompt that started the current (or last) turn.
    pub prompt: String,
    pub turn_started: Option<Instant>,
    pub turn_tools: u32,
    /// The tool running now, and since when.
    pub tool: Option<(String, Instant)>,
    /// The last few finished tool calls, newest last.
    pub recent: VecDeque<String>,
    /// The latest thing the session said, from its transcript.
    pub said: String,
    /// What a permission prompt or other notification is asking.
    pub notice: String,
    pub last_turn_secs: u64,
    /// When its last turn ended: after the prompt cache expires, the next wake re-reads
    /// the whole context at full price.
    pub last_turn_end: Option<Instant>,
    /// Work it started with run_in_background that is still running.
    pub jobs: Vec<Job>,
    /// The last line of its state file, and when it changed.
    pub state_line: String,
    pub state_changed: Option<Instant>,
    /// Set when the state file's last line is "BETWEEN UNITS <commit>".
    pub between_units: Option<String>,
    /// Commits landed since its read that it did not make.
    pub stale: Option<usize>,
    /// A wake after a gap: (gap in seconds, context at the time), measured when the next
    /// usage arrives.
    pub wake: Option<(u64, u64)>,
}

pub struct Session {
    pub rec: Record,
    pub parser: Arc<Mutex<vt100::Parser>>,
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    pub pid: Option<u32>,
    pub activity: Activity,
    pub transcript: Option<Transcript>,
    pub usage: Usage,
    /// (seconds since tackle started, context tokens)
    pub history: Vec<(f64, u64)>,
    /// A compaction happened (or was attempted and blocked).
    pub contaminated: bool,
    pub compaction_blocked: bool,
    /// Slash commands waiting for the session to be idle between turns.
    pub pending: Vec<String>,
    /// A first prompt to type once it is ready.
    pub brief: Option<String>,
    /// Nothing is typed into it while set (tackle is reading a /usage screen).
    pub hold: bool,
    /// Asked to exit; killed if still running at this time.
    pub stop_at: Option<Instant>,
    pub doing: Doing,
    pub size: (u16, u16),
}

pub fn find_claude() -> PathBuf {
    if let Some(p) = std::env::var_os("TACKLE_CLAUDE") {
        return p.into();
    }
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let exe = dir.join("claude.exe");
        if exe.is_file() {
            return exe;
        }
        // npm installs a claude.cmd shim; run the real exe so Ctrl+C isn't eaten by cmd.
        let shim = dir.join("node_modules/@anthropic-ai/claude-code/bin/claude.exe");
        if dir.join("claude.cmd").is_file() && shim.is_file() {
            return shim;
        }
    }
    PathBuf::from("claude.exe")
}

/// Answers the terminal queries a real terminal would, so nothing waits on a timeout.
fn answer_queries(bytes: &[u8], parser: &Mutex<vt100::Parser>, writer: &Mutex<Box<dyn Write + Send>>) {
    let has = |pat: &[u8]| bytes.windows(pat.len()).any(|w| w == pat);
    let mut reply = Vec::new();
    if has(b"\x1b[6n") {
        let (r, c) = parser.lock().unwrap().screen().cursor_position();
        reply.extend(format!("\x1b[{};{}R", r + 1, c + 1).bytes());
    }
    if has(b"\x1b[c") || has(b"\x1b[0c") {
        reply.extend(b"\x1b[?1;2c");
    }
    if !reply.is_empty() {
        let mut w = writer.lock().unwrap();
        let _ = w.write_all(&reply);
        let _ = w.flush();
    }
}

pub struct Launcher<'a> {
    pub claude: &'a Path,
    pub settings: &'a Path,
    pub mcp: Option<&'a Path>,
    pub port: u16,
    pub ctx: eframe::egui::Context,
}

impl Session {
    pub fn spawn(mut rec: Record, launch: Launch, brief: Option<String>, l: Launcher) -> Result<Session, String> {
        let size = (40u16, 120u16);
        let pair = native_pty_system()
            .openpty(PtySize { rows: size.0, cols: size.1, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| format!("openpty: {}", e))?;
        let mut cmd = CommandBuilder::new(l.claude);
        cmd.args(["--remote-control", &rec.name, "-n", &rec.name]);
        match launch {
            Launch::Resume if !rec.claude_id.is_empty() => cmd.args(["--resume", &rec.claude_id]),
            _ => {
                rec.claude_id = uuid::Uuid::new_v4().to_string();
                rec.read_commit = None;
                rec.compacted = false;
                cmd.args(["--session-id", &rec.claude_id]);
            }
        }
        cmd.arg("--settings");
        cmd.arg(l.settings);
        if let Some(mcp) = l.mcp {
            cmd.arg("--mcp-config");
            cmd.arg(mcp);
        }
        if !rec.model.is_empty() {
            cmd.args(["--model", &rec.model]);
        }
        cmd.cwd(&rec.cwd);
        // tackle may itself be started from a Claude session; its children must be top-level.
        for k in ["CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT"] {
            cmd.env_remove(k);
        }
        // No compaction, ever: auto-compaction off, and /compact itself refused.
        cmd.env("DISABLE_AUTO_COMPACT", "1");
        cmd.env("DISABLE_COMPACT", "1");
        cmd.env("TACKLE_SESSION", &rec.name);
        cmd.env("TACKLE_ID", &rec.tid);
        cmd.env("TACKLE_STATE_FILE", rec.state_file());
        cmd.env("TACKLE_PORT", l.port.to_string());
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(cmd).map_err(|e| format!("spawn {}: {}", l.claude.display(), e))?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer().map_err(|e| e.to_string())?));
        let parser = Arc::new(Mutex::new(vt100::Parser::new(size.0, size.1, 5000)));
        {
            let parser = parser.clone();
            let writer = writer.clone();
            let ctx = l.ctx;
            std::thread::spawn(move || {
                let mut buf = [0u8; 16384];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    parser.lock().unwrap().process(&buf[..n]);
                    answer_queries(&buf[..n], &parser, &writer);
                    ctx.request_repaint();
                }
            });
        }
        Ok(Session {
            rec,
            parser,
            master: pair.master,
            writer,
            pid: child.process_id(),
            child,
            activity: Activity::Starting,
            transcript: None,
            usage: Usage::default(),
            history: Vec::new(),
            contaminated: false,
            compaction_blocked: false,
            pending: Vec::new(),
            brief,
            hold: false,
            stop_at: None,
            doing: Doing::default(),
            size,
        })
    }

    pub fn name(&self) -> &str {
        &self.rec.name
    }

    pub fn write(&self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    /// Types text, then presses Enter if `submit`. Enter goes separately, a moment later,
    /// so the TUI doesn't take the burst for a paste and turn the Enter into a newline.
    pub fn type_text(&self, text: &str, submit: bool) {
        self.write(text.as_bytes());
        if !submit {
            return;
        }
        let writer = self.writer.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let mut w = writer.lock().unwrap();
            let _ = w.write_all(b"\r");
            let _ = w.flush();
        });
    }

    /// Types the brief, or the next queued slash command, if the session is between turns.
    pub fn run_pending(&mut self) {
        if self.activity != Activity::Idle || self.hold {
            return;
        }
        if let Some(brief) = self.brief.take() {
            self.type_text(&brief, true);
            self.activity = Activity::Busy;
            return;
        }
        if self.pending.is_empty() {
            return;
        }
        let cmd = self.pending.remove(0);
        self.type_text(&cmd, true);
        if cmd == "/clear" {
            // SessionStart (source "clear") brings it back to idle.
            self.activity = Activity::Starting;
        }
    }

    /// Minutes since its last turn ended, if it is past the cache lifetime.
    pub fn cold(&self, ttl_min: u64) -> Option<u64> {
        let t = self.doing.last_turn_end?;
        let m = t.elapsed().as_secs() / 60;
        (ttl_min > 0 && m >= ttl_min && matches!(self.activity, Activity::Idle | Activity::Waiting)).then_some(m)
    }

    /// Its state in one phrase: busy, idle, or idle but waiting on its own background work.
    pub fn state_phrase(&self) -> String {
        if self.activity == Activity::Idle && !self.doing.jobs.is_empty() {
            let what: Vec<String> = self.doing.jobs.iter().map(|j| format!("{} ({})", clip(&j.command, 50), ago(j.since))).collect();
            return format!("turn ended, background work running: {}", what.join("; "));
        }
        self.activity.label().to_string()
    }

    /// A few lines for hr: state, the turn's prompt, the tool running now, recent tool
    /// calls, what the session last said, and what tackle knows about its read.
    pub fn report(&self, ttl_min: u64) -> String {
        let d = &self.doing;
        let mut out = vec![match (self.activity, d.turn_started) {
            (Activity::Busy, Some(t)) => format!("{}: busy for {}, {} tool calls so far this turn", self.name(), ago(t), d.turn_tools),
            (Activity::Idle, _) => {
                let mut s = format!("{}: {}", self.name(), self.state_phrase());
                if let Some(t) = d.last_turn_end {
                    s += &format!("; last turn ended {} ago, took {}s", ago(t), d.last_turn_secs);
                }
                s
            }
            _ => format!("{}: {}", self.name(), self.activity.label()),
        }];
        if let Some(m) = self.cold(ttl_min) {
            out.push(format!("cache likely cold ({} min idle): a wake re-reads ~{}k tokens", m, self.usage.context / 1000));
        }
        if !d.notice.is_empty() {
            out.push(format!("waiting on: {}", d.notice));
        }
        if let Some((tool, t)) = &d.tool {
            out.push(format!("running: {} ({})", tool, ago(*t)));
        }
        if !d.prompt.is_empty() {
            out.push(format!("prompt: {}", d.prompt));
        }
        if !d.recent.is_empty() {
            out.push(format!("recent: {}", d.recent.iter().cloned().collect::<Vec<_>>().join(" | ")));
        }
        if !d.said.is_empty() {
            out.push(format!("last said: {}", d.said));
        }
        if !d.state_line.is_empty() {
            out.push(format!("state file: {}", d.state_line));
        }
        if !self.pending.is_empty() {
            out.push(format!("queued: {}", self.pending.join(" ")));
        }
        let mut k = format!("context: {} tokens", self.usage.context);
        if let Some(c) = &self.rec.read_commit {
            k += &format!("; read at {}", &c[..c.len().min(10)]);
        }
        if let Some(n) = d.stale {
            k += &format!(", stale by {} outside commits", n);
        }
        if self.contaminated {
            k += ", COMPACTED";
        }
        out.push(k);
        out.join("\n")
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.size = (rows, cols);
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        self.parser.lock().unwrap().set_size(rows, cols);
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        self.activity = Activity::Exited;
    }

    /// Asks Claude to exit with Ctrl+C (the first interrupts a turn, the next ones quit),
    /// so it signs off Remote Control instead of leaving a stale entry under its name.
    /// check_exit kills it if it is still running three seconds later.
    pub fn stop(&mut self) {
        if self.activity == Activity::Exited || self.stop_at.is_some() {
            return;
        }
        let writer = self.writer.clone();
        std::thread::spawn(move || {
            for _ in 0..3 {
                if let Ok(mut w) = writer.lock() {
                    let _ = w.write_all(b"\x03");
                    let _ = w.flush();
                }
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        });
        self.stop_at = Some(Instant::now() + std::time::Duration::from_secs(3));
    }

    pub fn check_exit(&mut self) {
        if self.activity != Activity::Exited && matches!(self.child.try_wait(), Ok(Some(_))) {
            self.activity = Activity::Exited;
        }
        if self.activity != Activity::Exited && self.stop_at.map(|t| Instant::now() >= t).unwrap_or(false) {
            self.kill();
        }
        if self.activity == Activity::Exited {
            self.stop_at = None;
        }
    }

    /// Updates its state from a hook. `blocked` is set when tackle refused the call.
    pub fn on_hook(&mut self, v: &Value, now: f64, blocked: Option<&str>) {
        let d = &mut self.doing;
        match v["hook_event_name"].as_str().unwrap_or_default() {
            "SessionStart" => {
                let source = v["source"].as_str().unwrap_or_default();
                if let Some(id) = v["session_id"].as_str() {
                    self.rec.claude_id = id.to_string();
                }
                if let Some(p) = v["transcript_path"].as_str() {
                    let p = PathBuf::from(p);
                    if self.transcript.as_ref().map(|t| &t.path) != Some(&p) {
                        self.transcript = Some(Transcript::new(p, source == "resume"));
                        self.usage = Usage::default();
                        self.history.push((now, 0));
                    }
                }
                // A fresh start or a clear loads CLAUDE.md and its imports from disk now.
                if matches!(source, "startup" | "clear") {
                    self.rec.read_commit = crate::git::head(&self.rec.cwd);
                    d.stale = Some(0);
                    d.between_units = None;
                }
                if source == "compact" {
                    self.contaminated = true;
                }
                self.activity = Activity::Idle;
            }
            "UserPromptSubmit" => {
                if let Some(t) = d.last_turn_end {
                    d.wake = Some((t.elapsed().as_secs(), self.usage.context));
                }
                self.activity = Activity::Busy;
                d.prompt = clip(v["prompt"].as_str().unwrap_or_default(), 300);
                d.turn_started = Some(Instant::now());
                d.turn_tools = 0;
                d.tool = None;
                d.notice.clear();
            }
            "PreToolUse" => {
                self.activity = Activity::Busy;
                d.notice.clear();
                match blocked {
                    Some(why) => {
                        d.recent.push_back(clip(&format!("BLOCKED {} ({})", tool_line(v), why), 160));
                    }
                    None => {
                        d.tool = Some((tool_line(v), Instant::now()));
                    }
                }
            }
            "PostToolUse" | "PostToolUseFailure" => {
                self.activity = Activity::Busy;
                d.tool = None;
                d.turn_tools += 1;
                let failed = if v["hook_event_name"] == "PostToolUseFailure" { " (failed)" } else { "" };
                d.recent.push_back(format!("{}{}", tool_line(v), failed));
            }
            "Notification" => {
                if v["notification_type"] != "idle_prompt" {
                    self.activity = Activity::Waiting;
                    d.notice = clip(v["message"].as_str().unwrap_or_default(), 200);
                }
            }
            "PermissionRequest" => {
                self.activity = Activity::Waiting;
                d.notice = clip(&format!("permission for {}", tool_line(v)), 200);
            }
            "Stop" | "StopFailure" => {
                self.activity = Activity::Idle;
                d.tool = None;
                d.notice.clear();
                d.last_turn_end = Some(Instant::now());
                if let Some(t) = d.turn_started.take() {
                    d.last_turn_secs = t.elapsed().as_secs();
                }
            }
            "PreCompact" => self.compaction_blocked = true,
            "PostCompact" => self.contaminated = true,
            _ => {}
        }
        while self.doing.recent.len() > 5 {
            self.doing.recent.pop_front();
        }
    }

    /// Reads new transcript lines. Returns a finished wake measurement, if one completed.
    pub fn poll_transcript(&mut self, now: f64) -> Option<String> {
        let t = self.transcript.as_mut()?;
        let before = self.usage.messages;
        if t.poll(&mut self.usage, &mut self.doing.said, &mut self.rec.cost_usd) {
            self.contaminated = true;
        }
        if self.usage.messages == before {
            return None;
        }
        self.history.push((now, self.usage.context));
        let (gap, context) = self.doing.wake.take()?;
        let u = &self.usage;
        Some(format!(
            "wake {} after {}m idle, context {}k: cache read {}, cache write {}, uncached input {}",
            self.rec.name,
            gap / 60,
            context / 1000,
            u.cache_read,
            u.cache_write,
            u.input
        ))
    }

    /// Reads the last line of its state file and notes a handover.
    pub fn poll_state_file(&mut self) {
        let Ok(text) = std::fs::read_to_string(self.rec.state_file()) else { return };
        let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().to_string();
        if last != self.doing.state_line {
            self.doing.state_line = last.clone();
            self.doing.state_changed = Some(Instant::now());
        }
        // "15:05 BETWEEN UNITS <commit>" or "BETWEEN UNITS <commit>"
        self.doing.between_units = last.find("BETWEEN UNITS").map(|i| last[i + "BETWEEN UNITS".len()..].trim().to_string());
    }

    /// Catches prompts that come before any hook fires, like Claude's folder-trust
    /// question on a session's first start in a new directory. Without this the session
    /// sits at "starting" and its brief never goes out.
    pub fn poll_screen(&mut self) {
        if self.activity != Activity::Starting {
            return;
        }
        if self.parser.lock().unwrap().screen().contents().contains("trust this folder") {
            self.activity = Activity::Waiting;
            self.doing.notice = "folder trust: Claude asks whether to trust this project directory (answer in tackle, or ask tk-hr)".into();
        }
    }

    /// Drops background jobs whose processes have all ended.
    pub fn poll_jobs(&mut self, alive: &std::collections::HashSet<u32>) {
        self.doing.jobs.retain(|j| j.pids.iter().any(|(pid, _)| alive.contains(pid)));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
