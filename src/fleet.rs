// The fleet of sessions and the engine that looks after it. This runs on its own thread,
// not in the GUI's update loop, because Windows stops repainting a window hidden in the
// tray and hr's tools, hooks and the rules must keep working while it is.

use crate::hooks;
use crate::session::{self, clip, Activity, Launch, Launcher, Record, Role, Session};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Every session tackle starts is named tk-..., so tackle's sessions stand out in
/// ListAgents and Remote Control from the ones it doesn't manage.
pub const PREFIX: &str = "tk-";
pub const HR: &str = "tk-hr";
/// hr's working directory under tackle's data directory.
const HR_DIR: &str = "hr";

pub fn tk(name: &str) -> String {
    if name.starts_with(PREFIX) { name.to_string() } else { format!("{}{}", PREFIX, name) }
}

pub fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub type Shared = Arc<Mutex<Fleet>>;

/// A session outside tackle that the human lets act for them, keyed by the address the
/// messaging harness gives it (stable while that session runs), with its name for display.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Delegate {
    pub name: String,
    pub address: String,
}

/// A request tackle refused, kept so the human can trust the requester with one click.
#[derive(Clone)]
pub struct Denial {
    pub when: String,
    pub name: String,
    pub address: String,
    pub what: String,
}

/// The build lock: one heavy command (lake, xelatex, ...) at a time across the fleet.
pub struct BuildLock {
    pub session: String,
    pub tool_use_id: String,
    pub command: String,
    pub since: Instant,
    pub background: bool,
    /// Its PreToolUse has been answered by a PostToolUse (the background shell exists).
    pub launched: bool,
    /// For a background build, the processes it started.
    pub pids: Vec<u32>,
}

/// Everything tackle keeps on disk in fleet.json. Fields missing from an older file take
/// their defaults.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub hr_clear_at: u64,
    /// Clear a session that has written BETWEEN UNITS once its context reaches this; 0
    /// means never.
    pub others_clear_at: u64,
    pub window: u64,
    /// Prompt-cache lifetime assumed for "cache likely cold" warnings.
    pub cache_ttl_min: u64,
    /// Words in a Bash command that take the build lock.
    pub lock_words: Vec<String>,
    /// Project directories where only the writer-token holder may edit or commit.
    pub one_writer: Vec<PathBuf>,
    /// Project directory -> session name holding its writer token.
    pub writers: BTreeMap<String, String>,
    pub delegates: Vec<Delegate>,
    /// Sessions tackle knows about, running or not (not hr).
    pub sessions: Vec<Record>,
    /// Minutes between /usage readings taken on hr; 0 means never.
    pub usage_every_min: u64,
    /// What the plan costs a month, to turn a share of the weekly limit into dollars.
    pub plan_usd_month: f64,
}

impl Default for Saved {
    fn default() -> Saved {
        Saved {
            hr_clear_at: 150_000,
            others_clear_at: 0,
            window: 1_000_000,
            cache_ttl_min: 60,
            lock_words: vec!["lake".into(), "xelatex".into(), "latexmk".into()],
            one_writer: Vec::new(),
            writers: BTreeMap::new(),
            delegates: Vec::new(),
            sessions: Vec::new(),
            usage_every_min: 10,
            plan_usd_month: 200.0,
        }
    }
}

