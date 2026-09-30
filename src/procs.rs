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

/// The process that owns the IPv4 TCP connection whose local end is 127.0.0.1:`port`
/// (tackle sees it as the peer of a connection it accepted).
pub fn tcp_owner(port: u16) -> Option<u32> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL};
    const AF_INET: u32 = 2;
    let mut size = 0u32;
    unsafe {
        GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, AF_INET, TCP_TABLE_OWNER_PID_ALL, 0);
        let mut buf = vec![0u8; size as usize + 1024];
        size = buf.len() as u32;
        if GetExtendedTcpTable(buf.as_mut_ptr() as *mut _, &mut size, 0, AF_INET, TCP_TABLE_OWNER_PID_ALL, 0) != 0 {
            return None;
        }
        let n = u32::from_ne_bytes(buf[0..4].try_into().ok()?) as usize;
        let rows = buf.as_ptr().add(4) as *const MIB_TCPROW_OWNER_PID;
        (0..n).map(|i| std::ptr::read_unaligned(rows.add(i))).find(|r| u16::from_be(r.dwLocalPort as u16) == port && r.dwOwningPid != std::process::id()).map(|r| r.dwOwningPid)
    }
}

/// `pid` and its parents, nearest first.
pub fn ancestors(all: &[Proc], pid: u32) -> Vec<u32> {
    let mut out = vec![pid];
    let mut cur = pid;
    for _ in 0..64 {
        match all.iter().find(|p| p.pid == cur) {
            Some(p) if p.parent != 0 && !out.contains(&p.parent) => {
                out.push(p.parent);
                cur = p.parent;
            }
            _ => break,
        }
    }
    out
}
