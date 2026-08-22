//! `nib-core` — the interim headless dictation app: the composition root that constructs the
//! Win32 platform implementations, wires them into `nib-pipeline`, and runs the loop. Mirrors the
//! validated `spikes/vslice` prototype, now on the real crate architecture. (The Tauri `app/`
//! shell replaces this later; the pipeline is unchanged.)

mod settings_ui;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use nib_asr::{Sidecar, SidecarConfig, SidecarKind};
use nib_audio::Capture;
use nib_cleanup::Mode;
use nib_pipeline::{hotkey_forwarder, PipeMsg, Pipeline, Stats};
use nib_platform::Autostart;
use nib_platform::{HotkeySource, InputWitness, PathLayout, TargetProbe, TextInjector};
use nib_win32::{
    OverlayStyle, TrayCommand, Win32Autostart, Win32Hotkey, Win32Injector, Win32Overlay,
    Win32Paths, Win32TargetProbe, Win32Tray, STYLE_MAX_INDEX,
};

/// Default overlay style (the tactical-comms HUD).
const DEFAULT_STYLE: u8 = OverlayStyle::Hud.index();

/// `--overlay <token>` → style index, via OverlayStyle's own token parser (unknown → default).
fn style_index(name: &str) -> u8 {
    OverlayStyle::from_token(&name.to_ascii_lowercase())
        .map(|s| s.index())
        .unwrap_or(DEFAULT_STYLE)
}

/// Locate the ASR model, downloading it on first run if necessary.
///
/// Precedence: `NIB_ASR_MODEL_DIR` → an existing install under `%LOCALAPPDATA%` → an existing dev
/// checkout (so contributors reuse the copy they already have) → download.
///
/// The download is the product's ONLY network access, so it announces itself plainly rather than
/// happening silently — "no egress" is the core promise and users should see the one exception.
fn acquire_model(data: &Path, dev: &Path) -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("NIB_ASR_MODEL_DIR") {
        return Ok(PathBuf::from(p));
    }
    let spec = &nib_models::PARAKEET_EN_INT8;
    let models_dir = data.join("models");
    // Reuse a dev checkout's copy rather than making contributors re-download 460 MB.
    let dev_copy = dev.join("spikes/s2-asr/models").join(spec.dir_name);
    if !models_dir.join(spec.dir_name).join(spec.sentinel).exists()
        && dev_copy.join(spec.sentinel).exists()
    {
        return Ok(dev_copy);
    }

    let mut announced = false;
    let installed = nib_models::ensure_model(spec, &models_dir, |p| match p {
        nib_models::Progress::Downloading { done, total } => {
            if !announced {
                announced = true;
                println!(
                    "\nFirst run: downloading the speech model ({}) — {}.\n\
                     This is the only time Nib uses the network; afterwards it runs fully offline.\n\
                     Source: {}\n",
                    spec.name,
                    nib_models::human_bytes(total),
                    spec.url
                );
            }
            let pct = if total > 0 { done * 100 / total } else { 0 };
            print!(
                "\r  {pct:>3}%  {} / {}          ",
                nib_models::human_bytes(done),
                nib_models::human_bytes(total)
            );
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
        nib_models::Progress::Verifying => println!("\n  verifying checksum..."),
        nib_models::Progress::Extracting => println!("  extracting..."),
    })?;
    if installed.freshly_downloaded {
        println!("  model installed to {}\n", installed.dir.display());
    }
    Ok(installed.dir)
}

/// How long a fatal error stays on screen when nobody presses Enter. Long enough to read and
/// screenshot, short enough that an unattended launcher isn't left waiting on a dead prompt.
const FATAL_PAUSE_SECS: u64 = 60;

/// Report a fatal startup failure and exit — WITHOUT the message disappearing with the window.
///
/// Every startup failure used to be `eprintln!` + `exit(1)`. When the app is launched from a
/// shortcut, a double-click or the installer's "run now" checkbox, Windows gives it a private
/// console that is destroyed the instant the process exits, so the user sees a window flash and
/// vanish with no explanation — the reported "it force closed and disappeared". Pause only when we
/// actually own the console; launched from a terminal (or with output redirected) the text stays
/// on screen and pausing would hang scripts.
fn fatal(msg: &str) -> ! {
    eprintln!("\n{msg}");
    if nib_win32::owns_console() {
        eprintln!(
            "\nPress Enter to close this window (closes automatically in {FATAL_PAUSE_SECS}s)..."
        );
        // BOUNDED wait. The pause exists so a human can read the error before the console is
        // destroyed — but plenty of launchers have no human at all: an installer's smoke test, a
        // CI sandbox, a scheduled task. There, `owns_console()` is still true and an unbounded
        // read blocks forever, turning a clean failure into a hung process. (Exactly what
        // winget's validation sandbox hit.) Reading on a worker thread and giving up on a timeout
        // serves both: the message stays up long enough to read, and nothing ever hangs.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut sink = String::new();
            let _ = std::io::stdin().read_line(&mut sink);
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(std::time::Duration::from_secs(FATAL_PAUSE_SECS));
    }
    std::process::exit(1);
}

