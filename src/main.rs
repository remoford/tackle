// No console window in any build: closing one would kill every session with it.
#![windows_subsystem = "windows"]

mod fleet;
mod hooks;
mod icon;
mod mcp;
mod notify;
mod pricing;
mod procs;
mod projects;
mod session;
mod term;
mod units;
mod usage;

use eframe::egui::{self, Color32, RichText};
use fleet::{Fleet, Shared};
use session::{Activity, Role};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static QUIT: AtomicBool = AtomicBool::new(false);

fn show_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE};
    unsafe {
        ShowWindow(hwnd as _, SW_RESTORE);
        SetForegroundWindow(hwnd as _);
    }
}

fn hide_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    unsafe {
        ShowWindow(hwnd as _, SW_HIDE);
    }
}

fn tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

fn activity_color(a: Activity) -> Color32 {
    match a {
        Activity::Starting => Color32::GRAY,
        Activity::Idle => Color32::from_rgb(80, 200, 120),
        Activity::Busy => Color32::from_rgb(240, 170, 60),
        Activity::Waiting => Color32::from_rgb(120, 150, 255),
        Activity::Exited => Color32::from_rgb(220, 70, 70),
    }
}

fn sparkline(ui: &mut egui::Ui, history: &[(f64, u64)], window: u64, now: f64) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    if history.is_empty() {
        return;
    }
    let t0 = history[0].0;
    let span = (now - t0).max(60.0);
    let pt = |t: f64, v: u64| egui::pos2(rect.left() + ((t - t0) / span) as f32 * rect.width(), rect.bottom() - (v as f32 / window as f32).min(1.0) * rect.height());
    // Context holds its value until the next reading, so draw steps.
    let mut pts = Vec::new();
    for (i, &(t, v)) in history.iter().enumerate() {
        if i > 0 {
            pts.push(pt(t, history[i - 1].1));
        }
        pts.push(pt(t, v));
    }
    pts.push(pt(now, history.last().unwrap().1));
    p.add(egui::Shape::line(pts, egui::Stroke::new(1.5, Color32::from_rgb(120, 170, 255))));
}

