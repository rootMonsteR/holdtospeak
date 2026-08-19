//! `nib-asr-sidecar` — the native ASR sidecar for the public free build. Loads a Parakeet TDT
//! int8 transducer via sherpa-onnx (ONNX Runtime, CPU) and speaks the SAME line protocol as the
//! Python sidecar, so `nib-asr::Sidecar` and the pipeline are unchanged:
//!
//!   stdout `READY caps=<...>` once warm (caps lists optional features; the free build has none)
//!   stdin  `<mode>\t<wav_path>`  → stdout the (deterministically) cleaned transcript
//!   stdin  `__learn__\t<w> => <m>` → stdout an ack; persists to the dictionary
//!   stdin  `__quit__`             → exit
//!   on any per-request failure     → stdout `__error__ <message>`
//!
//! Free-tier cleanup is fully deterministic (personal dictionary + Auto tidy, in `nib-cleanup`) —
//! no LLM, so the whole thing is one self-contained binary + the sherpa/onnxruntime DLLs. The Pro
//! tier keeps the Python sidecar for LLM Polish/Email.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use nib_cleanup::{auto_tidy, polish_tidy, Dictionary, Mode};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig, Wave};

struct Args {
    model_dir: PathBuf,
    dictionary: Option<PathBuf>,
    num_threads: i32,
}

fn parse_args() -> Args {
    let mut model_dir = PathBuf::new();
    let mut dictionary = None;
    // Match the Python sidecar's 8 (capped by the machine) — 4 was a straight latency regression
    // on the shipping default path.
    let mut num_threads = std::thread::available_parallelism()
        .map(|n| (n.get() as i32).min(8))
        .unwrap_or(4);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--model-dir" => model_dir = it.next().map(PathBuf::from).unwrap_or_default(),
            "--dictionary" => dictionary = it.next().map(PathBuf::from),
            "--num-threads" => num_threads = it.next().and_then(|s| s.parse().ok()).unwrap_or(4),
            // Accept-and-ignore the Pro-only flags so the same launcher args work for both
            // sidecars (the native build simply has no LLM).
            "--llm-model" | "--n-gpu-layers" | "--warm-mode" => {
                let _ = it.next();
            }
            _ => {}
        }
    }
    Args {
        model_dir,
        dictionary,
        num_threads,
    }
}

