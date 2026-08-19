//! `Win32Paths` — the `PathLayout` impl: standard per-user Windows locations for Nib's settings
//! (`%APPDATA%\Nib`) and content-addressed data / models (`%LOCALAPPDATA%\Nib`). Pure env lookups
//! (no `windows::`), but it lives here because it implements a platform trait. Falls back to the
//! temp dir if the env var is somehow unset.

use std::path::PathBuf;

use nib_platform::PathLayout;

#[derive(Debug, Default, Clone, Copy)]
pub struct Win32Paths;

impl PathLayout for Win32Paths {
    fn config_dir(&self) -> PathBuf {
        base_dir("APPDATA").join("HoldToSpeak")
    }

    fn data_dir(&self) -> PathBuf {
        base_dir("LOCALAPPDATA").join("HoldToSpeak")
    }
}

fn base_dir(var: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}
