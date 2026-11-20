// Shared stride for encoding line-scoped IDs into globally unique IDs.
// Keep in sync with the frontend FEATURE_ID_STRIDE.
pub const FEATURE_ID_STRIDE: u32 = 100_000;

/// The largest line id whose shots all encode inside `u32`.
///
/// A shot id is `line_id * FEATURE_ID_STRIDE + sequence_order`, and a
/// line holds at most `FEATURE_ID_STRIDE - 1` shots, so the last shot of
/// line `id` encodes as `id * 100_000 + 99_999`. The largest `id` that keeps
/// THAT inside `u32::MAX` is `(u32::MAX - 99_999) / 100_000` = 42_948.
///
/// Not 42_949: a line is entitled to its full `FEATURE_ID_STRIDE - 1` shots,
/// and that id cannot hold them. Its base multiply fits (4_294_900_000 <=
/// `u32::MAX`) and so do its first 67_296 shots, but sequence 67_296 leaves
/// `u32` (4_294_900_000 + 67_295 IS `u32::MAX`), so the line's own
/// `next_shot_id += 1` counter overflows part-way through filling it. Id 42_950
/// is the first that cannot even be created: 4_295_000_000 overflows the bare
/// multiply in `Line::new`, before a single shot exists.
///
/// Past the bound an encode site either panics (debug) or saturates, and
/// saturating ALIASES distinct shots onto one id, which silently moves the
/// station cross-references anchored on them - hence the bound is enforced
/// where line ids are MINTED (create-line, batch import, project merge,
/// the delete-and-split paths) rather than at each encode.
pub const MAX_LINE_ID: u32 = (u32::MAX - (FEATURE_ID_STRIDE - 1)) / FEATURE_ID_STRIDE;

/// The same bound expressed as a COUNT of lines.
///
/// Line ids are minted densely from 0 and never reused
/// (`Project::add_line`), so `Project::next_line_id` is exactly "how many
/// of these ids are already spent" and `next_line_id + incoming_lines`
/// against this constant is the whole budget check.
pub const MAX_LINES_IN_ID_SPACE: usize = MAX_LINE_ID as usize + 1;

// Compile-time proof of the two directions of the bound. Const arithmetic
// overflow is a build error rather than a wrap, so evaluating the encode of the
// last shot of the last usable line IS the proof that it fits; what is left
// above it being under one stride is the proof that no further line does.
const _: () = {
    assert!(MAX_LINE_ID == 42_948);
    assert!(MAX_LINES_IN_ID_SPACE == 42_949);
    let last_shot_id = MAX_LINE_ID * FEATURE_ID_STRIDE + (FEATURE_ID_STRIDE - 1);
    assert!(u32::MAX - last_shot_id < FEATURE_ID_STRIDE);
};
