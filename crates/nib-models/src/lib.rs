//! First-run model acquisition: download, verify, and install the ASR model.
//!
//! This is the **only** network access the product performs. Everything else — capture, ASR,
//! cleanup, injection — is local, and the privacy claim depends on that staying true, so this
//! module is deliberately small, explicit, and auditable:
//!
//! * one hard-coded URL per model (no update server, no telemetry),
//! * a **pinned SHA-256** verified over the bytes actually written, so a corrupted or substituted
//!   archive is rejected rather than installed,
//! * resumable via HTTP Range, because a 460 MB download over a flaky connection should not have
//!   to start from zero,
//! * atomic install (extract to a staging dir, then rename), so an interrupted run can never
//!   leave a half-extracted model that later looks "present".
#![deny(unsafe_code)]

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// A downloadable model: where it comes from, what it must hash to, and where it lands.
#[derive(Debug, Clone)]
pub struct ModelSpec {
    /// Human-readable name, used in progress output.
    pub name: &'static str,
    /// Direct download URL (a GitHub release asset).
    pub url: &'static str,
    /// Expected SHA-256 of the archive, lowercase hex. Verified before install.
    pub sha256: &'static str,
    /// Expected archive size in bytes (drives progress and catches truncation).
    pub bytes: u64,
    /// Directory name inside the archive, which becomes the installed directory name.
    pub dir_name: &'static str,
    /// A file that must exist inside the installed directory for it to count as complete.
    pub sentinel: &'static str,
}

/// Parakeet TDT 0.6B v2, int8 — the CPU English ASR model the free tier ships with.
///
/// Licensed **CC-BY-4.0** by NVIDIA: use requires attribution and a "modified" notice where
/// applicable. We do not redistribute it — it is fetched from the upstream sherpa-onnx release —
/// and the attribution is recorded in THIRD-PARTY-NOTICES.md.
pub const PARAKEET_EN_INT8: ModelSpec = ModelSpec {
    name: "Parakeet TDT 0.6B v2 (English, int8)",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2",
    sha256: "157c157bc51155e03e37d2466522a3a737dd9c72bb25f36eb18912964161e1ad",
    bytes: 482_468_385,
    dir_name: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8",
    sentinel: "tokens.txt",
};

/// Progress during acquisition, so the caller can render whatever UI it likes.
#[derive(Debug, Clone, Copy)]
pub enum Progress {
    /// Bytes downloaded so far, and the total expected.
    Downloading {
        done: u64,
        total: u64,
    },
    Verifying,
    Extracting,
}

/// Where a model is installed, and whether this call had to fetch it.
#[derive(Debug)]
pub struct Installed {
    pub dir: PathBuf,
    pub freshly_downloaded: bool,
}

/// Ensure `spec` is installed under `models_dir`, downloading only if needed.
///
/// Idempotent: an already-complete install returns immediately without touching the network, so
/// this is safe to call on every launch.
pub fn ensure_model(
    spec: &ModelSpec,
    models_dir: &Path,
    mut progress: impl FnMut(Progress),
) -> Result<Installed, String> {
    let dir = models_dir.join(spec.dir_name);
    if dir.join(spec.sentinel).exists() {
        return Ok(Installed {
            dir,
            freshly_downloaded: false,
        });
    }
    fs::create_dir_all(models_dir).map_err(|e| format!("cannot create {models_dir:?}: {e}"))?;

    // Download beside the destination (same volume, so the later rename is atomic).
    let archive = models_dir.join(format!("{}.tar.bz2.part", spec.dir_name));
    download_resumable(spec, &archive, &mut progress)?;

    progress(Progress::Verifying);
    let actual = sha256_file(&archive)?;
    if actual != spec.sha256 {
        // Refuse to install unverified bytes, and delete them so a retry starts clean.
        let _ = fs::remove_file(&archive);
        return Err(format!(
            "checksum mismatch for {} - expected {}, got {}. The download was corrupted or the \
             file was substituted; nothing was installed.",
            spec.name, spec.sha256, actual
        ));
    }

    progress(Progress::Extracting);
    let staging = models_dir.join(format!(".{}.staging", spec.dir_name));
    let _ = fs::remove_dir_all(&staging);
    extract_tar_bz2(&archive, &staging)?;

    // The archive holds a single top-level directory; move it into place atomically.
    let extracted = staging.join(spec.dir_name);
    let src = if extracted.exists() {
        extracted
    } else {
        first_subdir(&staging)?
    };
    fs::rename(&src, &dir).map_err(|e| format!("cannot install into {dir:?}: {e}"))?;
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_file(&archive);

    if !dir.join(spec.sentinel).exists() {
        return Err(format!(
            "install finished but {} is missing - the archive layout changed",
            spec.sentinel
        ));
    }
    Ok(Installed {
        dir,
        freshly_downloaded: true,
    })
}

