//! The settings window.
//!
//! A Tauri v2 WebView2 window that runs on its own thread (`Builder::any_thread`), is created on
//! demand from the tray's "Settings…" item and destroyed when closed. It is never on the dictation
//! hot path: everything it shows is read from shared atomics / `Arc`s the pipeline already
//! publishes, and everything it changes goes through the same channels the tray and hotkeys use
//! (`PipeMsg`, the mode/style atoms, `settings.toml`, `hotkeys.toml`, the dictionary).
//!
//! The frontend is static HTML/CSS/JS in `../ui/`, embedded at compile time — no Node, no bundler,
//! and (like the rest of the app) no network.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering::SeqCst};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use nib_audio::AudioMonitor;
use nib_cleanup::Mode;
use nib_config::{Hotkeys, Settings};
use nib_pipeline::{PipeMsg, Stats};
use nib_platform::{Autostart, Binding};
use nib_win32::{OverlayStyle, Win32Autostart, Win32Hotkey, STYLE_MAX_INDEX};
use serde::{Deserialize, Serialize};
use tauri::Manager;

/// Facts about the installed speech model (from `nib_models`'s spec + the resolved directory).
pub struct ModelInfo {
    pub name: String,
    pub dir: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

/// Everything the window can read or change. Built once in `main` and handed to [`spawn`].
pub struct Bridge {
    pub tx: Mutex<Sender<PipeMsg>>,
    pub mode: Arc<AtomicU8>,
    pub style: Arc<AtomicU8>,
    pub listening: Arc<AtomicBool>,
    pub overlay_enabled: Arc<AtomicBool>,
    pub silence_gate: Arc<AtomicU32>,
    pub monitor: AudioMonitor,
    pub stats: Arc<Stats>,
    pub settings: Mutex<Settings>,
    pub settings_path: PathBuf,
    pub hotkeys: Mutex<Hotkeys>,
    pub hotkeys_path: PathBuf,
    pub dictionary_path: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub engine: &'static str,
    pub llm: bool,
    pub mic_name: String,
    pub model: ModelInfo,
    pub started: Instant,
}

static HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

/// Start the UI thread. Returns immediately; the window itself is created lazily by [`open`].
pub fn spawn(bridge: Bridge) {
    std::thread::Builder::new()
        .name("settings-ui".into())
        .spawn(move || {
            let app = tauri::Builder::default()
                .any_thread()
                .manage(bridge)
                .invoke_handler(tauri::generate_handler![
                    get_state,
                    set_mode,
                    set_style,
                    set_overlay,
                    set_autostart,
                    set_silence_rms,
                    set_hotkeys,
                    dictionary_list,
                    dictionary_add,
                    dictionary_remove,
                    mic_level,
                    recent,
                    preview_overlay,
                    open_path,
                    open_url,
                    quit_app,
                ])
                .setup(|app| {
                    let _ = HANDLE.set(app.handle().clone());
                    Ok(())
                })
                .build(tauri::generate_context!());
            match app {
                Ok(app) => app.run(|_handle, event| {
                    // Closing the settings window must not exit the app: the tray keeps running.
                    if let tauri::RunEvent::ExitRequested { api, .. } = event {
                        api.prevent_exit();
                    }
                }),
                Err(e) => eprintln!("(settings window unavailable: {e})"),
            }
        })
        .expect("spawn settings-ui thread");
}

/// Open the settings window, or bring the existing one to the front. Safe from any thread.
pub fn open() {
    std::thread::spawn(|| {
        // The UI thread may still be initialising if the tray was clicked within the first
        // moments after launch; wait briefly rather than dropping the click.
        let deadline = Instant::now() + Duration::from_secs(5);
        let handle = loop {
            if let Some(h) = HANDLE.get() {
                break h.clone();
            }
            if Instant::now() > deadline {
                eprintln!("(settings window: UI thread not ready)");
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let h = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            if let Some(w) = h.get_webview_window("settings") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
                return;
            }
            let built = tauri::WebviewWindowBuilder::new(
                &h,
                "settings",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("HoldToSpeak — Settings")
            .inner_size(960.0, 720.0)
            .min_inner_size(820.0, 600.0)
            .resizable(true)
            .build();
            if let Err(e) = built {
                eprintln!("(could not open the settings window: {e})");
            }
        });
    });
}

// ---------------------------------------------------------------------------------------------
// Views (what the page receives)
// ---------------------------------------------------------------------------------------------

#[derive(Serialize)]
struct ModeView {
    index: u8,
    token: &'static str,
    label: &'static str,
    available: bool,
}

#[derive(Serialize)]
struct StyleView {
    index: u8,
    token: &'static str,
    label: &'static str,
}

#[derive(Serialize)]
struct HotkeysView {
    ptt: String,
    cycle_mode: Option<String>,
    cycle_style: Option<String>,
    quit: Option<String>,
}

#[derive(Serialize)]
struct ModelView {
    name: String,
    dir: String,
    bytes: u64,
    human: String,
    sha256: String,
    installed: bool,
}

#[derive(Serialize)]
struct PathsView {
    config_dir: String,
    data_dir: String,
    settings: String,
    hotkeys: String,
    dictionary: String,
}

#[derive(Serialize)]
struct StateView {
    mode: u8,
    modes: Vec<ModeView>,
    style: u8,
    styles: Vec<StyleView>,
    overlay: bool,
    listening: bool,
    autostart: bool,
    silence_rms: f32,
    hotkeys: HotkeysView,
    engine: &'static str,
    llm: bool,
    mic_name: String,
    model: ModelView,
    paths: PathsView,
    version: String,
    uptime_s: u64,
    total: u64,
    inserted: u64,
    /// `settings.toml`'s `mode` token — what the app will start in (the live `mode` may differ
    /// after a hotkey cycle).
    startup_mode: String,
}

#[derive(Serialize)]
struct DictEntry {
    heard: String,
    meant: String,
}

#[derive(Serialize)]
struct LevelView {
    rms: f32,
    /// 0..1, −60 dBFS → 0, 0 dBFS → 1 — what a meter wants.
    level: f32,
    gate: f32,
}

#[derive(Serialize)]
struct RecordView {
    at_unix_ms: u64,
    app: String,
    mode: String,
    words: usize,
    ms: u64,
    outcome: &'static str,
}

#[derive(Deserialize)]
pub struct HotkeysIn {
    ptt: String,
    cycle_mode: String,
    cycle_style: String,
    quit: String,
}

fn style_token(s: OverlayStyle) -> &'static str {
    match s {
        OverlayStyle::Bars => "bars",
        OverlayStyle::Wave => "wave",
        OverlayStyle::Volt => "volt",
        OverlayStyle::Hud => "hud",
    }
}