/// Plan usage from hr's /usage readings: each limit over the last day with its reset and
/// where the last hour's rate would take it, then who used the tokens in the current
/// session window.
fn usage_panel(ui: &mut egui::Ui, f: &mut Fleet) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Plan usage").strong());
        ui.label("every");
        ui.add(egui::DragValue::new(&mut f.s.usage_every_min).range(0..=240).suffix(" min")).on_hover_text("read /usage on hr this often while it is idle; 0 = never");
        if ui.small_button("read now").clicked() {
            f.usage_now();
        }
        ui.separator();
        ui.label("plan");
        ui.add(egui::DragValue::new(&mut f.s.plan_usd_month).range(0.0..=10_000.0).prefix("$").suffix("/mo")).on_hover_text("what your plan costs a month; turns a share of the weekly limit into dollars");
    });
    let Some(last) = f.usage.last().cloned() else {
        ui.label(RichText::new("no reading yet: tackle reads /usage on hr when hr is idle").weak());
        return;
    };
    let now = chrono::Local::now();
    let start = now - chrono::Duration::hours(24);
    let span = (now - start).num_seconds() as f32 * 1.25; // leave room on the right for the projection
    let blue = Color32::from_rgb(120, 170, 255);
    let red = Color32::from_rgb(230, 90, 90);
    for l in &last.limits {
        let proj = usage::projection(&f.usage, &l.title);
        let late = match (proj, l.resets_at) {
            (Some(p), Some(r)) => p < r,
            _ => false,
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&l.title).strong());
            ui.label(format!("{:.0}% used, resets {}", l.percent, l.resets));
            let outlook = usage::outlook(&f.usage, l);
            if !outlook.is_empty() {
                ui.label(RichText::new(outlook.trim_start_matches("; ")).color(if late { red } else { blue }));
            }
        });
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 70.0), egui::Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
        let x = |t: chrono::DateTime<chrono::Local>| rect.left() + ((t - start).num_seconds() as f32 / span) * rect.width();
        let y = |pct: f64| rect.bottom() - (pct as f32 / 100.0).clamp(0.0, 1.0) * rect.height();
        let pts: Vec<egui::Pos2> = f.usage.iter().filter(|r| r.at >= start).filter_map(|r| r.limits.iter().find(|x| x.title == l.title).map(|lim| egui::pos2(x(r.at), y(lim.percent)))).collect();
        p.add(egui::Shape::line(pts.clone(), egui::Stroke::new(1.5, blue)));
        p.vline(x(now), rect.y_range(), egui::Stroke::new(1.0, Color32::GRAY));
        if let Some(r) = l.resets_at.filter(|r| x(*r) <= rect.right()) {
            p.vline(x(r), rect.y_range(), egui::Stroke::new(1.0, Color32::from_rgb(80, 200, 120)));
        }
        if let (Some(pr), Some(end)) = (proj, pts.last()) {
            let target = egui::pos2(x(pr).min(rect.right()), y(100.0));
            p.add(egui::Shape::dashed_line(&[*end, target], egui::Stroke::new(1.0, if late { red } else { blue }), 4.0, 3.0));
        }
    }
    // Who used this week's limit: each rise in the weekly percentage split by what each
    // session spent at API prices; a rise with no tackle spending is "outside tackle".
    let week = usage::shares(&f.usage, usage::WEEK);
    if !week.is_empty() {
        let per_pct = usage::plan_usd_per_week_pct(f.s.plan_usd_month);
        let api: f64 = week.iter().map(|w| w.2).sum();
        let pct: f64 = week.iter().map(|w| w.1).sum();
        ui.label(RichText::new(format!(
            "this week since tackle's first reading: {:.1}% of the weekly limit ≈ ${:.2} of your plan; tackle's sessions ${:.2} at API prices",
            pct,
            pct * per_pct,
            api
        ))
        .strong());
        egui::Grid::new("week_shares").striped(true).show(ui, |ui| {
            ui.label(RichText::new("session").weak());
            ui.label(RichText::new("% of week").weak());
            ui.label(RichText::new("≈ plan $").weak());
            ui.label(RichText::new("API $").weak());
            ui.end_row();
            for (name, p, d) in &week {
                ui.label(RichText::new(name).monospace());
                ui.label(format!("{:.2}%", p));
                ui.label(format!("${:.2}", p * per_pct));
                ui.label(if *d > 0.0 { format!("${:.2}", d) } else { "-".into() });
                ui.end_row();
            }
        });
    }
    // Who used the tokens since the start of the current session window.
    if let Some(first) = f.usage.iter().find(|r| r.at >= last.at - chrono::Duration::hours(5)) {
        let used = usage::used_between(first, &last);
        let total: u64 = used.values().sum();
        if total > 0 {
            ui.label(RichText::new(format!("tokens since {}", first.at.format("%H:%M"))).strong());
            let mut v: Vec<(String, u64)> = used.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            for (name, n) in v {
                ui.horizontal(|ui| {
                    ui.add_sized([140.0, 16.0], egui::Label::new(RichText::new(&name).monospace()));
                    ui.add(egui::ProgressBar::new(n as f32 / total as f32).desired_width(300.0).text(format!("{} ({:.0}%)", tokens(n), n as f64 * 100.0 / total as f64)));
                });
            }
        }
    }
}

/// Messages tackle's sessions have sent each other: how many between each pair, then the
/// most recent, newest last.
fn traffic_panel(ui: &mut egui::Ui, f: &Fleet) {
    if f.traffic.is_empty() {
        ui.label(RichText::new("no messages yet: tackle logs every SendMessage its sessions send").weak());
        return;
    }
    let mut pairs: std::collections::BTreeMap<(String, String), usize> = std::collections::BTreeMap::new();
    for m in &f.traffic {
        *pairs.entry((m.from.clone(), m.to.clone())).or_default() += 1;
    }
    ui.horizontal_wrapped(|ui| {
        for ((a, b), n) in &pairs {
            ui.label(RichText::new(format!("{} → {}: {}", a, b, n)).monospace());
            ui.separator();
        }
    });
    egui::ScrollArea::vertical().max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
        for m in f.traffic.iter().rev().take(100).collect::<Vec<_>>().into_iter().rev() {
            ui.label(RichText::new(format!("{} {} → {}: {}", m.at, m.from, m.to, m.summary)).small().monospace());
        }
    });
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

fn reg(args: &[&str]) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("reg").args(args).creation_flags(0x0800_0000).output()
}

fn autostart_enabled() -> bool {
    reg(&["query", RUN_KEY, "/v", "tackle"]).map(|o| o.status.success()).unwrap_or(false)
}

/// Starts tackle when you sign in to Windows, through the per-user Run key.
fn set_autostart(on: bool) -> Result<(), String> {
    let out = if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        reg(&["add", RUN_KEY, "/v", "tackle", "/t", "REG_SZ", "/d", &format!("\"{}\"", exe.display()), "/f"])
    } else {
        reg(&["delete", RUN_KEY, "/v", "tackle", "/f"])
    };
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!("start with Windows: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(format!("start with Windows: {}", e)),
    }
}

