// [#576 A8 RV8a-5] The lock-ops span's proof and its one producer: the module
// body of `speleodb_session::lock_ops_span` (its doc, and the RV10a-3 compile-fail
// doctest that `include!`s this file, are on the declaration there). No inner
// attributes or `//!` docs here: the doctest includes this file as items.

use std::marker::PhantomData;

use super::AppState;
use crate::speleodb::SpeleoDbError;

/// [#576 A8] Proof that the holder owns `speleodb_lock_ops`, the span every SpeleoDB
/// lock operation serializes on (the acquire -> commit of Start Editing, the captured
/// and detached releases, logout, the adoption's acquire and verdict). A step that
/// must run INSIDE that span takes one, so it cannot be called before the span opens
/// or after it closes: the lock order is the type's, not a source scan's. Made only
/// by [`AppState::speleodb_lock_ops_guard`].
pub(in crate::core::state) struct LockOpsHeld<'a> {
    /// [#576 W88] This thread's mark on the span, removed before the guard unlocks
    /// (fields drop in order).
    _held_here: HeldHere,
    _guard: std::sync::MutexGuard<'a, ()>,
    /// The state whose span this is: the reads below are ITS reads.
    state: &'a AppState,
}

std::thread_local! {
    /// [#576 W88] The states (by address) whose lock-ops span THIS thread holds. A step
    /// reached both inside a span and outside one - the 401 auto-drop,
    /// `AppState::speleodb_forget`, met by a call that may run in either - asks it
    /// whether opening the span would wait on the span its own thread holds (the mutex
    /// is not reentrant). The guard is not `Send`, so a span opens and ends on one
    /// thread.
    static SPANS_HELD_HERE: std::cell::RefCell<Vec<usize>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// [#576 W88] A span's entry in [`SPANS_HELD_HERE`], for as long as the span lives.
struct HeldHere(usize);

impl HeldHere {
    fn mark(state: &AppState) -> Self {
        let at = std::ptr::from_ref(state).addr();
        SPANS_HELD_HERE.with(|held| held.borrow_mut().push(at));
        Self(at)
    }
}

impl Drop for HeldHere {
    fn drop(&mut self) {
        SPANS_HELD_HERE.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(at) = held.iter().rposition(|state| *state == self.0) {
                held.swap_remove(at);
            }
        });
    }
}

/// [#576 A8] A value read INSIDE the lock-ops span. Made only by the span's own
/// reads ([`LockOpsHeld::read_account`], [`LockOpsHeld::read_holder`]), so a step
/// that takes one cannot be handed a value read before the span opened - the
/// account a lock operation acts for, or the holder it decides on, is then the
/// span's fact rather than an earlier one.
///
/// [#576 RV9a-3] The reads are the span's methods, not a closure it runs: a
/// generic `read(|| v)` wrapped whatever `v` was, so an account read BEFORE the
/// span could be laundered into one.
///
/// [#576 RV10a-3] And it lives no longer than the span that read it: `'s` is the
/// borrow of that span's [`LockOpsHeld`], so ending the span - dropping the
/// guard, or handing it on by value - while one is still in use does not compile
/// (E0505). A value read in one span therefore cannot reach a step running in a
/// LATER span, read before THAT span opened (a logout and login can land between
/// two spans): a later span needs the same mutex, so it cannot open while the
/// earlier one - and with it every value it read - is alive.
pub(in crate::core::state) struct InSpan<'s, T> {
    value: T,
    span: PhantomData<&'s ()>,
}

impl LockOpsHeld<'_> {
    /// The signed-in account (endpoint, token), read while this span is held.
    pub(in crate::core::state) fn read_account(
        &self,
    ) -> InSpan<'_, Result<(String, String), SpeleoDbError>> {
        InSpan::read(self.state.speleodb_active_token())
    }

    /// Who holds `project_id`'s write mutex, asked with the account this span
    /// read, while this span is held.
    pub(in crate::core::state) fn read_holder(
        &self,
        account: &InSpan<'_, (String, String)>,
        insecure_http: bool,
        project_id: &str,
    ) -> InSpan<'_, Result<Option<String>, SpeleoDbError>> {
        let (endpoint, token) = account.get();
        InSpan::read(self.state.speleodb_lock_holder_checked(
            endpoint,
            token,
            insecure_http,
            project_id,
        ))
    }
}

impl<T> InSpan<'_, T> {
    /// Private: only the span's reads above (and `transpose`) wrap a value.
    fn read(value: T) -> Self {
        Self {
            value,
            span: PhantomData,
        }
    }

    pub(in crate::core::state) fn get(&self) -> &T {
        &self.value
    }

    pub(in crate::core::state) fn into_inner(self) -> T {
        self.value
    }
}

impl<'s, T, E> InSpan<'s, Result<T, E>> {
    /// A fallible read: the error, or the value still marked as read in the span.
    pub(in crate::core::state) fn transpose(self) -> Result<InSpan<'s, T>, E> {
        self.value.map(InSpan::read)
    }
}

impl AppState {
    /// The guard Start Editing holds across acquire -> commit (see the module
    /// doc). A poisoned mutex is recovered: the guarded value is `()`, so there
    /// is no state to protect - only the serialization matters.
    pub(in crate::core::state) fn speleodb_lock_ops_guard(&self) -> LockOpsHeld<'_> {
        let guard = self.speleodb_lock_ops.lock();
        LockOpsHeld {
            _held_here: HeldHere::mark(self),
            _guard: guard,
            state: self,
        }
    }

    /// [#576 W88] Whether THIS thread holds this state's lock-ops span, so a step
    /// that may run inside it runs there instead of waiting on itself.
    pub(in crate::core::state) fn speleodb_lock_ops_held_here(&self) -> bool {
        let at = std::ptr::from_ref(self).addr();
        SPANS_HELD_HERE.with(|held| held.borrow().contains(&at))
    }
}