/// Human-readable name for a configured chord, for the startup banner. An unset chord says so
/// rather than printing nothing, so the banner never advertises a key that does not exist.
fn combo_name(b: &Option<nib_platform::Binding>) -> String {
    let Some(b) = b else {
        return "(disabled)".to_string();
    };
    let name = |vk: u16| match vk {
        0x11 => "Ctrl".to_string(),
        0x12 => "Alt".to_string(),
        0x10 => "Shift".to_string(),
        0x5B | 0x5C => "Win".to_string(),
        0x30..=0x5A => ((vk as u8) as char).to_string(),
        0x70..=0x7B => format!("F{}", vk - 0x6F),
        other => format!("VK{other:#04X}"),
    };
    b.keys
        .iter()
        .map(|&k| name(k))
        .collect::<Vec<_>>()
        .join("+")
}

/// Resolve an asset: an explicit `env_var` override wins; else the installed location if it
/// exists; else the dev-checkout path (baked at compile time — only present in a source tree).
fn resolve_asset(env_var: &str, installed: PathBuf, dev: PathBuf) -> PathBuf {
    if let Some(p) = std::env::var_os(env_var) {
        return PathBuf::from(p);
    }
    if installed.exists() {
        return installed;
    }
    dev
}

fn main() {
    let paths = Win32Paths;

    // ---- settings.toml supplies the defaults; CLI flags below override them ----
    // Written as a commented template on first run so the file documents its own options.
    let settings_path = paths.config_dir().join("settings.toml");
    if let Err(e) = nib_config::settings::ensure_template(&settings_path) {
        eprintln!("(could not write {}: {e})", settings_path.display());
    }
    let settings = nib_config::settings::load(&settings_path);
    apply_autostart(&settings);

    // ---- args: [model-dir] [--mode …] [--sidecar …] [--overlay …] [--gpu-layers N] ----
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut mode = Mode::parse(&settings.mode);
    let mut n_gpu_layers = 0i32;
    let mut model_override: Option<String> = None;
    let mut overlay_enabled = settings.overlay;
    let mut open_settings_at_start = false;
    let mut style_idx = style_index(&settings.overlay_style);
    let mut sidecar_kind = match settings.sidecar.as_str() {
        "python" => SidecarKind::Python,
        _ => SidecarKind::Native,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--mode" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    mode = Mode::parse(v);
                }
            }
            "--gpu-layers" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    n_gpu_layers = v.parse().unwrap_or(0);
                }
            }
            "--overlay" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    style_idx = style_index(v);
                }
            }
            "--no-overlay" => overlay_enabled = false,
            // Open the settings window as soon as the app is up (handy for a first look and for
            // drivers/tests; the tray's "Settings…" item does the same thing later).
            "--settings" => open_settings_at_start = true,
            "--sidecar" => {
                i += 1;
                // Strict: a typo must not silently start the wrong sidecar (a Pro user would get
                // Raw/Auto only, with no clue why). Also don't swallow a following flag.
                match args.get(i).map(|s| s.to_ascii_lowercase()) {
                    Some(v) if v == "native" => sidecar_kind = SidecarKind::Native,
                    Some(v) if v == "python" => sidecar_kind = SidecarKind::Python,
                    other => {
                        eprintln!(
                            "--sidecar expects `native` or `python`, got {}",
                            other.as_deref().unwrap_or("nothing")
                        );
                        std::process::exit(2);
                    }
                }
            }
            s if !s.starts_with("--") => model_override = Some(s.to_string()),
            _ => {}
        }
        i += 1;
    }

    // ---- resolve assets: env override → installed (%LOCALAPPDATA%\Nib) → dev checkout ----
    // (ASR model is Parakeet TDT 0.6B **v2** int8 — the CPU-English stand-in; the plan pins v3 for
    // the multilingual GPU path.)
    let data = paths.data_dir();
    // Dev fallback root, baked at compile time (only present in a source checkout).
    let dev = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let cfg = SidecarConfig {
        kind: sidecar_kind,
        program: match sidecar_kind {
            // Shipping layout: the engine sits next to HoldToSpeak.exe (with its DLLs).
            SidecarKind::Native => resolve_asset(
                "NIB_SIDECAR",
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join("HoldToSpeak-engine.exe")))
                    .unwrap_or_default(),
                dev.join("target/release/HoldToSpeak-engine.exe"),
            ),
            SidecarKind::Python => resolve_asset(
                "NIB_SIDECAR",
                data.join("asr_sidecar.py"),
                dev.join("crates/nib-asr/sidecar/asr_sidecar.py"),
            ),
        },
        model_dir: match model_override {
            Some(p) => PathBuf::from(p),
            None => match acquire_model(&data, &dev) {
                Ok(p) => p,
                Err(e) => fatal(&format!(
                    "Could not obtain the speech model: {e}\n\
                     HoldToSpeak needs it once, then works entirely offline. Check your internet \
                     connection and run it again, or point it at an existing copy with \
                     --model-dir / NIB_ASR_MODEL_DIR."
                )),
            },
        },
        llm_model: resolve_asset(
            "NIB_LLM_MODEL",
            data.join("models/Qwen3-1.7B-Q8_0.gguf"),
            dev.join("spikes/vslice/models/Qwen3-1.7B-Q8_0.gguf"),
        ),
        // The dictionary is WRITABLE (the `learn` command appends to it), so unlike the read-only
        // assets it must not be exists()-gated — that would mean the installed path could never
        // start existing and `learn` would silently stop persisting off-tree. Always use the
        // config dir (created here); seed it from the dev copy on first run so learned jargon
        // carries over.
        dictionary: std::env::var_os("NIB_DICTIONARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let cfg_dir = paths.config_dir();
                let _ = std::fs::create_dir_all(&cfg_dir);
                let p = cfg_dir.join("dictionary.txt");
                if !p.exists() {
                    let seed = dev.join("crates/nib-asr/sidecar/dictionary.txt");
                    if seed.exists() {
                        let _ = std::fs::copy(&seed, &p);
                    } else {
                        // Create it empty so the sidecar gets a --dictionary arg and `learn`
                        // has somewhere to persist from the very first run.
                        let _ = std::fs::write(&p, "");
                    }
                }
                p
            }),
        n_gpu_layers,
        warm_mode: mode.token().to_string(),
    };

    // Kept for the settings window (the config is consumed by the sidecar spawn below).
    let dictionary_path = cfg.dictionary.clone();
    let model_dir = cfg.model_dir.clone();

    println!("=== HoldToSpeak ===");
    println!(
        "Loading ASR{} sidecar...",
        match sidecar_kind {
            SidecarKind::Native => "",
            SidecarKind::Python => " + cleanup",
        }
    );
    // Start the sidecar, retrying ONCE before giving up. When sherpa-onnx/onnxruntime fails while
    // loading the model it takes the whole sidecar process down — observed both as a null
    // dereference and as a fail-fast abort — rather than returning an error we could inspect, so
    // the recogniser's own `create() -> None` check never gets the chance to run. That failure has
    // been seen to be transient (the very next launch succeeds), which is exactly the case a retry
    // is for: it costs one extra model load instead of costing the user the whole session.
    let sidecar = match Sidecar::spawn(cfg.clone()) {
        Ok(s) => s,
        Err(first) => {
            eprintln!("ASR engine failed to start ({first}) — retrying once...");
            match Sidecar::spawn(cfg) {
                Ok(s) => s,
                Err(second) => {
                    let hint = match sidecar_kind {
                        SidecarKind::Native =>
                            "If you built from source, build the engine with:\n  \
                             cargo build --release -p nib-asr-sidecar\n\
                             Otherwise please report this at \
                             https://github.com/rootMonsteR/holdtospeak/issues with the text above.",
                        SidecarKind::Python =>
                            "Needs Python on PATH with sherpa-onnx + the Parakeet model — see README.",
                    };
                    fatal(&format!(
                        "The speech engine could not start (tried twice).\n  \
                         1st attempt: {first}\n  2nd attempt: {second}\n\n{hint}"
                    ))
                }
            }
        }
    };
    // Only offer modes the running sidecar can serve: without an LLM, Polish/Email fall back to
    // the deterministic Auto tidy rather than silently doing nothing.
    let llm = sidecar.has_llm();
    let mode = mode.clamp_available(llm);
    let capture = match Capture::start() {
        Ok(c) => c,
        Err(e) => fatal(&format!(
            "Could not open the microphone: {e}\n\
             Check that a microphone is connected, and that Windows allows desktop apps to use \
             it (Settings → Privacy & security → Microphone)."
        )),
    };
    println!("Ready.  mic: {}", capture.device_name());

    // ---- platform capabilities (the only place that names the concrete Win32 impls) ----
    let injector: Box<dyn TextInjector> = Box::new(Win32Injector);
    // Shared: the pipeline snapshots with it, and the hotkey forwarder warms it on key-down.
    let probe: Arc<dyn TargetProbe + Send + Sync> = Arc::new(Win32TargetProbe);
    // Count the user's own keystrokes so the pipeline can tell whether the caret is still where
    // it left it. Installed before the hook starts, or the first keystrokes would go unseen.
    let user_input = InputWitness::default();
    Win32Hotkey::watch_user_input(user_input.clone());
    let hotkey = Win32Hotkey;
    let hotkeys_path = resolve_asset(
        "NIB_HOTKEYS",
        paths.config_dir().join("hotkeys.toml"),
        dev.join("spikes/vslice/hotkeys.toml"),
    );
    let hk = nib_config::load(&hotkeys_path);

    // ---- shared mode/style, read by the tray (menu checks) and overlay (theme/label) ----
    let current_mode = Arc::new(AtomicU8::new(mode.index()));
    let current_style = Arc::new(AtomicU8::new(style_idx));
    // True only while PTT is held — gates the overlay's visibility (set by the hotkey forwarder).
    let current_listening = Arc::new(AtomicBool::new(false));

    // ---- floating voice-spectrum overlay (shown only while PTT is held) ----
    // The thread always runs; `current_overlay_enabled` is the user's switch, so the settings
    // window can turn the overlay on or off live. Idle it costs one poll every 60 ms.
    let current_overlay_enabled = Arc::new(AtomicBool::new(overlay_enabled));
    {
        let monitor = capture.monitor();
        let rate = monitor.sample_rate();
        Win32Overlay::spawn(
            Box::new(move |n| monitor.latest(n)),
            current_style.clone(),
            current_mode.clone(),
            current_listening.clone(),
            current_overlay_enabled.clone(),
            rate,
        );
    }

    // ---- wire the channels: hook + tray + console → pipeline ----
    // The forwarder freezes the capture window at key-event time (via CaptureControl) so a burst
    // of PTT presses during a prior transcription can't lose audio.
    let (tx, rx) = channel::<PipeMsg>();
    // Chord bindings and their meanings are built together (nib-config::Hotkeys::chord_slots) so
    // the indices the hook reports and the actions taken here cannot drift apart — positional
    // slots, so the settings window can rebind live without touching this table.
    let (chord_bindings, chord_actions) = hk.chord_slots();
    hotkey.start(
        hk.ptt.clone(),
        chord_bindings,
        hotkey_forwarder(
            tx.clone(),
            capture.control(),
            current_listening.clone(),
            probe.clone(),
            chord_actions,
            current_style.clone(),
            STYLE_MAX_INDEX + 1,
        ),
    );

    // Ctrl+C / console close: trigger the graceful quit (so Sidecar::Drop kills the Python child)
    // and wait briefly for the main thread to finish teardown before the handler exits the
    // process. Exiting immediately would skip Drop and leave an orphaned sidecar.
    let shutdown_done = Arc::new(AtomicBool::new(false));
    {
        let tx = tx.clone();
        let done = shutdown_done.clone();
        Win32Tray::install_ctrl_handler(move || {
            let _ = tx.send(PipeMsg::Quit);
            for _ in 0..100 {
                if done.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        });
    }

    let (tray_tx, tray_rx) = channel::<TrayCommand>();
    Win32Tray::spawn(tray_tx, current_mode.clone(), current_style.clone());
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for cmd in tray_rx {
                let msg = match cmd {
                    TrayCommand::SetMode(i) => PipeMsg::SetMode(i),
                    TrayCommand::SetStyle(i) => PipeMsg::SetStyle(i),
                    TrayCommand::CycleMode => PipeMsg::CycleMode,
                    TrayCommand::OpenSettings => {
                        settings_ui::open();
                        continue;
                    }
                    TrayCommand::Quit => PipeMsg::Quit,
                };
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
    }
    {
        let tx = tx.clone();
        std::thread::spawn(move || console_loop(tx));
    }

    // Spell out which keys are GLOBAL and which are typed into this window. The old banner listed
    // "m" and "q" without that distinction, so they read as hotkeys — and then did nothing.
    println!(
        "Hold your PTT keys (default Ctrl+Win) and speak → text at your cursor.\n\
         Mode: {}.\n\
         Hotkeys (anywhere):  {} = cycle mode · {} = cycle overlay · {} = quit.\n\
         Console (type here): m = cycle mode · learn <heard> => <meant> · q = quit.\n\
         (Focus a text field first — e.g. Notepad.)",
        mode.short_name(),
        combo_name(&hk.cycle_mode),
        combo_name(&hk.cycle_style),
        combo_name(&hk.quit),
    );

    // ---- shared with the settings window: recent dictations + the live silence gate ----
    let stats = Arc::new(Stats::default());
    let silence_gate = Arc::new(AtomicU32::new(settings.silence_rms.to_bits()));

    // ---- the settings window (its own UI thread; the window itself opens on demand) ----
    let engine: &'static str = match sidecar_kind {
        SidecarKind::Native => "native",
        SidecarKind::Python => "python",
    };
    let model_spec = &nib_models::PARAKEET_EN_INT8;
    settings_ui::spawn(settings_ui::Bridge {
        tx: Mutex::new(tx.clone()),
        mode: current_mode.clone(),
        style: current_style.clone(),
        listening: current_listening.clone(),
        overlay_enabled: current_overlay_enabled.clone(),
        silence_gate: silence_gate.clone(),
        monitor: capture.monitor(),
        stats: stats.clone(),
        settings: Mutex::new(settings.clone()),
        settings_path: settings_path.clone(),
        hotkeys: Mutex::new(hk),
        hotkeys_path: hotkeys_path.clone(),
        dictionary_path,
        config_dir: paths.config_dir(),
        data_dir: paths.data_dir(),
        engine,
        llm,
        mic_name: capture.device_name(),
        model: settings_ui::ModelInfo {
            name: model_spec.name.to_string(),
            dir: model_dir,
            bytes: model_spec.bytes,
            sha256: model_spec.sha256.to_string(),
        },
        started: std::time::Instant::now(),
    });
    if open_settings_at_start {
        settings_ui::open();
    }

    // Capture is !Send — build and run the pipeline here on the main thread.
    let mut pipeline = Pipeline::new(
        injector,
        probe,
        Box::new(capture),
        sidecar,
        mode,
        current_mode,
        user_input,
        stats,
        silence_gate,
    );
    pipeline.run(rx);
    drop(pipeline); // Sidecar::Drop shuts the Python child down here
    shutdown_done.store(true, std::sync::atomic::Ordering::SeqCst);
    Win32Tray::delete_icon();
    println!("bye.");
}