struct Form {
    open: bool,
    name: String,
    project: String,
    model: String,
    role: Role,
    brief: String,
    projects_open: bool,
    project_name: String,
    project_path: String,
    people_open: bool,
    locks_open: bool,
    usage_open: bool,
    traffic_open: bool,
    preset: String,
    lock_words: String,
}

struct App {
    fleet: Shared,
    selected: usize,
    hwnd: isize,
    _tray: Option<tray_icon::TrayIcon>,
    form: Form,
    focus_term: bool,
    /// A selection in the terminal view: (anchor, end) as (row, col).
    sel: Option<((u16, u16), (u16, u16))>,
    autostart: bool,
    quitting: bool,
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let dir = PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into())).join("Fonts");
    let mono = fonts.families.entry(egui::FontFamily::Monospace).or_default();
    let mut added = Vec::new();
    if let Some((name, bytes)) = ["CascadiaMono.ttf", "consola.ttf"].iter().find_map(|f| std::fs::read(dir.join(f)).ok().map(|b| (f.to_string(), b))) {
        mono.insert(0, name.clone());
        added.push((name, bytes));
    }
    // Claude's TUI uses symbols (⏺ ⎿ ✻ …) the bundled fonts lack.
    for f in ["seguisym.ttf"] {
        if let Ok(b) = std::fs::read(dir.join(f)) {
            mono.push(f.to_string());
            added.push((f.to_string(), b));
        }
    }
    for (name, bytes) in added {
        fonts.font_data.insert(name, egui::FontData::from_owned(bytes));
    }
    ctx.set_fonts(fonts);
}

