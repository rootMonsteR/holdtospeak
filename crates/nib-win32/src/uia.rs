//! UI Automation queries against the focused control:
//!
//! - **Read-back** for the S3 injection matrix — proves an injected sentinel actually landed,
//!   rather than trusting the injector's own "Inserted". Tries `ValuePattern` (Win32 edit
//!   controls, WPF, Chromium value fields) then `TextPattern` (documents and the modern XAML
//!   controls — Win11 Notepad only exposes this one, so ValuePattern alone false-reds it).
//! - **`IsPassword`** — the signal that makes password refusal real instead of a routing-table
//!   assumption. Dictation must never be typed into a credential field.
//!
//! Every call is time-boxed on a worker thread: UIA is cross-process COM and a hung provider would
//! otherwise stall the dictation pipeline (the design budgets 150 ms for exactly this reason).

use std::sync::mpsc;
use std::time::Duration;

use windows::core::Interface;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationValuePattern, UIA_IsPasswordPropertyId, UIA_TextPatternId, UIA_ValuePatternId,
};

/// Run `f` against the focused UIA element on a worker thread, giving up after `budget`.
///
/// A wedged provider leaks the worker (it's blocked in COM and can't be killed safely) but never
/// blocks the caller — the same "abandoned, not killed" rule the design specifies for the UIA
/// thread. Returns None on timeout or any COM failure.
fn with_focused<T, F>(budget: Duration, f: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce(&IUIAutomation, &IUIAutomationElement) -> Option<T> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| unsafe {
            // MTA (not STA): UI Automation clients should be multithreaded; STA without a message
            // pump risks intermittent hangs. RPC_E_CHANGED_MODE just means COM was already
            // initialized on this thread, which is fine.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            let element = automation.GetFocusedElement().ok()?;
            f(&automation, &element)
        })();
        let _ = tx.send(result);
    });
    rx.recv_timeout(budget).ok().flatten()
}

/// The focused control's text, via `ValuePattern` then `TextPattern`. None if neither is supported
/// (or UIA didn't answer in time).
pub fn uia_focused_text() -> Option<String> {
    with_focused(Duration::from_millis(1500), |_, element| unsafe {
        // ValuePattern first: cheaper, and exact for edit controls.
        if let Ok(p) = element.GetCurrentPattern(UIA_ValuePatternId) {
            if let Ok(vp) = p.cast::<IUIAutomationValuePattern>() {
                if let Ok(v) = vp.CurrentValue() {
                    let s = v.to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }
        // TextPattern: documents and modern XAML edits (Win11 Notepad exposes only this).
        let p = element.GetCurrentPattern(UIA_TextPatternId).ok()?;
        let tp = p.cast::<IUIAutomationTextPattern>().ok()?;
        let range = tp.DocumentRange().ok()?;
        // -1 = no truncation.
        let text = range.GetText(-1).ok()?;
        Some(text.to_string())
    })
}

/// Backwards-compatible alias for the ValuePattern-only read-back.
pub fn uia_focused_value() -> Option<String> {
    uia_focused_text()
}

/// Whether the focused control is a password field (UIA `IsPassword`).
///
/// `None` means "couldn't tell" (no UIA answer in the budget) — callers must treat that as
/// *unknown*, not as "not a password"; the safe default for an unknown target is the normal route
/// chain, but a `Some(true)` must always refuse. Budget is tight because this sits on the
/// dictation hot path.
pub fn uia_focused_is_password() -> Option<bool> {
    with_focused(Duration::from_millis(150), |_, element| unsafe {
        let v = element
            .GetCurrentPropertyValue(UIA_IsPasswordPropertyId)
            .ok()?;
        bool::try_from(&v).ok()
    })
}
