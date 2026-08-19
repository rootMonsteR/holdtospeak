//! S3 injection-matrix — the minimal-but-real de-risk harness. For each target it injects a
//! Unicode sentinel via the real route chain (`nib-inject` + `nib-win32`'s `Win32Injector`), reads
//! it back (UIA ValuePattern), and checks: exact match, clipboard restored, and — for password
//! targets — that the injector *refused*. Config-driven (`bench/targets.toml`) so the grid grows
//! toward the full 40-target matrix without code changes.
//!
//! The live cells actually type into a launched app, so run it on a real desktop:
//!   cargo run -p injection-matrix -- --targets bench/targets.toml
//! `cargo test` only exercises the pure parsing/refusal logic (no SendInput), so CI is safe.

use std::process::Command;
use std::time::Duration;

use nib_inject::inject_with_fallback;
use nib_platform::{InjectOutcome, TargetProbe, TargetProfile};
use nib_win32::{
    clipboard_get, clipboard_set, foreground_exe, uia_focused_text, Win32Injector, Win32TargetProbe,
};

const SENTINEL: &str = "ZQ7 Nib sentinel Ünïcödé ok";
const CLIP_MARKER: &str = "NIB-CLIP-RESTORE-MARKER-42";
const REPORT: &str = "bench/injection-matrix-report.jsonl";
const DEFAULT_WAIT_MS: u64 = 1200;

struct Target {
    name: String,
    /// Expected foreground process name; defaults to `name` when the config omits it.
    exe: Option<String>,
    class: String,
    launch: Option<String>,
    launch_args: Option<String>,
    focus_wait_ms: u64,
    readback: String,
    expect_refuse: bool,
    /// Probe the REAL focused control and require UIA to detect `is_password` (rather than
    /// asserting the routing table against a synthetic profile).
    live_refuse: bool,
    manual: bool,
}

impl Default for Target {
    fn default() -> Self {
        Target {
            name: String::new(),
            exe: None,
            class: String::new(),
            launch: None,
            launch_args: None,
            focus_wait_ms: DEFAULT_WAIT_MS,
            readback: String::new(),
            expect_refuse: false,
            live_refuse: false,
            manual: false,
        }
    }
}

impl Target {
    /// The process name we expect to hold foreground when injecting.
    fn expected_exe(&self) -> String {
        let base = self.exe.as_deref().unwrap_or(&self.name);
        if base.ends_with(".exe") {
            base.to_string()
        } else {
            format!("{base}.exe")
        }
    }
}

/// Parse the simple `[[target]]` + `key = value` config (a `hotkeys.toml`-style hand parser, so we
/// carry no TOML crate). Values may be optionally quoted; blank / `#`-comment lines are ignored.
fn parse_targets(text: &str) -> Vec<Target> {
    let mut out = Vec::new();
    let mut cur: Option<Target> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[target]]" {
            if let Some(t) = cur.take() {
                out.push(t);
            }
            cur = Some(Target::default());
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        // Strip a trailing ` # comment` before parsing. Without this, `live_refuse = true  # why`
        // parsed as the string "true  # why" != "true", silently disabling the live password
        // check while still reporting PASS — a false green found by actually running the harness.
        // Only a `#` preceded by whitespace counts, so a `#` inside a value (URLs, selectors)
        // survives.
        let v = match v.find(" #") {
            Some(i) => &v[..i],
            None => v,
        };
        let (k, v) = (k.trim(), v.trim().trim_matches('"'));
        if let Some(t) = cur.as_mut() {
            match k {
                "name" => t.name = v.to_string(),
                "exe" => t.exe = Some(v.to_string()),
                "class" => t.class = v.to_string(),
                "launch" => t.launch = Some(v.to_string()),
                "launch_args" => t.launch_args = Some(v.to_string()),
                "focus_wait_ms" => t.focus_wait_ms = v.parse().unwrap_or(DEFAULT_WAIT_MS),
                "readback" => t.readback = v.to_string(),
                "expect_refuse" => t.expect_refuse = v == "true",
                "live_refuse" => t.live_refuse = v == "true",
                "manual" => t.manual = v == "true",
                _ => {}
            }
        }
    }
    if let Some(t) = cur.take() {
        out.push(t);
    }
    out
}

struct Trial {
    name: String,
    class: String,
    outcome: String,
    exact_match: Option<bool>,
    clipboard_restored: Option<bool>,
    refused_as_expected: Option<bool>,
    pass: bool,
    manual: bool,
    note: String,
}

