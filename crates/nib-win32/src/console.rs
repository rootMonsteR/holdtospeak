//! Console ownership — used to decide whether a fatal error is readable before we exit.
//!
//! `nib-core` is a console application. Launched from an existing terminal, the shell is attached
//! to that console too, so anything we print survives our exit. But launched from a shortcut, a
//! double-click, or the installer's "run now" checkbox, Windows creates a console *for us alone*
//! and destroys it the instant we exit — taking the error message with it. That is what turned a
//! recoverable sidecar failure into the user-visible symptom "it force closed and disappeared".

use windows::Win32::System::Console::GetConsoleProcessList;

/// True if this process is the ONLY one attached to its console, i.e. the console exists solely
/// for us and will vanish (with our output) the moment we exit.
///
/// `GetConsoleProcessList` returns the number of processes attached. A count of exactly 1 means
/// nobody else — no shell — is sharing it. A count of 0 means the call failed (typically because
/// we have no console at all, e.g. output is redirected), in which case there is no window to
/// keep open and pausing would just hang a script.
pub fn owns_console() -> bool {
    // The buffer only needs to be big enough to distinguish "1" from "more than 1"; when it is
    // too small the call still returns the REQUIRED count, which is all we compare against.
    let mut pids = [0u32; 4];
    let n = unsafe { GetConsoleProcessList(&mut pids) };
    n == 1
}
