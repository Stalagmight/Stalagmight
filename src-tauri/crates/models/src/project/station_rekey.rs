//! Moving stored station references with their stations, for the structural
//! edits that move stations to another section or index (Split Section Here,
//! Combine Sections).
//!
//! A [`StationKey`] is `(section_id, anchor)`, so an anchor cannot follow its
//! shot into another section, and a `SectionStart` anchor cannot follow its
//! station when shots are prepended. The edit therefore resolves every stored
//! key to its positional station BEFORE it changes the structure, works out
//! each moved station's new home, and rebuilds the key from that home against
//! the new structure.
//!
//! Both passes are keyed by the stored key, never by traversal position: the
//! read pass walks [`Project::for_each_station_ref`] and the write pass walks
//! [`Project::map_station_refs`], which rebuild the keyed collections from
//! independent `HashMap` iterations (the same reasoning as `reverse.rs`).

use std::collections::{HashMap, HashSet};

use crate::{SectionId, StationKey, StationRef};

use super::reverse::frozen_remaps;
use super::Project;

impl Project {
    /// Every stored key that names a station of one of `sections`, resolved
    /// against the CURRENT structure. Keys that no longer resolve (dangling)
    /// are left out: there is no station to move them with.
    pub(super) fn resolve_stored_keys_in(
        &self,
        sections: &[SectionId],
    ) -> HashMap<StationKey, StationRef> {
        let mut out: HashMap<StationKey, StationRef> = HashMap::new();
        self.for_each_station_ref(|_label, key| {
            if !sections.contains(&key.section_id) || out.contains_key(&key) {
                return;
            }
            if let Some(station) = self.resolve_key(&key) {
                out.insert(key, station);
            }
        });
        out
    }

    /// Re-key every stored key in `homes` (old key -> its station's new
    /// positional home) against the CURRENT, already-edited structure. Keys
    /// not in `homes` are untouched.
    ///
    /// The keyed collections (station offsets, comments, names) are rebuilt by
    /// collecting into a `HashMap`, so two of their keys that come out equal
    /// would silently drop one entry. The caller resolves every collision it
    /// expects (the join station of a combine) BEFORE calling this; any left is
    /// unexpected (a legacy `Index` anchor beside its canonical twin) and is
    /// handled the way `reverse_section` handles it: every member of the clash
    /// keeps its old key, with a warning, rather than one vanishing. The
    /// clash check runs per keyed collection: an endpoint of a merge and a
    /// comment naming the same station do not collide.
    ///
    /// Returns the keys left unmoved: frozen on a clash, or not re-keyable
    /// (an anchoring shot without a uuid).
    pub(super) fn rekey_station_refs(
        &mut self,
        homes: &HashMap<StationKey, StationRef>,
        what: &str,
    ) -> HashSet<StationKey> {
        let mut unmoved: HashSet<StationKey> = HashSet::new();
        let mut rebuilt: HashMap<StationKey, StationKey> = HashMap::new();
        for (old, home) in homes {
            let new = self.key_for_ref(*home);
            // Every shot carries a uuid once loaded, so a moved station is
            // always re-keyable; trip loudly in dev if that stops holding.
            debug_assert!(
                new.is_some(),
                "{what}: station {home:?} could not be re-keyed; its reference stays \
                 on the old key"
            );
            match new {
                Some(new) if new == *old => {}
                Some(new) => {
                    rebuilt.insert(old.clone(), new);
                }
                None => {
                    unmoved.insert(old.clone());
                }
            }
        }

        let mut frozen: HashSet<StationKey> = HashSet::new();
        for keys in [
            self.station_offsets.keys().cloned().collect::<HashSet<_>>(),
            self.station_comments.keys().cloned().collect(),
            self.station_names.keys().cloned().collect(),
        ] {
            frozen.extend(frozen_remaps(&keys, &rebuilt));
        }
        if !frozen.is_empty() {
            tracing::warn!(
                what,
                count = frozen.len(),
                "station refs collided on their new key; left on their old station to \
                 avoid dropping one"
            );
        }
        self.map_station_refs(|key| {
            if frozen.contains(&key) {
                return key;
            }
            rebuilt.get(&key).cloned().unwrap_or(key)
        });
        unmoved.extend(frozen);
        unmoved
    }
}