fn main() {
    let mut path = String::from("bench/targets.toml");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--targets" {
            if let Some(p) = args.get(i + 1) {
                path = p.clone();
                i += 1;
            }
        }
        i += 1;
    }

    // Diagnostic: launch a real password field and watch what UIA reports over time. Chromium
    // enables its accessibility tree lazily, so the question is whether a repeat query starts
    // reporting IsPassword — which decides how the product must probe.
    if args.iter().any(|a| a == "--probe-password") {
        probe_password_diagnostic();
        return;
    }

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(1);
        }
    };
    let targets = parse_targets(&text);

    println!("S3 injection-matrix — sentinel {SENTINEL:?}\n");
    let trials: Vec<Trial> = targets.iter().map(run_target).collect();

    println!(
        "{:<22} {:<12} {:<10} {:<6} {:<6} {:<7} result",
        "target", "class", "outcome", "exact", "clip", "refuse"
    );
    for t in &trials {
        let result = if t.manual {
            "manual".to_string()
        } else if t.pass {
            "PASS".to_string()
        } else {
            "FAIL".to_string()
        };
        println!(
            "{:<22} {:<12} {:<10} {:<6} {:<6} {:<7} {}",
            t.name,
            t.class,
            t.outcome,
            fmt_opt(t.exact_match),
            fmt_opt(t.clipboard_restored),
            fmt_opt(t.refused_as_expected),
            result
        );
    }

    let automated: Vec<&Trial> = trials.iter().filter(|t| !t.manual).collect();
    let passed = automated.iter().filter(|t| t.pass).count();
    println!("\n{passed}/{} automated cells passed.", automated.len());

    let jsonl = trials.iter().map(trial_json).collect::<Vec<_>>().join("\n");
    if std::fs::write(REPORT, jsonl).is_ok() {
        println!("report → {REPORT}");
    }
}

/// Launch a real `<input type=password>` and poll UIA, printing what it reports each time.
/// Answers: does `IsPassword` ever become true, and if so after how long?
fn probe_password_diagnostic() {
    let t = Target {
        name: "msedge".into(),
        launch: Some("msedge".into()),
        launch_args: Some(
            "--app=data:text/html,<input type=password autofocus style='width:99%'> \
             --new-window --no-first-run"
                .into(),
        ),
        ..Default::default()
    };
    println!("launching a password field...");
    let mut child = launch(&t);
    for i in 0..12 {
        std::thread::sleep(Duration::from_millis(500));
        let fg = foreground_exe().unwrap_or_default();
        let pw = nib_win32::uia_focused_is_password();
        let text = uia_focused_text();
        println!(
            "  t={:>4}ms  fg={:<14} is_password={:<7} text={:?}",
            (i + 1) * 500,
            fg,
            match pw {
                Some(true) => "TRUE",
                Some(false) => "false",
                None => "none",
            },
            text.map(|t| t.chars().take(24).collect::<String>())
        );
    }
    if let Some(mut c) = child.take() {
        let _ = c.kill();
    }
}

fn run_target(t: &Target) -> Trial {
    if t.manual {
        return trial(
            t,
            "skipped",
            None,
            None,
            None,
            false,
            "manual — focus it, then automate",
        );
    }
    if t.expect_refuse && t.live_refuse {
        return run_live_refuse(t);
    }
    if t.expect_refuse {
        // Routing-level check: a password profile must yield Refuse (no live field needed).
        let profile = TargetProfile {
            exe: "creds.exe".into(),
            is_password: true,
            ..Default::default()
        };
        let outcome = inject_with_fallback(&Win32Injector, SENTINEL, &profile);
        let refused = outcome == InjectOutcome::Refused;
        return trial(
            t,
            &format!("{outcome:?}"),
            None,
            None,
            Some(refused),
            refused,
            if refused {
                "password refused as required"
            } else {
                "DID NOT REFUSE — injection safety bug"
            },
        );
    }
    run_live(t)
}

/// A live cell: seed the clipboard with a marker, launch + focus the app, inject, read back, then
/// verify the clipboard was restored. Kills the launched app afterward.
/// Resolve a launch command to something spawnable: as given (PATH), else a known install
/// location. Browsers in particular aren't on PATH on Windows, which showed up as a spurious
/// "could not launch target" rather than a real injection result.
fn resolve_launch(cmd: &str) -> Option<String> {
    if Command::new(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut c| {
            let _ = c.kill();
        })
        .is_ok()
    {
        return Some(cmd.to_string());
    }
    let candidates: &[&str] = match cmd {
        "msedge" => &[
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        ],
        "wordpad" | "write" => &[
            r"C:\Program Files\Windows NT\Accessories\wordpad.exe",
            r"C:\Program Files (x86)\Windows NT\Accessories\wordpad.exe",
        ],
        _ => &[],
    };
    candidates
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(|p| p.to_string())
}

/// Launch the target and return its child handle, or None if it couldn't start.
fn launch(t: &Target) -> Option<std::process::Child> {
    let exe = resolve_launch(t.launch.as_ref()?)?;
    let mut cmd = Command::new(exe);
    if let Some(args) = &t.launch_args {
        // Split on spaces but keep `--flag=value with spaces` intact by only splitting on the
        // boundaries between arguments that start with `-`.
        for arg in split_args(args) {
            cmd.arg(arg);
        }
    }
    cmd.spawn().ok()
}

