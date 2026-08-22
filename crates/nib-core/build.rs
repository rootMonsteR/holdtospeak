//! Embed the application icon into `nib-core.exe`.
//!
//! Without an icon resource Explorer, the taskbar, the Start-menu shortcut and the uninstall entry
//! all fall back to the generic Windows executable icon, which makes an installed app look like a
//! stray binary. The tray also reads the icon straight out of this resource (see `nib-win32`'s
//! `tray.rs`), so embedding it here is what gives the whole product one identity.
//!
//! The `.ico` itself is generated, not hand-drawn — see `assets/make-icon.py`, which is committed
//! alongside it so the art is reproducible rather than an opaque binary nobody can regenerate.

fn main() {
    // Rebuild when the art changes, not just when the source does.
    println!("cargo:rerun-if-changed=../../assets/HoldToSpeak.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/HoldToSpeak.ico");
        res.set("ProductName", "HoldToSpeak");
        res.set(
            "FileDescription",
            "HoldToSpeak — local push-to-talk dictation",
        );
        res.set("CompanyName", "rootMonsteR");
        res.set(
            "LegalCopyright",
            "Copyright (c) 2026 rootMonsteR. MIT licensed.",
        );
        // A failure here must not be fatal: a source build on a machine without the Windows SDK
        // resource compiler should still produce a working (if plain-looking) binary.
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the app icon: {e}");
        }
    }
}
