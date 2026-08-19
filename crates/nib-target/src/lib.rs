//! Pure classification of the focused target: which apps must force verbatim (Literal/Raw)
//! dictation because a generative rewrite would corrupt a command or identifier. The exe name
//! comes from the platform `TargetProbe`; this crate decides what it means.
#![forbid(unsafe_code)]

use nib_platform::TargetProfile;

/// Terminals and code editors where dictation must stay verbatim (no LLM rewrite). Lowercased
/// exe basenames, matched against `TargetProfile::exe`.
pub const LITERAL_EXES: &[&str] = &[
    "windowsterminal.exe",
    "wt.exe",
    "openconsole.exe",
    "conhost.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "alacritty.exe",
    "ghostty.exe",
    "wezterm-gui.exe",
    "mintty.exe",
    "putty.exe",
    "code.exe",
    "cursor.exe",
    "devenv.exe",
    "idea64.exe",
    "pycharm64.exe",
    "goland64.exe",
    "webstorm64.exe",
    "clion64.exe",
    "rider64.exe",
    "zed.exe",
    "sublime_text.exe",
];

/// True if `exe` (lowercased basename) is a terminal/IDE that should use Literal mode.
pub fn is_literal_exe(exe: &str) -> bool {
    LITERAL_EXES.contains(&exe)
}

/// Refine a probed profile in place: set `is_terminal` from the exe name so the injector's
/// route chain and the cleanup mode both treat terminals/IDEs as verbatim targets.
pub fn classify(profile: &mut TargetProfile) {
    profile.is_terminal = is_literal_exe(&profile.exe);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminals_and_ides_are_literal() {
        assert!(is_literal_exe("cmd.exe"));
        assert!(is_literal_exe("code.exe"));
        assert!(is_literal_exe("windowsterminal.exe"));
        assert!(!is_literal_exe("notepad.exe"));
        assert!(!is_literal_exe("chrome.exe"));
    }

    #[test]
    fn classify_sets_is_terminal() {
        let mut p = TargetProfile {
            exe: "pwsh.exe".into(),
            ..Default::default()
        };
        classify(&mut p);
        assert!(p.is_terminal);

        let mut q = TargetProfile {
            exe: "slack.exe".into(),
            ..Default::default()
        };
        classify(&mut q);
        assert!(!q.is_terminal);
    }
}