pub struct Fleet {
    pub sessions: Vec<Session>,
    /// Sessions from an earlier run of tackle, or stopped ones, that can be resumed.
    pub dormant: Vec<Record>,
    pub s: Saved,
    pub error: String,
    pub data: PathBuf,
    pub lock: Option<BuildLock>,
    pub denials: VecDeque<Denial>,
    /// Recent entries of actions.log, newest last.
    pub log: VecDeque<String>,
    /// Commit hash -> tid of the session that made it (commits.json).
    commits: BTreeMap<String, String>,
    /// Plan-usage readings, oldest first (usage.jsonl, last 8 days).
    pub usage: Vec<crate::usage::Reading>,
    usage_next: Instant,
    /// When /usage was typed into hr, while tackle waits to read it.
    usage_capture: Option<Instant>,
    /// Process ids alive just before each run_in_background call, by tool_use_id.
    launch_before: BTreeMap<String, Vec<u32>>,
    claude: PathBuf,
    settings: PathBuf,
    hr_settings: PathBuf,
    mcp_config: PathBuf,
    port: u16,
    epoch: Instant,
    hr_restart: Option<Instant>,
    last_slow: Instant,
    last_git: Instant,
    last_saved: String,
    ctx: eframe::egui::Context,
}

const HR_CLAUDE_MD: &str = include_str!("hr.md");

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn boot(ctx: eframe::egui::Context) -> Shared {
    let data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("tackle");
    let mut errors = Vec::new();
    let _ = std::fs::create_dir_all(data.join(HR_DIR));
    if let Err(e) = std::fs::write(data.join(HR_DIR).join("CLAUDE.md"), HR_CLAUDE_MD) {
        errors.push(format!("hr CLAUDE.md: {}", e));
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0");
    let port = listener.as_ref().ok().and_then(|l| l.local_addr().ok()).map(|a| a.port()).unwrap_or(0);
    let mut write = |file: &str, r: std::io::Result<PathBuf>| {
        r.unwrap_or_else(|e| {
            errors.push(format!("{}: {}", file, e));
            data.join(file)
        })
    };
    let settings = write("settings.json", hooks::write_settings(&data, "settings.json", &[]));
    let hr_settings = write("hr-settings.json", hooks::write_settings(&data, "hr-settings.json", &["mcp__tackle"]));
    let mcp_config = write("mcp.json", hooks::write_mcp_config(&data, port, HR));
    // For the `tk` command line.
    let _ = std::fs::write(data.join("port"), port.to_string());
    let s: Saved = read_json(&data.join("fleet.json")).unwrap_or_default();
    let commits = read_json(&data.join("commits.json")).unwrap_or_default();
    let dormant = s.sessions.clone();
    let cutoff = chrono::Local::now() - chrono::Duration::days(8);
    let usage: Vec<crate::usage::Reading> = std::fs::read_to_string(data.join("usage.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<crate::usage::Reading>(l).ok())
        .filter(|r| r.at >= cutoff)
        .collect();
    let fleet = Arc::new(Mutex::new(Fleet {
        sessions: Vec::new(),
        dormant,
        s,
        error: errors.join("; "),
        data,
        lock: None,
        denials: VecDeque::new(),
        log: VecDeque::new(),
        commits,
        usage,
        usage_next: Instant::now() + Duration::from_secs(30),
        usage_capture: None,
        launch_before: BTreeMap::new(),
        claude: session::find_claude(),
        settings,
        hr_settings,
        mcp_config,
        port,
        epoch: Instant::now(),
        hr_restart: None,
        last_slow: Instant::now(),
        last_git: Instant::now(),
        last_saved: String::new(),
        ctx,
    }));
    match listener {
        Ok(l) => hooks::serve(l, fleet.clone()),
        Err(e) => fleet.lock().unwrap().error = format!("could not open tackle port: {}", e),
    }
    {
        let mut f = fleet.lock().unwrap();
        f.check_compaction_config();
        f.spawn_hr();
    }
    let engine = fleet.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(500));
        engine.lock().unwrap().tick();
    });
    fleet
}

/// Who is asking hr for something, as far as tackle can tell.
#[derive(Clone)]
pub enum Who {
    Human,
    Delegate(String),
    Session(String),
    Unknown { name: String, address: String },
}

impl Who {
    pub fn label(&self) -> String {
        match self {
            Who::Human => "human".into(),
            Who::Delegate(n) => format!("{} (delegate)", n),
            Who::Session(n) => n.clone(),
            Who::Unknown { name, .. } => format!("{} (not trusted)", name),
        }
    }
}

