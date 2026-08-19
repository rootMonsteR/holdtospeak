//! `Win32Autostart` — start-with-Windows via the per-user Run key.
//!
//! `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run` is deliberate: it needs no
//! administrator rights, it is trivially inspectable and removable by the user (regedit, or
//! Task Manager's Startup tab), and it does not install a service or scheduled task. For an app
//! that already asks to hook the keyboard and open the microphone, the least surprising
//! persistence mechanism is the right one.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
};

use nib_platform::Autostart;

/// Value name under the Run key. Stable — renaming it would orphan an existing entry.
const VALUE_NAME: PCWSTR = w!("HoldToSpeak");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

#[derive(Debug, Default, Clone, Copy)]
pub struct Win32Autostart;

impl Autostart for Win32Autostart {
    /// Add or remove the Run entry. The command is this executable's own path, quoted so a path
    /// containing spaces (`C:\Program Files\...`) is not split into arguments.
    fn set(&self, on: bool) -> std::io::Result<()> {
        let exe = std::env::current_exe()?;
        unsafe {
            let mut key = HKEY::default();
            let rc = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_WRITE, &mut key);
            if rc != ERROR_SUCCESS {
                return Err(std::io::Error::other(format!(
                    "cannot open the Run key for writing (error {})",
                    rc.0
                )));
            }
            let result = if on {
                let cmd = format!("\"{}\"", exe.display());
                let wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
                // Length is in BYTES and includes the terminating NUL.
                let bytes = std::slice::from_raw_parts(
                    wide.as_ptr() as *const u8,
                    std::mem::size_of_val(&wide[..]),
                );
                RegSetValueExW(key, VALUE_NAME, 0, REG_SZ, Some(bytes))
            } else {
                let rc = RegDeleteValueW(key, VALUE_NAME);
                // Deleting an entry that was never there is success, not failure.
                if rc.0 == 2 {
                    ERROR_SUCCESS
                } else {
                    rc
                }
            };
            let _ = RegCloseKey(key);
            if result != ERROR_SUCCESS {
                return Err(std::io::Error::other(format!(
                    "cannot update the Run key (error {})",
                    result.0
                )));
            }
        }
        Ok(())
    }

    /// Whether the Run entry currently exists.
    fn get(&self) -> bool {
        unsafe {
            let mut key = HKEY::default();
            if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_READ, &mut key) != ERROR_SUCCESS {
                return false;
            }
            let present =
                RegQueryValueExW(key, VALUE_NAME, None, None, None, None) == ERROR_SUCCESS;
            let _ = RegCloseKey(key);
            present
        }
    }
}
