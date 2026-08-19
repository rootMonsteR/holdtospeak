//! ASR + cleanup sidecar client. Spawns the Python sidecar (Parakeet ASR + llama.cpp cleanup +
//! learning dictionary) and speaks its line protocol: `"<mode>\t<wav_path>"` → transcript, and
//! `"__learn__\t<wrote> => <meant>"` → ack. The sidecar stays warm across requests.
//!
//! This increment keeps the Python sidecar; production swaps it for a parakeet.cpp Rust sidecar
//! behind the same line protocol (that's what makes crash isolation real).
#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// Which sidecar implementation backs ASR + cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarKind {
    /// The shipping free-tier sidecar: a native Rust binary (sherpa-onnx). No Python, no LLM.
    Native,
    /// The dev/Pro sidecar: `python asr_sidecar.py` with llama.cpp cleanup (Polish/Email).
    Python,
}

/// Where to find the sidecar (native exe or Python script), the ASR model dir, the cleanup GGUF,
/// and the dictionary.
///
/// `Clone` so a failed startup can be retried with the same configuration.
#[derive(Clone)]
pub struct SidecarConfig {
    pub kind: SidecarKind,
    /// The native sidecar exe, or the Python script — whichever `kind` selects.
    pub program: PathBuf,
    pub model_dir: PathBuf,
    pub llm_model: PathBuf,
    pub dictionary: PathBuf,
    /// llama.cpp GPU offload: 0 = CPU, -1 = all layers on the GPU (needs a CUDA/Vulkan build).
    pub n_gpu_layers: i32,
    /// Mode to warm the cleanup LLM on at startup (raw/auto/polish/email).
    pub warm_mode: String,
}

/// A running, warm ASR/cleanup sidecar process. Owns its config so it can respawn itself if the
/// process dies (see [`Sidecar::request`]).
pub struct Sidecar {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    cfg: SidecarConfig,
    /// Optional features the running sidecar advertised in its `READY caps=…` line. The free
    /// (native) build advertises none; the Python build advertises `llm` when a GGUF loaded.
    caps: Vec<String>,
}

/// The live pipes of a spawned sidecar process.
struct Proc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    caps: Vec<String>,
}

impl Sidecar {
    /// Spawn the sidecar and block until it prints `READY`. Err on spawn failure or early exit.
    pub fn spawn(cfg: SidecarConfig) -> Result<Sidecar, String> {
        let p = Self::spawn_proc(&cfg)?;
        Ok(Sidecar {
            child: p.child,
            stdin: p.stdin,
            stdout: p.stdout,
            cfg,
            caps: p.caps,
        })
    }

    /// True if the running sidecar can do LLM cleanup (Polish/Email). False for the free build.
    pub fn has_llm(&self) -> bool {
        self.caps.iter().any(|c| c == "llm")
    }

