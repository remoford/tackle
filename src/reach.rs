// The reading rule as a mechanical check. A project's rule names the files a full read
// starts from and, optionally, the file it ends at, e.g.
//
//     book/main.tex until book/bsplines_arbitrary.tex
//
// The files the rule requires are those reachable from the start files through \input,
// \include and \subfile, in depth-first order, up to and including the `until` file.
// tackle compares that list with what each session has read, line range by line range.

use std::path::{Path, PathBuf};

pub struct Rule {
    pub roots: Vec<String>,
    pub until: Option<String>,
}

pub fn parse_rule(spec: &str) -> Rule {
    let mut roots = Vec::new();
    let mut until = None;
    let mut words = spec.split_whitespace();
    while let Some(w) = words.next() {
        if w == "until" {
            until = words.next().map(str::to_string);
        } else {
            roots.push(w.to_string());
        }
    }
    Rule { roots, until }
}

fn norm(p: &Path) -> String {
    p.display().to_string().to_lowercase().replace('/', r"\")
}

/// The \input / \include / \subfile targets in a .tex file, in order, comments stripped.
fn includes(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        // Drop a comment: a % not escaped by a backslash.
        let mut code = String::new();
        let mut prev = ' ';
        for c in line.chars() {
            if c == '%' && prev != '\\' {
                break;
            }
            code.push(c);
            prev = c;
        }
        for cmd in ["\\input{", "\\include{", "\\subfile{"] {
            let mut rest = code.as_str();
            while let Some(i) = rest.find(cmd) {
                rest = &rest[i + cmd.len()..];
                if let Some(j) = rest.find('}') {
                    out.push(rest[..j].trim().to_string());
                    rest = &rest[j..];
                }
            }
        }
    }
    out
}

fn resolve(target: &str, from: &Path, root_dir: &Path, project: &Path) -> Option<PathBuf> {
    let names = if target.ends_with(".tex") { vec![target.to_string()] } else { vec![format!("{}.tex", target), target.to_string()] };
    for base in [from.parent().unwrap_or(project), root_dir, project] {
        for n in &names {
            let p = base.join(n);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// The files the rule requires, as normalised absolute paths, in reading order.
pub fn required(project: &Path, rule: &Rule) -> Vec<String> {
    let until = rule.until.as_ref().map(|u| norm(&project.join(u)));
    let mut out: Vec<String> = Vec::new();
    let mut done = false;
    for root in &rule.roots {
        let root = project.join(root);
        let root_dir = root.parent().unwrap_or(project).to_path_buf();
        walk(&root, &root_dir, project, &until, &mut out, &mut done);
        if done {
            break;
        }
    }
    out
}

fn walk(file: &Path, root_dir: &Path, project: &Path, until: &Option<String>, out: &mut Vec<String>, done: &mut bool) {
    let key = norm(file);
    if *done || out.contains(&key) {
        return;
    }
    out.push(key.clone());
    if until.as_deref() == Some(key.as_str()) {
        *done = true;
        return;
    }
    let text = std::fs::read_to_string(file).unwrap_or_default();
    for t in includes(&text) {
        if let Some(p) = resolve(&t, file, root_dir, project) {
            walk(&p, root_dir, project, until, out, done);
            if *done {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_inputs_in_order_and_stops_at_until() {
        let dir = std::env::temp_dir().join(format!("tackle-reach-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("book/part")).unwrap();
        let w = |p: &str, t: &str| std::fs::write(dir.join(p), t).unwrap();
        w("book/main.tex", "\\input{a}\n% \\input{commented}\n50\\% done \\include{part/b}\n\\subfile{c.tex}\n\\input{after}\n");
        w("book/a.tex", "A\n");
        w("book/part/b.tex", "\\input{part/deep}\n");
        w("book/part/deep.tex", "D\n");
        w("book/c.tex", "C\n");
        w("book/after.tex", "never reached\n");
        w("book/commented.tex", "never\n");
        let got = required(&dir, &parse_rule("book/main.tex until book/c.tex"));
        let names: Vec<String> = got.iter().map(|p| p.rsplit('\\').next().unwrap().to_string()).collect();
        assert_eq!(names, vec!["main.tex", "a.tex", "b.tex", "deep.tex", "c.tex"]);
        assert_eq!(required(&dir, &parse_rule("book/main.tex")).len(), 6);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
