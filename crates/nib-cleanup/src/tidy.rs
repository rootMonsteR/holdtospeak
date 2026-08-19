//! Deterministic "Auto" tidy for the free tier — conservative, rule-based cleanup that can never
//! hallucinate: whitespace normalization, standalone-filler removal, sentence-start
//! capitalization, and terminal punctuation. Parakeet already emits casing + punctuation, so this
//! only patches what ASR commonly leaves ragged. (The Pro tier's LLM cleanup replaces this.)
//!
//! Guiding rule, learned the hard way in review: **deleting or mangling a real word is far worse
//! than leaving a filler in.** Every rule here is deliberately narrow — when in doubt, do nothing.

/// Filler tokens stripped when they appear as standalone words.
///
/// Deliberately excludes `mm`/`hmm`: "mm" is millimetres ("cut 5 mm off"), a roman numeral, and
/// part of "M&M" — stripping it silently ate real content in testing. Fillers that double as
/// meaningful words don't belong here at any confidence level.
const FILLERS: &[&str] = &["um", "uh", "erm", "uhm"];

/// Sentence-terminating punctuation, including the CJK forms (so we neither double up nor miss).
const TERMINATORS: &[char] = &['.', '!', '?', '。', '！', '？'];
/// Closing punctuation that may legitimately follow a terminator.
const CLOSERS: &[char] = &['"', '\'', ')', ']', '}', '»', '”', '’', '）', '」', '』'];

/// True when a token is a bare filler, optionally wrapped in punctuation — but never when it
/// carries other letters/digits (so "M&M", "5mm", "Umbrella" are all safe).
///
/// Requires the token to be **lowercase**: ASR writes disfluencies lowercase but capitalizes
/// proper nouns, so this is what separates a filler from "Um Al Quwain" / "Uh Huh" (a place, a
/// name). The cost is leaving an utterance-initial "Um" (which ASR capitalizes) — a cosmetic
/// miss, never data loss, which is the right side to fail on.
fn filler_body(token: &str, allow_capitalized: bool) -> Option<&str> {
    let core = token.trim_matches(|c: char| !c.is_alphanumeric());
    // Reject interior non-alphanumerics ("M&M") and digits ("5mm").
    if core.is_empty() || !core.chars().all(|c| c.is_alphabetic()) {
        return None;
    }
    let lower = core.to_lowercase();
    if core != lower && !allow_capitalized {
        return None;
    }
    FILLERS.contains(&lower.as_str()).then_some(core)
}

/// Whether an utterance-INITIAL capitalized token may still be treated as a filler. ASR
/// capitalizes the first word regardless, so casing alone can't decide there; use the shape of
/// the utterance instead: a trailing comma ("Uh, take the…") or a lowercase next word ("Um so
/// the…") means disfluency, while a capitalized next word ("Um Al Quwain") means proper noun.
fn initial_filler_allowed(token: &str, next: Option<&&str>) -> bool {
    if token.ends_with([',', '.', '!', '?', ';', ':']) {
        return true;
    }
    next.is_some_and(|n| {
        n.chars()
            .find(|c| c.is_alphabetic())
            .is_some_and(|c| c.is_lowercase())
    })
}

/// Apply the deterministic tidy: collapse spacing, strip standalone fillers, capitalize sentence
/// starts, and ensure terminal punctuation.
pub fn auto_tidy(text: &str) -> String {
    let raw_tokens: Vec<&str> = text.split_whitespace().collect();
    let mut words: Vec<String> = Vec::new();
    for (i, raw) in raw_tokens.iter().enumerate() {
        let allow_capitalized = i == 0 && initial_filler_allowed(raw, raw_tokens.get(1));
        if filler_body(raw, allow_capitalized).is_none() {
            words.push(raw.to_string());
            continue;
        }
        // A filler adjacent to a digit is probably a unit, not a filler — keep it.
        let neighbour_is_numeric =
            |t: Option<&&str>| t.is_some_and(|t| t.chars().any(|c| c.is_numeric()));
        if neighbour_is_numeric(raw_tokens.get(i.wrapping_sub(1)))
            || neighbour_is_numeric(raw_tokens.get(i + 1))
        {
            words.push(raw.to_string());
            continue;
        }
        // The filler may have carried the sentence's punctuation ("...email um."): re-attach it
        // to the previous word, in its ORIGINAL order.
        let trailing: String = raw
            .chars()
            .skip_while(|c| !TERMINATORS.contains(c))
            .filter(|c| TERMINATORS.contains(c))
            .collect();
        if let (Some(last), false) = (words.last_mut(), trailing.is_empty()) {
            if !last.ends_with(TERMINATORS) {
                last.push_str(&trailing);
            }
        }
    }
    if words.is_empty() {
        return String::new();
    }

    let mut out = words.join(" ");
    // Terminal punctuation, judged on the last CONTENT char (skipping closing quotes/brackets) so
    // `she said "hello."` doesn't become `she said "hello.".`
    let last_content = out
        .trim_end_matches(|c| CLOSERS.contains(&c))
        .chars()
        .last();
    match last_content {
        // A dangling comma (often where a filler was cut) reads as a mistake — close it.
        // NOTE: only the comma. A trailing `;` is meaningful in dictated code/lists.
        Some(',') => {
            let idx = out.rfind(',').unwrap();
            out.replace_range(idx..idx + 1, ".");
        }
        Some(c) if TERMINATORS.contains(&c) || matches!(c, ':' | ';' | '…' | '、') => {}
        Some(_) => out.push('.'),
        None => {}
    }
    capitalize_sentences(&out)
}

