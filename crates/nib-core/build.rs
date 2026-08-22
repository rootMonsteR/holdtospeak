//! Build script: `tauri-build` embeds the settings window's assets manifest, the application icon
//! (resource id 32512, which the tray reads back — see `nib-win32`'s `tray.rs`), the version
//! info, and the per-monitor-DPI application manifest the WebView2 window needs to render crisply.
//!
//! The `.ico` itself is generated, not hand-drawn — see `assets/make-icon.py`, which is committed
//! alongside it so the art is reproducible rather than an opaque binary nobody can regenerate.

fn main() {
    // Rebuild when the art or the UI changes, not just when Rust source does.
    println!("cargo:rerun-if-changed=../../assets/HoldToSpeak.ico");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=build.rs");
    // The product version shown in Settings → About (set by packaging/build-installer.ps1; a plain
    // source build shows the 0.0.0 crate version as "source build").
    println!("cargo:rerun-if-env-changed=HTS_VERSION");
    tauri_build::build();
}
