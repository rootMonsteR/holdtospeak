//! Pin `onnxruntime.dll` to the copy we ship, beside our own executable.
//!
//! # Why this exists
//!
//! Windows 11 ships **its own** `onnxruntime.dll` in `System32` (the Windows ML component,
//! currently ORT 1.17). We ship 1.27.1, which is the version `sherpa-onnx-c-api.dll` is built
//! against. Normally the executable's directory is searched before `System32`, so ours wins — but
//! if loading our copy fails for any reason, the loader simply continues down the search order and
//! finds the operating system's older one instead.
//!
//! That fallback is silent and lethal: sherpa asks for ONNX Runtime **API version 27**, the 1.17
//! DLL only offers up to 17, `GetApi(27)` returns NULL, and sherpa dereferences it — an access
//! violation reading address `0x18` with no usable error. It was observed in the wild exactly once
//! per fresh install (the first execution of a newly written 17 MB DLL is the moment a load is most
//! likely to fail transiently), which made it look like a random crash rather than a wrong library.
//!
//! Loading it ourselves, by absolute path, up front removes the guesswork: Windows keys loaded
//! modules by **base name**, so once `onnxruntime.dll` is in the process from our path, the
//! implicit import in `sherpa-onnx-c-api.dll` binds to that one. And if it genuinely cannot be
//! loaded we fail with a sentence a human can act on, instead of crashing inside a C++ library.
//!
//! `*-sys` naming is deliberate: `cargo xtask check-layering` allows `windows::` only in
//! `nib-win32` and `*-sys` crates, and the sidecar links sherpa directly so it cannot go through
//! the `nib-platform` trait wall for this.
#![allow(unsafe_code)]

use std::ffi::{c_char, c_void, CStr};
use std::path::{Path, PathBuf};

use windows::core::{s, PCWSTR};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleW, GetProcAddress, LoadLibraryExW,
    LOAD_WITH_ALTERED_SEARCH_PATH,
};

/// The ONNX Runtime C API version `sherpa-onnx-c-api.dll` requests.
///
/// Tied to the pinned `sherpa-onnx = "=1.13.5"`, whose prebuilt libs ship ORT 1.27.x. Windows'
/// own `System32\onnxruntime.dll` is 1.17, which tops out at API 17 — asking it for 27 returns
/// NULL, and sherpa dereferences that without checking. If sherpa is ever upgraded and this
/// number goes stale, the check fails loudly at startup rather than corrupting anything.
const REQUIRED_ORT_API: u32 = 27;

/// The first two members of ONNX Runtime's `OrtApiBase`. Only these two are read, and both are
/// at the head of the struct, so the rest of the (versioned) layout is irrelevant here.
#[repr(C)]
struct OrtApiBase {
    get_api: Option<unsafe extern "system" fn(u32) -> *const c_void>,
    get_version_string: Option<unsafe extern "system" fn() -> *const c_char>,
}

