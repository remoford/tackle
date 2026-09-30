// The working rules for sessions under tackle, compiled in from docs/orchestration.md so
// that tk-hr (and `tk protocol`) can hand any agent the exact text: the state file, the
// handover, the reading rule, the writer token and build lock, and so on. Formats are
// passed on verbatim, never retold.

const TEXT: &str = include_str!("../docs/orchestration.md");

/// (title, body) for the overview and each "## " section, in order.
fn sections() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![("Overview".into(), String::new())];
    let mut fenced = false;
    for line in TEXT.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        if !fenced && line.starts_with("## ") {
            out.push((line[3..].trim().to_string(), String::new()));
            continue;
        }
        if !fenced && line.starts_with("# ") && out.len() == 1 {
            continue;
        }
        let body = &mut out.last_mut().unwrap().1;
        body.push_str(line);
        body.push('\n');
    }
    out.into_iter().map(|(t, b)| (t, b.trim().to_string())).collect()
}

/// Other words an agent might use for a section.
const ALIASES: &[(&str, &str)] = &[
    // Questions about who may do something are about authority, whatever the action.
    ("who may", "authority"),
    ("who can", "authority"),
    ("allowed", "authority"),
    ("handover", "clearing"),
    ("between units", "clearing"),
    ("clear", "clearing"),
    ("restart", "clearing"),
    ("stale", "reading"),
    ("reading", "reading"),
    ("read", "reading"),
    ("refresh", "reading"),
    ("compaction", "clearing"),
    ("lock", "writer token"),
    ("build", "writer token"),
    ("commit", "writer token"),
    ("pull", "writer token"),
    ("units", "assignments"),
    ("unit", "assignments"),
    ("state", "state file"),
    ("brief", "state file"),
    ("rulings", "state file"),
    ("permission", "authority"),
    ("held", "held messages"),
    ("message", "held messages"),
];

/// One section by topic, or the overview and the list of topics.
pub fn lookup(topic: &str) -> String {
    let all = sections();
    let topic = topic.trim().to_lowercase();
    let list = || all.iter().skip(1).map(|(t, _)| format!("- {}", t)).collect::<Vec<_>>().join("\n");
    if topic.is_empty() || topic == "overview" || topic == "topics" {
        return format!("{}\n\nTopics (ask for any by name):\n{}", all[0].1, list());
    }
    let wanted = ALIASES.iter().find(|(a, _)| topic.contains(a)).map(|(_, s)| s.to_string()).unwrap_or_else(|| topic.clone());
    match all.iter().find(|(t, _)| t.to_lowercase().contains(&wanted) || wanted.contains(&t.to_lowercase())) {
        Some((t, b)) => format!("## {}\n\n{}", t, b),
        None => format!("No topic matches {:?}. Topics:\n{}", topic, list()),
    }
}