/// True if `token` looks like something whose internal dots must not trigger capitalization:
/// URLs, emails, filenames, versions — anything with another structural character.
fn is_structured_token(token: &str) -> bool {
    let dots = token.matches('.').count();
    dots > 1 || token.contains(['@', '/', '\\', ':']) || token.chars().any(|c| c.is_numeric())
}

/// Uppercase the first letter of the text and of each new sentence.
///
/// A sentence break requires a terminator **followed by whitespace**, and is suppressed when the
/// terminator belongs to a structured token (`www.example.com`, `foo@bar.com`, `node.js`, `3.5`)
/// or a short abbreviation (`e.g.`, `i.e.`, `p.m.`, `Dr.`, `vs.`) — capitalizing after those
/// mangles real text, which is worse than a missed capital.
fn capitalize_sentences(text: &str) -> String {
    let mut out = String::new();
    let mut first = true;
    for (i, token) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let capitalize = if first {
            true
        } else {
            // Look back at the previous token to decide if a sentence just ended.
            let prev = text.split(' ').nth(i - 1).unwrap_or("");
            ends_sentence(prev)
        };
        if capitalize && !token.is_empty() {
            out.push_str(&capitalize_first(token));
            if token.chars().any(|c| c.is_alphanumeric()) {
                first = false;
            }
        } else {
            out.push_str(token);
            if token.chars().any(|c| c.is_alphanumeric()) {
                first = false;
            }
        }
    }
    out
}

/// Whether `token` terminates a sentence (so the NEXT token starts one).
fn ends_sentence(token: &str) -> bool {
    let trimmed = token.trim_end_matches(|c| CLOSERS.contains(&c));
    if !trimmed.ends_with(TERMINATORS) {
        return false;
    }
    // `!`/`?` are unambiguous; a `.` needs the abbreviation/structure checks.
    if trimmed.ends_with(['!', '?', '！', '？', '。']) {
        return true;
    }
    if is_structured_token(trimmed) {
        return false; // www.example.com, node.js, 3.5, foo@bar.com
    }
    // A very short word before the dot is almost always an abbreviation (e.g. / Dr. / vs. / p.m.).
    let word: String = trimmed
        .trim_end_matches('.')
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    word.chars().count() > 2
}

