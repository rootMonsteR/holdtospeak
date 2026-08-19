//! `Win32TargetProbe` — snapshots the focused window's exe name and remote-session state, plus
//! integrity-level detection for the "that window runs as admin — text kept" warning. Terminal /
//! IDE classification (Literal mode) is `nib-target`'s pure job; UIA password detection lands
//! with the S3 read-back work.

use core::ffi::c_void;
use std::time::Duration;

use nib_platform::{TargetProbe, TargetProfile};
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, SM_REMOTESESSION,
};

/// Lowercased basename of the foreground window's process, e.g. `"code.exe"`. None if the
/// process can't be queried.
pub fn foreground_exe() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 260];
        let mut size = buf.len() as u32;
        let r =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size);
        let _ = CloseHandle(h);
        r.ok()?;
        let full = String::from_utf16_lossy(&buf[..size as usize]);
        Some(
            full.rsplit(['\\', '/'])
                .next()
                .unwrap_or(&full)
                .to_ascii_lowercase(),
        )
    }
}

/// True when running inside an RDP/Citrix remote session (drives Unicode-first injection).
pub fn is_remote_session() -> bool {
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

/// Human-readable name for an integrity-level RID (0x2000 = Medium, 0x3000 = High, …).
pub fn integrity_level_name(rid: i32) -> &'static str {
    match rid {
        r if r >= 0x4000 => "System",
        r if r >= 0x3000 => "High (elevated)",
        r if r >= 0x2000 => "Medium",
        r if r >= 0x1000 => "Low",
        _ => "Untrusted",
    }
}

/// Integrity-level RID of a process. None if inaccessible (e.g. a higher-IL process). Used to
/// warn on an IL mismatch instead of silently dropping a UIPI-blocked injection.
pub fn integrity_level(pid: u32) -> Option<i32> {
    unsafe {
        let hproc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut token = HANDLE::default();
        if OpenProcessToken(hproc, TOKEN_QUERY, &mut token).is_err() {
            let _ = CloseHandle(hproc);
            return None;
        }
        let mut len = 0u32;
        // First call discovers the required size (returns ERROR_INSUFFICIENT_BUFFER).
        let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let result = if GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr() as *mut c_void),
            len,
            &mut len,
        )
        .is_ok()
        {
            let tml = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
            let psid = tml.Label.Sid;
            let count = *GetSidSubAuthorityCount(psid);
            let sub = GetSidSubAuthority(psid, (count - 1) as u32);
            Some(*sub as i32)
        } else {
            None
        };
        let _ = CloseHandle(token);
        let _ = CloseHandle(hproc);
        result
    }
}

/// Snapshots the focused target: exe name, remote-session state, and the UIA `IsPassword` flag.
/// The pure `nib-target` classifier then adds `is_terminal`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Win32TargetProbe;

impl TargetProbe for Win32TargetProbe {
    fn snapshot(&self, _budget: Duration) -> TargetProfile {
        TargetProfile {
            exe: foreground_exe().unwrap_or_default(),
            is_remote_session: is_remote_session(),
            // UIA is time-boxed internally; `None` (no answer) is treated as "not a password" so a
            // slow provider can't block ordinary dictation. A definite `true` always refuses.
            is_password: crate::uia_focused_is_password().unwrap_or(false),
            is_elevated: foreground_is_elevated(),
            ..Default::default()
        }
    }

    /// Fire-and-forget UIA query so a lazy provider (Chromium) builds its accessibility tree now,
    /// making the `IsPassword` read at inject time truthful. See the trait docs for the measured
    /// activation delay.
    fn warm(&self) {
        std::thread::spawn(|| {
            let _ = crate::uia_focused_is_password();
        });
    }
}

/// Whether the foreground window runs at a higher integrity level than we do — meaning UIPI will
/// silently swallow our injection. The product must warn ("that window runs as administrator —
/// text kept") rather than drop a dictated sentence with no explanation.
pub fn foreground_is_elevated() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let (Some(target), Some(own)) =
            (integrity_level(pid), integrity_level(GetCurrentProcessId()))
        else {
            // A target we can't even query is almost certainly higher-IL than us.
            return integrity_level(pid).is_none();
        };
        target > own
    }
}
