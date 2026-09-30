// Who holds which unit of work, in the project's gitignored orchestration/assignments.md.
// The book says what the units are; this file says who is doing them right now. tackle
// keeps it, but it is plain text meant to be read, and kept by hand, without tackle:
//
//     # unit | holder | state | since | note
//     part2-step3 | tk-ic-1 | working | 2026-09-30 14:02 | waiting on the axiom audit
//
// One line per unit; lines starting with # are comments. States are free text; tackle uses
// working, review, blocked and done.

use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Unit {
    pub unit: String,
    pub holder: String,
    pub state: String,
    pub since: String,
    pub note: String,
}

pub fn file(project: &Path) -> PathBuf {
    project.join("orchestration").join("assignments.md")
}

pub fn load(project: &Path) -> Vec<Unit> {
    std::fs::read_to_string(file(project))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').map(str::trim).collect();
            (f.len() >= 2 && !f[0].is_empty()).then(|| Unit {
                unit: f[0].into(),
                holder: f[1].into(),
                state: f.get(2).copied().unwrap_or("working").into(),
                since: f.get(3).copied().unwrap_or_default().into(),
                note: f.get(4..).map(|n| n.join(" | ")).unwrap_or_default(),
            })
        })
        .collect()
}

pub fn save(project: &Path, units: &[Unit]) -> Result<(), String> {
    let path = file(project);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut text = String::from("# Unit assignments, kept by tackle (ask tk-hr, or run `tk protocol assignments`).\n# unit | holder | state | since | note\n");
    for u in units {
        text += &format!("{} | {} | {} | {} | {}\n", u.unit, u.holder, u.state, u.since, u.note.replace('\n', " "));
    }
    std::fs::write(&path, text).map_err(|e| format!("{}: {}", path.display(), e))
}

/// Adds or replaces the line for `unit`.
pub fn upsert(project: &Path, u: Unit) -> Result<(), String> {
    let mut all = load(project);
    match all.iter_mut().find(|x| x.unit == u.unit) {
        Some(x) => *x = u,
        None => all.push(u),
    }
    save(project, &all)
}

pub fn remove(project: &Path, unit: &str) -> Result<bool, String> {
    let mut all = load(project);
    let before = all.len();
    all.retain(|x| x.unit != unit);
    save(project, &all)?;
    Ok(all.len() != before)
}

pub fn render(project: &Path) -> String {
    let all = load(project);
    if all.is_empty() {
        return format!("no units assigned in {}", file(project).display());
    }
    all.iter().map(|u| format!("{}: {} ({}, since {}){}", u.unit, u.holder, u.state, u.since, if u.note.is_empty() { String::new() } else { format!(" — {}", u.note) })).collect::<Vec<_>>().join("\n")
}
