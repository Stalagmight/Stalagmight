//! Explicit "not yet ported" failures for the station-graph switch (C0).
//!
//! Stream C switched `AppState` to `models::survey::Project` in one step and
//! left every command executor, query, IPC path and subsystem it did not port
//! as a stub that answers with a typed failure instead of a panic, so the app
//! compiles and the ported parts can be tested. A stub is the line
//! `crate::core::stub::not_ported("<module>::<function>")`; its original body
//! is in the history of `feat/survey-graph` (the C0 base, `ee49b1299`). The
//! work packages that port them are listed in the C0 work breakdown, and
//! `grep -rn 'not_ported(' src-tauri` lists what is left.

use std::fmt;

use crate::core::CommandResult;

/// The error a stub returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotPorted(pub &'static str);

impl fmt::Display for NotPorted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not yet ported: {}", self.0)
    }
}

impl std::error::Error for NotPorted {}

impl From<NotPorted> for String {
    fn from(value: NotPorted) -> Self {
        value.to_string()
    }
}

impl From<NotPorted> for std::io::Error {
    fn from(value: NotPorted) -> Self {
        std::io::Error::other(value.to_string())
    }
}

impl From<NotPorted> for crate::core::loading::LoadError {
    fn from(value: NotPorted) -> Self {
        crate::core::loading::LoadError::new("NOT_PORTED", value.to_string())
    }
}

impl From<NotPorted> for CommandResult {
    fn from(value: NotPorted) -> Self {
        CommandResult::failure(value.to_string())
    }
}

impl From<NotPorted> for crate::speleodb::SpeleoDbError {
    fn from(value: NotPorted) -> Self {
        crate::speleodb::SpeleoDbError::Io(value.to_string())
    }
}

impl From<NotPorted> for crate::speleodb::SpeleoErrorDto {
    fn from(value: NotPorted) -> Self {
        crate::speleodb::contract::dto_for_error(
            crate::speleodb::contract::Endpoint::GetProject,
            &crate::speleodb::SpeleoDbError::Io(value.to_string()),
        )
    }
}

/// A value a stub can answer with: a failure where the type has one, else
/// the empty value.
pub trait Stub {
    fn not_ported(what: &'static str) -> Self;
}

impl Stub for CommandResult {
    fn not_ported(what: &'static str) -> Self {
        CommandResult::failure(NotPorted(what).to_string())
    }
}

impl<T, E: From<NotPorted>> Stub for Result<T, E> {
    fn not_ported(what: &'static str) -> Self {
        Err(E::from(NotPorted(what)))
    }
}

impl<T> Stub for Option<T> {
    fn not_ported(_: &'static str) -> Self {
        None
    }
}

impl<T> Stub for Vec<T> {
    fn not_ported(_: &'static str) -> Self {
        Vec::new()
    }
}

impl<K, V, S: Default> Stub for std::collections::HashMap<K, V, S> {
    fn not_ported(_: &'static str) -> Self {
        Self::default()
    }
}

impl<K, V> Stub for std::collections::BTreeMap<K, V> {
    fn not_ported(_: &'static str) -> Self {
        Self::new()
    }
}

impl<T, S: Default> Stub for std::collections::HashSet<T, S> {
    fn not_ported(_: &'static str) -> Self {
        Self::default()
    }
}

macro_rules! stub_default {
    ($($t:ty),*) => {$(
        impl Stub for $t {
            fn not_ported(_: &'static str) -> Self {
                <$t>::default()
            }
        }
    )*};
}

stub_default!((), bool, u32, u64, usize, i64, f64, String);

/// The body of a stub: logs once per call that `what` was reached, then
/// answers with [`Stub::not_ported`].
pub fn not_ported<T: Stub>(what: &'static str) -> T {
    tracing::warn!(what, "reached a function the station-graph switch has not ported yet");
    T::not_ported(what)
}