    /// Launch the process and wait for `READY`. The LLM/dictionary args are only passed when those
    /// files exist (cleanup then degrades to Raw, matching the prototype).
    fn spawn_proc(cfg: &SidecarConfig) -> Result<Proc, String> {
        // Name the missing path up front — "exited before READY" with raw Python stderr is a
        // miserable first-run error when the real cause is an unresolved install/dev path.
        if !cfg.program.exists() {
            return Err(format!(
                "sidecar not found: {} (kind {:?})",
                cfg.program.display(),
                cfg.kind
            ));
        }
        if !cfg.model_dir.exists() {
            return Err(format!(
                "ASR model dir not found: {}",
                cfg.model_dir.display()
            ));
        }
        let mut cmd = match cfg.kind {
            // The native sidecar IS the program.
            SidecarKind::Native => Command::new(&cfg.program),
            SidecarKind::Python => {
                let mut c = Command::new("python");
                // Force UTF-8 stdio on Windows Python (default is the ANSI code page): curly
                // quotes from the cleanup LLM would otherwise arrive as invalid UTF-8 and break
                // read_line.
                c.env("PYTHONUTF8", "1");
                c.arg(&cfg.program);
                c
            }
        };
        cmd.arg("--model-dir").arg(&cfg.model_dir);
        // LLM args are Pro-only; the native sidecar accepts-and-ignores them.
        if cfg.kind == SidecarKind::Python && cfg.llm_model.exists() {
            cmd.arg("--llm-model")
                .arg(&cfg.llm_model)
                .arg("--n-gpu-layers")
                .arg(cfg.n_gpu_layers.to_string())
                .arg("--warm-mode")
                .arg(&cfg.warm_mode);
        }
        if cfg.dictionary.exists() {
            cmd.arg("--dictionary").arg(&cfg.dictionary);
        }
        // For the native sidecar the usual failure is a missing DLL beside the exe (CreateProcess
        // → ERROR_MOD_NOT_FOUND), which otherwise surfaces as an opaque spawn error — name it.
        if cfg.kind == SidecarKind::Native {
            if let Some(dir) = cfg.program.parent() {
                for dll in ["sherpa-onnx-c-api.dll", "onnxruntime.dll"] {
                    if !dir.join(dll).exists() {
                        return Err(format!(
                            "{dll} is missing next to {} — the native sidecar needs its runtime \
                             DLLs in the same folder",
                            cfg.program.display()
                        ));
                    }
                }
            }
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn {:?} asr sidecar: {e}", cfg.kind))?;
        let stdin = child.stdin.take().ok_or("no sidecar stdin")?;
        let mut stdout = BufReader::new(child.stdout.take().ok_or("no sidecar stdout")?);
        // Handshake: `READY` (legacy) or `READY caps=a,b` — caps advertises optional features
        // (currently just `llm`) so the UI can offer only the modes the sidecar can serve.
        let mut line = String::new();
        let caps = loop {
            line.clear();
            if stdout.read_line(&mut line).unwrap_or(0) == 0 {
                // Name the exit code. A crash inside sherpa-onnx/onnxruntime surfaces here as an
                // access violation (0xC0000005) or a fail-fast abort (0xC0000409); a bad argument
                // surfaces as our own exit(1). Without the code every one of those reads as the
                // same opaque "exited before READY", which is useless in a bug report.
                let how = match child.wait() {
                    Ok(st) => match st.code() {
                        Some(c) => format!("exit code {c} (0x{:08X})", c as u32),
                        None => "terminated without an exit code".to_string(),
                    },
                    Err(e) => format!("exit status unavailable: {e}"),
                };
                return Err(format!("asr sidecar exited before READY — {how}"));
            }
            let t = line.trim();
            if t == "READY" {
                break Vec::new();
            }
            if let Some(rest) = t.strip_prefix("READY caps=") {
                break rest
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
        };
        Ok(Proc {
            child,
            stdin,
            stdout,
            caps,
        })
    }

    /// One request/response round trip, with a single respawn-and-retry if the sidecar died.
    ///
    /// A crashed sidecar used to silently drop the utterance. Now we notice the dead pipe, restart
    /// the process, and replay the request once — so a one-off crash costs a model reload, not the
    /// user's sentence. `None` means it's still unusable after the retry.
    fn request(&mut self, line: &str) -> Option<String> {
        if let Some(out) = self.try_request(line) {
            return Some(out);
        }
        match self.child.try_wait() {
            Ok(Some(status)) => eprintln!("  ASR sidecar died (exited: {status}) — restarting..."),
            _ => eprintln!("  ASR sidecar is not responding — restarting..."),
        }
        if !self.restart() {
            return None;
        }
        let out = self.try_request(line);
        if out.is_some() {
            eprintln!("  ASR sidecar restarted.");
        } else {
            eprintln!("  ASR sidecar still unusable after restart — utterance dropped.");
        }
        out
    }

    /// Write one line and read one line back. `None` on a broken pipe / EOF — including a
    /// PARTIAL line (no trailing newline): a sidecar crashing mid-write must not have its
    /// truncated output treated as a transcript (a cut-off `__error_` fragment would even evade
    /// the error guard). Partial output triggers the restart path like any other death.
    fn try_request(&mut self, line: &str) -> Option<String> {
        if writeln!(self.stdin, "{line}").is_err() || self.stdin.flush().is_err() {
            return None;
        }
        let mut out = String::new();
        match self.stdout.read_line(&mut out) {
            Ok(n) if n > 0 && out.ends_with('\n') => Some(out.trim().to_string()),
            _ => None,
        }
    }

    /// Reap the dead process and spawn a fresh one. False if the respawn failed.
    fn restart(&mut self) -> bool {
        let _ = self.child.kill();
        let _ = self.child.wait();
        match Self::spawn_proc(&self.cfg) {
            Ok(p) => {
                self.child = p.child;
                self.stdin = p.stdin;
                self.stdout = p.stdout;
                self.caps = p.caps;
                true
            }
            Err(e) => {
                eprintln!("  ASR sidecar restart failed: {e}");
                false
            }
        }
    }

    /// Transcribe `wav`, cleaned per the `mode` token (raw/auto/polish/email). None if the
    /// sidecar died.
    pub fn transcribe(&mut self, mode: &str, wav: &Path) -> Option<String> {
        self.request(&format!("{mode}\t{}", wav.display()))
    }

    /// Teach a permanent jargon fix (`"<wrote> => <meant>"`), persisted to the dictionary. None
    /// if the sidecar died.
    pub fn learn(&mut self, mapping: &str) -> Option<String> {
        self.request(&format!("__learn__\t{mapping}"))
    }
}

impl Drop for Sidecar {
    /// Ensure the sidecar process is gone on quit: ask it to exit (`__quit__`), give the graceful
    /// path a short window to win, then force-kill — a wedged / mid-model-load sidecar isn't
    /// reading stdin and would otherwise linger. (std's `Child` does NOT kill on drop, so relying
    /// on stdin-EOF alone leaves orphans.)
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "__quit__");
        let _ = self.stdin.flush();
        for _ in 0..10 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return; // exited gracefully
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
