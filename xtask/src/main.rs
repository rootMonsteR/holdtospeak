//! `cargo xtask <command>` — repo dev tasks.
//!
//! Commands (see docs/design/01-core-app-design.md §9):
//!   check-layering   Fail if any crate other than nib-win32 / *-sys imports `windows::`.
//!   export-oss       Assemble the public open-source repo into a directory.
//!   inject-matrix    (stub) run the 40-target injection harness — lands W5.
//!   latency          (stub) run the key-up->inserted latency harness — lands W4.
//!   package          (stub) build MSI/EXE — lands W8.
#![deny(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        "check-layering" => check_layering(),
        "export-oss" => export_oss(),
        "inject-matrix" | "latency" | "package" => {
            eprintln!("xtask: `{cmd}` is a scaffold stub (lands in a later milestone)");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "usage: cargo xtask <check-layering|export-oss|inject-matrix|latency|package>"
            );
            ExitCode::from(2)
        }
    }
}

/// The layering rule: only `nib-win32` and `*-sys` crates may name `windows::`. Everything else
/// must go through the `nib-platform` trait wall.
///
/// Scans every crate under `crates/` AND `bench/` (the increment added `bench/injection-matrix`,
/// which links `nib-win32` and is the most likely place to reach for `windows::` directly). Matches
/// the `windows::` / `windows_sys::` token anywhere on a non-comment line — so `pub use`, bare
/// inline paths, and `use ::windows::` can't slip past a line-prefix match. `xtask` itself is
/// excluded (it references the token in string literals, by necessity). `spikes/` is out of the
/// workspace.
fn check_layering() -> ExitCode {
    let root = workspace_root();
    let mut violations = Vec::new();

    for group in ["crates", "bench"] {
        let Ok(entries) = std::fs::read_dir(root.join(group)) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let crate_name = entry.file_name().to_string_lossy().into_owned();
            if crate_name == "nib-win32" || crate_name.ends_with("-sys") {
                continue;
            }
            scan_dir(&entry.path().join("src"), &crate_name, &mut violations);
        }
    }

    if violations.is_empty() {
        println!("check-layering: OK — no forbidden `windows::` imports outside nib-win32/*-sys");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "check-layering: FAILED — {} violation(s):",
            violations.len()
        );
        for v in &violations {
            eprintln!("  {v}");
        }
        ExitCode::FAILURE
    }
}

fn scan_dir(dir: &Path, crate_name: &str, violations: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, crate_name, violations);
        } else if path.extension().is_some_and(|e| e == "rs") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                for (i, line) in text.lines().enumerate() {
                    // Drop any line comment, then look for the crate token anywhere in the code.
                    // Known limits of this text scan (fine for a lint, documented for honesty):
                    // a string literal containing "//" (e.g. a URL) truncates the check for the
                    // rest of that line, and a /* block comment */ or string literal that merely
                    // MENTIONS windows:: would be flagged. Neither occurs in the workspace today;
                    // the dependency graph is the real wall (non-win32 crates don't declare the
                    // `windows` dep, so an evaded import wouldn't compile anyway).
                    let code = line.split("//").next().unwrap_or("");
                    if code.contains("windows::") || code.contains("windows_sys::") {
                        violations.push(format!(
                            "{crate_name}: {}:{} -> {}",
                            path.display(),
                            i + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
    }
}

fn workspace_root() -> PathBuf {
    // xtask/ lives directly under the workspace root.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().map(Path::to_path_buf).unwrap_or(manifest)
}

// ---- open-source export -----------------------------------------------------------------------

/// Paths (relative to the workspace root) that are PRIVATE and must never reach the public repo.
///
/// This is the single source of truth for the open-core boundary. Anything not listed here is
/// published, so the default is "public" — a new private asset must be added deliberately.
const PRIVATE_PATHS: &[&str] = &[
    // Internal planning and research notes.
    "docs",
    // Throwaway prototypes kept for reference.
    "spikes",
    // Sidecar variant that is not part of the open-source distribution.
    "crates/nib-asr/sidecar",
    // Internal working notes.
    "TODO.md",
    // Local build inputs / outputs.
    "vendor",
    "target",
    ".git",
    // The overlay directory itself is copied to the ROOT of the export, not nested inside it.
    "oss",
];

/// Assemble the publishable open-source repo.
///
/// Rather than keeping a second copy of every crate in-tree (which would drift the moment someone
/// edits one), the public repo is *derived*: everything except [`PRIVATE_PATHS`] is copied, then
/// the `oss/` overlay (public README, LICENSE, notices, privacy) is laid over the root.
///
/// Usage: `cargo xtask export-oss [out_dir]`  (default: `../nib-public`)
fn export_oss() -> ExitCode {
    let root = workspace_root();
    let out = std::env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../nib-public"));

    if out.exists() {
        if let Err(e) = std::fs::remove_dir_all(&out) {
            eprintln!("export-oss: cannot clear {}: {e}", out.display());
            return ExitCode::FAILURE;
        }
    }
    let mut copied = 0usize;
    if let Err(e) = copy_public(&root, &root, &out, &mut copied) {
        eprintln!("export-oss: {e}");
        return ExitCode::FAILURE;
    }
    // Overlay the public-facing docs at the root of the export.
    if let Err(e) = copy_tree(&root.join("oss"), &out, &mut copied) {
        eprintln!("export-oss: overlay failed: {e}");
        return ExitCode::FAILURE;
    }

    // Verify: nothing private leaked. Cheap, and the whole point of having a boundary.
    let mut leaks = Vec::new();
    for p in PRIVATE_PATHS {
        if *p == "oss" {
            continue; // deliberately overlaid at the root
        }
        if out.join(p).exists() {
            leaks.push(*p);
        }
    }
    if !leaks.is_empty() {
        eprintln!("export-oss: FAILED — private paths leaked into the export:");
        for l in leaks {
            eprintln!("  {l}");
        }
        return ExitCode::FAILURE;
    }

    println!(
        "export-oss: wrote {copied} files to {}\n  \
         private paths excluded: {}\n  \
         next: cd into it, `git init`, verify `cargo build --workspace`, then push.",
        out.display(),
        PRIVATE_PATHS.join(", ")
    );
    ExitCode::SUCCESS
}

/// Recursively copy `dir` into `out`, skipping anything under [`PRIVATE_PATHS`].
fn copy_public(root: &Path, dir: &Path, out: &Path, copied: &mut usize) -> Result<(), String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    for entry in rd.flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map_err(|e| format!("strip prefix: {e}"))?;
        let rel_str = rel
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        if PRIVATE_PATHS
            .iter()
            .any(|p| rel_str == *p || rel_str.starts_with(&format!("{p}/")))
        {
            continue;
        }
        let dest = out.join(rel);
        if path.is_dir() {
            std::fs::create_dir_all(&dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
            copy_public(root, &path, out, copied)?;
        } else {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
            }
            std::fs::copy(&path, &dest).map_err(|e| format!("copy {}: {e}", path.display()))?;
            *copied += 1;
        }
    }
    Ok(())
}

/// Copy every file in `src` into `dest` (flat overlay, used for `oss/`).
fn copy_tree(src: &Path, dest: &Path, copied: &mut usize) -> Result<(), String> {
    let rd = std::fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))?;
    for entry in rd.flatten() {
        let path = entry.path();
        let target = dest.join(entry.file_name());
        if path.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| format!("mkdir: {e}"))?;
            copy_tree(&path, &target, copied)?;
        } else {
            std::fs::copy(&path, &target).map_err(|e| format!("copy: {e}"))?;
            *copied += 1;
        }
    }
    Ok(())
}