fn hotkeys_view(hk: &Hotkeys) -> HotkeysView {
    HotkeysView {
        ptt: nib_config::combo_name(&hk.ptt),
        cycle_mode: hk.cycle_mode.as_ref().map(nib_config::combo_name),
        cycle_style: hk.cycle_style.as_ref().map(nib_config::combo_name),
        quit: hk.quit.as_ref().map(nib_config::combo_name),
    }
}

/// The product version: set by the packaging script; a plain source build has none.
fn version() -> String {
    match option_env!("HTS_VERSION") {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => format!("{} (source build)", env!("CARGO_PKG_VERSION")),
    }
}

fn save_settings(b: &Bridge, edit: impl FnOnce(&mut Settings)) -> Result<(), String> {
    let mut s = b.settings.lock().unwrap();
    edit(&mut s);
    nib_config::settings::save(&b.settings_path, &s).map_err(|e| e.to_string())
}

fn send(b: &Bridge, msg: PipeMsg) -> Result<(), String> {
    b.tx.lock()
        .unwrap()
        .send(msg)
        .map_err(|_| "the dictation pipeline is not running".to_string())
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

#[tauri::command]
fn get_state(b: tauri::State<'_, Bridge>) -> StateView {
    let s = b.settings.lock().unwrap().clone();
    let hk = hotkeys_view(&b.hotkeys.lock().unwrap());
    let modes = (0u8..4)
        .map(|i| {
            let m = Mode::from_index(i);
            ModeView {
                index: i,
                token: m.token(),
                label: m.short_name(),
                available: !m.needs_llm() || b.llm,
            }
        })
        .collect();
    let styles = OverlayStyle::ALL
        .iter()
        .map(|st| StyleView {
            index: st.index(),
            token: style_token(*st),
            label: st.menu_label().trim_start_matches("Overlay: "),
        })
        .collect();
    let installed = b.model.dir.join("tokens.txt").exists();
    StateView {
        mode: b.mode.load(SeqCst),
        modes,
        style: b.style.load(SeqCst),
        styles,
        overlay: b.overlay_enabled.load(SeqCst),
        listening: b.listening.load(SeqCst),
        autostart: Win32Autostart.get(),
        silence_rms: f32::from_bits(b.silence_gate.load(SeqCst)),
        hotkeys: hk,
        engine: b.engine,
        llm: b.llm,
        mic_name: b.mic_name.clone(),
        model: ModelView {
            name: b.model.name.clone(),
            dir: b.model.dir.display().to_string(),
            bytes: b.model.bytes,
            human: nib_models::human_bytes(b.model.bytes),
            sha256: b.model.sha256.clone(),
            installed,
        },
        paths: PathsView {
            config_dir: b.config_dir.display().to_string(),
            data_dir: b.data_dir.display().to_string(),
            settings: b.settings_path.display().to_string(),
            hotkeys: b.hotkeys_path.display().to_string(),
            dictionary: b.dictionary_path.display().to_string(),
        },
        version: version(),
        uptime_s: b.started.elapsed().as_secs(),
        total: b.stats.total.load(SeqCst),
        inserted: b.stats.inserted.load(SeqCst),
        startup_mode: s.mode,
    }
}

/// Applies live AND becomes the startup default (the General page's "cleanup mode at startup").
#[tauri::command]
fn set_mode(b: tauri::State<'_, Bridge>, index: u8) -> Result<(), String> {
    let m = Mode::from_index(index).clamp_available(b.llm);
    send(&b, PipeMsg::SetMode(m.index()))?;
    save_settings(&b, |s| s.mode = m.token().to_string())
}

#[tauri::command]
fn set_style(b: tauri::State<'_, Bridge>, index: u8) -> Result<(), String> {
    let i = index.min(STYLE_MAX_INDEX);
    b.style.store(i, SeqCst);
    send(&b, PipeMsg::SetStyle(i))?;
    save_settings(&b, |s| {
        s.overlay_style = style_token(OverlayStyle::from_index(i)).to_string()
    })
}

#[tauri::command]
fn set_overlay(b: tauri::State<'_, Bridge>, on: bool) -> Result<(), String> {
    b.overlay_enabled.store(on, SeqCst);
    save_settings(&b, |s| s.overlay = on)
}

#[tauri::command]
fn set_autostart(b: tauri::State<'_, Bridge>, on: bool) -> Result<(), String> {
    Win32Autostart.set(on).map_err(|e| e.to_string())?;
    save_settings(&b, |s| s.autostart = on)
}

#[tauri::command]
fn set_silence_rms(b: tauri::State<'_, Bridge>, value: f32) -> Result<f32, String> {
    if !value.is_finite() {
        return Err("not a number".into());
    }
    let v = value.clamp(0.0005, 0.05);
    b.silence_gate.store(v.to_bits(), SeqCst);
    save_settings(&b, |s| s.silence_rms = v)?;
    Ok(v)
}

/// Validate, save, and rebind live. Push-to-talk is modifiers-only; a chord needs modifiers AND a
/// main key; an empty / `off` chord disables it. Returns the normalised names for the page.
#[tauri::command]
fn set_hotkeys(b: tauri::State<'_, Bridge>, h: HotkeysIn) -> Result<HotkeysView, String> {
    let ptt = {
        let (mods, key) = nib_config::parse_combo(&h.ptt);
        if mods == 0 {
            return Err("Push-to-talk needs at least one modifier (Ctrl, Alt, Shift, Win).".into());
        }
        if key != 0 {
            return Err(
                "Push-to-talk is modifiers only — a main key would swallow that key everywhere."
                    .into(),
            );
        }
        nib_config::binding_from_combo(&h.ptt, true).ok_or("Invalid push-to-talk combo.")?
    };
    let chord = |label: &str, v: &str| -> Result<Option<Binding>, String> {
        let v = v.trim();
        if v.is_empty() || v.eq_ignore_ascii_case("off") || v.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        let (mods, key) = nib_config::parse_combo(v);
        if mods == 0 || key == 0 {
            return Err(format!(
                "{label}: needs at least one modifier and one main key (letter, digit, F1–F12, Space, Enter, Tab)."
            ));
        }
        Ok(nib_config::binding_from_combo(v, true))
    };
    let hk = Hotkeys {
        ptt,
        cycle_mode: chord("Cycle cleanup mode", &h.cycle_mode)?,
        cycle_style: chord("Cycle overlay theme", &h.cycle_style)?,
        quit: chord("Quit", &h.quit)?,
    };
    nib_config::save(&b.hotkeys_path, &hk).map_err(|e| e.to_string())?;
    let (slots, _) = hk.chord_slots();
    Win32Hotkey::rebind(&hk.ptt, &slots);
    let view = hotkeys_view(&hk);
    *b.hotkeys.lock().unwrap() = hk;
    Ok(view)
}

/// The dictionary file maps `term => misheard1, misheard2`; the page shows one row per mishearing.
fn read_dictionary(path: &Path) -> Vec<DictEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let ln = line.trim();
        if ln.is_empty() || ln.starts_with('#') {
            continue;
        }
        if let Some((term, rest)) = ln.split_once("=>") {
            let term = term.trim();
            for mis in rest.split(',') {
                let mis = mis.trim();
                if !mis.is_empty() && !term.is_empty() {
                    out.push(DictEntry {
                        heard: mis.to_string(),
                        meant: term.to_string(),
                    });
                }
            }
        }
    }
    out
}

