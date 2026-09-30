// A snapshot of the process tree, so tackle can see what a session has left running in the
// background (a lake build, say) after its turn ended.

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};

pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
}

pub fn snapshot() -> Vec<Proc> {
    let mut all = Vec::new();
    unsafe {
        let h = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if h == INVALID_HANDLE_VALUE {
            return all;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(h, &mut e) != 0 {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                all.push(Proc { pid: e.th32ProcessID, parent: e.th32ParentProcessID, name: String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase() });
                if Process32NextW(h, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(h);
    }
    all
}

/// Every process below `root`, excluding tackle's own hook and MCP helpers and console hosts.
pub fn descendants(all: &[Proc], root: u32) -> Vec<&Proc> {
    let mut out: Vec<&Proc> = Vec::new();
    let mut frontier = vec![root];
    while let Some(pid) = frontier.pop() {
        for p in all.iter().filter(|p| p.parent == pid && p.pid != pid) {
            if out.iter().any(|o| o.pid == p.pid) {
                continue;
            }
            frontier.push(p.pid);
            if !matches!(p.name.as_str(), "tackle.exe" | "conhost.exe" | "openconsole.exe") {
                out.push(p);
            }
        }
    }
    out
}
