//! `Win32Injector` — executes the injection routes (clipboard paste, Shift+Insert, Unicode
//! keystrokes) and reports the outcome. Route *selection* is `nib_platform::default_routes`;
//! the pipeline drives the ordered chain and falls through on a non-`Inserted` outcome.

use std::sync::atomic::Ordering::SeqCst;
use std::thread::sleep;
use std::time::Duration;

use nib_platform::{default_routes, InjectOutcome, InjectRoute, TargetProfile, TextInjector};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_INSERT, VK_SHIFT};

use crate::state::{clipboard_get_text, clipboard_set_text, send_unicode, send_vk, CTRL};

/// Win32 text injector. Stateless; reads the shared logical-Ctrl flag so it never desyncs the
/// OS modifier state when pasting while the user still holds a PTT modifier.
#[derive(Debug, Default, Clone, Copy)]
pub struct Win32Injector;

impl Win32Injector {
    /// Paste `text` via the clipboard using Ctrl+V (or Shift+Insert when `alt`). Saves and
    /// restores the user's clipboard. Returns false if the clipboard couldn't be set — the
    /// caller then falls through to the next route rather than paste stale contents.
    fn clipboard_paste(&self, text: &str, alt: bool) -> bool {
        let saved = clipboard_get_text();
        if !clipboard_set_text(text) {
            return false;
        }
        sleep(Duration::from_millis(15));
        if alt {
            send_vk(VK_SHIFT.0, false);
            send_vk(VK_INSERT.0, false);
            send_vk(VK_INSERT.0, true);
            send_vk(VK_SHIFT.0, true);
        } else {
            // Don't re-press Ctrl if the user already holds it (dictating utterance after
            // utterance without releasing PTT): the injected Ctrl-up would clear the OS's
            // logical Ctrl state while the key is physically down. Ctrl already held → bare V.
            let ctrl_held = CTRL.load(SeqCst);
            if !ctrl_held {
                send_vk(VK_CONTROL.0, false);
            }
            send_vk(b'V' as u16, false);
            send_vk(b'V' as u16, true);
            if !ctrl_held {
                send_vk(VK_CONTROL.0, true);
            }
        }
        if let Some(s) = saved {
            sleep(Duration::from_millis(150)); // let the target consume the paste first
            clipboard_set_text(&s);
        }
        true
    }
}

impl TextInjector for Win32Injector {
    fn routes(&self, target: &TargetProfile) -> Vec<InjectRoute> {
        default_routes(target)
    }

    fn inject(&self, text: &str, route: InjectRoute, _target: &TargetProfile) -> InjectOutcome {
        match route {
            InjectRoute::Refuse => InjectOutcome::Refused,
            InjectRoute::UnicodeKeystroke => {
                send_unicode(text);
                InjectOutcome::Inserted
            }
            InjectRoute::ClipboardPaste => {
                if self.clipboard_paste(text, false) {
                    InjectOutcome::Inserted
                } else {
                    InjectOutcome::Blocked
                }
            }
            InjectRoute::ClipboardPasteAlt => {
                if self.clipboard_paste(text, true) {
                    InjectOutcome::Inserted
                } else {
                    InjectOutcome::Blocked
                }
            }
        }
    }
}