/// Download `spec.url` to `dest`, resuming from whatever is already there.
fn download_resumable(
    spec: &ModelSpec,
    dest: &Path,
    progress: &mut impl FnMut(Progress),
) -> Result<(), String> {
    let have = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    if have == spec.bytes {
        return Ok(()); // already fully fetched; the caller still verifies the hash
    }
    if have > spec.bytes {
        // Longer than expected means this is not our file. Start over rather than "resume"
        // into garbage that would fail the hash after another full download.
        let _ = fs::remove_file(dest);
    }
    let have = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);

    let mut req = ureq::get(spec.url);
    if have > 0 {
        req = req.set("Range", &format!("bytes={have}-"));
    }
    let resp = req
        .call()
        .map_err(|e| format!("download failed: {e}. Check your connection and retry."))?;

    // 206 = the server honoured the range; 200 = it ignored it and is sending the whole file.
    let resuming = resp.status() == 206 && have > 0;
    let mut file = if resuming {
        fs::OpenOptions::new()
            .append(true)
            .open(dest)
            .map_err(|e| format!("cannot append to {dest:?}: {e}"))?
    } else {
        fs::File::create(dest).map_err(|e| format!("cannot create {dest:?}: {e}"))?
    };
    let mut done = if resuming { have } else { 0 };

    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut last_report = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("download interrupted after {done} bytes: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("cannot write {dest:?}: {e}"))?;
        done += n as u64;
        // Report roughly every 4 MB: often enough to feel live, rare enough not to spam.
        if done - last_report >= 4 << 20 {
            last_report = done;
            progress(Progress::Downloading {
                done,
                total: spec.bytes,
            });
        }
    }
    progress(Progress::Downloading {
        done,
        total: spec.bytes,
    });
    Ok(())
}

/// Stream a file through SHA-256 (never loads 460 MB into memory).
fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(|e| format!("cannot read {path:?}: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("read failed: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn extract_tar_bz2(archive: &Path, dest: &Path) -> Result<(), String> {
    let f = fs::File::open(archive).map_err(|e| format!("cannot open {archive:?}: {e}"))?;
    let mut ar = tar::Archive::new(bzip2::read::BzDecoder::new(f));
    ar.unpack(dest)
        .map_err(|e| format!("cannot extract {archive:?}: {e}"))
}

fn first_subdir(dir: &Path) -> Result<PathBuf, String> {
    fs::read_dir(dir)
        .map_err(|e| format!("cannot read {dir:?}: {e}"))?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir())
        .ok_or_else(|| format!("archive contained no directory in {dir:?}"))
}

/// Human-readable byte count for progress output.
pub fn human_bytes(n: u64) -> String {
    const MB: f64 = (1 << 20) as f64;
    format!("{:.0} MB", n as f64 / MB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_internally_consistent() {
        // A wrong-length or non-hex pin would only fail after a 460 MB download - catch it here.
        // Through owned Strings so these are runtime checks; comparing the consts directly
        // const-folds to `assert!(true)` and stops guarding anything.
        let (sha, url) = (
            PARAKEET_EN_INT8.sha256.to_string(),
            PARAKEET_EN_INT8.url.to_string(),
        );
        assert_eq!(sha.len(), 64, "sha: {sha:?}");
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()), "sha: {sha:?}");
        assert!(url.starts_with("https://"), "url: {url:?}");
        // (A `bytes > 0` assertion would const-fold away and guard nothing; the real check is
        // the checksum, which covers length implicitly.)
    }

    /// Guards against a line-continuation bug leaving whitespace inside the URL, which would
    /// 404 at runtime on a user machine but never in a unit test that does not fetch.
    #[test]
    fn url_has_no_embedded_whitespace() {
        // Via a String so the check happens at runtime; comparing the const directly const-folds
        // to `assert!(true)` and would silently stop guarding anything.
        let url = PARAKEET_EN_INT8.url.to_string();
        assert!(!url.chars().any(char::is_whitespace), "url: {url:?}");
        assert!(url.ends_with(".tar.bz2"), "url: {url:?}");
    }

    #[test]
    fn already_installed_is_detected_without_network() {
        let root = std::env::temp_dir().join(format!("nib_models_test_{}", std::process::id()));
        let dir = root.join(PARAKEET_EN_INT8.dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(PARAKEET_EN_INT8.sentinel), "x").unwrap();
        let got = ensure_model(&PARAKEET_EN_INT8, &root, |_| {}).unwrap();
        assert!(!got.freshly_downloaded);
        assert_eq!(got.dir, dir);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn human_bytes_reads_sensibly() {
        assert_eq!(human_bytes(482_468_385), "460 MB");
    }
}