fn capitalize_first(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    let mut done = false;
    for c in token.chars() {
        if !done && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            done = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// Discourse markers — meaningless in writing, but ONLY when they interrupt the sentence, which in
/// a transcript shows up as commas on both sides (or the start of the utterance on the left).
///
/// That delimiter requirement is the entire safety story. "like" is filler in "it's, like, fine"
/// and load-bearing in "I like this" or "things like that"; "you know" is filler in "it's, you
/// know, tricky" and a real clause in "you know the answer". Without the commas there is no
/// deterministic way to tell them apart, so we do not guess.
const DISCOURSE: &[&[&str]] = &[
    &["like"],
    &["you", "know"],
    &["i", "mean"],
    &["you", "see"],
    &["sort", "of"],
    &["kind", "of"],
    &["basically"],
    &["actually"],
    &["literally"],
    &["obviously"],
    &["honestly"],
    &["essentially"],
];

/// Vague trailers ("...and stuff"). These hang off the END of a clause rather than interrupting
/// it, so they are recognised by a following comma or the end of the utterance instead.
const VAGUE_TRAILERS: &[&[&str]] = &[
    &["and", "stuff"],
    &["and", "things"],
    &["and", "all", "that"],
    &["and", "so", "on"],
    &["or", "whatever"],
    &["or", "something"],
];

/// Sentence punctuation carried at the end of a token: `"stuff,"` -> `","`.
fn punct_suffix(tok: &str) -> String {
    let tail: Vec<char> = tok
        .chars()
        .rev()
        .take_while(|c| matches!(c, ',' | '.' | ';' | ':' | '!' | '?'))
        .collect();
    tail.into_iter().rev().collect()
}

/// A token reduced to its comparable word: surrounding punctuation stripped, lowercased.
/// Apostrophes are kept so "I'm" stays one word.
fn word_of(tok: &str) -> String {
    tok.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
        .to_ascii_lowercase()
}

/// Does `phrase` begin at `i`? Interior tokens must carry no punctuation — "you know" is a
/// phrase, "you, know" is not.
fn phrase_at(toks: &[&str], i: usize, phrase: &[&str]) -> bool {
    if i + phrase.len() > toks.len() {
        return false;
    }
    phrase.iter().enumerate().all(|(k, w)| {
        word_of(toks[i + k]) == *w
            && (k + 1 == phrase.len() || punct_suffix(toks[i + k]).is_empty())
    })
}

/// Length of the filler phrase starting at `i`, if there is one.
fn filler_len_at(toks: &[&str], i: usize, after_comma: bool) -> Option<usize> {
    for p in DISCOURSE {
        if after_comma && phrase_at(toks, i, p) && punct_suffix(toks[i + p.len() - 1]).contains(',')
        {
            return Some(p.len());
        }
    }
    for p in VAGUE_TRAILERS {
        if phrase_at(toks, i, p)
            && (i + p.len() == toks.len() || !punct_suffix(toks[i + p.len() - 1]).is_empty())
        {
            return Some(p.len());
        }
    }
    None
}

/// Remove conversational scaffolding that adds nothing to the written sentence.
///
/// Rule-based and delete-only: it can drop whole filler phrases but can never invent, reorder or
/// reword anything. That is the honest ceiling of the deterministic approach — it makes a spoken
/// sentence read cleanly; it cannot restructure one.
pub fn strip_discourse(text: &str) -> String {
    let toks: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        // Start-of-utterance counts as "after a comma": a leading "Basically," is just as much
        // scaffolding as a mid-sentence one.
        let after_comma = out
            .last()
            .map(|t| punct_suffix(t).contains(','))
            .unwrap_or(true);
        match filler_len_at(&toks, i, after_comma) {
            Some(len) => {
                // The phrase may have been carrying the clause's punctuation ("the software and
                // stuff, just to..."). Hand it back to the preceding word so the sentence keeps
                // its shape instead of losing a comma.
                let trailing = punct_suffix(toks[i + len - 1]);
                if !trailing.is_empty() {
                    if let Some(last) = out.last_mut() {
                        if punct_suffix(last).is_empty() {
                            last.push_str(&trailing);
                        }
                    }
                }
                i += len;
            }
            None => {
                out.push(toks[i].to_string());
                i += 1;
            }
        }
    }
    out.join(" ")
}

/// Free-tier **Polish**: [`strip_discourse`] then [`auto_tidy`].
///
/// Auto stays deliberately light (fillers, casing, terminal punctuation) because it is the safe
/// everyday default. Polish is where the user has explicitly opted into heavier editing.
pub fn polish_tidy(text: &str) -> String {
    auto_tidy(&strip_discourse(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polish_cleans_the_sentence_from_the_bug_report() {
        // Verbatim from a real dictation. Auto left every discourse marker in place, which is
        // what made Polish look broken.
        assert_eq!(
            polish_tidy(
                "I'm just testing this, like, you know, testing the software and stuff, \
                 just to make sure it can come up with a proper coherent sentence."
            ),
            "I'm just testing this, testing the software, just to make sure it can come up \
             with a proper coherent sentence."
        );
    }

    #[test]
    fn polish_keeps_words_that_merely_look_like_fillers() {
        // The whole reason the rule demands commas on both sides. Get this wrong and Polish
        // silently eats real words, which is far worse than leaving a filler in.
        assert_eq!(polish_tidy("I like this one"), "I like this one.");
        assert_eq!(polish_tidy("do things like that"), "Do things like that.");
        assert_eq!(polish_tidy("you know the answer"), "You know the answer.");
        assert_eq!(polish_tidy("it actually works"), "It actually works.");
        assert_eq!(polish_tidy("sort of thing"), "Sort of thing.");
    }

    #[test]
    fn polish_removes_leading_scaffolding_and_vague_trailers() {
        assert_eq!(
            polish_tidy("Basically, we ship on Friday"),
            "We ship on Friday."
        );
        assert_eq!(
            polish_tidy("we tested the parser and stuff"),
            "We tested the parser."
        );
        // The trailer hands its comma back, so the clause keeps its shape.
        assert_eq!(
            polish_tidy("we tested the parser and stuff, then shipped"),
            "We tested the parser, then shipped."
        );
    }

    #[test]
    fn auto_stays_light_so_only_polish_is_aggressive() {
        assert_eq!(auto_tidy("it's, like, fine"), "It's, like, fine.");
        // strip_discourse only DELETES; casing and terminal punctuation stay auto_tidy's job.
        assert_eq!(strip_discourse("it's, like, fine"), "it's, fine");
    }

    #[test]
    fn strips_standalone_fillers_only() {
        assert_eq!(
            auto_tidy("um so the umbrella is uh red"),
            "So the umbrella is red."
        );
        assert_eq!(auto_tidy("Uh, take the umbrella"), "Take the umbrella.");
    }

    /// Regression: these all lost real words before review.
    #[test]
    fn never_eats_real_words() {
        assert_eq!(auto_tidy("cut 5 mm off the end"), "Cut 5 mm off the end.");
        assert_eq!(auto_tidy("the gap is 3 mm wide"), "The gap is 3 mm wide.");
        assert_eq!(
            auto_tidy("buy some M&M for the party"),
            "Buy some M&M for the party."
        );
        // Capitalized => a proper noun (a place), not a disfluency.
        assert_eq!(
            auto_tidy("she is from Um Al Quwain"),
            "She is from Um Al Quwain."
        );
        assert_eq!(
            auto_tidy("MM is two thousand in roman numerals"),
            "MM is two thousand in roman numerals."
        );
    }

    /// Regression: capitalization used to mangle these.
    #[test]
    fn does_not_mangle_structured_tokens() {
        assert_eq!(
            auto_tidy("visit www.example.com today"),
            "Visit www.example.com today."
        );
        assert_eq!(
            auto_tidy("mail foo.bar@example.com now"),
            "Mail foo.bar@example.com now."
        );
        assert_eq!(auto_tidy("we use node.js here"), "We use node.js here.");
        assert_eq!(auto_tidy("e.g. the budget"), "E.g. the budget.");
        assert_eq!(auto_tidy("5 p.m. tomorrow"), "5 p.m. tomorrow.");
        assert_eq!(
            auto_tidy("3.5 inches of rain fell"),
            "3.5 inches of rain fell."
        );
        assert_eq!(auto_tidy("vs. the other one"), "Vs. the other one.");
    }

    #[test]
    fn keeps_sentence_punctuation_when_filler_carried_it() {
        assert_eq!(auto_tidy("send the email um."), "Send the email.");
        // ...and in the original order, not reversed.
        assert_eq!(auto_tidy("send it um?!"), "Send it?!");
    }

    #[test]
    fn adds_terminal_punctuation_and_capitalizes_sentences() {
        assert_eq!(auto_tidy("hello world"), "Hello world.");
        assert_eq!(auto_tidy("done. next item"), "Done. Next item.");
        assert_eq!(auto_tidy("really?"), "Really?");
    }

    #[test]
    fn no_double_punctuation_after_closers_or_cjk() {
        assert_eq!(
            auto_tidy(r#"she said "hello there.""#),
            r#"She said "hello there.""#
        );
        assert_eq!(auto_tidy("你好世界。"), "你好世界。");
    }

    #[test]
    fn closes_orphaned_comma_but_leaves_semicolon() {
        assert_eq!(auto_tidy("that works,"), "That works.");
        // Dictated code/lists: a trailing `;` is meaningful.
        assert_eq!(auto_tidy("let x = 5;"), "Let x = 5;");
    }

    #[test]
    fn whitespace_is_collapsed() {
        assert_eq!(auto_tidy("  spaced   out\ttext  "), "Spaced out text.");
    }

    #[test]
    fn empty_and_filler_only_yield_empty() {
        assert_eq!(auto_tidy("   "), "");
        assert_eq!(auto_tidy("um uh"), "");
    }
}