fn main() {
    let args = parse_args();

    // Load OUR onnxruntime.dll before sherpa can trigger the implicit import. Windows ships an
    // older ONNX Runtime in System32, and if our copy fails to load the loader silently falls back
    // to that one — which sherpa then crashes inside, because it asks for an API version the older
    // runtime does not have. Doing it explicitly turns that into a readable error, and the retry
    // inside absorbs the transient first-run load failure that triggered it in the first place.
    if let Err(e) = nib_ortload_sys::pin_beside_exe("onnxruntime.dll") {
        eprintln!("nib-asr-sidecar: {e}");
        std::process::exit(1);
    }

    // Parakeet TDT transducer: encoder/decoder/joiner + tokens. Found by substring (preferring
    // int8) rather than exact filename, so any standard sherpa export works — the Python sidecar
    // globs the same way, and hardcoding `*.int8.onnx` rejected dirs it accepts.
    let enc = find_model(&args.model_dir, "encoder");
    let dec = find_model(&args.model_dir, "decoder");
    let joi = find_model(&args.model_dir, "joiner");
    let tok = args.model_dir.join("tokens.txt");
    let (Some(enc), Some(dec), Some(joi)) = (enc, dec, joi) else {
        eprintln!(
            "nib-asr-sidecar: no encoder/decoder/joiner .onnx found in {}",
            args.model_dir.display()
        );
        std::process::exit(1);
    };
    if !tok.exists() {
        eprintln!("nib-asr-sidecar: missing {}", tok.display());
        std::process::exit(1);
    }

    let mut cfg = OfflineRecognizerConfig::default();
    cfg.model_config.transducer = OfflineTransducerModelConfig {
        encoder: Some(path_str(&enc)),
        decoder: Some(path_str(&dec)),
        joiner: Some(path_str(&joi)),
    };
    cfg.model_config.tokens = Some(path_str(&tok));
    cfg.model_config.model_type = Some("nemo_transducer".to_string());
    cfg.model_config.num_threads = args.num_threads;
    cfg.model_config.provider = Some("cpu".to_string());

    let recognizer = match OfflineRecognizer::create(&cfg) {
        Some(r) => r,
        None => {
            eprintln!("nib-asr-sidecar: failed to create recognizer (bad model?)");
            std::process::exit(1);
        }
    };

    let mut dict = args.dictionary.as_deref().map(Dictionary::load);

    // Warm before announcing READY. ORT picks kernels and grows its arena on the first real
    // decode, and that work is shape-sensitive — warming on digital silence barely touches it, so
    // use a real sample wav (models ship `test_wavs/`) and repeat, like the Python sidecar. Falls
    // back to silence if the model dir has no samples.
    {
        let sample = find_test_wav(&args.model_dir);
        for _ in 0..2 {
            let stream = recognizer.create_stream();
            match sample.as_ref().and_then(|p| Wave::read(&path_str(p))) {
                Some(w) => stream.accept_waveform(w.sample_rate(), w.samples()),
                None => stream.accept_waveform(16000, &[0.0f32; 16000]),
            }
            recognizer.decode(&stream);
            let _ = stream.get_result();
        }
    }

    // Free build advertises no optional capabilities; the client shows only Raw/Auto.
    println!("READY caps=");
    let _ = std::io::stdout().flush();

    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line == "__quit__" {
            break;
        }
        if let Some(mapping) = line.strip_prefix("__learn__\t") {
            let ack = match dict.as_mut() {
                Some(d) => d.learn(mapping),
                None => "learn: no dictionary configured".to_string(),
            };
            let _ = writeln!(out, "{ack}");
            let _ = out.flush();
            continue;
        }
        // `<mode>\t<wav_path>`
        let reply = match line.split_once('\t') {
            Some((mode, wav)) => transcribe(&recognizer, dict.as_ref(), mode, wav),
            None => "__error__ malformed request (expected <mode>\\t<wav>)".to_string(),
        };
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
}

/// Decode one wav and apply free-tier cleanup for the mode. Returns the transcript, an empty
/// string for no speech, or `__error__ <msg>` on failure — matching the Python protocol.
fn transcribe(
    recognizer: &OfflineRecognizer,
    dict: Option<&Dictionary>,
    mode: &str,
    wav_path: &str,
) -> String {
    let wave = match Wave::read(wav_path) {
        Some(w) => w,
        None => return format!("__error__ could not read wav: {wav_path}"),
    };
    let stream = recognizer.create_stream();
    stream.accept_waveform(wave.sample_rate(), wave.samples());
    recognizer.decode(&stream);
    let raw = match stream.get_result() {
        Some(r) => r.text,
        None => return String::new(),
    };
    if raw.trim().is_empty() {
        return String::new();
    }

    // Dictionary fixes apply in EVERY mode (incl. Raw). Auto adds the light deterministic tidy;
    // Polish adds discourse-marker stripping on top. Email needs an LLM this build does not have,
    // so it gets Polish — the strongest cleanup actually available — rather than pretending.
    let dicted = match dict {
        Some(d) => d.apply(&raw),
        None => raw.clone(),
    };
    match Mode::parse(mode) {
        Mode::Raw => dicted,
        Mode::Auto => auto_tidy(&dicted),
        Mode::Polish | Mode::Email => polish_tidy(&dicted),
    }
}

fn path_str(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Find `<model_dir>/*<part>*.onnx`, preferring an int8 quantization when both are present.
fn find_model(dir: &std::path::Path, part: &str) -> Option<PathBuf> {
    let mut best: Option<PathBuf> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        let name = p.file_name()?.to_string_lossy().to_ascii_lowercase();
        if !name.ends_with(".onnx") || !name.contains(part) {
            continue;
        }
        let is_int8 = name.contains("int8");
        if best.is_none() || is_int8 {
            best = Some(p.clone());
        }
        if is_int8 {
            break;
        }
    }
    best
}

/// A sample wav shipped with the model, used to warm the recognizer realistically.
fn find_test_wav(model_dir: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(model_dir.join("test_wavs"))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")))
}