/// Split a launch-args string into arguments, keeping `--flag=<value containing spaces>` whole.
fn split_args(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in s.split(' ') {
        if tok.starts_with('-') || out.is_empty() {
            out.push(tok.to_string());
        } else if let Some(last) = out.last_mut() {
            last.push(' ');
            last.push_str(tok);
        }
    }
    out
}

/// A live password cell: focus a REAL credential field and require the target probe to detect it
/// (UIA `IsPassword`) and the route chain to refuse. Nothing is ever typed.
fn run_live_refuse(t: &Target) -> Trial {
    let mut child = launch(t);
    if t.launch.is_some() && child.is_none() {
        return trial(
            t,
            "spawn-failed",
            None,
            None,
            None,
            false,
            "could not launch target",
        );
    }
    std::thread::sleep(Duration::from_millis(t.focus_wait_ms));

    let fg = foreground_exe().unwrap_or_default();
    let expected = t.expected_exe();
    let focused = fg.eq_ignore_ascii_case(&expected);

    // Warm first, exactly as the product does on PTT key-down: a lazy provider's FIRST answer is
    // stale (Chromium reports is_password=false for ~500 ms until its tree is built), and the
    // whole point of this cell is to test what the product actually sees at inject time.
    Win32TargetProbe.warm();
    std::thread::sleep(Duration::from_millis(700));
    let mut profile = Win32TargetProbe.snapshot(Duration::from_millis(500));
    nib_target::classify(&mut profile);
    let detected = profile.is_password;
    // Only inject if the probe says it is NOT a password — that is the dangerous case we must
    // prove cannot happen. If detection worked, the chain refuses without typing anything.
    let outcome = inject_with_fallback(&Win32Injector, SENTINEL, &profile);
    if let Some(mut c) = child.take() {
        let _ = c.kill();
    }

    let refused = outcome == InjectOutcome::Refused;
    let pass = focused && detected && refused;
    let note = if !focused {
        format!("target not focused (foreground = {fg})")
    } else if !detected {
        "UIA did not report IsPassword — SENTINEL MAY HAVE BEEN TYPED INTO A PASSWORD FIELD".into()
    } else {
        "IsPassword detected live; chain refused".into()
    };
    trial(
        t,
        &format!("{outcome:?}"),
        None,
        None,
        Some(refused),
        pass,
        &note,
    )
}

fn run_live(t: &Target) -> Trial {
    let _ = clipboard_set(CLIP_MARKER);
    let mut child = launch(t);
    // Never inject blind: if we asked to launch but couldn't, bail instead of typing somewhere.
    if t.launch.is_some() && child.is_none() {
        return trial(
            t,
            "spawn-failed",
            None,
            None,
            None,
            false,
            "could not launch target",
        );
    }
    std::thread::sleep(Duration::from_millis(t.focus_wait_ms));

    // Confirm the intended target actually holds foreground before typing. Focus-stealing
    // prevention (common on Win11) can leave the launched window in the background — injecting then
    // would paste the sentinel into whatever WAS focused (the harness console / the user's editor)
    // and the UIA read-back would read that control: a false green for the wrong window.
    let fg = foreground_exe().unwrap_or_default();
    let expected = t.expected_exe();
    if !fg.eq_ignore_ascii_case(&expected) {
        if let Some(mut c) = child.take() {
            let _ = c.kill();
        }
        return trial(
            t,
            "no-focus",
            None,
            None,
            None,
            false,
            &format!("target not focused (foreground = {fg})"),
        );
    }

    // Route via the REAL focused control, not a fabricated profile — this exercises the actual
    // routing decision (is_terminal / is_password) the product would make.
    let mut profile = Win32TargetProbe.snapshot(Duration::from_millis(150));
    nib_target::classify(&mut profile);

    let outcome = inject_with_fallback(&Win32Injector, SENTINEL, &profile);
    std::thread::sleep(Duration::from_millis(400));

    // `uia_text` and `uia_value` both go through the same reader now (ValuePattern then
    // TextPattern) — Win11's XAML Notepad only exposes TextPattern, so a ValuePattern-only read
    // reported a false FAIL for a perfectly good injection.
    let readback = if t.readback.starts_with("uia_") {
        uia_focused_text()
    } else {
        None
    };
    std::thread::sleep(Duration::from_millis(200));
    let clip_after = clipboard_get();
    if let Some(mut c) = child.take() {
        let _ = c.kill();
    }

    // A live cell must actually verify the text landed — no read-back is a FAIL, not a free pass.
    let exact = Some(readback.as_deref().is_some_and(|r| r.contains(SENTINEL)));
    let restored = Some(clip_after.as_deref() == Some(CLIP_MARKER));
    let pass = outcome == InjectOutcome::Inserted && exact == Some(true) && restored == Some(true);
    let note = match &readback {
        Some(r) => format!("read back {} chars", r.chars().count()),
        None => "no read-back (fail)".to_string(),
    };
    trial(
        t,
        &format!("{outcome:?}"),
        exact,
        restored,
        None,
        pass,
        &note,
    )
}

