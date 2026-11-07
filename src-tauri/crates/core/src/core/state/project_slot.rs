//! [#576 A9] The open project, held copy-on-write.
//!
//! `AppState::project` used to hold the `Project` by value, so every reader that
//! had to release the lock before it was done - the state refresh after every
//! edit, the warnings list, the integrity check, the exports - deep-cloned the
//! whole survey first (about 20 ms at 75,000 shots, three times per edit).
//!
//! The slot now holds it behind an [`Arc`]:
//!
//! - a READER takes an O(1) [`ProjectSlot::snapshot`] and reads it with no lock
//!   held. The snapshot is immutable: it is the project as it stood when taken,
//!   whatever happens to the slot afterwards.
//! - a WRITER reaches the project through [`ProjectSlot::as_mut`], which is
//!   `Arc::make_mut`: free while no snapshot is alive, one deep copy when one is
//!   (the writer gets the copy; the snapshot keeps the original). So a write
//!   never changes what a reader is looking at, and a reader never blocks a write.
//!
//! The method names mirror `Option`'s (`as_ref`, `as_mut`, `is_some`) so
//! the code that reached the project through `Option<Project>` reads the same.

use std::sync::Arc;

use crate::models::Project;

/// The `project` slot of `AppState`: the open project, if any, copy-on-write.
#[derive(Default)]
pub(crate) struct ProjectSlot(Option<Arc<Project>>);

impl ProjectSlot {
    /// The slot holding `project` (`None`: no project open).
    pub(crate) fn new(project: Option<Project>) -> Self {
        Self(project.map(Arc::new))
    }

    /// Replace what the slot holds.
    pub(crate) fn set(&mut self, project: Option<Project>) {
        self.0 = project.map(Arc::new);
    }

    /// The open project, to read under the lock.
    pub(crate) fn as_ref(&self) -> Option<&Project> {
        self.0.as_deref()
    }

    /// The open project, to change under the lock. Copies it first if a snapshot
    /// of it is still alive (`Arc::make_mut`), so the snapshot is never changed.
    pub(crate) fn as_mut(&mut self) -> Option<&mut Project> {
        self.0.as_mut().map(Arc::make_mut)
    }

    /// An O(1) snapshot of the open project, readable after the lock is released.
    pub(crate) fn snapshot(&self) -> Option<Arc<Project>> {
        self.0.as_ref().map(Arc::clone)
    }

    pub(crate) fn is_some(&self) -> bool {
        self.0.is_some()
    }

    pub(crate) fn is_none(&self) -> bool {
        self.0.is_none()
    }

    /// The project, owned: moved out when this slot holds the only reference,
    /// copied otherwise. For tests that inspect a restored slot.
    #[cfg(test)]
    pub(crate) fn into_project(self) -> Option<Project> {
        self.0.map(Arc::unwrap_or_clone)
    }
}

/// A snapshot is read from several threads at once (the commands run on the
/// blocking pool), so `Project` must stay shareable: its memos are `OnceLock`s,
/// not `RefCell`s. A `RefCell` added to it fails here, not as a data race.
const _: fn() = || {
    fn shareable<T: Send + Sync>() {}
    shareable::<Project>();
};

impl From<Option<Project>> for ProjectSlot {
    fn from(project: Option<Project>) -> Self {
        Self::new(project)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> Project {
        Project::new(name.to_string(), None)
    }

    #[test]
    fn a_write_with_no_snapshot_alive_copies_nothing() {
        let mut slot = ProjectSlot::new(Some(named("CaveA")));
        let before = slot.snapshot().map(|p| Arc::as_ptr(&p));
        if let Some(project) = slot.as_mut() {
            project.name = "CaveB".to_string();
        }
        let after = slot.snapshot().map(|p| Arc::as_ptr(&p));
        assert_eq!(before, after, "the project was changed in place");
        assert_eq!(slot.as_ref().map(|p| p.name.as_str()), Some("CaveB"));
    }

    #[test]
    fn a_snapshot_never_sees_a_later_write() {
        let mut slot = ProjectSlot::new(Some(named("CaveA")));
        let snapshot = slot.snapshot();
        if let Some(project) = slot.as_mut() {
            project.name = "CaveB".to_string();
        }
        assert_eq!(
            snapshot.as_deref().map(|p| p.name.as_str()),
            Some("CaveA"),
            "the reader keeps the project it took"
        );
        assert_eq!(slot.as_ref().map(|p| p.name.as_str()), Some("CaveB"));
    }

    #[test]
    fn snapshots_share_one_project() {
        let slot = ProjectSlot::new(Some(named("CaveA")));
        let (a, b) = (slot.snapshot(), slot.snapshot());
        assert!(matches!((a, b), (Some(a), Some(b)) if Arc::ptr_eq(&a, &b)));
        assert!(ProjectSlot::default().snapshot().is_none());
    }
}
