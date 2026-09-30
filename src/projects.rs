// Projects are named working directories, so a session can be started "for ic" instead of
// for a path. They are added by hand in tackle and kept in `projects.json` in tackle's
// data directory as {"name": "path"}.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Project {
    pub name: String,
    pub path: PathBuf,
}

fn file(data: &Path) -> PathBuf {
    data.join("projects.json")
}

fn read(data: &Path) -> BTreeMap<String, PathBuf> {
    std::fs::read_to_string(file(data)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write(data: &Path, map: &BTreeMap<String, PathBuf>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    std::fs::write(file(data), text).map_err(|e| format!("{}: {}", file(data).display(), e))
}

pub fn load(data: &Path) -> Vec<Project> {
    read(data).into_iter().map(|(name, path)| Project { name, path }).collect()
}

pub fn add(data: &Path, name: &str, path: &str) -> Result<(), String> {
    let (name, path) = (name.trim(), PathBuf::from(path.trim()));
    if name.is_empty() || name.contains(char::is_whitespace) {
        return Err("project name must be one word".into());
    }
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    let mut map = read(data);
    map.insert(name.to_string(), path);
    write(data, &map)
}

pub fn remove(data: &Path, name: &str) -> Result<(), String> {
    let mut map = read(data);
    map.remove(name);
    write(data, &map)
}

/// A project name (any case) or an existing directory.
pub fn resolve(data: &Path, project: &str) -> Result<Project, String> {
    let project = project.trim();
    if let Some(p) = load(data).into_iter().find(|p| p.name.eq_ignore_ascii_case(project)) {
        return Ok(p);
    }
    let path = PathBuf::from(project);
    if path.is_dir() {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "session".into());
        return Ok(Project { name, path });
    }
    Err(format!("no project or directory called {:?}; projects are added in tackle", project))
}