#[tauri::command]
fn dictionary_list(b: tauri::State<'_, Bridge>) -> Vec<DictEntry> {
    read_dictionary(&b.dictionary_path)
}

/// Goes through the pipeline's `learn` path, which teaches the running recognizer AND appends to
/// the file — the same thing typing `learn heard => meant` in the console does.
#[tauri::command]
fn dictionary_add(b: tauri::State<'_, Bridge>, heard: String, meant: String) -> Result<(), String> {
    let (heard, meant) = (heard.trim(), meant.trim());
    if heard.is_empty() || meant.is_empty() {
        return Err("Both fields are needed.".into());
    }
    if heard.contains("=>") || meant.contains("=>") || heard.contains(',') {
        return Err("Please avoid “=>” and commas — they separate entries in the file.".into());
    }
    send(&b, PipeMsg::Learn(format!("{heard} => {meant}")))
}

/// Removes one mishearing from the file. The running recognizer keeps it until the next start
/// (the page says so).
#[tauri::command]
fn dictionary_remove(
    b: tauri::State<'_, Bridge>,
    heard: String,
    meant: String,
) -> Result<(), String> {
    let path = &b.dictionary_path;
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for line in text.lines() {
        let ln = line.trim();
        if let Some((term, rest)) = ln.split_once("=>") {
            if term.trim() == meant.trim() {
                let kept: Vec<&str> = rest
                    .split(',')
                    .map(str::trim)
                    .filter(|m| !m.is_empty() && *m != heard.trim())
                    .collect();
                if kept.is_empty() {
                    continue; // the whole line is gone
                }
                out.push(format!("{} => {}", term.trim(), kept.join(", ")));
                continue;
            }
        }
        out.push(line.to_string());
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    std::fs::write(path, joined).map_err(|e| e.to_string())
}

#[tauri::command]
fn mic_level(b: tauri::State<'_, Bridge>) -> LevelView {
    let n = (b.monitor.sample_rate() / 10).max(160) as usize; // last 100 ms
    let samples = b.monitor.latest(n);
    let rms = if samples.is_empty() {
        0.0
    } else {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    };
    let db = if rms > 0.0 { 20.0 * rms.log10() } else { -90.0 };
    LevelView {
        rms,
        level: ((db + 60.0) / 60.0).clamp(0.0, 1.0),
        gate: f32::from_bits(b.silence_gate.load(SeqCst)),
    }
}

#[tauri::command]
fn recent(b: tauri::State<'_, Bridge>) -> Vec<RecordView> {
    b.stats
        .recent()
        .into_iter()
        .map(|r| RecordView {
            at_unix_ms: r.at_unix_ms,
            app: r.app,
            mode: r.mode,
            words: r.words,
            ms: r.ms,
            outcome: r.outcome,
        })
        .collect()
}

/// Show the overlay for a few seconds with live microphone audio — no dictation, nothing typed.
#[tauri::command]
fn preview_overlay(b: tauri::State<'_, Bridge>) {
    let listening = b.listening.clone();
    std::thread::spawn(move || {
        listening.store(true, SeqCst);
        std::thread::sleep(Duration::from_secs(4));
        listening.store(false, SeqCst);
    });
}

/// Reveal a folder or file in Explorer. Only the app's own locations — the page can't pass paths.
#[tauri::command]
fn open_path(b: tauri::State<'_, Bridge>, which: String) -> Result<(), String> {
    let (path, select) = match which.as_str() {
        "config" => (b.config_dir.clone(), false),
        "data" => (b.data_dir.clone(), false),
        "model" => (b.model.dir.clone(), false),
        "settings" => (b.settings_path.clone(), true),
        "hotkeys" => (b.hotkeys_path.clone(), true),
        "dictionary" => (b.dictionary_path.clone(), true),
        _ => return Err("unknown location".into()),
    };
    let mut cmd = std::process::Command::new("explorer.exe");
    if select && path.exists() {
        cmd.arg(format!("/select,{}", path.display()));
    } else {
        let dir = if path.is_dir() {
            path.clone()
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or(path.clone())
        };
        let _ = std::fs::create_dir_all(&dir);
        cmd.arg(dir);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Open one of the project's own pages in the default browser. Allow-listed: this is a
/// zero-egress app, and the only links the window offers are its GitHub project.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    const ALLOWED: &[&str] = &[
        "https://github.com/rootMonsteR/holdtospeak",
        "https://github.com/microsoft/winget-pkgs/pull/420408",
    ];
    if !ALLOWED.iter().any(|p| url.starts_with(p)) {
        return Err("link not allowed".into());
    }
    std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", &url])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn quit_app(b: tauri::State<'_, Bridge>) -> Result<(), String> {
    send(&b, PipeMsg::Quit)
}
