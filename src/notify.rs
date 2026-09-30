// Windows notifications for things that need the human: a session waiting on input or
// holding a message, a compaction, a session that exited on its own. They come from the
// engine thread, so they work while the window is hidden in the tray: each one adds a
// short-lived notification-area icon to the main window with a balloon (shown as a toast
// on Windows 10 and 11), then removes it.

use std::sync::atomic::{AtomicU32, Ordering};
use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW};
use windows_sys::Win32::UI::WindowsAndMessaging::{LoadIconW, IDI_WARNING};

static NEXT_ID: AtomicU32 = AtomicU32::new(0x7AC0);

fn copy(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
}

pub fn toast(hwnd: isize, title: &str, body: &str) {
    if hwnd == 0 {
        return;
    }
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || unsafe {
        let mut d: NOTIFYICONDATAW = std::mem::zeroed();
        d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        d.hWnd = hwnd as _;
        d.uID = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        d.uFlags = NIF_ICON | NIF_TIP | NIF_INFO;
        d.hIcon = LoadIconW(std::ptr::null_mut(), IDI_WARNING);
        copy(&mut d.szTip, "tackle");
        copy(&mut d.szInfoTitle, &title);
        copy(&mut d.szInfo, &body);
        d.dwInfoFlags = NIIF_WARNING;
        if Shell_NotifyIconW(NIM_ADD, &d) != 0 {
            std::thread::sleep(std::time::Duration::from_secs(10));
            Shell_NotifyIconW(NIM_DELETE, &d);
        }
    });
}
