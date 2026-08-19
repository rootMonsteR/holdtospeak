//! Shared Win32 primitives: our-own-event tagging, synthesized keystrokes, and the clipboard
//! save / set / restore helpers. Used by the injector, and (once ported) the keyboard hook.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VIRTUAL_KEY,
};

pub(crate) const CF_UNICODETEXT: u32 = 13;
/// Tags our own `SendInput` events so the low-level hook can recognize and ignore them.
pub(crate) const INJECT_MAGIC: usize = 0x4E49_4232; // "NIB2"

/// Logical Ctrl state, kept in sync by the hook. The injector reads it so a paste issued while
/// the user still holds a PTT modifier never desyncs the OS's Ctrl state.
pub(crate) static CTRL: AtomicBool = AtomicBool::new(false);

/// Synthesize a virtual-key down/up, tagged with `INJECT_MAGIC` so the hook ignores it.
pub(crate) fn send_vk(vk: u16, up: bool) {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECT_MAGIC,
            },
        },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

/// Type `text` as Unicode keystrokes (`KEYEVENTF_UNICODE`) — the RDP/Citrix-safe route where
/// clipboard redirection is often DLP-blocked.
pub(crate) fn send_unicode(text: &str) {
    let mut inputs = Vec::new();
    for u in text.encode_utf16() {
        for up in [false, true] {
            let mut flags = KEYEVENTF_UNICODE;
            if up {
                flags |= KEYEVENTF_KEYUP;
            }
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: u,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: INJECT_MAGIC,
                    },
                },
            });
        }
    }
    if !inputs.is_empty() {
        unsafe {
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }
}

/// The clipboard is a shared resource; another app briefly holding it open is normal.
unsafe fn open_clipboard_retry() -> bool {
    for _ in 0..5 {
        if OpenClipboard(HWND::default()).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Put `s` on the clipboard as CF_UNICODETEXT. Returns false if the clipboard couldn't be
/// taken — the caller must then NOT paste (a Ctrl+V would insert the user's stale clipboard).
pub(crate) fn clipboard_set_text(s: &str) -> bool {
    unsafe {
        let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
        let Ok(hglobal) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else {
            return false;
        };
        let ptr = GlobalLock(hglobal) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(hglobal);
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        let _ = GlobalUnlock(hglobal);
        if !open_clipboard_retry() {
            let _ = GlobalFree(hglobal);
            return false;
        }
        let _ = EmptyClipboard();
        // Ownership of hglobal transfers to the system only on success; on failure it's ours.
        let ok = SetClipboardData(CF_UNICODETEXT, HANDLE(hglobal.0)).is_ok();
        let _ = CloseClipboard();
        if !ok {
            let _ = GlobalFree(hglobal);
        }
        ok
    }
}

/// Read the clipboard's CF_UNICODETEXT, if any — used to restore the user's clipboard.
pub(crate) fn clipboard_get_text() -> Option<String> {
    unsafe {
        if !open_clipboard_retry() {
            return None;
        }
        let out = GetClipboardData(CF_UNICODETEXT).ok().and_then(|h| {
            let ptr = GlobalLock(HGLOBAL(h.0)) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(HGLOBAL(h.0));
            Some(s)
        });
        let _ = CloseClipboard();
        out
    }
}
