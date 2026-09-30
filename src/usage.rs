// Plan usage, read off hr's screen: tackle types /usage into hr when it is idle, reads the
// limits ("Current session", "Current week (all models)", ...) with their "% used" and
// "Resets ..." lines, and presses Esc. /usage is a local command, so no model turn and no
// tokens. Each reading is stored with every session's token totals, which is what lets
// tackle say who used the budget and when it will run out.

use chrono::{DateTime, Datelike, Duration as CDuration, Local, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
pub struct Limit {
    pub title: String,
    pub percent: f64,
    /// As shown, e.g. "6pm (Europe/London)" or "Oct 3, 9am".
    pub resets: String,
    /// `resets` understood as a time, when it could be.
    pub resets_at: Option<DateTime<Local>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Reading {
    pub at: DateTime<Local>,
    pub limits: Vec<Limit>,
    /// Session name -> tokens it has used so far (input + cache writes + cache reads +
    /// output), as tackle counted them at this moment.
    pub sessions: BTreeMap<String, u64>,
}

/// Reads the limits from the text of hr's screen while /usage is showing.
pub fn parse(screen: &str) -> Vec<Limit> {
    let lines: Vec<&str> = screen.lines().map(str::trim).collect();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if !(l.starts_with("Current session") || l.starts_with("Current week")) {
            continue;
        }
        let title = l.split("  ").next().unwrap_or(l).trim().to_string();
        let mut percent = None;
        let mut resets = String::new();
        for next in lines.iter().skip(i).take(4) {
            if let Some(p) = next.find("% used") {
                let digits: String = next[..p].chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
                percent = digits.parse().ok();
            }
            if let Some(r) = next.find("Resets") {
                resets = next[r + "Resets".len()..].trim().to_string();
            }
        }
        if let Some(percent) = percent {
            let resets_at = parse_reset(&resets);
            out.push(Limit { title, percent, resets, resets_at });
        }
    }
    out
}

/// Understands "6pm", "6:30pm", "Oct 3, 9am", "Oct 3 at 9:15am", each optionally followed
/// by a time zone in parentheses (taken to be local).
pub fn parse_reset(s: &str) -> Option<DateTime<Local>> {
    let s = s.split('(').next().unwrap_or(s).replace(" at ", " ").replace(',', " ");
    let words: Vec<&str> = s.split_whitespace().collect();
    let time_word = words.iter().rev().find(|w| w.ends_with("am") || w.ends_with("pm"))?;
    let (clock, pm) = (&time_word[..time_word.len() - 2], time_word.ends_with("pm"));
    let (h, m) = match clock.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (clock.parse::<u32>().ok()?, 0),
    };
    let h = (h % 12) + if pm { 12 } else { 0 };
    let time = NaiveTime::from_hms_opt(h, m, 0)?;
    let now = Local::now();
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let month = words.iter().position(|w| MONTHS.iter().any(|m| w.to_lowercase().starts_with(m)));
    let date = match month {
        Some(i) => {
            let mo = MONTHS.iter().position(|m| words[i].to_lowercase().starts_with(m))? as u32 + 1;
            let day: u32 = words.get(i + 1)?.trim_end_matches(|c: char| !c.is_ascii_digit()).parse().ok()?;
            let mut d = chrono::NaiveDate::from_ymd_opt(now.year(), mo, day)?;
            if d < now.date_naive() {
                d = chrono::NaiveDate::from_ymd_opt(now.year() + 1, mo, day)?;
            }
            d
        }
        None => {
            let today = now.date_naive();
            if today.and_time(time) > now.naive_local() { today } else { today.succ_opt()? }
        }
    };
    Local.from_local_datetime(&date.and_time(time)).single()
}

/// When a limit will reach 100% at its recent rate: a straight line through the readings
/// since its last reset, over the last hour. None if it isn't rising.
pub fn projection(readings: &[Reading], title: &str) -> Option<DateTime<Local>> {
    let points: Vec<(f64, f64)> = since_reset(readings, title);
    let last = points.last()?;
    let recent: Vec<&(f64, f64)> = points.iter().filter(|p| last.0 - p.0 <= 3600.0).collect();
    if recent.len() < 2 {
        return None;
    }
    let (first, last) = (recent[0], recent[recent.len() - 1]);
    let rate = (last.1 - first.1) / (last.0 - first.0).max(1.0);
    if rate <= 0.0 {
        return None;
    }
    let secs = (100.0 - last.1) / rate;
    let at = readings.last()?.at + CDuration::seconds(secs as i64);
    Some(at)
}

/// (seconds since the first reading, percent) for one limit, from its last reset on.
pub fn since_reset(readings: &[Reading], title: &str) -> Vec<(f64, f64)> {
    let Some(t0) = readings.first().map(|r| r.at) else { return Vec::new() };
    let mut points: Vec<(f64, f64)> = Vec::new();
    for r in readings {
        if let Some(l) = r.limits.iter().find(|l| l.title == title) {
            let t = (r.at - t0).num_seconds() as f64;
            if points.last().map(|p| l.percent < p.1 - 0.5).unwrap_or(false) {
                points.clear();
            }
            points.push((t, l.percent));
        }
    }
    points
}

/// Tokens each session used between two readings. A session whose count went down was
/// cleared or restarted; everything it counted since then is new.
pub fn used_between(a: &Reading, b: &Reading) -> BTreeMap<String, u64> {
    b.sessions
        .iter()
        .map(|(name, &now)| {
            let before = a.sessions.get(name).copied().unwrap_or(0);
            (name.clone(), if now >= before { now - before } else { now })
        })
        .filter(|(_, n)| *n > 0)
        .collect()
}

/// A few lines for hr: each limit, its reset, and when it runs out at the recent rate;
/// then who used what since the first reading in the current session window.
pub fn report(readings: &[Reading]) -> String {
    let Some(last) = readings.last() else { return "no /usage reading yet".into() };
    let mut out = vec![format!("plan usage at {}:", last.at.format("%H:%M"))];
    for l in &last.limits {
        let mut line = format!("- {}: {:.0}% used, resets {}", l.title, l.percent, l.resets);
        if let Some(p) = projection(readings, &l.title) {
            let before_reset = l.resets_at.map(|r| p < r).unwrap_or(false);
            line += &format!("; at the last hour's rate 100% at {}{}", p.format("%a %H:%M"), if before_reset { " (BEFORE the reset)" } else { "" });
        }
        out.push(line);
    }
    let window_start = last.at - CDuration::hours(5);
    if let Some(first) = readings.iter().find(|r| r.at >= window_start) {
        let used = used_between(first, last);
        let total: u64 = used.values().sum();
        if total > 0 {
            let mut v: Vec<(String, u64)> = used.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            let parts: Vec<String> = v.iter().take(8).map(|(n, t)| format!("{} {}k ({:.0}%)", n, t / 1000, *t as f64 * 100.0 / total as f64)).collect();
            out.push(format!("tokens since {}: {}", first.at.format("%H:%M"), parts.join(", ")));
        }
    }
    out.join("\n")
}