/// Ask the loaded runtime whether it can actually serve the API sherpa will request.
///
/// This is the check that catches the real-world failure directly: it does not care WHERE the DLL
/// came from, only whether it is the right version — so a wrong copy sitting in the right folder
/// is caught just as reliably as the operating system's one being picked up from System32.
fn verify_ort_api(module: HMODULE) -> Result<String, String> {
    let Some(proc) = (unsafe { GetProcAddress(module, s!("OrtGetApiBase")) }) else {
        return Err("onnxruntime.dll exports no OrtGetApiBase — it is not an ONNX Runtime".into());
    };
    // Safe in the only sense that matters: this is ORT's documented, stable entry point, and we
    // have just confirmed the export exists.
    let get_base: unsafe extern "system" fn() -> *const OrtApiBase =
        unsafe { std::mem::transmute(proc) };
    let base = unsafe { get_base() };
    if base.is_null() {
        return Err("OrtGetApiBase() returned NULL".into());
    }
    let base = unsafe { &*base };
    let version = base
        .get_version_string
        .map(|f| {
            unsafe { CStr::from_ptr(f()) }
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_else(|| "unknown".to_string());
    let api = base
        .get_api
        .map_or(std::ptr::null(), |f| unsafe { f(REQUIRED_ORT_API) });
    if api.is_null() {
        return Err(format!(
            "the ONNX Runtime that loaded is version {version}, which does not provide API \
             version {REQUIRED_ORT_API} that the speech engine requires. Windows ships its own \
             older onnxruntime.dll in System32; this is almost certainly that one rather than the \
             copy installed with the app."
        ));
    }
    Ok(version)
}

/// How many times to attempt the load before giving up.
///
/// The failure this guards against is transient — a just-installed DLL being scanned or still
/// settling — so a couple of quick retries turn a broken first run into a slightly slower one.
const ATTEMPTS: u32 = 3;
const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Handle and full path of the module currently bound to `name` (e.g. `onnxruntime.dll`).
///
/// Windows keys loaded modules by base name, so this is the copy any implicit import will resolve
/// to — which is the thing worth checking, not merely whatever we managed to load ourselves.
fn bound_module(name: &str) -> Option<(HMODULE, PathBuf)> {
    let w = wide(name);
    let module: HMODULE = unsafe { GetModuleHandleW(PCWSTR(w.as_ptr())) }.ok()?;
    let mut buf = [0u16; 32768];
    let n = unsafe { GetModuleFileNameW(module, &mut buf) };
    (n > 0).then(|| {
        (
            module,
            PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])),
        )
    })
}

/// Compare two paths for "same file", tolerating case and `\\?\` verbatim prefixes.
fn same_file(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .to_ascii_lowercase()
    };
    norm(a) == norm(b)
}

/// Load `dll` from the directory containing the current executable, and verify that this is the
/// copy the process will actually use.
///
/// Returns the resolved path on success. On failure the message is intended to be shown to a user
/// verbatim, because at that point the alternative is an unexplained crash.
pub fn pin_beside_exe(dll: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate our own exe: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", exe.display()))?;
    let full = dir.join(dll);
    if !full.exists() {
        return Err(format!(
            "{dll} is missing from {} — the app's runtime files are incomplete; reinstall it",
            dir.display()
        ));
    }

    let w = wide(&full.to_string_lossy());
    let mut last = String::new();
    for attempt in 1..=ATTEMPTS {
        // LOAD_WITH_ALTERED_SEARCH_PATH: resolve this DLL's OWN dependencies from its directory
        // too, so a partially-shipped runtime can't pull halves from two different places.
        match unsafe { LoadLibraryExW(PCWSTR(w.as_ptr()), None, LOAD_WITH_ALTERED_SEARCH_PATH) } {
            Ok(_) => {
                // The real question is not "did our load succeed" but "is OUR copy the one bound
                // to this name", since that is what sherpa's implicit import resolves to. A static
                // import can already have bound a different copy before main() ever ran, in which
                // case loading ours now changes nothing — so check, and refuse rather than crash.
                let Some((module, actual)) = bound_module(dll) else {
                    return Err(format!("{dll} reported loaded but could not be located"));
                };
                if !same_file(&actual, &full) {
                    return Err(format!(
                        "{dll} is loaded from {} instead of the copy installed with the app at {}. \
                         Those are different versions and mixing them crashes.",
                        actual.display(),
                        full.display()
                    ));
                }
                // Right file, but is it the right version? Belt and braces: a mismatched copy in
                // the correct folder would pass the path check and then crash inside sherpa.
                verify_ort_api(module)?;
                return Ok(actual);
            }
            Err(e) => {
                last = e.to_string();
                if attempt < ATTEMPTS {
                    std::thread::sleep(RETRY_DELAY);
                }
            }
        }
    }
    Err(format!(
        "could not load {} after {ATTEMPTS} attempts: {last}. Windows ships an older \
         onnxruntime.dll in System32; refusing to fall back to it, because the versions are \
         incompatible and using it would crash.",
        full.display()
    ))
}
