//! The personal learning dictionary — deterministic jargon fixes applied in ALL modes (incl.
//! Raw). Ported faithfully from the Python sidecar's `apply_dict`/`load_dictionary`/
//! `learn_mapping` so the native and Python sidecars behave identically.
//!
//! File format (one entry per line; `#` comments):
//!   `term`                        — a hint-only term (biases the Pro LLM cleanup; no replacement)
//!   `term => misheard1, misheard2` — each misheard token is replaced by `term`
//!
//! Replacement is case-insensitive and requires a standalone token: a match glued to a word
//! character on either side is skipped, so "cube ctl" won't fire inside "cubectler" and ".net"
//! won't fire inside "asp.net". This is exactly what the Python `word_pattern`'s `\b` /
//! `(?<!\w)` lookarounds amount to — both forms assert "no adjacent word character".

use std::path::{Path, PathBuf};

/// A loaded dictionary: deterministic replacements + hint terms (hints feed the Pro LLM only).
#[derive(Debug, Default)]
pub struct Dictionary {
    /// `(misheard_lowercased_chars, term)` pairs, applied in file order. The misheard side is
    /// lowercased once at load so `apply` never re-does it per utterance.
    replacements: Vec<(Vec<char>, String)>,
    hints: Vec<String>,
    /// Where `learn` persists; None = session-only.
    path: Option<PathBuf>,
}

impl Dictionary {
    /// Load from `path`. A missing/unreadable file yields an empty dictionary that still knows
    /// where to persist new entries.
    pub fn load(path: &Path) -> Dictionary {
        let mut d = Dictionary {
            path: Some(path.to_path_buf()),
            ..Default::default()
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return d;
        };
        for ln in text.lines() {
            let ln = ln.trim();
            if ln.is_empty() || ln.starts_with('#') {
                continue;
            }
            if let Some((term, rest)) = ln.split_once("=>") {
                let term = term.trim();
                d.hints.push(term.to_string());
                for mis in rest.split(',') {
                    let mis = mis.trim();
                    if !mis.is_empty() {
                        d.replacements.push((lower_key(mis), term.to_string()));
                    }
                }
            } else {
                d.hints.push(ln.to_string());
            }
        }
        d
    }

    /// Apply every deterministic replacement to `text`, in order (each entry sees the previous
    /// entry's output — same chaining as the Python `re.sub` loop).
    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (mis_l, term) in &self.replacements {
            if mis_l.is_empty() {
                continue;
            }
            // Decompose once per entry (not three times), then scan.
            let chars: Vec<char> = out.chars().collect();
            let lower = lower_chars(&chars);
            out = replace_word(&chars, &lower, mis_l, term);
        }
        out
    }

    /// Comma-joined hint terms (biases the Pro LLM cleanup; empty when none).
    pub fn hint(&self) -> String {
        self.hints.join(", ")
    }

    /// Teach a fix from `"<misheard> => <correct>"`: adds a live replacement and appends
    /// `correct => misheard` to the dictionary file (note the reversal — the FILE maps
    /// term => mishearings). Returns the user-facing ack, mirroring the Python messages.
    pub fn learn(&mut self, mapping: &str) -> String {
        let Some((mis, correct)) = mapping.split_once("=>") else {
            return "learn: expected  learn <what it wrote> => <what you meant>".into();
        };
        let (mis, correct) = (mis.trim(), correct.trim());
        if mis.is_empty() || correct.is_empty() {
            return "learn: empty misheard or correct".into();
        }
        self.replacements
            .push((lower_key(mis), correct.to_string()));
        if let Some(p) = &self.path {
            if let Err(e) = append_line(p, &format!("\n{correct} => {mis}")) {
                return format!("learned (session only; save failed: {e}): '{mis}' -> '{correct}'");
            }
        }
        format!("learned: '{mis}' -> '{correct}'")
    }
}

fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The lookup key for a misheard phrase: lowercased per character (see [`lower_chars`]).
fn lower_key(s: &str) -> Vec<char> {
    lower_chars(&s.chars().collect::<Vec<_>>())
}