fn build_tray(ctx: &egui::Context, hwnd: isize) -> Option<tray_icon::TrayIcon> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let menu = Menu::new();
    menu.append_items(&[&MenuItem::with_id("show", "Show tackle", true, None), &MenuItem::with_id("quit", "Quit tackle", true, None)]).ok()?;
    let icon = tray_icon::Icon::from_rgba(icon::tackle(), icon::SIZE as u32, icon::SIZE as u32).ok()?;
    let tray = TrayIconBuilder::new().with_menu(Box::new(menu)).with_menu_on_left_click(false).with_tooltip("tackle").with_icon(icon).build().ok()?;
    let c = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            show_window(hwnd);
            c.request_repaint();
        }
    }));
    let c = ctx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id.0 == "quit" {
            QUIT.store(true, Ordering::SeqCst);
        }
        show_window(hwnd);
        c.request_repaint();
    }));
    Some(tray)
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        install_fonts(&cc.egui_ctx);
        let hwnd = match cc.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(w)) => w.hwnd.get(),
            _ => 0,
        };
        let fleet = fleet::boot(cc.egui_ctx.clone());
        fleet.lock().unwrap().hwnd = hwnd;
        App {
            fleet,
            selected: 0,
            hwnd,
            _tray: if hwnd != 0 { build_tray(&cc.egui_ctx, hwnd) } else { None },
            form: Form { open: false, name: String::new(), project: String::new(), model: "opus".into(), role: Role::Worker, brief: String::new(), projects_open: false, project_name: String::new(), project_path: String::new(), people_open: false, locks_open: false, usage_open: false, traffic_open: false, preset: String::new(), lock_words: String::new() },
            sel: None,
            autostart: autostart_enabled(),
            focus_term: true,
            quitting: false,
        }
    }

    fn new_session_form(&mut self, ui: &mut egui::Ui, f: &mut Fleet) {
        ui.horizontal(|ui| {
            ui.label("preset");
            let before = self.form.preset.clone();
            egui::ComboBox::from_id_salt("preset").selected_text(if self.form.preset.is_empty() { "none" } else { &self.form.preset }).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.form.preset, String::new(), "none");
                for p in &f.s.presets {
                    ui.selectable_value(&mut self.form.preset, p.name.clone(), &p.name);
                }
            });
            if self.form.preset != before {
                if let Some(p) = f.s.presets.iter().find(|p| p.name == self.form.preset) {
                    self.form.project = p.project.clone();
                    self.form.model = p.model.clone();
                    self.form.role = p.role;
                    self.form.brief = p.brief.clone();
                }
            }
            ui.add(egui::TextEdit::singleline(&mut self.form.preset).hint_text("preset name").desired_width(120.0));
            if ui.button("Save as preset").on_hover_text("remember this project, model, role and brief under the preset name").clicked() {
                let name = self.form.preset.trim().to_string();
                if name.is_empty() {
                    f.error = "give the preset a name".into();
                } else {
                    f.s.presets.retain(|p| p.name != name);
                    f.s.presets.push(fleet::Preset { name: name.clone(), project: self.form.project.clone(), model: self.form.model.clone(), role: self.form.role, brief: self.form.brief.clone() });
                    f.add_log(format!("human -> preset {} saved", name));
                }
            }
            if ui.add_enabled(f.s.presets.iter().any(|p| p.name == self.form.preset), egui::Button::new("Delete preset")).clicked() {
                let name = self.form.preset.clone();
                f.s.presets.retain(|p| p.name != name);
                self.form.preset.clear();
            }
        });
        ui.horizontal(|ui| {
            ui.label("project");
            let projects = projects::load(&f.data);
            egui::ComboBox::from_id_salt("project").selected_text(&self.form.project).width(160.0).show_ui(ui, |ui| {
                for p in &projects {
                    ui.selectable_value(&mut self.form.project, p.name.clone(), &p.name).on_hover_text(p.path.display().to_string());
                }
            });
            ui.add(egui::TextEdit::singleline(&mut self.form.project).hint_text("or a path").desired_width(260.0));
            ui.label("name");
            ui.add(egui::TextEdit::singleline(&mut self.form.name).hint_text("automatic").desired_width(120.0));
            egui::ComboBox::from_id_salt("model").selected_text(&self.form.model).show_ui(ui, |ui| {
                for m in ["opus", "sonnet", "haiku"] {
                    ui.selectable_value(&mut self.form.model, m.to_string(), m);
                }
            });
            egui::ComboBox::from_id_salt("role").selected_text(self.form.role.label()).show_ui(ui, |ui| {
                for r in [Role::Worker, Role::Orchestrator] {
                    ui.selectable_value(&mut self.form.role, r, r.label());
                }
            });
            if ui.button("Start").clicked() {
                let brief = Some(self.form.brief.clone()).filter(|b| !b.trim().is_empty());
                match f.start_in(&self.form.project, &self.form.name, &self.form.model, self.form.role, None, brief) {
                    Ok(name) => {
                        f.add_log(format!("human -> started {}", name));
                        self.selected = f.sessions.iter().position(|s| s.rec.name == name).unwrap_or(0);
                        self.focus_term = true;
                        self.form.name.clear();
                        self.form.brief.clear();
                        self.form.open = false;
                        f.error.clear();
                    }
                    Err(e) => f.error = e,
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("brief");
            ui.add(egui::TextEdit::multiline(&mut self.form.brief).hint_text("optional first prompt, typed in once the session is ready").desired_rows(2).desired_width(f32::INFINITY));
        });
    }

    /// Delegates (sessions outside tackle the human lets act for them), recent refusals
    /// with a button to trust the requester, and tackle's action log.
    fn people_panel(&mut self, ui: &mut egui::Ui, f: &mut Fleet) {
        ui.label(RichText::new("Delegates: sessions outside tackle that may act for you").strong());
        let mut revoke = None;
        for d in &f.s.delegates {
            ui.horizontal(|ui| {
                if ui.small_button("revoke").clicked() {
                    revoke = Some(d.address.clone());
                }
                ui.label(RichText::new(&d.name).strong());
                ui.label(RichText::new(&d.address).weak().small());
            });
        }
        if f.s.delegates.is_empty() {
            ui.label(RichText::new("none").weak());
        }
        if let Some(a) = revoke {
            f.revoke(&a);
        }
        if !f.denials.is_empty() {
            ui.label(RichText::new("Refused requests").strong());
            let mut trust = None;
            for d in f.denials.iter().rev() {
                ui.horizontal(|ui| {
                    if ui.small_button("trust").on_hover_text("make this requester a delegate").clicked() {
                        trust = Some((d.name.clone(), d.address.clone()));
                    }
                    ui.label(format!("{} {} wanted to {}", d.when, d.name, d.what));
                });
            }
            if let Some((n, a)) = trust {
                f.trust(&n, &a);
            }
        }
        ui.label(RichText::new("Log").strong());
        egui::ScrollArea::vertical().max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
            for l in &f.log {
                ui.label(RichText::new(l).small().monospace());
            }
        });
    }

    /// Writer tokens per project, which projects require one, and the build lock.
    fn locks_panel(&mut self, ui: &mut egui::Ui, f: &mut Fleet) {
        let names: Vec<String> = f.sessions.iter().filter(|s| s.rec.role != Role::Hr && s.activity != Activity::Exited).map(|s| s.rec.name.clone()).collect();
        for p in projects::load(&f.data) {
            ui.horizontal(|ui| {
                let key = fleet::key(&p.path);
                let mut on = f.s.one_writer.iter().any(|x| fleet::key(x) == key);
                if ui.checkbox(&mut on, "").on_hover_text("only the writer-token holder may edit and commit here").changed() {
                    f.s.one_writer.retain(|x| fleet::key(x) != key);
                    if on {
                        f.s.one_writer.push(p.path.clone());
                    }
                    f.add_log(format!("human {} one-writer for {}", if on { "set" } else { "cleared" }, p.name));
                }
                ui.label(RichText::new(&p.name).strong());
                let holder = f.s.writers.get(&key).cloned().unwrap_or_default();
                let mut pick = holder.clone();
                egui::ComboBox::from_id_salt(("writer", &p.name)).selected_text(if holder.is_empty() { "nobody" } else { &holder }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut pick, String::new(), "nobody");
                    for n in &names {
                        ui.selectable_value(&mut pick, n.clone(), n);
                    }
                });
                if pick != holder {
                    if pick.is_empty() {
                        f.release_writer(&p.path, "human");
                    } else {
                        f.grant_writer(&p.path, &pick, "human");
                    }
                }
            });
        }
        ui.horizontal(|ui| {
            match &f.lock {
                Some(l) => {
                    ui.label(format!("build lock: {} ({}, {})", l.session, session::clip(&l.command, 60), session::ago(l.since)));
                    if ui.small_button("release").clicked() {
                        f.release_lock("human");
                    }
                }
                None => {
                    ui.label("build lock: free");
                }
            }
            ui.separator();
            ui.label("lock words");
            if self.form.lock_words.is_empty() {
                self.form.lock_words = f.s.lock_words.join(" ");
            }
            let edit = ui.add(egui::TextEdit::singleline(&mut self.form.lock_words).desired_width(200.0)).on_hover_text("a Bash command containing any of these words takes the build lock");
            if !edit.has_focus() && self.form.lock_words != f.s.lock_words.join(" ") && !edit.lost_focus() {
                self.form.lock_words = f.s.lock_words.join(" ");
            }
            if edit.lost_focus() {
                f.s.lock_words = self.form.lock_words.split_whitespace().map(str::to_string).collect();
            }
        });
    }

    fn projects_panel(&mut self, ui: &mut egui::Ui, f: &mut Fleet) {
        let mut result = Ok(());
        for p in projects::load(&f.data) {
            ui.horizontal(|ui| {
                if ui.small_button("x").on_hover_text("remove").clicked() {
                    result = projects::remove(&f.data, &p.name);
                }
                ui.label(RichText::new(&p.name).strong());
                ui.label(RichText::new(p.path.display().to_string()).weak());
            });
            for u in units::load(&p.path) {
                ui.label(RichText::new(format!("      {}: {} ({}, since {}){}", u.unit, u.holder, u.state, u.since, if u.note.is_empty() { String::new() } else { format!(" — {}", u.note) })).small());
            }
        }
        ui.horizontal(|ui| {
            ui.label("name");
            ui.add(egui::TextEdit::singleline(&mut self.form.project_name).desired_width(120.0));
            ui.label("directory");
            ui.add(egui::TextEdit::singleline(&mut self.form.project_path).desired_width(420.0));
            if ui.button("Add project").clicked() {
                result = projects::add(&f.data, &self.form.project_name, &self.form.project_path);
                if result.is_ok() {
                    self.form.project_name.clear();
                    self.form.project_path.clear();
                }
            }
        });
        if let Err(e) = result {
            f.error = e;
        }
    }

    fn session_list(&mut self, ui: &mut egui::Ui, f: &mut Fleet) {
        let now = f.now();
        let window = f.s.window;
        let ttl = f.s.cache_ttl_min;
        for (i, s) in f.sessions.iter().enumerate() {
            let selected = i == self.selected;
            let frame = egui::Frame::none()
                .inner_margin(6.0)
                .rounding(4.0)
                .fill(if selected { ui.visuals().selection.bg_fill.gamma_multiply(0.35) } else { Color32::TRANSPARENT });
            let resp = frame
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 5.0, activity_color(s.activity));
                        ui.label(RichText::new(&s.rec.name).strong());
                        ui.label(RichText::new(format!("{} · {}", s.rec.role.label(), s.rec.model)).weak());
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(s.state_phrase()).color(activity_color(s.activity)));
                        if let Some(h) = s.usage.hit_rate() {
                            ui.label(RichText::new(format!("cache {:.0}%", h * 100.0)).weak());
                        }
                        ui.label(RichText::new(format!("${:.2}", s.rec.cost_usd)).weak()).on_hover_text("what it has cost so far at API list prices");
                        if !s.pending.is_empty() {
                            ui.label(RichText::new(format!("queued {}", s.pending.join(" "))).weak());
                        }
                        if !s.doing.stale.is_empty() {
                            ui.label(RichText::new(format!("{} read files changed", s.doing.stale.len())).color(Color32::from_rgb(230, 170, 60)))
                                .on_hover_text(s.doing.stale.iter().map(|p| session::short_path(p, &s.rec.cwd)).collect::<Vec<_>>().join("\n"));
                        }
                        if let Some(m) = s.cold(ttl) {
                            ui.label(RichText::new(format!("cold {}m", m)).color(Color32::from_rgb(120, 170, 255)));
                        }
                        if s.doing.between_units.is_some() {
                            ui.label(RichText::new("between units").weak());
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        if !s.doing.notice.is_empty() {
                            ui.label(RichText::new(session::clip(&s.doing.notice, 60)).color(Color32::from_rgb(120, 150, 255)).strong());
                        }
                        if let Some((tool, _)) = &s.doing.tool {
                            ui.label(RichText::new(session::clip(tool, 40)).weak().small());
                        }
                        if s.contaminated {
                            ui.label(RichText::new("COMPACTED").color(Color32::RED).strong());
                        }
                    });
                    let frac = s.usage.context as f32 / window as f32;
                    ui.add(egui::ProgressBar::new(frac.min(1.0)).desired_height(14.0).text(format!("{} / {}", tokens(s.usage.context), tokens(window))));
                    sparkline(ui, &s.history, window, now);
                })
                .response;
            if ui.interact(resp.rect, ui.id().with(("row", i)), egui::Sense::click()).clicked() {
                self.selected = i;
                self.focus_term = true;
            }
            ui.add_space(2.0);
        }
        // Sessions from an earlier run, or stopped: resume by id, start fresh, or forget.
        if !f.dormant.is_empty() {
            ui.separator();
            ui.label(RichText::new("Not running").strong());
            let mut act: Option<(&str, String)> = None;
            for r in &f.dormant {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&r.name).strong());
                    ui.label(RichText::new(format!("{} · {}", r.role.label(), r.model)).weak());
                    if r.compacted {
                        ui.label(RichText::new("COMPACTED").color(Color32::RED));
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(!r.compacted && !r.claude_id.is_empty(), egui::Button::new("Resume").small()).on_hover_text("same Claude session, by id").clicked() {
                        act = Some(("resume", r.name.clone()));
                    }
                    if ui.small_button("Fresh").on_hover_text("new session, told to resume from its state file").clicked() {
                        act = Some(("fresh", r.name.clone()));
                    }
                    if ui.small_button("Forget").clicked() {
                        act = Some(("forget", r.name.clone()));
                    }
                });
            }
            if let Some((what, name)) = act {
                let r = match what {
                    "resume" => f.resume(&name),
                    "fresh" => f.fresh(&name, None),
                    _ => f.forget(&name),
                };
                match r {
                    Ok(()) => f.add_log(format!("human -> {} {}", what, name)),
                    Err(e) => f.error = e,
                }
            }
        }
    }

    fn session_view(&mut self, ui: &mut egui::Ui, f: &mut Fleet) {
        let Some(s) = f.sessions.get_mut(self.selected) else {
            ui.centered_and_justified(|ui| ui.label("no sessions"));
            return;
        };
        let mut act: Option<&str> = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(&s.rec.name).strong().size(16.0));
            ui.label(RichText::new(s.state_phrase()).color(activity_color(s.activity)));
            ui.label(RichText::new(s.rec.cwd.display().to_string()).weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if s.activity == Activity::Exited {
                    if ui.button("Fresh").on_hover_text("new session, told to resume from its state file").clicked() {
                        act = Some("fresh");
                    }
                    if ui.add_enabled(!s.rec.compacted, egui::Button::new("Resume")).on_hover_text("same Claude session, by id").clicked() {
                        act = Some("resume");
                    }
                    if ui.button("Forget").clicked() {
                        act = Some("forget");
                    }
                } else if ui.button("Stop").on_hover_text("ask it to exit; killed after 3 s if it hasn't").clicked() {
                    s.stop();
                    act = Some("stopped");
                }
                let live = s.activity != Activity::Exited;
                for cmd in ["/clear", "/context"] {
                    if ui.add_enabled(live, egui::Button::new(cmd)).on_hover_text("typed when the session is next idle between turns").clicked() {
                        s.pending.push(cmd.to_string());
                    }
                }
            });
        });
        let manager = s.rec.manager.clone().unwrap_or_else(|| "human".into());
        let mut line = format!("manager {}  ·  has read {} files", manager, s.rec.reads.len());
        if !s.doing.stale.is_empty() {
            line += &format!(" ({} changed since)", s.doing.stale.len());
        }
        if !s.doing.state_line.is_empty() {
            line += &format!("  ·  state: {}", session::clip(&s.doing.state_line, 80));
        }
        if !s.rec.rc_url.is_empty() {
            line += &format!("  ·  {}", s.rec.rc_url);
        }
        ui.label(RichText::new(line).weak().small());
        let u = &s.usage;
        ui.label(
            RichText::new(format!(
                "context {}  ·  last: in {} / cache read {} / cache write {} / out {}  ·  total over {} msgs: cache read {} / cache write {} / in {} / out {}",
                tokens(u.context), tokens(u.input), tokens(u.cache_read), tokens(u.cache_write), tokens(u.output),
                u.messages, tokens(u.total_cache_read), tokens(u.total_cache_write), tokens(u.total_input), tokens(u.total_output)
            ))
            .weak()
            .small(),
        );
        ui.add_space(4.0);

        let m = term::metrics(ui.ctx(), 13.0);
        let avail = ui.available_size();
        let cols = ((avail.x / m.cell.x).floor() as u16).max(20);
        let rows = ((avail.y / m.cell.y).floor() as u16).max(5);
        if (rows, cols) != s.size {
            s.resize(rows, cols);
        }
        let (rect, _) = ui.allocate_exact_size(avail, egui::Sense::hover());
        let id = ui.make_persistent_id("terminal");
        let resp = ui.interact(rect, id, egui::Sense::click_and_drag());
        if resp.clicked() || std::mem::take(&mut self.focus_term) {
            resp.request_focus();
        }
        // Drag to select; a click clears the selection.
        let (rows, cols) = s.size;
        let cell_at = |p: egui::Pos2| {
            let r = (((p.y - rect.top()) / m.cell.y).floor().max(0.0) as u16).min(rows.saturating_sub(1));
            let c = (((p.x - rect.left()) / m.cell.x).floor().max(0.0) as u16).min(cols.saturating_sub(1));
            (r, c)
        };
        if resp.clicked() {
            self.sel = None;
        }
        if let Some(p) = resp.interact_pointer_pos() {
            if resp.drag_started() {
                self.sel = Some((cell_at(p), cell_at(p)));
            } else if resp.dragged() {
                if let Some((a, _)) = self.sel {
                    self.sel = Some((a, cell_at(p)));
                }
            }
        }
        let focused = resp.has_focus();
        let mut parser = s.parser.lock().unwrap();
        if focused {
            ui.memory_mut(|mem| mem.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
            // With a selection, Ctrl+C copies it (like Windows Terminal); without one it
            // goes to the session as an interrupt.
            let events: Vec<egui::Event> = ui.input(|i| i.events.clone());
            let copy = self.sel.is_some() && events.iter().any(|e| matches!(e, egui::Event::Copy));
            if copy {
                if let Some((a, b)) = self.sel.take() {
                    let (start, end) = if a <= b { (a, b) } else { (b, a) };
                    let text = parser.screen().contents_between(start.0, start.1, end.0, end.1 + 1);
                    ui.ctx().copy_text(text);
                }
            }
            let events: Vec<egui::Event> = events.into_iter().filter(|e| !(copy && matches!(e, egui::Event::Copy))).collect();
            let screen = parser.screen();
            let bytes = term::input(&events, screen.application_cursor(), screen.bracketed_paste());
            if !bytes.is_empty() {
                self.sel = None;
                parser.set_scrollback(0);
                drop(parser);
                s.write(&bytes);
                parser = s.parser.lock().unwrap();
            }
        }
        if resp.hovered() {
            let dy = ui.input(|i| i.raw_scroll_delta.y);
            if dy != 0.0 {
                let lines = (dy / m.cell.y).round() as isize * 3;
                let back = (parser.screen().scrollback() as isize + lines).max(0) as usize;
                parser.set_scrollback(back);
            }
        }
        term::paint(&ui.painter_at(rect), rect, &m, parser.screen(), focused);
        if let Some((a, b)) = self.sel {
            let (start, end) = if a <= b { (a, b) } else { (b, a) };
            let painter = ui.painter_at(rect);
            for r in start.0..=end.0 {
                let c0 = if r == start.0 { start.1 } else { 0 };
                let c1 = if r == end.0 { end.1 + 1 } else { cols };
                let min = egui::pos2(rect.left() + c0 as f32 * m.cell.x, rect.top() + r as f32 * m.cell.y);
                let max = egui::pos2(rect.left() + c1 as f32 * m.cell.x, min.y + m.cell.y);
                painter.rect_filled(egui::Rect::from_min_max(min, max), 0.0, Color32::from_rgba_unmultiplied(120, 170, 255, 70));
            }
        }
        drop(parser);
        if let Some(what) = act {
            let name = f.sessions[self.selected].rec.name.clone();
            let r = match what {
                "fresh" => f.fresh(&name, None),
                "resume" => f.resume(&name),
                "forget" => f.forget(&name),
                _ => Ok(()),
            };
            match r {
                Ok(()) => f.add_log(format!("human -> {} {}", what, name)),
                Err(e) => f.error = e,
            }
            self.selected = self.selected.min(f.sessions.len().saturating_sub(1));
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let fleet = self.fleet.clone();
        let mut f = fleet.lock().unwrap();

        if ctx.input(|i| i.viewport().close_requested()) {
            if QUIT.load(Ordering::SeqCst) {
                // Let every session sign off Remote Control, then make sure they are gone.
                for s in &mut f.sessions {
                    s.stop();
                }
                std::thread::sleep(Duration::from_millis(2500));
                for s in &mut f.sessions {
                    s.kill();
                }
            } else {
                // Closing would kill every session; the window goes to the tray instead.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                hide_window(self.hwnd);
            }
        }
        if QUIT.load(Ordering::SeqCst) && !self.quitting {
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(|i| i.viewport().minimized) == Some(true) {
            hide_window(self.hwnd);
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("tackle");
                ui.toggle_value(&mut self.form.open, "+ New session");
                ui.toggle_value(&mut self.form.projects_open, "Projects");
                ui.toggle_value(&mut self.form.people_open, "People & log");
                ui.toggle_value(&mut self.form.locks_open, "Locks");
                ui.toggle_value(&mut self.form.usage_open, "Usage");
                ui.toggle_value(&mut self.form.traffic_open, "Traffic");
                ui.separator();
                ui.label("window");
                ui.add(egui::DragValue::new(&mut f.s.window).speed(10_000).range(10_000..=2_000_000)).on_hover_text("tokens shown as a full context bar");
                ui.label("clear hr at");
                ui.add(egui::DragValue::new(&mut f.s.hr_clear_at).speed(1_000).range(0..=2_000_000)).on_hover_text("tokens; 0 = never");
                ui.label("clear at handover at");
                ui.add(egui::DragValue::new(&mut f.s.others_clear_at).speed(10_000).range(0..=2_000_000))
                    .on_hover_text("clear a session that has written BETWEEN UNITS in its state file once its context reaches this; 0 = never");
                ui.label("cache ttl");
                ui.add(egui::DragValue::new(&mut f.s.cache_ttl_min).range(0..=1440).suffix(" min")).on_hover_text("idle time after which a session's cache is assumed cold");
                ui.separator();
                ui.checkbox(&mut f.s.alerts, "alerts").on_hover_text("Windows notifications when a session needs input, holds a message, compacts, or exits on its own");
                if ui.checkbox(&mut self.autostart, "start with Windows").changed() {
                    if let Err(e) = set_autostart(self.autostart) {
                        f.error = e;
                        self.autostart = autostart_enabled();
                    }
                }
                if !f.error.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(&f.error).color(Color32::from_rgb(230, 90, 90)));
                    if ui.small_button("x").clicked() {
                        f.error.clear();
                    }
                }
            });
            if self.form.open {
                self.new_session_form(ui, &mut f);
            }
            if self.form.projects_open {
                self.projects_panel(ui, &mut f);
            }
            if self.form.people_open {
                self.people_panel(ui, &mut f);
            }
            if self.form.locks_open {
                self.locks_panel(ui, &mut f);
            }
            if self.form.usage_open {
                usage_panel(ui, &mut f);
            }
            if self.form.traffic_open {
                traffic_panel(ui, &f);
            }
        });
        egui::SidePanel::left("sessions").default_width(260.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.session_list(ui, &mut f));
        });
        egui::CentralPanel::default().show(ctx, |ui| self.session_view(ui, &mut f));

        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--hook") {
        hooks::run_hook();
        return Ok(());
    }
    if std::env::args().any(|a| a == "--mcp") {
        hooks::run_mcp_bridge();
        return Ok(());
    }
    let icon = egui::IconData { rgba: icon::tackle(), width: icon::SIZE as u32, height: icon::SIZE as u32 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("tackle").with_inner_size([1400.0, 900.0]).with_icon(icon),
        ..Default::default()
    };
    eframe::run_native("tackle", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