impl Fleet {
    pub fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// Finds a session by name, with or without the tk- prefix.
    pub fn find(&mut self, name: &str) -> Option<&mut Session> {
        let name = tk(name.trim());
        self.sessions.iter_mut().find(|s| s.rec.name == name)
    }

    pub fn find_ref(&self, name: &str) -> Option<&Session> {
        let name = tk(name.trim());
        self.sessions.iter().find(|s| s.rec.name == name)
    }

    fn record_of(&self, name: &str) -> Option<&Record> {
        let name = tk(name.trim());
        self.find_ref(&name).map(|s| &s.rec).or_else(|| self.dormant.iter().find(|r| r.name == name))
    }

    pub fn add_log(&mut self, line: String) {
        let line = format!("{} {}", stamp(), line);
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.data.join("actions.log")) {
            let _ = writeln!(f, "{}", line);
        }
        self.log.push_back(line);
        while self.log.len() > 200 {
            self.log.pop_front();
        }
    }

    /// Rule 3 depends on this: sessions get DISABLE_AUTO_COMPACT and DISABLE_COMPACT, and
    /// tackle also reports the user-wide switch so a change to it is visible.
    fn check_compaction_config(&mut self) {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        let off = [home.join(".claude").join("settings.json"), home.join(".claude.json")]
            .iter()
            .filter_map(|p| read_json::<Value>(p))
            .any(|v| v["autoCompactEnabled"] == false);
        if !off {
            self.add_log("warning: neither ~/.claude/settings.json nor ~/.claude.json sets autoCompactEnabled to false; tk sessions rely on DISABLE_AUTO_COMPACT alone".into());
        }
    }

    // ---- launching ----

    fn launch(&mut self, rec: Record, launch: Launch, brief: Option<String>) -> Result<(), String> {
        let hr = rec.role == Role::Hr;
        let l = Launcher {
            claude: &self.claude,
            settings: if hr { &self.hr_settings } else { &self.settings },
            mcp: if hr { Some(self.mcp_config.as_path()) } else { None },
            port: self.port,
            ctx: self.ctx.clone(),
        };
        let s = Session::spawn(rec, launch, brief, l)?;
        self.dormant.retain(|r| r.name != s.rec.name);
        match self.sessions.iter().position(|x| x.rec.name == s.rec.name) {
            Some(i) => self.sessions[i] = s,
            None => self.sessions.push(s),
        }
        Ok(())
    }

    fn running(&self, name: &str) -> bool {
        self.sessions.iter().any(|s| s.rec.name == name && s.activity != Activity::Exited)
    }

    /// Starts a new session in a project (a name from projects.json, or a directory). With
    /// no name given, it is named after the project: tk-ic-1, tk-ic-2, ...
    pub fn start_in(&mut self, project: &str, name: &str, model: &str, role: Role, manager: Option<String>, brief: Option<String>) -> Result<String, String> {
        let project = crate::projects::resolve(&self.data, project)?;
        let taken = |f: &Fleet, n: &str| f.running(n) || f.dormant.iter().any(|r| r.name == n);
        let name = match name.trim() {
            "" => {
                let base: String = project.name.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
                (1..).map(|i| format!("{}{}-{}", PREFIX, base, i)).find(|n| !taken(self, n)).unwrap()
            }
            n if n.contains(char::is_whitespace) => return Err("name must be one word".into()),
            n if self.running(&tk(n)) => return Err(format!("{} is already running", tk(n))),
            n => tk(n),
        };
        let model = if model.trim().is_empty() { "opus" } else { model.trim() };
        let rec = Record::new(name.clone(), role, project.path, model.to_string(), manager);
        let brief = brief.filter(|b| !b.trim().is_empty()).map(|b| self.brief_for(&rec, &b));
        self.launch(rec, Launch::Fresh, brief)?;
        Ok(name)
    }

    /// The first prompt for a session: who it is, where its state file lives, then the
    /// manager's brief. It goes in as a prompt, after the corpus, so it never disturbs the
    /// cached prefix every session shares.
    fn brief_for(&self, rec: &Record, brief: &str) -> String {
        let manager = rec.manager.clone().unwrap_or_else(|| "the human".into());
        format!(
            "[tackle] You are {} ({}; your manager is {}). Keep your state file at {} as described in {}.\n\n{}",
            rec.name,
            rec.role.label(),
            manager,
            rec.state_file().display(),
            orchestration_doc().display(),
            brief
        )
    }

    /// Resumes a stopped or dormant session by its Claude session id. Not for a compacted
    /// one: its transcript carries the compacted context.
    pub fn resume(&mut self, name: &str) -> Result<(), String> {
        let rec = self.record_of(name).cloned().ok_or_else(|| format!("no session named {:?}", name))?;
        if self.running(&rec.name) {
            return Err(format!("{} is still running", rec.name));
        }
        if rec.compacted {
            return Err(format!("{} was compacted; it can only be started fresh from its state file", rec.name));
        }
        if rec.claude_id.is_empty() {
            return Err(format!("{} has no Claude session to resume; start it fresh", rec.name));
        }
        self.launch(rec, Launch::Resume, None)
    }

    /// Starts a stopped, dormant or compacted session again as a new Claude session with
    /// the same name, role, project and manager, told to pick up from its state file.
    pub fn fresh(&mut self, name: &str, brief: Option<String>) -> Result<(), String> {
        let rec = self.record_of(name).cloned().ok_or_else(|| format!("no session named {:?}", name))?;
        if self.running(&rec.name) {
            return Err(format!("{} is still running; stop it first", rec.name));
        }
        let text = brief.filter(|b| !b.trim().is_empty()).unwrap_or_else(|| "Resume from your state file: read it first, then carry on with your unit.".into());
        let brief = Some(self.brief_for(&rec, &text));
        self.launch(rec, Launch::Fresh, brief)
    }

    pub fn forget(&mut self, name: &str) -> Result<(), String> {
        let name = tk(name.trim());
        if self.running(&name) {
            return Err(format!("{} is still running", name));
        }
        self.sessions.retain(|s| s.rec.name != name);
        self.dormant.retain(|r| r.name != name);
        Ok(())
    }

    fn spawn_hr(&mut self) {
        let rec = Record::new(HR.into(), Role::Hr, self.data.join(HR_DIR), "haiku".into(), None);
        if let Err(e) = self.launch(rec, Launch::Fresh, None) {
            self.error = format!("hr: {}", e);
        }
    }

    // ---- authority ----

    /// Identifies a requester from what hr passes on: "human", or the `from` address and
    /// `from-name` the messaging harness attached to a cross-session message.
    pub fn who(&self, requested_by: &str, requester_name: &str) -> Who {
        let (by, name) = (requested_by.trim(), requester_name.trim());
        if by.eq_ignore_ascii_case("human") {
            return Who::Human;
        }
        if name.starts_with(PREFIX) && self.running(name) {
            return Who::Session(name.to_string());
        }
        if let Some(d) = self.s.delegates.iter().find(|d| d.address == by) {
            return Who::Delegate(d.name.clone());
        }
        Who::Unknown { name: if name.is_empty() { "unnamed".into() } else { name.into() }, address: by.into() }
    }

    /// True if `ancestor` started `target`, directly or through sessions it started.
    pub fn is_ancestor(&self, ancestor: &str, target: &str) -> bool {
        let mut cur = self.record_of(target).and_then(|r| r.manager.clone());
        for _ in 0..16 {
            match cur {
                Some(m) if m == ancestor => return true,
                Some(m) => cur = self.record_of(&m).and_then(|r| r.manager.clone()),
                None => return false,
            }
        }
        false
    }

    /// May `who` act on session `target` (type, press keys, clear, stop, restart)? Only
    /// the human, their delegates, the session itself, and the sessions above it in the
    /// chain that started it.
    pub fn may_act(&self, who: &Who, target: &str) -> bool {
        match who {
            Who::Human | Who::Delegate(_) => true,
            Who::Session(n) => *n == tk(target) || self.is_ancestor(n, &tk(target)),
            Who::Unknown { .. } => false,
        }
    }

    pub fn deny(&mut self, who: &Who, what: &str) -> String {
        if let Who::Unknown { name, address } = who {
            if !self.denials.iter().any(|d| d.address == *address && d.what == what) {
                self.denials.push_back(Denial { when: stamp(), name: name.clone(), address: address.clone(), what: what.into() });
                while self.denials.len() > 20 {
                    self.denials.pop_front();
                }
            }
        }
        self.add_log(format!("DENIED {}: {}", who.label(), what));
        format!(
            "tackle refused: {} may not {}. Only the human, their delegates, the session itself and the sessions that started it (directly or indirectly) may act on a session.",
            who.label(),
            what
        )
    }

    pub fn trust(&mut self, name: &str, address: &str) {
        if !self.s.delegates.iter().any(|d| d.address == address) {
            self.s.delegates.push(Delegate { name: name.into(), address: address.into() });
        }
        self.denials.retain(|d| d.address != address);
        self.add_log(format!("human trusted {} ({}) as a delegate", name, address));
    }

    pub fn revoke(&mut self, address: &str) {
        let gone: Vec<String> = self.s.delegates.iter().filter(|d| d.address == address).map(|d| d.name.clone()).collect();
        self.s.delegates.retain(|d| d.address != address);
        for n in gone {
            self.add_log(format!("human revoked delegate {} ({})", n, address));
        }
    }

    // ---- hooks: state, and the calls tackle refuses ----

    /// Handles one hook event and returns the reply for the hook process: {"block": why}
    /// makes it exit 2, which blocks the tool call or the compaction.
    pub fn on_hook(&mut self, name: &str, body: &Value) -> Value {
        let now = self.now();
        let event = body["hook_event_name"].as_str().unwrap_or_default().to_string();
        let block = match event.as_str() {
            "PreCompact" => {
                self.add_log(format!("{}: compaction attempted and blocked", name));
                Some("tackle: compaction is forbidden in this fleet (a compacted context is contaminated). Finish your unit, write your state file, and ask your manager to clear you.".to_string())
            }
            "PreToolUse" => self.check_tool(name, body),
            _ => None,
        };
        let id = body["tool_use_id"].as_str().unwrap_or_default().to_string();
        if event == "PreToolUse" && block.is_none() && body["tool_input"]["run_in_background"] == true {
            self.launch_before.insert(id.clone(), crate::procs::snapshot().iter().map(|p| p.pid).collect());
        }
        if event == "PostToolUse" {
            if let Some(before) = self.launch_before.remove(&id) {
                self.record_job(name, &id, body, &before);
            }
        }
        if event == "PostToolUseFailure" {
            self.launch_before.remove(&id);
        }
        if matches!(event.as_str(), "PostToolUse" | "PostToolUseFailure") {
            self.after_tool(name, body);
        }
        if let Some(s) = self.find(name) {
            s.on_hook(body, now, block.as_deref());
        }
        self.ctx.request_repaint();
        match block {
            Some(why) => json!({ "block": why }),
            None => json!({}),
        }
    }

    fn check_tool(&mut self, name: &str, body: &Value) -> Option<String> {
        let cwd = self.find_ref(name)?.rec.cwd.clone();
        let tool = body["tool_name"].as_str().unwrap_or_default();
        let command = body["tool_input"]["command"].as_str().unwrap_or_default();
        let words: Vec<String> = command.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-')).map(|w| w.to_lowercase()).collect();
        let has = |w: &str| words.iter().any(|x| x == w);

        // One writer at a time on a shared checkout.
        if self.s.one_writer.iter().any(|p| same_dir(p, &cwd)) {
            // orchestration/ is operational and gitignored: every session keeps its own state there.
            let file = body["tool_input"]["file_path"].as_str().or(body["tool_input"]["notebook_path"].as_str()).unwrap_or_default();
            let own_state = key(Path::new(file)).starts_with(&(key(&cwd) + r"\orchestration\"));
            let edits = matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit") && !own_state;
            let writes = edits || (tool == "Bash" && has("git") && (has("commit") || has("push")));
            let holder = self.s.writers.get(&key(&cwd)).cloned();
            if writes && holder.as_deref() != Some(name) {
                let who = holder.map(|h| format!("{} holds", h)).unwrap_or_else(|| "nobody holds".into());
                self.add_log(format!("{}: refused {} (writer token: {})", name, clip(&session::tool_line(body), 80), who));
                return Some(format!(
                    "tackle: {} the writer token for {}; only its holder may edit or commit. Ask your manager for it (pull before editing), or wait.",
                    who,
                    cwd.display()
                ));
            }
        }

        // One heavy build at a time.
        if tool == "Bash" && self.s.lock_words.iter().any(|w| has(&w.to_lowercase())) {
            if let Some(l) = &self.lock {
                let msg = format!(
                    "tackle: build lock held by {} ({}, {}). Only one {} at a time; wait and try again.",
                    l.session,
                    clip(&l.command, 60),
                    session::ago(l.since),
                    self.s.lock_words.join("/")
                );
                self.add_log(format!("{}: refused {} (build lock held by {})", name, clip(command, 60), l.session));
                return Some(msg);
            }
            self.lock = Some(BuildLock {
                session: name.to_string(),
                tool_use_id: body["tool_use_id"].as_str().unwrap_or_default().to_string(),
                command: command.to_string(),
                since: Instant::now(),
                background: body["tool_input"]["run_in_background"] == true,
                launched: false,
                pids: Vec::new(),
            });
            self.add_log(format!("{}: took the build lock for {}", name, clip(command, 60)));
        }
        None
    }

    /// A background command has launched: the processes that appeared since its
    /// PreToolUse are its job. They are orphans, so this is the only way to find them.
    fn record_job(&mut self, name: &str, id: &str, body: &Value, before: &[u32]) {
        let skip = ["tackle.exe", "conhost.exe", "openconsole.exe", "claude.exe", "git.exe", "tk.exe"];
        let new: Vec<(u32, String)> = crate::procs::snapshot().into_iter().filter(|p| !before.contains(&p.pid) && !skip.contains(&p.name.as_str())).map(|p| (p.pid, p.name)).collect();
        if new.is_empty() {
            return;
        }
        if let Some(l) = &mut self.lock {
            if l.tool_use_id == id {
                l.pids = new.iter().map(|(p, _)| *p).collect();
            }
        }
        if let Some(s) = self.find(name) {
            s.doing.jobs.push(session::Job { command: body["tool_input"]["command"].as_str().unwrap_or_default().to_string(), since: Instant::now(), pids: new });
        }
    }

    fn after_tool(&mut self, name: &str, body: &Value) {
        let id = body["tool_use_id"].as_str().unwrap_or_default();
        if let Some(l) = &mut self.lock {
            if l.session == name && l.tool_use_id == id {
                if l.background && body["hook_event_name"] == "PostToolUse" {
                    l.launched = true;
                } else {
                    let msg = format!("{}: released the build lock", name);
                    self.lock = None;
                    self.add_log(msg);
                }
            }
        }
        // Remember who made each commit, so staleness counts only outside commits.
        let command = body["tool_input"]["command"].as_str().unwrap_or_default();
        if body["tool_name"] == "Bash" && body["hook_event_name"] == "PostToolUse" && command.contains("git") && command.contains("commit") {
            if let Some(s) = self.find_ref(name) {
                if let Some(head) = crate::git::head(&s.rec.cwd) {
                    let tid = s.rec.tid.clone();
                    self.commits.insert(head, tid);
                    let _ = std::fs::write(self.data.join("commits.json"), serde_json::to_string(&self.commits).unwrap_or_default());
                }
            }
        }
    }

    pub fn release_lock(&mut self, by: &str) {
        if let Some(l) = self.lock.take() {
            self.add_log(format!("{} released {}'s build lock", by, l.session));
        }
    }

    pub fn grant_writer(&mut self, project: &Path, to: &str, by: &str) {
        self.s.writers.insert(key(project), tk(to));
        self.add_log(format!("{} granted the writer token for {} to {}", by, project.display(), tk(to)));
    }

    pub fn release_writer(&mut self, project: &Path, by: &str) {
        if let Some(h) = self.s.writers.remove(&key(project)) {
            self.add_log(format!("{} released {}'s writer token for {}", by, h, project.display()));
        }
    }

    // ---- the engine ----

    fn tick(&mut self) {
        let now = self.now();
        let slow = self.last_slow.elapsed() >= Duration::from_secs(3);
        let git = self.last_git.elapsed() >= Duration::from_secs(20);
        if slow {
            self.last_slow = Instant::now();
        }
        if git {
            self.last_git = Instant::now();
        }
        let alive: std::collections::HashSet<u32> = if slow { crate::procs::snapshot().iter().map(|p| p.pid).collect() } else { Default::default() };
        let mut logs = Vec::new();
        for s in &mut self.sessions {
            s.check_exit();
            if let Some(wake) = s.poll_transcript(now) {
                logs.push(wake);
            }
            if slow {
                s.poll_state_file();
                s.poll_jobs(&alive);
                s.poll_screen();
                if s.rec.rc_url.is_empty() {
                    s.rec.rc_url = rc_url(&s.parser.lock().unwrap());
                }
            }
            if git && s.activity != Activity::Exited {
                if let Some(read) = s.rec.read_commit.clone() {
                    if let Some(new) = crate::git::since(&s.rec.cwd, &read) {
                        s.doing.stale = Some(new.iter().filter(|h| self.commits.get(*h) != Some(&s.rec.tid)).count());
                    }
                }
            }
            // Rule 3: a compacted context is contaminated. Stop it; it may only come back fresh.
            if s.contaminated && !s.rec.compacted {
                s.rec.compacted = true;
                s.kill();
                logs.push(format!("{} compacted: stopped; start it fresh from its state file", s.rec.name));
            }
            // Rule 1: clear only at a handover. hr carries no state and clears on size alone.
            let clear = match s.rec.role {
                Role::Hr => self.s.hr_clear_at > 0 && s.usage.context >= self.s.hr_clear_at,
                _ => self.s.others_clear_at > 0 && s.doing.between_units.is_some() && s.usage.context >= self.s.others_clear_at,
            };
            if clear && s.activity != Activity::Exited && !s.pending.iter().any(|c| c == "/clear") {
                s.pending.push("/clear".into());
                if s.rec.role != Role::Hr {
                    logs.push(format!("{}: between units with {}k context; clear queued", s.rec.name, s.usage.context / 1000));
                }
            }
            s.run_pending();
        }
        for l in logs {
            self.add_log(l);
        }

        // A background build keeps the lock until the processes it started are gone.
        if slow {
            let release = match &self.lock {
                Some(l) if l.background && l.launched => !l.pids.iter().any(|p| alive.contains(p)),
                Some(l) if l.background => false,
                Some(l) => self.find_ref(&l.session).map(|s| s.activity == Activity::Exited).unwrap_or(true),
                None => false,
            };
            if release {
                self.release_lock("tackle");
            }
        }

        self.poll_usage();

        // hr is always alive: bring it back a few seconds after it exits.
        let hr_alive = self.sessions.iter().any(|s| s.rec.role == Role::Hr && s.activity != Activity::Exited);
        match self.hr_restart {
            _ if hr_alive => self.hr_restart = None,
            None => self.hr_restart = Some(Instant::now() + Duration::from_secs(3)),
            Some(t) if Instant::now() >= t => {
                self.hr_restart = None;
                self.spawn_hr();
            }
            _ => {}
        }
        if slow {
            self.save();
        }
        self.ctx.request_repaint();
    }

    /// Takes a /usage reading on hr when it is idle and one is due: types /usage, reads
    /// the screen three seconds later, presses Esc.
    fn poll_usage(&mut self) {
        let Some(hr) = self.sessions.iter_mut().find(|s| s.rec.role == Role::Hr) else { return };
        match self.usage_capture {
            None => {
                let due = self.s.usage_every_min > 0 && Instant::now() >= self.usage_next;
                if due && hr.activity == Activity::Idle && hr.pending.is_empty() && hr.brief.is_none() {
                    hr.hold = true;
                    hr.type_text("/usage", true);
                    self.usage_capture = Some(Instant::now());
                }
            }
            Some(t) if t.elapsed() >= Duration::from_secs(3) => {
                let screen = hr.parser.lock().unwrap().screen().contents();
                hr.write(b"\x1b");
                hr.hold = false;
                self.usage_capture = None;
                self.usage_next = Instant::now() + Duration::from_secs(self.s.usage_every_min.max(1) * 60);
                let limits = crate::usage::parse(&screen);
                if limits.is_empty() {
                    self.add_log("could not read plan usage from hr's /usage screen".into());
                    return;
                }
                let sessions = self
                    .sessions
                    .iter()
                    .map(|s| {
                        let u = &s.usage;
                        (s.rec.name.clone(), u.total_input + u.total_cache_write + u.total_cache_read + u.total_output)
                    })
                    .collect();
                let mut costs: BTreeMap<String, f64> = self.sessions.iter().map(|s| (s.rec.name.clone(), s.rec.cost_usd)).collect();
                for r in &self.dormant {
                    costs.entry(r.name.clone()).or_insert(r.cost_usd);
                }
                let reading = crate::usage::Reading { at: chrono::Local::now(), limits, sessions, costs };
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.data.join("usage.jsonl")) {
                    let _ = writeln!(f, "{}", serde_json::to_string(&reading).unwrap_or_default());
                }
                self.usage.push(reading);
            }
            _ => {}
        }
    }

    /// Takes a /usage reading at the next chance.
    pub fn usage_now(&mut self) {
        self.usage_next = Instant::now();
    }

    fn save(&mut self) {
        let mut recs: Vec<Record> = self.sessions.iter().filter(|s| s.rec.role != Role::Hr).map(|s| s.rec.clone()).collect();
        recs.extend(self.dormant.iter().cloned());
        self.s.sessions = recs;
        let text = serde_json::to_string_pretty(&self.s).unwrap_or_default();
        if text != self.last_saved {
            if std::fs::write(self.data.join("fleet.json"), &text).is_ok() {
                self.last_saved = text;
            }
        }
    }
}

pub fn key(p: &Path) -> String {
    p.display().to_string().to_lowercase().replace('/', r"\").trim_end_matches('\\').to_string()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}

/// The Remote Control link a session prints at startup.
fn rc_url(parser: &vt100::Parser) -> String {
    let text = parser.screen().contents();
    text.find("https://claude.ai/code/session_")
        .map(|i| text[i..].split_whitespace().next().unwrap_or_default().to_string())
        .unwrap_or_default()
}

/// Where the orchestration file formats are written down.
pub fn orchestration_doc() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.ancestors().find(|p| p.join("docs").join("orchestration.md").is_file()).map(|p| p.join("docs").join("orchestration.md")))
        .unwrap_or_else(|| PathBuf::from("docs/orchestration.md"))
}