/// Case-insensitive, standalone-token replacement of every occurrence of `mis` with `term`.
///
/// Boundary rule: the match must not be glued to a word character on either side. Python's
/// `word_pattern` picks `\b` or a `(?<!\w)`/`(?!\w)` lookaround depending on the term's edge, but
/// **both forms reduce to exactly this** — `\b` after a word char and `(?<!\w)` are the same
/// assertion when the neighbour is a word char. (An earlier version made the check conditional on
/// the term's edge being a word char, which left non-word edges unconstrained and replaced *inside*
/// words: `asp.net` → `aspDOTNET`.)
///
/// `chars`/`lower` are the caller's one-time decomposition of the text (see [`Dictionary::apply`]),
/// and `mis_l` is pre-lowercased at load — so this is a single pass with no per-entry allocation.
fn replace_word(chars: &[char], lower: &[char], mis_l: &[char], term: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let end = i + mis_l.len();
        let matches = end <= chars.len()
            && lower[i..end] == *mis_l
            && (i == 0 || !is_word(chars[i - 1]))
            && (end == chars.len() || !is_word(chars[end]));
        if matches {
            out.push_str(term);
            i = end;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Lowercase per character so the result always has the same char count as the input — a plain
/// `to_lowercase()` can change length (Turkish `İ` → `i̇`), which would break index alignment.
/// Per-char keeps the mapping 1:1; the rare multi-char expansion just keeps its first char, which
/// only affects matching of that exotic char, never the surrounding text.
fn lower_chars(chars: &[char]) -> Vec<char> {
    chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict_from(entries: &[(&str, &str)]) -> Dictionary {
        Dictionary {
            replacements: entries
                .iter()
                .map(|(m, t)| (lower_key(m), t.to_string()))
                .collect(),
            hints: vec![],
            path: None,
        }
    }

    /// Regression: an earlier boundary check was vacuous for terms with non-word edges, so they
    /// matched *inside* words — diverging from the Python lookarounds.
    #[test]
    fn never_replaces_inside_a_word() {
        let d = dict_from(&[(".net", "DOTNET"), ("c++", "CPP"), ("#tag", "TAG")]);
        assert_eq!(d.apply("asp.net rocks"), "asp.net rocks");
        assert_eq!(d.apply("c++variable"), "c++variable");
        assert_eq!(d.apply("c++4"), "c++4");
        assert_eq!(d.apply("say a#tag now"), "say a#tag now");
        // …but standalone tokens still match.
        assert_eq!(d.apply("i use .net daily"), "i use DOTNET daily");
        assert_eq!(d.apply("write c++ code"), "write CPP code");
    }

    /// Regression: `to_lowercase()` on the whole string can change its length (Turkish `İ`),
    /// which used to disable the entire dictionary for that utterance.
    #[test]
    fn length_changing_lowercase_does_not_disable_replacements() {
        let d = dict_from(&[("cube ctl", "kubectl")]);
        assert_eq!(d.apply("İstanbul cube ctl"), "İstanbul kubectl");
    }

    #[test]
    fn replaces_case_insensitively_at_word_boundaries() {
        let d = dict_from(&[("cube ctl", "kubectl"), ("cube system", "kube-system")]);
        assert_eq!(
            d.apply("run Cube CTL get pods in cube system"),
            "run kubectl get pods in kube-system"
        );
        // No match inside a larger word.
        assert_eq!(d.apply("cubectler cube ctlx"), "cubectler cube ctlx");
    }

    #[test]
    fn nonword_edges_use_standalone_token_rule() {
        let d = dict_from(&[("see plus plus", "C++")]);
        assert_eq!(d.apply("i like see plus plus a lot"), "i like C++ a lot");
        // And a term with non-word edges can itself be a misheard source.
        let d2 = dict_from(&[("c + +", "C++")]);
        assert_eq!(d2.apply("code in c + + today"), "code in C++ today");
    }

    #[test]
    fn load_parses_both_line_forms() {
        let dir = std::env::temp_dir().join(format!("nib_dict_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("dictionary.txt");
        std::fs::write(&p, "# comment\nkubectl => cube ctl, cubctl\nsherpa\n").unwrap();
        let d = Dictionary::load(&p);
        assert_eq!(d.apply("use cube ctl or cubctl"), "use kubectl or kubectl");
        assert_eq!(d.hint(), "kubectl, sherpa");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn learn_persists_reversed_and_acks() {
        let dir = std::env::temp_dir().join(format!("nib_learn_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("dictionary.txt");
        std::fs::write(&p, "").unwrap();
        let mut d = Dictionary::load(&p);
        let ack = d.learn("cube cuddle => kubectl");
        assert_eq!(ack, "learned: 'cube cuddle' -> 'kubectl'");
        assert_eq!(d.apply("cube cuddle apply"), "kubectl apply");
        // The FILE stores term => misheard (reversed from the learn input).
        let saved = std::fs::read_to_string(&p).unwrap();
        assert!(saved.contains("kubectl => cube cuddle"));
        // And a reload round-trips it.
        let d2 = Dictionary::load(&p);
        assert_eq!(d2.apply("cube cuddle apply"), "kubectl apply");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn learn_rejects_malformed() {
        let mut d = dict_from(&[]);
        assert!(d.learn("no arrow here").starts_with("learn: expected"));
        assert!(d.learn(" => kubectl").starts_with("learn: empty"));
    }
}
