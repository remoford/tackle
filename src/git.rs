// The few git queries tackle needs: which commit a session read, and what has landed since.

use std::os::windows::process::CommandExt;
use std::path::Path;

/// tackle has no console, so a console child like git would flash a window of its own.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").current_dir(cwd).args(args).creation_flags(CREATE_NO_WINDOW).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn head(cwd: &Path) -> Option<String> {
    git(cwd, &["rev-parse", "HEAD"])
}

/// Commits reachable from HEAD but not from `commit`, newest first.
pub fn since(cwd: &Path, commit: &str) -> Option<Vec<String>> {
    git(cwd, &["rev-list", &format!("{}..HEAD", commit)]).map(|s| s.lines().map(str::to_string).collect())
}
