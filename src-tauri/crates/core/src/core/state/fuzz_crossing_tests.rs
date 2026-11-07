//! What a save or a reload legitimately changes, subtracted from the
//! `fuzz_undo_tests` comparisons that cross one.
//!
//! Split out of that module (#576 NF-W14BE-1) when a save gained one more thing
//! it fills in, a section's creation time: the harness file is frozen by the
//! test file-size ratchet, and these two predicates are the part of it that
//! states what a save and a reload are ALLOWED to change.

use super::fuzz_undo_tests::{offset_key_of, path_is, Diff, ABSENT};

/// Changes a SAVE makes to live state with no undo entry of its own
/// (`AppState::save_project`), subtracted only from comparisons that cross a
/// save. Deliberately narrow by direction: an anchor may only be GAINED, a uuid
/// filled in, a creation time stamped (#576 NF-W14BE-1), an offset entry
/// dropped.
pub(super) fn attributable_to_save(diff: &Diff) -> bool {
    ((path_is(diff, "sections[#].gauge_shot_uuid")
        || path_is(diff, "sections[#].gauge_neighbour_uuid"))
        && diff.expected == "null"
        && diff.actual != "null")
        || (path_is(diff, "sections[#].created_at")
            && diff.expected == ABSENT
            && diff.actual != ABSENT)
        || (path_is(diff, "sections[#].shots[#].uuid")
            && (diff.expected == ABSENT || diff.expected == "null")
            && diff.actual != ABSENT)
        || (path_is(diff, "section_offsets.#") && diff.actual == ABSENT)
}

/// Derived geometry a RELOAD re-solves from scratch: `redrive_offsets_after_load`
/// ends in an unconditional `resolve_offsets()` ("Zero-trust: never honor a
/// persisted derivable offset"), so `section_offsets` and the station offsets
/// derived from them are RECOMPUTED rather than restored.
pub(super) fn attributable_to_reload(diff: &Diff) -> bool {
    // BOTH spellings of an offset difference: the whole key when it is present
    // on one side only, and the element when it exists on both and the differ
    // recurses into the pair. The re-solve recomputes the whole map, so the two
    // are the same fact - and matching only the first left every value change
    // across a reload reading as unexplained.
    offset_key_of(&diff.path).is_some() || diff.path.starts_with("station_offsets")
}