/// Read console commands and forward them to the pipeline.
fn console_loop(tx: Sender<PipeMsg>) {
    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        line.clear();
        if stdin.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let t = line.trim();
        let lower = t.to_ascii_lowercase();
        if matches!(lower.as_str(), "q" | "quit" | "exit") {
            let _ = tx.send(PipeMsg::Quit);
            break;
        } else if matches!(lower.as_str(), "m" | "mode") {
            let _ = tx.send(PipeMsg::CycleMode);
        } else if let Some(rest) = lower
            .strip_prefix("learn ")
            .map(|_| t[6..].trim().to_string())
        {
            let _ = tx.send(PipeMsg::Learn(rest));
        }
    }
}

/// Reconcile the Run-key entry with `settings.autostart`.
///
/// The settings file is the source of truth: editing it (or toggling from the tray, which writes
/// it) is what turns start-with-Windows on or off. Failure is reported but never fatal — not being
/// able to write a registry value is no reason to refuse to dictate.
fn apply_autostart(settings: &nib_config::Settings) {
    let auto = Win32Autostart;
    if auto.get() == settings.autostart {
        return;
    }
    match auto.set(settings.autostart) {
        Ok(()) => println!(
            "start with Windows: {}",
            if settings.autostart {
                "enabled"
            } else {
                "disabled"
            }
        ),
        Err(e) => eprintln!("(could not change start-with-Windows: {e})"),
    }
}
