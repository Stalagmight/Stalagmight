//! Derived replacement shot uuids (#423): the id a repeated shot gets, and how
//! a key stored outside the project against an id a uniqueness heal replaced
//! (an ignored warning, a dive-plan anchor) still finds its shot.
//!
//! The replacement id is DERIVED, not random ([`reminted_shot_uuid`]): it is a
//! function of the line and the id it replaces. A file that is opened and
//! never saved therefore reads to the same ids every time, two independent
//! loads of one revision agree (SpeleoDB Unwind), and a key stored against the
//! old id elsewhere can still be recognised ([`stored_shot_uuid_matches`]).
//! The frozen 0.6.0 reader (`stalagmight-legacy`) heals with the same rule.

use crate::LineId;

/// Namespace of the derived replacement ids. Changing it changes every derived
/// id, so a file healed by one build would be healed differently by the next.
pub const SHOT_UUID_REMINT_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x3c2a_91d4_5e7b_4f06_a1c8_6d0e_9b47_2f15);

/// The id a repeated shot gets in `line_id` in place of `old`.
///
/// A v5 hash of the line and the old id, re-stamped as a v4: the SpeleoDB
/// sidecar keeps only v4 values (`speleodb_uuid`), and the version nibble is all
/// that distinguishes the two. The old id is length-prefixed so no pair of
/// (line, id) inputs can produce the same hash input.
pub fn reminted_shot_uuid(line_id: LineId, old: &str) -> String {
    remint_attempt(line_id, old, 0)
}

/// [`reminted_shot_uuid`], stepped past any id `taken` reports as in use. The
/// first attempt is the plain derived id; a collision with a 122-bit hash is not
/// a practical case, so stepping exists only to keep uniqueness unconditional.
pub fn reminted_shot_uuid_avoiding(
    line_id: LineId,
    old: &str,
    taken: impl Fn(&str) -> bool,
) -> String {
    (0..u32::MAX)
        .map(|attempt| remint_attempt(line_id, old, attempt))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| remint_attempt(line_id, old, 0))
}

fn remint_attempt(line_id: LineId, old: &str, attempt: u32) -> String {
    let name = if attempt == 0 {
        format!("{line_id}:{}:{old}", old.len())
    } else {
        format!("{line_id}:{}:{old}#{attempt}", old.len())
    };
    let hashed = uuid::Uuid::new_v5(&SHOT_UUID_REMINT_NAMESPACE, name.as_bytes());
    uuid::Builder::from_random_bytes(*hashed.as_bytes())
        .into_uuid()
        .to_string()
}

/// Whether an id stored against `line_id` somewhere outside the project (an
/// ignored warning, a dive-plan anchor) names the live shot whose id is `live`:
/// either it is that id, or the shot was renamed from it by a uniqueness heal.
pub fn stored_shot_uuid_matches(line_id: LineId, stored: &str, live: &str) -> bool {
    stored == live || reminted_shot_uuid(line_id, stored) == live
}

/// The live id that a key stored against `stored` in `line_id` names, given
/// `holds` (whether the line holds a shot with that id now): `stored` itself,
/// else the derived replacement a uniqueness heal gave that shot, else `None`.
///
/// THE one rule every key stored outside the project follows (ignored warnings,
/// dive-plan anchors, an Add's carried ignores). It tries the FIRST derived
/// candidate only: a replacement stepped past an id already in use exists only
/// when a file holds a derived value itself, and then the key names whichever
/// shot holds that value - a shot the same file could have named directly.
pub fn live_shot_uuid_for_stored(
    line_id: LineId,
    stored: &str,
    holds: impl Fn(&str) -> bool,
) -> Option<String> {
    if holds(stored) {
        return Some(stored.to_string());
    }
    let renamed = reminted_shot_uuid(line_id, stored);
    holds(&renamed).then_some(renamed)
}
