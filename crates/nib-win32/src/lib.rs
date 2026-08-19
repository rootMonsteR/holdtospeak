//! `nib-win32` — the ONLY crate (besides `*-sys`) permitted to import `windows::`.
//!
//! Implements the `nib-platform` traits against Win32: the WH_KEYBOARD_LL hook, the injection
//! route chain, target/integrity-level probing, the layered-window overlay, and the tray. See
//! docs/design/01-core-app-design.md §§2,4,7,11.
//!
//! `unsafe` is allowed here (Win32 FFI). Kept behind the trait wall so the rest of the
//! workspace stays `#![deny(unsafe_code)]`. `cargo xtask check-layering` enforces that only
//! this crate and `*-sys` crates name `windows::`.
#![allow(unsafe_code)]

mod autostart;
mod hook;
mod inject;
mod overlay;
mod paths;
mod state;
mod target;
mod tray;
mod uia;

pub use autostart::Win32Autostart;
pub use hook::Win32Hotkey;
pub use inject::Win32Injector;
pub use overlay::Win32Overlay;
// The overlay's styles are platform-independent (they live in nib-overlay); re-exported here so
// consumers keep a single `nib_win32::{OverlayStyle, Win32Overlay, STYLE_MAX_INDEX}` surface.
pub use nib_overlay::{OverlayStyle, STYLE_MAX_INDEX};
pub use paths::Win32Paths;
pub use target::{
    foreground_exe, foreground_is_elevated, integrity_level, integrity_level_name,
    is_remote_session, Win32TargetProbe,
};
pub use tray::{TrayCommand, Win32Tray};
pub use uia::{uia_focused_is_password, uia_focused_text, uia_focused_value};

/// Read the clipboard's text (CF_UNICODETEXT), if any. Exposed for the injection-matrix's
/// clipboard-restore check.
pub fn clipboard_get() -> Option<String> {
    state::clipboard_get_text()
}

/// Replace the clipboard text. Returns false if the clipboard couldn't be taken. Exposed for the
/// injection-matrix harness (to seed a marker before a trial).
pub fn clipboard_set(text: &str) -> bool {
    state::clipboard_set_text(text)
}
