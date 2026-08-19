//! Pure injection orchestration: drive a `TextInjector`'s ordered route chain, falling through
//! to the next route on a recoverable failure and stopping on success or a hard refusal. The
//! actual OS injection lives in `nib-win32`; this crate is the testable policy loop.
#![forbid(unsafe_code)]

use nib_platform::{InjectOutcome, TargetProfile, TextInjector};

/// Try each route from `injector.routes(target)` in order until one inserts the text.
///
/// - `Inserted` → done (success).
/// - `Refused` → stop immediately (e.g. a password field — never fall through).
/// - `Blocked` / `FocusChanged` → try the next route.
///
/// Returns the final outcome (`Blocked` if the chain was exhausted without inserting).
pub fn inject_with_fallback(
    injector: &dyn TextInjector,
    text: &str,
    target: &TargetProfile,
) -> InjectOutcome {
    let mut last = InjectOutcome::Blocked;
    for route in injector.routes(target) {
        last = injector.inject(text, route, target);
        match last {
            InjectOutcome::Inserted | InjectOutcome::Refused => return last,
            InjectOutcome::Blocked | InjectOutcome::FocusChanged => continue,
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;
    use nib_platform::{default_routes, InjectRoute};
    use std::cell::RefCell;

    /// A mock injector that returns a scripted outcome per attempt and records the routes tried.
    struct MockInjector {
        outcomes: RefCell<Vec<InjectOutcome>>,
        tried: RefCell<Vec<InjectRoute>>,
    }

    impl TextInjector for MockInjector {
        fn routes(&self, target: &TargetProfile) -> Vec<InjectRoute> {
            default_routes(target)
        }
        fn inject(&self, _t: &str, route: InjectRoute, _p: &TargetProfile) -> InjectOutcome {
            self.tried.borrow_mut().push(route);
            self.outcomes.borrow_mut().remove(0)
        }
    }

    #[test]
    fn falls_through_blocked_then_inserts() {
        let m = MockInjector {
            outcomes: RefCell::new(vec![InjectOutcome::Blocked, InjectOutcome::Inserted]),
            tried: RefCell::new(vec![]),
        };
        let target = TargetProfile::default(); // routes: [ClipboardPaste, UnicodeKeystroke]
        assert_eq!(
            inject_with_fallback(&m, "hi", &target),
            InjectOutcome::Inserted
        );
        assert_eq!(m.tried.borrow().len(), 2);
    }

    #[test]
    fn refused_stops_immediately() {
        let m = MockInjector {
            outcomes: RefCell::new(vec![InjectOutcome::Refused]),
            tried: RefCell::new(vec![]),
        };
        let pw = TargetProfile {
            is_password: true,
            ..Default::default()
        };
        assert_eq!(inject_with_fallback(&m, "hi", &pw), InjectOutcome::Refused);
        assert_eq!(m.tried.borrow().len(), 1); // only the Refuse route
    }
}
