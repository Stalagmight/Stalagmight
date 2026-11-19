/// [#303] Byte budget for a Full-mode SpeleoDB-linked project's undo history.
///
/// Full mode bypasses the entry-count window (`undo_history_limit` /
/// `undo_persist_limit`) so the project keeps its whole edit history since the last
/// commit - but "unbounded" is not safe here: this history rides in every SpeleoDB
/// commit, which is the storage cost the issue exists to REDUCE, and an import or an
/// Add `.stalx` records what it brought in (its entry IS the data it adds).
///
/// So depth is bounded by total size instead of entry count. At the measured ~13x gzip
/// ratio for this JSON shape, 32 MiB of JSON is roughly 2.5 MB per commit. A command
/// whose effect cannot be inverted field by field (a delete, a reverse, a merge, a
/// split, a batch) once recorded TWO whole-project snapshots - ~4.2 MB per entry on a
/// 5,000-shot project, ~17 MB on a 20,000-shot one, two of which filled the budget.
/// Such an entry is now a compact delta (`UndoData::Delta`) of what the command
/// changed, held as its JSON text, which is what the budget counts; the undo size gate
/// (`undo_size_gate_tests.rs`) holds every action's entry to 1% of the saved file:
///
/// | entry kind | size each | entries that fit |
/// |---|---|---|
/// | ordinary edit (`PreviousValues`, `InverseCommand`) | ~0.2-1 KB | ~32,000+ |
/// | compact entry, a delete, a reverse or a split, 20,000-shot project | ~1-10 KB | ~3,000+ |
/// | a 1,000-cell paste, 20,000-shot project | ~35 KB | ~900 |
/// | an import or an Add `.stalx` | the data it brought | the budget's share of it |
///
/// The number is a LATENCY budget as much as a storage one, because every save rewrites
/// the whole persisted stack (see `UndoStack::rows_to_persist` for why it cannot be
/// incremental): the worst-case save gzips this many bytes, ~0.3 s at the measured
/// throughput, which an autosave every 30 s can absorb. Raising it deepens
/// history and slows every save in proportion; that is the whole trade, and this
/// constant is the only lever.
///
/// Note what it replaces: the old fixed window persisted `undo_persist_limit` (20)
/// entries with NO size cap, which on a 20,000-shot project is ~336 MB of JSON per save.
/// So this is a large improvement in worst-case cost and an enormous one in ordinary
/// depth (100 entries -> ~32,000); only imports and Adds of large files still make the
/// history shallow.
///
/// Measured against `UndoStack::estimated_size`, NOT against a real serialization:
/// the point is to bound growth, and serializing the whole stack to measure it would
/// cost exactly what the budget exists to avoid.
pub const UNDO_BYTE_BUDGET_BYTES: usize = 32 * 1024 * 1024;

/// Command JSON plus enum/field scaffolding for any entry.
///
/// Module-level rather than local to `UndoStack::estimated_size` because
/// [`MAX_PERSISTED_HISTORY_ROWS`] is derived from it and the derivation is only
/// sound while the two are the same number.
pub const ENTRY_OVERHEAD_BYTES: usize = 512;

/// [#469] Hard ceiling on how many persisted history rows ONE load may read.
///
/// The persisted tables are attacker-controlled: a `.stalx` is a file the user opens,
/// and nothing in the schema bounds `undo_stack` / `redo_stack`. The loader used to
/// `SELECT` and gunzip every row before anything trimmed, so a crafted (or merely
/// ancient) file with a million rows in it was read whole during open, with the
/// in-memory bound armed but applied too late to help.
///
/// **What this bounds is the ROW COUNT, and only that.** It takes THREE bounds to close
/// the exposure, and the reason is that the first two multiply rather than compose. The
/// other two live next to `decompress_json_under` in persistence/sqlite.rs (#473):
/// `MAX_DECOMPRESSED_BLOB_BYTES` caps how large one row may inflate, and
/// `MAX_HISTORY_LOAD_BYTES` caps the total across BOTH stacks of one project open.
/// Without that third one a file inside BOTH of the first two still OOMs - 100 rows x 2
/// blobs x 256 MiB is 50 GiB on the default read window, and this constant's own 65536
/// rows make it 32 TiB nominal - which is not a hypothetical: a measured 14 MB file
/// allocated 3.1 GB and the load succeeded. None of the three may be claimed as covering
/// another's half.
///
/// A count window project reads its own window (`undo_history_limit`), which is exact.
/// Full mode has no count at all - its bound is [`UNDO_BYTE_BUDGET_BYTES`] - so this
/// is the ceiling it reads instead, and it is derived rather than picked: EVERY
/// restored entry is deserialized with `command_bytes: None`, so
/// `UndoStack::estimated_size` charges it at least [`ENTRY_OVERHEAD_BYTES`]. Once
/// more than `budget / ENTRY_OVERHEAD_BYTES` entries are in the stack the running
/// total necessarily exceeds the budget and `trim_to_bytes` evicts from the front.
/// Reading past this row therefore CANNOT change the loaded stack: the extra rows are
/// gunzipped, deserialized and then immediately evicted.
///
/// So the cap is lossless by construction EXCEPT for one shape: a count window
/// configured above this ceiling. `AppState::history_load_row_cap` applies
/// `.min(MAX_PERSISTED_HISTORY_ROWS)`, and `undo_history_limit` is an unclamped `usize`
/// in `config.toml` - the settings UI validates it to 10..1000, but a hand-edited
/// config is not validated at all. Such a project is NOT byte-bounded, so the rows past
/// the ceiling are ones its restore would have kept: that load is genuinely lossy. It is
/// accepted rather than fixed because the ceiling is 65536 entries (32 MiB / 512 B),
/// sixty-five times the deepest window the settings UI will accept, so only a config
/// edited by hand past 65536 reaches it - and the alternative is letting that same edit
/// hand an unbounded row count to a hostile file.
///
/// A byte ceiling on the load was considered and rejected: the in-memory accounting is
/// an estimate (see `UndoStack::estimated_size`), and a count-window project is not
/// byte-bounded at all, so any byte cutoff would silently drop history that today's
/// load keeps for a legitimately large project. Bounding the row count is the part
/// that can be proven, at every window the UI can configure, not to lose anything.
pub const MAX_PERSISTED_HISTORY_ROWS: usize = UNDO_BYTE_BUDGET_BYTES / ENTRY_OVERHEAD_BYTES;