#[allow(clippy::too_many_arguments)]
fn trial(
    t: &Target,
    outcome: &str,
    exact_match: Option<bool>,
    clipboard_restored: Option<bool>,
    refused_as_expected: Option<bool>,
    pass: bool,
    note: &str,
) -> Trial {
    Trial {
        name: t.name.clone(),
        class: t.class.clone(),
        outcome: outcome.to_string(),
        exact_match,
        clipboard_restored,
        refused_as_expected,
        pass,
        manual: t.manual,
        note: note.to_string(),
    }
}

fn fmt_opt(b: Option<bool>) -> String {
    match b {
        Some(true) => "yes".into(),
        Some(false) => "NO".into(),
        None => "-".into(),
    }
}

fn json_bool(b: Option<bool>) -> String {
    match b {
        Some(v) => v.to_string(),
        None => "null".into(),
    }
}

fn trial_json(t: &Trial) -> String {
    format!(
        "{{\"name\":{:?},\"class\":{:?},\"outcome\":{:?},\"exact_match\":{},\"clipboard_restored\":{},\"refused_as_expected\":{},\"pass\":{},\"manual\":{},\"note\":{:?}}}",
        t.name,
        t.class,
        t.outcome,
        json_bool(t.exact_match),
        json_bool(t.clipboard_restored),
        json_bool(t.refused_as_expected),
        t.pass,
        t.manual,
        t.note
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets_with_defaults() {
        let s = r#"
            # a comment
            [[target]]
            name = "notepad"
            launch = "notepad"
            readback = "uia_value"

            [[target]]
            name = "pw"
            expect_refuse = true
        "#;
        let targets = parse_targets(s);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].name, "notepad");
        assert_eq!(targets[0].launch.as_deref(), Some("notepad"));
        assert_eq!(targets[0].focus_wait_ms, 1200); // default applied
        assert!(targets[1].expect_refuse);
        assert!(!targets[1].manual); // default false
    }

    /// Regression: an inline `# comment` used to become part of the value, silently turning
    /// `live_refuse = true` into false — the live password check never ran, yet reported PASS.
    #[test]
    fn inline_comments_are_stripped_from_values() {
        let targets = parse_targets(
            "[[target]]\nname = \"pw\"\nexpect_refuse = true   # must refuse\nlive_refuse = true  # live probe\n",
        );
        assert_eq!(targets.len(), 1);
        assert!(targets[0].expect_refuse, "trailing comment broke the flag");
        assert!(targets[0].live_refuse, "trailing comment broke the flag");
    }

    /// …but a `#` that's part of a value (URL fragments, CSS ids) must survive.
    #[test]
    fn hash_inside_a_value_is_preserved() {
        let targets =
            parse_targets("[[target]]\nlaunch_args = \"--app=data:text/html,<a id=#x>\"\n");
        assert!(targets[0].launch_args.as_deref().unwrap().contains("#x"));
    }

    #[test]
    fn launch_args_keep_flag_values_whole() {
        // The browser cells pass an --app=<html> value containing spaces; splitting naively would
        // hand the browser a dozen bogus arguments and it would never show the test page.
        let args = split_args("--app=data:text/html,<textarea a='1 2'></textarea> --new-window");
        assert_eq!(args.len(), 2);
        assert!(args[0].starts_with("--app=") && args[0].contains("a='1 2'"));
        assert_eq!(args[1], "--new-window");
    }

    #[test]
    fn expected_exe_defaults_to_name_and_honours_override() {
        let mut t = Target {
            name: "notepad".into(),
            ..Default::default()
        };
        assert_eq!(t.expected_exe(), "notepad.exe");
        t.exe = Some("msedge".into());
        assert_eq!(t.expected_exe(), "msedge.exe");
    }

    #[test]
    fn json_line_is_wellformed() {
        let t = Trial {
            name: "notepad".into(),
            class: "office".into(),
            outcome: "Inserted".into(),
            exact_match: Some(true),
            clipboard_restored: Some(true),
            refused_as_expected: None,
            pass: true,
            manual: false,
            note: "ok".into(),
        };
        let j = trial_json(&t);
        assert!(j.starts_with('{') && j.ends_with('}'));
        assert!(j.contains("\"refused_as_expected\":null"));
        assert!(j.contains("\"pass\":true"));
    }
}
