// Shared stride for encoding section-scoped IDs into globally unique IDs.
// Keep in sync with the frontend FEATURE_ID_STRIDE.
pub const FEATURE_ID_STRIDE: u32 = 100_000;

/// The largest section id whose shots all encode inside `u32`.
///
/// A shot id is `section_id * FEATURE_ID_STRIDE + sequence_order`, and a
/// section holds at most `FEATURE_ID_STRIDE - 1` shots, so the last shot of
/// section `id` encodes as `id * 100_000 + 99_999`. The largest `id` that keeps
/// THAT inside `u32::MAX` is `(u32::MAX - 99_999) / 100_000` = 42_948.
///
/// Not 42_949: a section is entitled to its full `FEATURE_ID_STRIDE - 1` shots,
/// and that id cannot hold them. Its base multiply fits (4_294_900_000 <=
/// `u32::MAX`) and so do its first 67_296 shots, but sequence 67_296 leaves
/// `u32` (4_294_900_000 + 67_295 IS `u32::MAX`), so the section's own
/// `next_shot_id += 1` counter overflows part-way through filling it. Id 42_950
/// is the first that cannot even be created: 4_295_000_000 overflows the bare
/// multiply in `Section::new`, before a single shot exists.
///
/// Past the bound an encode site either panics (debug) or saturates, and
/// saturating ALIASES distinct shots onto one id, which silently moves the
/// station cross-references anchored on them - hence the bound is enforced
/// where section ids are MINTED (create-section, batch import, project merge,
/// the delete-and-split paths) rather than at each encode.
pub const MAX_SECTION_ID: u32 = (u32::MAX - (FEATURE_ID_STRIDE - 1)) / FEATURE_ID_STRIDE;

/// The same bound expressed as a COUNT of sections.
///
/// Section ids are minted densely from 0 and never reused
/// (`Project::add_section`), so `Project::next_section_id` is exactly "how many
/// of these ids are already spent" and `next_section_id + incoming_sections`
/// against this constant is the whole budget check.
pub const MAX_SECTIONS_IN_ID_SPACE: usize = MAX_SECTION_ID as usize + 1;

/// The most shots one section may hold: one short of its `FEATURE_ID_STRIDE`
/// window, so a free id always remains for a re-numbered or replayed shot.
/// `Project::add_shot` stops there, a file holding more is refused on load
/// (A18(d), #576), and `create_section` and the batch import gate refuse a
/// longer section (#576 NF-AN2-8) - `Section::new` itself does not check.
pub const MAX_SHOTS_PER_SECTION: usize = (FEATURE_ID_STRIDE - 1) as usize;

// Compile-time proof of the two directions of the bound. Const arithmetic
// overflow is a build error rather than a wrap, so evaluating the encode of the
// last shot of the last usable section IS the proof that it fits; what is left
// above it being under one stride is the proof that no further section does.
const _: () = {
    assert!(MAX_SECTION_ID == 42_948);
    assert!(MAX_SECTIONS_IN_ID_SPACE == 42_949);
    let last_shot_id = MAX_SECTION_ID * FEATURE_ID_STRIDE + (FEATURE_ID_STRIDE - 1);
    assert!(u32::MAX - last_shot_id < FEATURE_ID_STRIDE);
};

#[cfg(test)]
mod tests {
    use super::{FEATURE_ID_STRIDE, MAX_SECTIONS_IN_ID_SPACE, MAX_SECTION_ID};

    /// The numbers themselves, spelled out: every boundary check in the import,
    /// merge and create-section paths is stated against these, and a silent
    /// change to the stride would move them.
    #[test]
    fn the_section_id_bound_is_42_948_and_the_count_is_42_949() {
        assert_eq!(FEATURE_ID_STRIDE, 100_000);
        assert_eq!(MAX_SECTION_ID, 42_948);
        assert_eq!(MAX_SECTIONS_IN_ID_SPACE, 42_949);
    }

    /// The property the constants exist for, checked in `u64` so the assertion
    /// itself cannot be the thing that wraps.
    #[test]
    fn the_last_section_encodes_and_the_next_one_does_not() {
        let stride = u64::from(FEATURE_ID_STRIDE);
        let last = u64::from(MAX_SECTION_ID) * stride + (stride - 1);
        assert!(
            u32::try_from(last).is_ok(),
            "the last shot of section {MAX_SECTION_ID} must fit u32, got {last}"
        );
        let next = (u64::from(MAX_SECTION_ID) + 1) * stride + (stride - 1);
        assert!(
            u32::try_from(next).is_err(),
            "section {} must not fit, got {next}",
            MAX_SECTION_ID + 1
        );
    }
}
