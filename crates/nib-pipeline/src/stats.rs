//! What the pipeline tells the settings window: a short ring of recent dictations and a couple of
//! counters. Kept deliberately small and lock-light — `push` takes one mutex for a VecDeque of
//! twenty entries, once per utterance, off the hot path (after injection).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// How many recent dictations are kept (the Diagnostics page shows exactly these).
pub const RECENT_KEEP: usize = 20;

/// One finished utterance, as the Diagnostics page reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictationRecord {
    /// Wall-clock time the utterance finished, in Unix milliseconds.
    pub at_unix_ms: u64,
    /// Lowercased exe name of the target app (`""` if unknown).
    pub app: String,
    /// Effective cleanup mode token (`raw` / `auto` / `polish` / `email`).
    pub mode: String,
    /// Words in the inserted text (0 when nothing was inserted).
    pub words: usize,
    /// Release → text-ready latency in milliseconds (transcribe + cleanup).
    pub ms: u64,
    /// `inserted` · `refused` (password field) · `blocked` (elevated / focus moved) ·
    /// `silence` · `too-short` · `failed` · `no-speech`.
    pub outcome: &'static str,
}

/// Shared between the pipeline (writer) and the settings window (reader).
#[derive(Debug, Default)]
pub struct Stats {
    recent: Mutex<VecDeque<DictationRecord>>,
    /// Utterances that ended with text inserted, since launch.
    pub inserted: AtomicU64,
    /// Every utterance the pipeline processed, whatever the outcome.
    pub total: AtomicU64,
}

impl Stats {
    /// Record one finished utterance (newest first when read back).
    pub fn push(&self, mut rec: DictationRecord) {
        if rec.at_unix_ms == 0 {
            rec.at_unix_ms = now_unix_ms();
        }
        self.total.fetch_add(1, SeqCst);
        if rec.outcome == "inserted" {
            self.inserted.fetch_add(1, SeqCst);
        }
        let mut q = self.recent.lock().unwrap();
        q.push_front(rec);
        q.truncate(RECENT_KEEP);
    }

    /// The most recent dictations, newest first.
    pub fn recent(&self) -> Vec<DictationRecord> {
        self.recent.lock().unwrap().iter().cloned().collect()
    }
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(outcome: &'static str) -> DictationRecord {
        DictationRecord {
            at_unix_ms: 1,
            app: "notepad.exe".into(),
            mode: "auto".into(),
            words: 3,
            ms: 400,
            outcome,
        }
    }

    #[test]
    fn keeps_the_newest_twenty_and_counts_outcomes() {
        let s = Stats::default();
        for i in 0..25u64 {
            let mut r = rec(if i % 5 == 0 { "silence" } else { "inserted" });
            r.ms = i;
            s.push(r);
        }
        let recent = s.recent();
        assert_eq!(recent.len(), RECENT_KEEP);
        assert_eq!(recent[0].ms, 24, "newest first");
        assert_eq!(s.total.load(SeqCst), 25);
        assert_eq!(s.inserted.load(SeqCst), 20);
    }

    #[test]
    fn a_zero_timestamp_is_filled_in() {
        let s = Stats::default();
        let mut r = rec("inserted");
        r.at_unix_ms = 0;
        s.push(r);
        assert!(s.recent()[0].at_unix_ms > 0);
    }
}
