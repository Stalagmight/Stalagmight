//! A seeded undo/redo fuzzer that runs INSIDE the Rust test suite, and the CI
//! gate built on it.
//!
//! # Why this exists next to the agent harness
//!
//! `e2e/agent/scenarios/undoRedoFuzz.ts` and its persistence sibling found real
//! bugs - eleven of them are on
//! <https://github.com/Stalagmight/Stalagmight/issues/413> - but neither of them
//! GATES anything. The harness drives a built desktop app through WebDriver and
//! never runs in CI, and the red tests in `open_undo_bugs_tests.rs` were
//! `#[ignore]`d by design. So an undo regression could land without a single
//! check going red.
//!
//! This module closes that hole. It is the same model - a canonical project
//! snapshot per undo depth, asserted after every history move - driven directly
//! against `AppState`, which makes it roughly three orders of magnitude cheaper
//! per sequence and therefore something a `#[test]` can afford to run on every
//! push.
//!
//! The three tiers are complementary, not redundant:
//!
//! | tier | drives | catches | runs |
//! |---|---|---|---|
//! | this module | `AppState` directly | undo/redo model + persistence regressions | EVERY CI run |
//! | `metamorphic_undo_tests.rs` | two `AppState`s from one file | two routes to the same edit disagreeing | EVERY CI run |
//! | `format_property_tests.rs` | the `.stalx` on disk | a writer or reader that is not a function | EVERY CI run |
//! | `crash_property_tests.rs` | the files a kill mid-save leaves on disk | a save whose interruption leaves a blend of old and new | EVERY CI run |
//! | `adversarial_tests.rs` | `AppState` with hostile arguments | a refusal that mutates, and any panic | EVERY CI run |
//! | `concurrency_tests.rs` | one `AppState` from several threads | a deadlock, a poisoned lock, a race | EVERY CI run |
//! | agent harness | the real app over IPC | anything the frontend store or the IPC boundary breaks | on demand |
//! | `open_undo_bugs_tests.rs` | `AppState` directly | the reductions each finding was FIXED against, pinned | EVERY CI run |
//!
//! `fuzz_shrink_tests.rs` is not a tier: it finds nothing. It MINIMIZES what
//! this module finds - see "The trace, and the shrinker" below.
//!
//! `fuzz_rarity_tests.rs` is not a tier either: it is a MEASUREMENT, and its
//! answer is the second negative result on corpus guidance. See
//! [`fuzz_corpus_sweep`] for the first one and for what the two of them say
//! together.
//!
//! # THE BUG IDENTIFICATION MATRIX
//!
//! Every finding the agent-harness fuzzers found is reachable HERE, and each one
//! was CLASSIFIED by id rather than merely hit while it was open. The table below
//! is the evidence: for each one, the mode, seed and move count under which this
//! module reached it and named it, and what is there NOW.
//!
//! Every one of them is FIXED, so every classifier is deleted and
//! [`CLASSIFIERS`] is EMPTY: `classify` answers `None` for every difference and
//! nothing can bless a diff. Each row therefore records the FIX and the sweep
//! that is clean over the shape the row used to be found in, which is the only
//! thing a deleted classifier can be held to.
//!
//! Read a row as: run `fuzz_seed_survey` with the listed environment, and the
//! per-seed summary line for that seed carries no hit at all. `SEEDS=n` runs
//! seeds 1..=n, so the named seed is included.
//!
//! | # | was classified as | now | mode env | seed | moves |
//! |---|---|---|---|---|---|
//! | 1 (uuids) | `finding-1-redo-of-create-section-remints-shot-uuids` | FIXED - `CreatedSectionSnapshot` carries a `SectionIdentity`; seeds 1..40 x100 are clean | (none) | 1 | 60 |
//! | 1 (anchors) | `finding-1-redo-of-create-section-drops-gauge-anchors` | FIXED with the uuids, by the same identity | (none), persistence half | 1 | 60 |
//! | 2 | `finding-2-orphan-zero-section-offset` | FIXED - `Project::write_section_offset` drops the dust ball around zero | (none) | 1 | 60 |
//! | 3 (offset) | `finding-3-undo-of-the-first-shot-displaces-an-empty-section` | FIXED - `Project::remove_appended_first_shot` is the undo of an append | (none) | 2 | 60 |
//! | 3 (data loss) | `finding-3-undo-of-the-first-shot-drops-station-zero-refs` | FIXED with the offset half - the undo no longer routes through the general delete | (none) | 37 | 60 |
//! | 4 | `finding-4-closure-reordered-on-undo` | FIXED - `UndoData::ClosureInfo` records the index the `Vec::remove` took | `CLOSURE_SHAPES=1` | 1 | 40 |
//! | 5 | `finding-5-undo-cannot-restore-null-section-field` | FIXED - `SectionUpdates` carries `double_option` | (none) | 6 | 60 |
//! | 5 (command) | `finding-5-a-reloaded-command-can-no-longer-clear-a-section-field` | FIXED with it; schema v17 drops pre-v17 history rather than reinterpret it | (none), persistence half | 1 | 60 |
//! | 6 | `finding-6-undo-refused-by-a-delete-guard` | FIXED - the undo removes the created shot through `AppState::remove_created_shot`, which consults no user-intent guard | `CLOSURES=1` | 4 | 60 |
//! | 7 | `finding-7-a-jump-is-not-n-single-steps` | FIXED - `undo_to`/`redo_to` reconcile per step; `bug_7_a_jump_is_n_single_steps` is no longer ignored and GATES it | `JUMPS=1 CLOSURE_SHAPES=1`, persistence half | 6 | 150 |
//! | 8 | `finding-8-stale-id-deletes-the-wrong-shot` | FIXED - every restore path addresses its shot by `Shot::uuid` | `DIVERGENCE=1` | 1 | 12 |
//! | 10 | `finding-10-reload-converts-an-orphan-closure-to-a-merge` | FIXED at the LOAD boundary - the commands that orphan a closure convert it themselves. NOT fixed in-session: see the open observation below | `CLOSURE_SHAPES=1`, persistence half | 1 | 40 |
//! | 11 | `finding-11-pre-save-entry-reverts-the-saves-heals` | FIXED - `reapply_heals_after_restore` brings a restored snapshot up to a saved project's standard | (none), persistence half | 1 | 60 |
//! | 12 | `finding-12-stale-id-edits-the-wrong-shot` | FIXED with finding 8 | `DIVERGENCE=1` | 1 | 12 |
//! | 13 | `finding-13-stale-id-misses-and-wedges-the-history` | FIXED with finding 8, plus a load-time pass that DROPS an entry whose target no longer resolves; a refused history move fails the run again | (none), persistence half | 5 | 60 |
//! | 14 | `finding-14-insert-leaves-a-merge-metres-out-of-alignment` | FIXED - the insert compensates only when the tail alone is anchored; ANY alignment assert fails the run again | `CLOSURES=1` | 6 | 60 |
//! | 14 (mirror) | `finding-14-variant-a-shot-removal-leaves-a-merge-out-of-alignment` | FIXED with 14, in `remove_shot_with_rewire` | `JUMPS=1 CLOSURE_SHAPES=1` | 1 | 150 |
//! | 15 | = finding 3, see below | FIXED with it | (none) | 5 | 20 |
//! | 16 | `finding-16-redo-of-a-create-shot-duplicates-a-shot-uuid` | FIXED - a redo refuses to duplicate a uuid | (none), persistence half | 13 | 60 |
//! | 17 | `finding-17-zero-datum-reordered-on-undo` | FIXED with finding 4, on `UndoData::ZeroDatumInfo` | (none) | 1 | 60 |
//! | 18 | `finding-18-raw-and-resolved-merge-alignment-disagree` | DELETED with finding 14 - `expect_raw_and_resolved_agree` still runs and now FAILS; the id never recorded a hit in any mode, before the fix or after | n/a | n/a | n/a |
//! | 20 | `finding-20-an-unanchored-merge-group-is-never-realigned` | FIXED on both halves - an unanchored group is realigned against the edited section, and the heal gate is the app's own invariant; every drifted merge fails, at any magnitude | `HORIZON=1` | 1 | 2000 |
//! | 21 | red test only - `the_adjusted_solve_is_a_function_of_the_project` | FIXED - `adjust_least_squares` numbers from a SORTED sequence; that test is no longer ignored and `ADJUSTED_TOLERANCE_REL` is deleted | n/a | n/a | n/a |
//! | 22 | `finding-22-a-range-shift-delete-is-not-its-single-deletes` | RESOLVED as INTENDED - the metamorphic pair ASSERTS the difference now instead of tolerating it | metamorphic gate | 3 | n/a |
//! | 23 | `finding-23-a-save-load-cycle-is-not-a-fixpoint` | FIXED - `serde_json`'s `float_roundtrip`, plus `cascade_bfs` declining a sub-dust correction; the format gate compares two generations EXACTLY | format-property gate | 1 | n/a |
//! | 23 (display) | `finding-23-history-float-dust-moves-a-displayed-value` | FIXED with it; a history row that reads differently after a reload fails the run | `CROSSINGS=3`, persistence half | 16 | 60 |
//! | 24 | `finding-24-shot-display-position-overflows-before-validation` | FIXED - `shot_display_position` saturates; the adversarial gate tolerates no panic | adversarial gate | 4 | 120 |
//! | 25 | UNCLASSIFIED - failed the run | FIXED as finding 26's sibling - a shot edit HOLDS the datum anchor where it found it | `HORIZON=1 JUMPS=1` | 2 | 10000 |
//!
//! Finding 19 is an id this module still uses and is not a row: it is tolerated
//! float dust ACCUMULATING past the tolerance, which the horizon mode watches for
//! and which has not fired. It is the one id `Outcome::hits` can still carry, and
//! it is not a classifier - `classify` cannot produce it. It stays reserved so
//! the numbering does not collide, which is why the new ones start at 20.
//!
//! Findings 21 to 24 are not roundtrip-oracle rows because none of them is a
//! roundtrip defect. 21 is measured by re-solving one project
//! (`open_undo_bugs_tests.rs`); 22 by the METAMORPHIC engine
//! (`metamorphic_undo_tests.rs`), which compares two routes rather than two
//! moments; 23 by the FILE-FORMAT engine (`format_property_tests.rs`), which
//! compares two saves rather than two states; and 24 by the ADVERSARIAL engine
//! (`adversarial_tests.rs`), which compares nothing at all - it asserts that a
//! refusal is a no-op and that nothing panics.
//!
//! The two rows with a qualifier - 5 (command) and 23 (display) - are the SAME
//! defects reaching a second surface, not new numbers, and they are here because
//! the ENTRY-CONTENT oracle is the only thing that looks at that surface. Finding
//! 5's write-up is about what an UNDO cannot restore; the command half is about
//! what a reloaded entry can no longer say or REDO. Finding 23's is about two
//! saves differing; the display half is about one ULP of that difference landing
//! on a rounding boundary and changing a number the user reads. Both are cross-
//! referenced from the file that owns the original.
//!
//! ## What the strict engine measures, on this branch
//!
//! Every number below is a `fuzz_seed_survey` run at the stated environment, and
//! "clean" means the run reached NOTHING - not one classified hit, because there
//! is nothing left to classify.
//!
//! | mode env | seeds x moves | clean |
//! |---|---|---|
//! | (none) | 40 x 60 | 40/40 |
//! | (none) | 40 x 100 | 40/40 |
//! | `CLOSURES=1` | 20 x 60 | 20/20 |
//! | `CLOSURE_SHAPES=1` | 20 x 40 | 20/20 |
//! | `CLOSURE_SHAPES=1` | 40 x 150 | 40/40 - the regime that found the dust-gate observation below |
//! | `DIVERGENCE=1` | 20 x 12 | 20/20 |
//! | `JUMPS=1 CLOSURE_SHAPES=1` | 20 x 150 | 20/20 |
//! | `COMBINED=1` | 40 x 300 | 40/40 |
//! | `HORIZON=1` | 3 x 5000 | 3/3 |
//!
//! Environment prefix for every row: `STALAGMIGHT_RUST_FUZZ_`. So the finding-7
//! row is
//!
//! ```text
//! STALAGMIGHT_RUST_FUZZ_JUMPS=1 STALAGMIGHT_RUST_FUZZ_CLOSURE_SHAPES=1 \
//!   STALAGMIGHT_RUST_FUZZ_SEEDS=6 STALAGMIGHT_RUST_FUZZ_ITERATIONS=150 \
//!   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
//!   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
//!   --lib fuzz_seed_survey -- --ignored --nocapture
//! ```
//!
//! Three findings on the issue are NOT rows because they are not this tier's:
//! finding 9 was triaged out as float cancellation from the fuzzer's own inputs,
//! and 15 turned out not to exist separately - see below.
//!
//! ## What finding 15 was
//!
//! Finding 15 ("an undo leaves a `section_offsets` entry carrying REAL geometry")
//! is FINDING 3, re-measured. Finding 3 displaced the empty section on the undo
//! that removed its first shot; nothing ever put that back, because the redo
//! re-inserts through the offset-neutral append path - which is finding 3's own
//! ratchet half. So the leftover was still in the project several undos later,
//! when the entry being consumed is unrelated and the snapshot it is compared
//! against never had the key.
//!
//! Confirmed against the reported instance itself: seed 7 at 100 moves, depth 14,
//! `section_offsets.1` = `[7.11187043796, 0.315846281796]` - whose two
//! immediately preceding moves are `createShot(s=1,n=0,append)` and the undo of
//! it, into and out of section 1, the very section that ends up displaced. The
//! write-up read the move log from the wrong end.
//!
//! The classifiers carried that shape as `finding-3-cascade-...` until finding 3
//! was fixed, which fixed this with it and took both classifiers with it.
//! `unwinding_the_history_leaves_no_displaced_section_behind` in
//! `open_undo_bugs_tests.rs` pins the consequence a user sees and now passes.
//!
//! ## Observations, and what each one turned out to be
//!
//! NOTHING IS OPEN. The dust-gate story was the last one - the first entry
//! below, `CLOSURE_SHAPES=1` seed 18 - and it is fixed, with that regime back at
//! 40/40. Every other observation this section has carried is resolved too: the
//! orphan-closure story's two halves (the LOAD BOUNDARY half and the UNDO half),
//! the closure that asserted its own coincidence, three into findings (20, 21,
//! 25), two into model fixes, one into a duplicate of finding 12, and one
//! triaged out as the measuring engine's own artifact. The record stays because
//! the next run of this programme should start from the evidence - including the
//! shape of the reductions that did NOT reproduce - rather than from a surprise.
//!
//! The bar an entry has to clear to be listed as OPEN is unchanged:
//! deterministic, reproducing twice, and FAILING its run rather than classified.
//!
//! * **RESOLVED, and FIXED: the offset cascade's dust gate was ABSOLUTE, and the
//!   coordinates stopped being metre-scale.** `CLOSURE_SHAPES=1 SEEDS=40
//!   ITERATIONS=150` failed seed 18 on `undo x1 did not restore the state
//!   recorded at depth 11 | at section_offsets.8`, `expected: <absent>`,
//!   `actual: [1.78e-14, -3.61e-12]` - a picometre key on a section the consumed
//!   entry (`create_shot` on another section) never touched.
//!
//!   `fuzz_shrink_tests.rs` took the 84 recorded moves to THIRTEEN in 281
//!   probes. Measured, not argued: a backtrace on the write names
//!   `remove_shot_with_rewire` -> `cascade_connection_offsets` -> `cascade_bfs`,
//!   on the UNDO of an appended `createShot`; the same value was written once
//!   before, by a forward `deleteShot`, and cleared in between by
//!   `removeZeroDatum`'s zero-trust `resolve_offsets`. The two cascades run from
//!   DIFFERENT seed sets (the edited section plus every datum section, versus
//!   one anchor per component), so they sum the same chains in different orders
//!   and disagree in the last bits.
//!
//!   Why then: `cascade_bfs` declines a correction at or under
//!   `Project::OFFSET_DUST_M`, and that bound is ABSOLUTE while the residue it
//!   exists to swallow is RELATIVE - it tracks the magnitude of the coordinates
//!   being summed. #469 item 1 renormalises the whole survey onto a promoted
//!   datum, which puts coordinates at the inter-datum distance (kilometres)
//!   instead of metres, and 16 km x 2.2e-16 is 3.6e-12: the residue crossed the
//!   1e-12 gate it used to sit two decades under. Both halves of the branch were
//!   NECESSARY, which is what made it one observation rather than two: with the
//!   renormalisation disabled the minimized trace produced no failure, and
//!   replayed on the commit before #469 item 2 (so the same `CLOSURE_SHAPES`
//!   closures are `is_aligned` by assertion rather than by measurement, and the
//!   cascade reaches different sections) it produced none either. Item 2 supplied
//!   the path; item 1 supplied the magnitude.
//!
//!   FIXED by re-measuring the constant rather than by bumping it. Every gate
//!   site was instrumented and the whole distribution of `max(|dx|, |dy|)`
//!   recorded per site, over this file's own corpora, with the generator's datum
//!   draw left alone and then widened to scatter datums worldwide:
//!
//!   | corpus | coordinates reached | worst residue |
//!   |---|---|---|
//!   | `CLOSURE_SHAPES=1` 40x150, real GPS spread | 7.9e4 m | 1.46e-11 m |
//!   | `COMBINED=1` 40x300, real GPS spread | 7.9e4 m | 1.96e-11 m |
//!   | `CLOSURE_SHAPES=1` 40x150, datums worldwide | 5.7e6 m | 9.31e-10 m |
//!   | `COMBINED=1` 40x300, datums worldwide | 4.7e6 m | 9.31e-10 m |
//!   | `HORIZON=1` 3x5000, datums worldwide | 1.8e7 m | 4.66e-9 m |
//!
//!   Each is one to five ULPs of the coordinate magnitude, and between the
//!   residue and the smallest real correction (1e-1 m in these corpora) the
//!   histogram is EMPTY for eight decades - so the gate belongs in that valley,
//!   far from both ends. `Project::OFFSET_DUST_M` is now 1e-6 m, two decades
//!   above the worst residue measured past the georeference ceiling and four
//!   below `MERGE_ALIGNMENT_INVARIANT_M`; the constant's doc carries the whole
//!   argument, and two tests in `models/project/tests.rs` re-run the measurement
//!   and pin the clearances. A scale-aware gate was rejected on purpose: the
//!   bound has to stay a pure function of the value written, or two routes
//!   disagree BY DESIGN.
//!
//! * **RESOLVED, and FIXED: a closure orphaned by a command that did not convert
//!   was reclassified by the next LOAD.** Surfaced by the fix in the next entry:
//!   with the undo half closed, `COMBINED=1 SEEDS=40 ITERATIONS=300` went to
//!   39/40, seed 20's PERSISTENCE half failing on `reload did not restore the
//!   state recorded at depth # | at loop_closures.length` with
//!   `station_merges.length` moving the other way. Pre-existing rather than new,
//!   and measured rather than argued: with `models/project/offsets.rs` and
//!   `models/project/shots.rs` reverted to their pre-fix state, a project built
//!   as `createLoopClosure` + `removeZeroDatum` (NO shot removal anywhere, so
//!   none of that code runs at all) saved and reopened still moved the entry from
//!   `loop_closures` to `station_merges`. What the undo fix changed is that the
//!   seed reached the reload comparison instead of aborting at the undo one
//!   several moves earlier.
//!
//!   `fuzz_shrink_tests.rs` took seed 20's 168 recorded moves to 14 in 178 probes
//!   (`PERSISTENCE=1 CROSSINGS=3` - the shrinker defaults `CROSSINGS` to 1 while
//!   the survey defaults it to `COMBINED_CROSSINGS`, and the persistence half
//!   does not reproduce under the shorter crossing schedule). The two that matter
//!   opened the list: `createLoopClosure(7/0,3/2,Move)` then
//!   `removeZeroDatum(7/0)`; the rest was filler carrying the project to a
//!   `save/close/reopen`.
//!
//!   Root cause, and it was one asymmetry rather than a bug in either half.
//!   `RemoveZeroDatum` un-anchors one side of a closure that bridges two
//!   otherwise unconnected groups. It changes no topology, so the closure was
//!   already a sole bridge; what it changes is that `merge_would_double_anchor`
//!   stops refusing the conversion, there being no second GPS anchor left to fuse
//!   into one rigid component. The command was not flagged
//!   `can_orphan_closures`, so nothing converted in session, while
//!   `redrive_offsets_after_load` converts unconditionally. Save
//!   `(1 closure, 0 merges)`, reopen `(0 closures, 1 merge)`.
//!
//!   FIXED by converging the forward path with the loader rather than by teaching
//!   the loader to leave things alone (which would give back finding 10's
//!   boundary fix): `RemoveZeroDatum` is now classified `can_orphan_closures` -
//!   its own profile, `REMOVE_DATUM` in core/state/commands.rs, because the
//!   reason is not the usual one - so `execute()`'s epilogue converts at the
//!   instant the closure becomes convertible. `SetZeroDatum` deliberately does
//!   NOT change: ADDING an anchor can only move the guard the other way, turning
//!   a convertible orphan into a refused one, and a refused orphan is one the
//!   loader leaves alone too, so session and disk already agree.
//!
//!   The conversion is not something `ZeroDatumInfo` can invert - it can put back
//!   a datum and a map of offsets, not a closure that has become a merge - so the
//!   command's undo capture moved to a `ProjectSnapshotPair` (core/state/
//!   undo_capture.rs). That is the established self-refreshing wall: the epilogue's
//!   conversion lands in the `after` half by construction, the undo restores
//!   closure + datum + offsets verbatim, and finding 17's index-preserving datum
//!   restore comes with it. `SetZeroDatum` keeps `ZeroDatumInfo`, and the legacy
//!   restore arm stays for stacks persisted before the change. Pinned by
//!   `removing_a_zero_datum_converts_the_closure_it_makes_convertible` and
//!   `removing_a_zero_datum_leaves_nothing_for_the_next_load_to_reclassify` in
//!   `open_undo_bugs_tests.rs`.
//!
//!   The invariant this establishes, and the reason no other command needed the
//!   same treatment: a closure becomes CONVERTIBLE either by losing the
//!   connectivity that made it a loop (the removals, all already flagged) or by
//!   losing one of the two anchors that made the conversion unsafe (a datum
//!   removal - this command - or a delete that takes the datum's station with it,
//!   also already flagged). Nothing else moves either quantity, so no forward
//!   command can now leave the session holding a convertible orphan for the
//!   loader to find.
//! * **RESOLVED, and FIXED: an orphan closure converted IN SESSION by an undo
//!   was not one the undo was inverting.** `COMBINED=1 SEEDS=40 ITERATIONS=300`
//!   failed seeds 20 (both halves) and 35 on `undo x# did not restore the state
//!   recorded at depth # | at loop_closures.length`, with
//!   `station_merges.length` moving the other way in the same comparison - one
//!   entry leaving `loop_closures` and arriving in `station_merges` across a
//!   single undo. No save and no reload were involved (`saves=0 reloads=0`),
//!   which is what separated it from FINDING 10.
//!
//!   `fuzz_shrink_tests.rs` took seed 20's 74 recorded moves to THREE in 55
//!   probes: `createLoopClosure(7/0,3/2)`, `removeZeroDatum(7/0)`,
//!   `createShot(s=5,append)`, then `undo x1`. The mechanism is visible in that
//!   sequence: removing the zero datum makes the closure convertible (the entry
//!   above), nothing converts it, and the undo of the `createShot` then ran
//!   `Project::convert_orphan_closures_to_merges` - a WHOLE-PROJECT sweep - from
//!   the shot-removal path it takes as its inverse (`remove_appended_first_shot`
//!   or `remove_shot_with_rewire`, models/project/shots.rs). So the undo
//!   converted a closure orphaned two moves EARLIER by an unrelated command and
//!   landed on a state the history never recorded.
//!
//!   (A correction to the mechanism as first written here: the conversion does
//!   NOT run from `realign_merges`, and therefore not after every command.
//!   `reconcile_geometry` calls `realign_merges`, which re-solves offsets and
//!   never touches the closure/merge classification. The conversion runs from
//!   the two shot-removal paths, `execute()`'s `can_orphan_closures` epilogue,
//!   `execute_batch`, the three custom delete entry points, Repair, and the
//!   load.)
//!
//!   FIXED by scoping the inverse rather than by recording the conversion in an
//!   `UndoData` variant: the two shot-removal paths snapshot the ALREADY-orphaned
//!   closure ids before mutating (`Project::orphan_closure_ids`) and convert only
//!   what they themselves orphaned (`Project::convert_closures_orphaned_since`,
//!   models/project/offsets.rs). An inverse then does no more than invert, while
//!   a closure genuinely stranded by the removal - including one whose endpoints
//!   are nowhere near the deleted station, stranded because a merge that WAS
//!   anchored there went with it - still converts. The forward paths are
//!   untouched: `execute()`'s epilogue still sweeps the whole project after a
//!   user `DeleteShot`, and the load and Repair sweeps still convert everything
//!   they find. Pinned at the MODEL level by
//!   `removing_a_shot_converts_the_closure_it_orphans_and_not_an_older_one` and
//!   its `remove_appended_first_shot` sibling in `models/project/tests.rs`, which
//!   is where it has to live now: the entry above converged the forward path, so
//!   a project holding a pre-existing convertible orphan is no longer
//!   constructible through the command API and this shape cannot be built from
//!   `AppState` at all. `undo_of_an_appended_first_shot_restores_the_recorded_closure_state`
//!   and its sibling in `open_undo_bugs_tests.rs` still walk the two inverses
//!   end to end against the recorded state.
//!
//!   NOT introduced by the #413 fixes and NOT reachable by any gate. The
//!   conversion has been on `remove_shot_with_rewire` since before this
//!   programme, and this tier could not see it because
//!   `diffs_without_orphan_conversion` SUBTRACTED exactly this shape from every
//!   comparison under the id
//!   `finding-10-reload-converts-an-orphan-closure-to-a-merge`, unconditionally
//!   and despite the id naming a reload. Deleting that subtraction
//!   is what made it visible. Finding 10's own fix is real and is a different
//!   half: the commands that ORPHAN a closure convert it themselves, so the LOAD
//!   has nothing left to reclassify - and with the entry above, that now holds
//!   for the one command it did not cover.
//! * **RESOLVED, and FIXED: a closure ASSERTED that its endpoints coincided.**
//!   Recorded on #413 as "arguably intended staleness after large geometry
//!   edits" and carried to #469 item 2: `COMBINED=1` runs left the post-run
//!   integrity sweep reporting a closure "marked closed but its ends are 5.86 m
//!   apart", with every IN-RUN oracle green. That combination is the whole
//!   signature: `Verdict` compares one state's integrity messages against
//!   another's, and this message was reproduced identically in both directions,
//!   so nothing here could see it - only the harness's post-run sweep
//!   (`e2e/agent/lib/oracles.ts`) named it.
//!
//!   The mechanism was the CREATE, not the drift. `create_loop_closure` stamped
//!   `is_aligned: strategy == Move` without measuring, one line after
//!   `compute_closure_metrics` had computed the refutation - and this module's
//!   generator draws `Move` for half its closures over an arbitrary cross-section
//!   pair, with `moveSection(+/-1000 m)` supplying the distance. Nothing could
//!   ever heal it: the flag was never set back to false, `realign_merges` looks at
//!   merges only, and `cascade_connection_offsets` deliberately preserves a
//!   redundant closure's residual.
//!
//!   FIXED by making the flag a MEASUREMENT re-derived after every geometry
//!   change (`Project::closure_alignment_stamp` /
//!   `remeasure_closure_alignment`, models/project/offsets.rs), which is safe to
//!   run on an inverse because the stamp is total and blind to the flags it
//!   computes - see that function and docs/undo-redo.md. `COMBINED=1 SEEDS=40
//!   ITERATIONS=300` is 40/40 on both halves after it, and the reductions are
//!   pinned in `open_undo_bugs_tests.rs` (six tests, from the birth route to the
//!   save/reopen convergence) and `models/project/tests.rs` (three).
//! * RESOLVED as FINDING 20, and now FIXED. `HORIZON=1 SEEDS=1 ITERATIONS=2000`
//!   left a merged pair 1.58 cm apart after an `updateShot`, because
//!   `realign_after_shot_edit` returned early when the edited section's merge
//!   group held no zero datum and the epilogue's `realign_merges` only re-solved
//!   past the 5 cm REPORTING threshold, while the app's own `#[cfg(test)]`
//!   invariant for the same pair was 1 cm. Both halves are closed: an unanchored
//!   group is now realigned against the edited section, and the heal gate and the
//!   assert are one constant (`Project::MERGE_ALIGNMENT_INVARIANT_M`), so the app
//!   cannot decline to repair a state it also asserts cannot exist. The
//!   classifier is deleted and a drifted merge FAILS again, at any magnitude and
//!   anchored or not.
//! * RESOLVED as a MODEL FIX, and since made moot. Seeds 2, 7 and 8 were
//!   described as failing on finding-3-shaped `section_offsets` values the
//!   cascade classifier no longer matched. Half of that was true and was fixed:
//!   the classifier froze one value where finding 3 RATCHETS, so it stopped
//!   matching after the second cycle; following the trajectory and requiring BOTH
//!   sides to be on it took the survey from 35 to 39 clean seeds of 40 at 100
//!   moves. Finding 3 is now FIXED at the source, so there is no trajectory and
//!   no classifier - a `section_offsets` key gaining real geometry across a
//!   history move fails again, and all 40 seeds are clean at 100.
//! * RESOLVED as a MODEL FIX, and since made moot as well. `COMBINED=1
//!   ITERATIONS=300` failed seeds 6 and 35, both halves, on
//!   `zero_datums[#].gps.latitude` - a signature no other regime produced, which
//!   is what made it look like a new defect. It was finding 17's own signature:
//!   `zero_datums[0]` and `[1]` SWAPPED, value for value, on the undo of a
//!   `remove_zero_datum`. What stopped it being reached was the CHAIN - findings
//!   4 and 17 are one defect on two lists, one comparison can hit BOTH, and the
//!   two subtractions sat behind a single slot, so whichever matched first took
//!   it and the other was left unexplained. They were subtracted TOGETHER after
//!   that, and both are FIXED now: the undo of `RemoveLoopClosure` and of
//!   `RemoveZeroDatum` re-insert at the recorded index, so neither list can come
//!   back permuted and `diffs_without_list_reorders` is deleted with them.
//! * RESOLVED as FINDING 21, and now FIXED. The adjusted-geometry oracle's
//!   28-micron divergence over a byte-identical project was real, and the
//!   recorded hypothesis (`station_offsets` / `station_comments` hash order
//!   feeding `resolve_offsets`) was wrong in every clause: `station_comments`
//!   never feeds geometry, `station_offsets` is EMPTY in every run this module
//!   makes, and `resolve_offsets` is not on `get_adjusted_stations`' path and
//!   sorts its cascade seeds anyway. The mechanism was one level down, in
//!   `Project::adjust_least_squares`: it numbered its unknowns by iterating
//!   `groups.keys()` over a `HashMap` built fresh inside the call, and the
//!   reduction then peeled in index order and summed over `HashMap` neighbour
//!   sets. It now numbers from a SORTED sequence and reduces over a `BTreeMap`,
//!   so the solve is bit-identical run to run and the oracle compares with the
//!   absolute tolerance alone.
//! * RESOLVED as FINDING 25, and now FIXED. At horizon depths - seeds 2, 3, 4 and
//!   5 somewhere past depth 1000 - an `undo` translated a whole CONNECTED
//!   COMPONENT by a constant vector, with one section of it landing on exactly
//!   `[0, 0]` (measured on seed 2 at depth 1189: sections 65 and 81 both off by
//!   `(22230.0946076, -10480.744175)`, 81 on the origin).
//!
//!   `fuzz_shrink_tests.rs` took the 1770-move trace to SEVEN moves in 497
//!   probes - `createSection(0 shots)`, `createShot(append)`,
//!   `createSection(2 shots)`, `setZeroDatum`, `deleteShot`, `mergeStations`,
//!   `updateShot` - and the reduction off that is four commands with no history
//!   depth in it at all. The mechanism: `zero_datums[0]` IS the project's
//!   coordinate origin, because every later datum is placed at
//!   `gps_to_local(gps, zero_datums.first().gps)` and only the first sits at the
//!   local origin. Removing the primary datum - directly, or by deleting the shot
//!   whose station it is anchored to - therefore moved the origin, and nothing
//!   re-solved for it: `resolve_offsets` deliberately never touches an anchor's
//!   own offset. The survivor kept the offset it held while it was SECONDARY, and
//!   the next unrelated `update_shot` re-derived the rule against the new first
//!   datum and rigidly translated the whole component - 6.4 km in the reduction.
//!
//!   FIXED: the realign now HOLDS the datum station where the edit found it
//!   (`Project::datum_anchor_position`, measured before the edit), which is the
//!   same position for a self-consistent project. The three
//!   `..._primary_zero_datum...` / `..._re_anchored_a_component...` tests in
//!   `open_undo_bugs_tests.rs` pin it and are no longer ignored. What the fix
//!   does NOT address, and what stays open on #413, is the frame question
//!   underneath: removing the first datum still re-defines the world origin
//!   silently, so the survey is left stably mis-georeferenced (Ariane export
//!   roots on `zero_datums.first()`). That wants a renormalisation on datum
//!   removal, which moves every offset and so needs its own capture - a geometry
//!   change, not an undo fix.
//! * RESOLVED as a DUPLICATE of FINDING 12, cascading, and now FIXED with it.
//!   Seed 10 under `crossings: 3` found three fields of one shot changed when it
//!   undid a `set_shot_kind` entry whose own persisted id was MEASURED `Fresh` -
//!   which is what made it look like none of findings 8, 12 or 13.
//!   `fuzz_shrink_tests.rs` cut the 59-move run to THREE moves in 105 probes:
//!   `createShot(s=5,before)`, `setShotKind(6:600001,Jump)`,
//!   `updateShot(5:500001)`. The insert-BEFORE makes the allocator's ids disagree
//!   with positional order, the reload renumbers them, the undo of `update_shot`
//!   writes the pre-edit values onto the NEIGHBOUR, and the failure is one move
//!   later where the entry being consumed is one whose id did not move: the
//!   corruption sits in the PROJECT, not in the entry. Exactly finding 15's
//!   relationship to finding 3. Fixed at the source with findings 8/12/13/16 -
//!   entries address their shot by `Shot::uuid` - so seed 10 is back in the
//!   multi-crossing gate (`MULTI_CROSSING_SEEDS` is `GATE_SEEDS` again) and
//!   `unwinding_past_a_stale_id_edit_leaves_no_shot_holding_it` is no longer
//!   ignored.
//! * TRIAGED OUT, and not a defect in the app: the METAMORPHIC engine's
//!   `observation-split-routes-disagree-on-station-comment-keys` - the two
//!   delete/split routes intermittently disagreeing about which section a station
//!   comment is keyed to. Every reported instance turned out to hold the SAME
//!   multiset of `(section_id, comment)` pairs on both sides with one entry - the
//!   newly minted section B - in a different slot, i.e. a PERMUTATION rather than
//!   a re-key. `canonical_snapshot` sorts `station_comments` by the pair's whole
//!   JSON text, which for a `StationKey::AfterShot` anchor includes the shot
//!   uuid; both routes mint section B's shots, and `Mask::MintedUuids` erased the
//!   uuids only AFTER the sort had used them. Measured: 40 seeds x 6 repeats gave
//!   7 divergences over 240 built instances before, 0 over the same 240 with a
//!   re-sort after the mask, every other pair byte-identical. The record and the
//!   fix are in `metamorphic_undo_tests.rs` (`resort_set_like`); the earlier
//!   `shots.rs:772` lead stays refuted.
//!
//! ## Modes, and why the gate uses none of them
//!
//! | env | what it adds | why it is not in the gate |
//! |---|---|---|
//! | `JUMPS=1` | multi-step `undo_to`/`redo_to` | was finding 7; now gated on its own by `bug_7_a_jump_is_n_single_steps` |
//! | `CLOSURES=1` | a real loop closure in the starting project | was findings 6 and 14; 14 is now gated on its own by `finding_14_no_insert_leaves_a_merge_out_of_alignment` |
//! | `DIVERGENCE=1` | the id-vs-position divergence findings 8/12/13 need, built | deliberate corruption |
//! | `CLOSURE_SHAPES=1` | the two closure shapes findings 4 and 10 need, built | deliberate, and implies `CLOSURES` |
//! | `HORIZON=1` | thousands of moves, past the app's own history limit: the FIFO trim and its sliding window, accumulated float dust, allocator and growth sanity | minutes per seed AT THE APP'S OWN LIMIT - a DRAWN limit reaches the same window in 300 moves, which is what the combined gate uses |
//! | `CROSSINGS=n` | n save/close/reopen boundaries per persistence run instead of one: heals stacking on heals, and a snapshot compared across two epochs | IS gated - see `undo_redo_fuzz_gate_multi_crossing` |
//! | `COMBINED=1` | every one of the above at once (bar `DIVERGENCE`), over a drawn history limit | IS gated - see `undo_redo_fuzz_gate_combined` and "The COMBINED regime" below |
//! | `Plan::prologue` | replay `(seed, moves)` as a corpus prologue, then continue with fresh randomness | measured NOT to widen coverage - see `fuzz_corpus_sweep` |
//!
//! A mode that CONSTRUCTS a known defect cannot also be a regression gate: its
//! hit counts say nothing about whether the model acquired a new way to be
//! wrong, which is the only thing the gate is for. That argument is about a mode
//! that BUILDS a shape and it survives every finding being fixed; the modes stay
//! out of the plain gate for it.
//!
//! The combined gate does not break that rule, it applies it one level up: what
//! it guards is the INTERACTIONS - a trimmed history being reloaded, a jump
//! taken over a sliding window. It used to pin the id SET it reached; every one
//! of those ids is fixed, so the pin is INVERTED and it now asserts that the
//! regime reaches NOTHING, with the three structural properties (it trimmed, it
//! crossed more than once, it jumped) asserted one by one so a regime that
//! stopped combining cannot pass by finding nothing.
//!
//! ## The knobs that are not modes
//!
//! | env | what it does | default |
//! |---|---|---|
//! | `SEEDS=n` | seeds `1..=n` (every sweep runs each seed in BOTH modes, so `n` seeds are `2n` runs) | per sweep: 40 survey, 50 soak, 2 horizon |
//! | `ITERATIONS=n` | moves per run | per sweep: 60 survey, 1000 soak, 10000 horizon |
//! | `THREADS=n` | workers the sweep runs its seeds on - see below | `available_parallelism` less one core |
//! | `TRACE=1` | stream every move as it happens (and pin one worker, so the stream stays a stream) | off |
//! | `VERBOSE=1` | every differing path rather than the first eight, and the whole panic payload per seed in the survey | off |
//! | `PERSISTENCE=1` | horizon only: cross the save boundary as well | off |
//! | `CONFIG=1` | draw this run's undo-relevant CONFIG - see below | off |
//! | `SHRINK=`, `REPLAY=` | the shrinker's own knobs - see below, and `fuzz_shrink_tests.rs` | off |
//!
//! ## The CONFIG space: fuzzing the app, not just the moves
//!
//! Every run before this one drove a DEFAULT app. The move stream was fuzzed and
//! the settings the model runs against were fixed, which left one whole axis
//! unexercised: `undo_history_limit` is a user-facing number in Settings, and at
//! 1 it changes what an undo stack IS - every push trims, the stack length never
//! moves, and a walk can never get more than one step back.
//!
//! `CONFIG=1` (or `Plan::config_fuzz`) draws it per seed from
//! [`config_history_ladder`]: 1, 2, 3, 5, 8, 13, 20 (the persisted window), 40
//! (the ordinary modes' depth cap), 100 (the app's own default, so the drawn
//! regime CONTAINS the undrawn one) and `iterations + 1` (larger than the run can
//! reach - the control). See [`DrawnConfig`] for what is NOT drawn and why:
//! `undo_persist_limit` cannot be, because writing it into the config struct
//! would persist it into the developer's real `config.toml`.
//!
//! Three properties make it safe to add to a file whose whole value is that its
//! seeds are repros:
//!
//! * **The draw is ADDITIVE.** It comes from a second generator seeded
//!   `seed ^ CONFIG_STREAM`, so no primary stream moves and every row of the
//!   matrix above still reproduces. Measured, not asserted: with the knob
//!   compiled in, all three gates and both spot-checked repros
//!   (`DIVERGENCE=1 SEEDS=1 ITERATIONS=12`, `SEEDS=13 ITERATIONS=60`) print
//!   byte-identical lines to the run before it.
//! * **The drawn value is in the run header.** Every summary line and every
//!   failure payload ends with `undo_history_limit=n` when the knob is on, and
//!   nothing when it is off, because a seed alone is not a repro once the same
//!   moves can run against differently configured apps.
//! * **The oracles were generalized rather than special-cased.** `absorb_trim`
//!   and `expect_stacks` were already written against `self.history_limit`;
//!   what was NOT general is in `crossing_would_truncate` (`top` stops being the
//!   entry count once the trim is active) and in `record_mutation`'s push
//!   detection, whose newest-entry FINGERPRINT is the only thing that can see a
//!   push at a limit of 1 - a length delta reads it as a no-op. That fingerprint
//!   carries the entry's timestamp, and `UndoStack::next_stamp` keeps stamps
//!   strictly increasing (it bumps by a nanosecond on a tie), so two consecutive
//!   pushes of the same command cannot look like one entry sitting still.
//!
//! Measured at 32 seeds x 60 moves, all ten rungs drawn and all 64 runs clean;
//! four seeds drew 1 and trimmed 19 to 26 entries each while still crossing the
//! save boundary. At 64 seeds x 200 moves, 59 are clean and the five failures
//! carry the same signatures the UNDRAWN survey produces at that length
//! (`section_offsets.#[#]`, `station_comments[#][#].anchor`,
//! `sections[#].shots[#].length`) - three of them at a drawn limit of 100, which
//! is the default, so the knob re-shuffles the space rather than manufacturing
//! failures of its own.
//!
//! ## The COMBINED regime
//!
//! The modes above are exercised one at a time, which is how they stay readable
//! and is also how their INTERACTIONS went unexercised. `COMBINED=1` runs
//! horizon, closures, closure shapes, jumps, three crossings and a drawn history
//! limit in one run - and combining them needed three conflicts resolved. Each
//! resolution is conditional - on `Plan::combined`, or on the drawn config it
//! implies - so every mode that existed before is byte-identical.
//!
//! 1. **A horizon run could never jump.** `run_fuzz`'s climb test compares
//!    `cursor - floor` against `history_limit * 2`, but `absorb_trim` pins
//!    `cursor - floor` at exactly `history_limit` from the first trim onwards -
//!    so the difference never reaches the cap, a horizon run counts as
//!    "climbing" for its whole life, and the `plan.jumps && !climbing` guard
//!    means `HORIZON=1 JUMPS=1` draws ZERO jumps however long it runs. (Visible
//!    in any horizon summary line: `0 jumps`.) The combined regime ends the
//!    climb where it was meant to end - when the window starts sliding, which is
//!    `floor` moving off zero - and the gate's seed 21 draws 30 jumps and
//!    reaches finding 7 because of it.
//! 2. **A horizon run could never cross.** `cross_boundary` refused whenever
//!    `top > UNDO_PERSIST_LIMIT`, and a horizon run passes 20 within its first
//!    twenty mutations, so `HORIZON=1 PERSISTENCE=1 CROSSINGS=3` did nothing at
//!    all: measured at 8 seeds x 300 moves, `0 saves, 0 reloads` on every one.
//!    The guard is not wrong, it is stated in the wrong currency - `top` counts
//!    DEPTHS, and the thing that has to fit through the file is ENTRIES. With a
//!    drawn limit the two differ by everything the trim has dropped, so
//!    `crossing_would_truncate` measures per stack, and the gate's seeds cross
//!    three times each at depths of 75 and 127.
//! 3. **`DIVERGENCE=1` cannot join.** Its prologue crosses the boundary itself
//!    and `boundary_iterations` returns NO boundaries when it is on, so adding
//!    it would silently delete the multi-crossing half of the regime. It is also
//!    the one mode that corrupts by construction. It stays out, and
//!    `combined_plan` says so.
//!
//! ## The trace, and the shrinker
//!
//! A run's move stream is RECORDABLE and REPLAYABLE, which is what
//! `fuzz_shrink_tests.rs` minimizes a failure with. Three things in here make
//! that work, and they are worth knowing before changing a generator:
//!
//! * **The generators DRAW, they do not execute.** `draw_op` returns an [`Op`] -
//!   a value with every reference symbolic - and `apply_op` is the ONLY thing
//!   that runs a command. A replayed `Op` goes through the same `apply_op`, so
//!   there is no second execution path for a shrunk trace to differ from, and
//!   the per-move metadata the failure note reads (`target`, `anchor`) is
//!   computed once, in one place.
//! * **A trace names what survives being shrunk.** Shots travel as
//!   `Shot::uuid`, because `Shot::id` is renumbered by the loader and re-issued
//!   by the allocator; sections and closures travel as ids, PAIRED between the
//!   recorded run and the replay through what each move created ([`Created`],
//!   [`Refs`]). A reference that no longer maps makes its move inapplicable, and
//!   an inapplicable move is skipped rather than fatal.
//! * **A trace holds the move stream and nothing else.** The starting project,
//!   the prologues and the closing `finish()` walk are functions of the `Plan`,
//!   so a replay re-runs them (`start_run` / `finish_run`) and no shrink step can
//!   delete them. Each recorded move also carries the `Rng` state it ran at, so a
//!   crossing that draws (finding 10's orphan shape) draws the same values.
//!
//! Recording is OFF unless armed (`recorder_arm`), so no sweep pays for it, and
//! `a_recorded_run_matches_an_unrecorded_one` measures that rather than asserting
//! it in prose. `a_full_trace_replays_to_the_same_run` pins the case with a known
//! answer: a complete trace replays to exactly the run it came from.
//!
//! ## Parallel sweeps, and why the output does not change
//!
//! `undo_redo_fuzz_soak`, `fuzz_seed_survey` and `undo_redo_fuzz_horizon` run
//! their seeds ACROSS THREADS (`run_seed_jobs`). Measured on this module at 32
//! seeds x 300 moves x 2 modes: 13.9 s at one worker, 3.7 s at four and 2.7 s
//! at eight, with byte-identical output at every count.
//!
//! It is sound for the reason the whole module is cheap to reason about: a run
//! is a PURE FUNCTION of `(seed, Plan)`. The `Rng` is integer-only and lives in
//! the `Fuzzer`, every `AppState` is built by the run that uses it, a
//! persistence run saves into its own `TempDir`, and the two engine-level env
//! knobs are latched once per process (`fuzz_verbose`, `fuzz_trace`) rather than
//! re-read per move. Seeds therefore share NO mutable state, and the results are
//! merged in SEED ORDER, so what gets printed is a function of the seeds and not
//! of the scheduler. `THREADS=1` is not a separate path - it is the same runner
//! with one worker - so a difference between one worker and eight is a bug in
//! `run_seed_jobs`, not a mode of the fuzzer.
//!
//! Three consequences worth knowing before reading a parallel run:
//!
//! * `summarize`'s `oracle-us` triple is WALL TIME, so it inflates under
//!   contention. The cost model above was measured at one worker and re-measuring
//!   it needs `THREADS=1`.
//! * A panicking seed is caught, attributed to its `(seed, mode)` and re-raised
//!   after every other seed has finished. The soak used to stop at the first
//!   one; at soak scale that threw away the rest of the evidence.
//! * No sweep prints its worker count, deliberately: two runs at different
//!   thread counts have to diff clean.
//!
//! The three GATES stay sequential. They are seconds, `cargo test` already runs
//! them alongside every other test in the binary, and a gate that spawns a pool
//! of its own would be competing with libtest's.
//!
//! # What it does NOT cover
//!
//! Everything above the `AppState` boundary: the projectStore actions, the IPC
//! serialization (notably serde's double-option, which is how bug 5 was found),
//! and the frontend's own bookkeeping. Those are the harness's job and the
//! reason it stays.
//!
//! # Five oracles, not one
//!
//! | oracle | what it compares | what only it can see |
//! |---|---|---|
//! | ROUNDTRIP | a canonical serialized `Project` per undo depth | anything stored that a history move fails to restore |
//! | ALIGNMENT | every cross-section merge, RAW and RESOLVED | a move that leaves the survey network geometrically broken, and a disagreement between the app's two measures of that |
//! | INTEGRITY DELTA | `AppState::integrity_check`'s own verdict per depth | a state that roundtrips while the app's verdict on it does not |
//! | ADJUSTED GEOMETRY | `get_adjusted_stations` per depth - what the map draws | a SOLVE whose result moved, including through a stale memo, while the stored inputs agree |
//! | ENTRY CONTENT | the `UndoHistoryItem` rows themselves - what the history dropdown SAYS | an entry that misdescribes itself, whatever its effects do |
//!
//! The middle two are ports of the harness's, and the integrity one is there
//! because at that tier it caught a finding 7 manifestation ALONE: the state
//! compared equal (the geometry paths were absorbed as reload-resolved across an
//! epoch boundary) while `check_project_integrity` reported a merge 5.00 m out.
//!
//! They cost little next to the roundtrip oracle they join. Measured over seed 1
//! at 60 moves, 59 comparisons: the project snapshot is ~1.8 ms per comparison,
//! the integrity verdict ~81 us (4.6% of it) and the adjusted solve ~208 us
//! (12%). Both run on EVERY comparison; no checkpointing was needed, and the
//! per-run totals are printed in the summary line so the number stays honest as
//! the model grows.
//!
//! The consistency rule they need is in `expect_derived`: when a classified
//! known bug resyncs the state model, the verdict and the adjusted geometry
//! resync at the same moment. A derived model that kept the pre-bug expectation
//! would report the same defect at every later depth as a fresh failure.
//!
//! ## The ENTRY-CONTENT oracle
//!
//! Everything above measures what a history move DOES. None of it looks at what
//! the entry SAYS - and the entry's metadata is a user-facing artifact in its own
//! right: `get_undo_stack` returns an `UndoHistoryItem` per entry
//! (`commands/history.rs`), carrying the description, the timestamp, the command
//! type and a `preview_data` tree of before/after field changes, and that list IS
//! the history dropdown. An entry whose label is wrong sends a user to undo the
//! wrong thing; an entry whose preview is wrong tells them a field was changed
//! that was not.
//!
//! It runs in two halves, because the two things it can compare are pinnable to
//! different degrees:
//!
//! * **WITHIN-RUN** (`expect_entry_prints`, every mode, on every move via
//!   `expect_stacks`). `entry_prints[d]` records `timestamp|command_type|
//!   description` when the app pushes the entry at depth `d`, and every later
//!   reading of either stack has to agree with it, entry for entry, at the depth
//!   the stack layout puts it at. All three are pure functions of the stored
//!   `UndoEntry` - `Command::description()` reads the command and nothing else,
//!   and `shot_display_position` is arithmetic on the ids the command carries -
//!   so nothing a later command does to the project may change them. That is
//!   what makes an undo/redo cycle that rewrites the metadata of an entry it
//!   never touched, or two entries silently swapping places, a failure here.
//! * **PERSISTENCE** (`expect_history_rows`, at every crossing). The WHOLE item,
//!   `preview_data` included, is read before the save and compared after the
//!   reload, on both stacks, with nothing masked - not the timestamps (they are
//!   parsed back from a column, not re-taken, and #303's push cutoff compares
//!   against them), and not the ids (they ride inside the serialized command, so
//!   the loader's shot-id renumbering cannot reach them).
//!
//! `preview_data` is in the second half only, and that asymmetry is deliberate
//! rather than a gap: it is a LIVE VIEW. `derive_preview_data` resolves section
//! NAMES against the current project, so renaming a section - or undoing a
//! rename - legitimately changes what an older entry's preview reads. It is
//! stable exactly where the project is, which the save/reload boundary is by
//! construction and a history move is not.
//!
//! It found two things on its first run, both of them existing findings reaching
//! a surface nothing else was looking at, and both were classified rather than
//! fixed at the time:
//! `finding-5-a-reloaded-command-can-no-longer-clear-a-section-field` and
//! `finding-23-history-float-dust-moves-a-displayed-value`. Both are fixed now
//! and both classifiers are deleted - see the matrix rows above - so a history
//! row that reads differently after a reload than it did before the save FAILS
//! the run, with the entry's `command` and `undo_data` bytes in the payload.
//!
//! # The oracle sees MORE than the harness one
//!
//! The harness compares `get_project_state`, which is `ProjectIpc` - and that
//! type omits `station_offsets` and `station_comments` entirely (they are only
//! observable there through derived `stations[]` entries). This module compares a
//! serialized `Project`, so those two maps are IN the comparison. That was a
//! deliberate blind spot to close.
//!
//! # Data
//!
//! Everything is generated in code. No fixture is read, copied or derived from -
//! CI has none, and the repo forbids adding any.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom"
)]
#![expect(
    clippy::print_stdout,
    reason = "a fuzzer's per-seed summary IS its output: the move and oracle counts are what a               reviewer reads to see the gate is still gating, and `cargo test -- --nocapture` is               the only channel a test has"
)]
#![expect(
    clippy::panic,
    reason = "the fuzzer's failure payload (seed, depth, move log, first diffs) is far more useful               than any assert! message, and a panic IS how a test fails"
)]
#![expect(
    clippy::needless_pass_by_ref_mut,
    reason = "the generators take &mut Fuzzer uniformly; some branches happen not to draw, and               splitting the signature by branch would make the generator table unreadable"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use tempfile::TempDir;

use super::AppState;
use crate::core::undo::history_view::UndoHistoryItem;
use crate::core::{Command, CommandResult};
use crate::models::{
    ClosureStrategy, Coordinates, GpsCoordinates, SectionId, SectionUpdates, ShotData, ShotId,
    ShotKind, ShotUpdates, StationRef, SurveyDirection,
};
// CONFIG ISOLATION. Every test in this module opens with
// `let _config = isolated_config();`: `AppState::new()` LOADS the global config,
// and every save reaches `note_recent_project` -> `Config::save()`, which WRITES
// it. Config resolution is per-thread (`crate::test_helpers` documents what that
// does and does not protect, and why it is not opt-in), so the guard is no longer
// what keeps these tests off other files; what it buys is ONE dir for the whole
// test, whose path the test can name and hand to a worker thread
// (`ConfigHandoff`). It is held for the whole test because the saves are.
use crate::test_helpers::isolated_config;

// ===========================================================================
// Deterministic PRNG
// ===========================================================================

/// mulberry32, integer-only.
///
/// The same generator the TypeScript harness uses (`scenarios/framework.ts`), so
/// the two tiers share a mental model, and no new dependency: nothing in
/// `Cargo.toml` gives test code a PRNG, and adding one to gate a test would be a
/// poor trade.
///
/// Determinism is the whole contract, so every draw is integer arithmetic on
/// `u32` with explicit wrapping. No float is used to MAKE a decision anywhere in
/// this module - `f64` values are generated as data, never used to branch - which
/// keeps a run bit-identical across platforms and optimization levels.
pub(super) struct Rng {
    state: u32,
}

impl Rng {
    pub(super) const fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// Next raw 32-bit value.
    fn next_u32(&mut self) -> u32 {
        self.state = self.state.wrapping_add(0x6d2b_79f5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 0b11_1101));
        t ^ (t >> 14)
    }

    /// Uniform in `0..bound` (`bound` > 0). Modulo bias is irrelevant here and
    /// costs nothing to keep: the alternative (rejection sampling) makes the draw
    /// count depend on the values drawn, which makes a seed's trajectory harder
    /// to reason about when a generator changes.
    pub(super) fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next_u32() as usize) % bound
    }

    /// Inclusive integer range.
    pub(super) fn range(&mut self, min: i64, max: i64) -> i64 {
        if max <= min {
            return min;
        }
        let span = (max - min + 1) as usize;
        min + self.below(span) as i64
    }

    /// True with probability `percent`/100.
    pub(super) fn chance(&mut self, percent: u32) -> bool {
        (self.next_u32() % 100) < percent
    }

    /// A float in `[min, max]`, quantized to 1/1000 so the value is a function of
    /// an INTEGER draw. Generating it from the raw u32 as a fraction would make
    /// the produced value depend on f64 rounding; this cannot.
    pub(super) fn float(&mut self, min: f64, max: f64) -> f64 {
        let steps = self.range(0, 1000) as f64;
        min + (max - min) * (steps / 1000.0)
    }

    pub(super) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// The generator's WHOLE state, which is one `u32`.
    ///
    /// A recorded move carries it so a replay can put the generator back where
    /// the recorded run had it. That matters for exactly one thing today - a
    /// boundary crossing under `Plan::closure_shapes` builds finding 10's orphan
    /// shape and DRAWS while doing it - and it costs four bytes to be right
    /// about every future one too: a replay makes none of the draws the recorded
    /// run's generators made, so without this the stream at a crossing would be
    /// whatever the prologue left behind.
    const fn state(&self) -> u32 {
        self.state
    }

    const fn restore(&mut self, state: u32) {
        self.state = state;
    }
}

/// Largest magnitude any generated value that feeds coordinate arithmetic may
/// take.
///
/// Ported, not re-derived. The harness spent a finding on this (issue #413,
/// finding 9): a 1e6 m displacement puts coordinates in a regime where
/// re-deriving `section_offsets` loses ~4.6e-6 m ABSOLUTE to catastrophic
/// cancellation, three orders above the oracle's 1e-9 floor, so it reads as a
/// roundtrip failure when it is really the injected input. The bound goes on the
/// INPUT rather than on the oracle, which would have blinded it to real
/// sub-centimetre drift.
pub(super) const GEOMETRY_EXTREME: f64 = 1e3;

// ===========================================================================
// The snapshot oracle
// ===========================================================================

/// Absolute tolerance, a nanometre. Geometry is RE-DERIVED after every undo/redo
/// rather than copied, so a value whose true result is 0 can land on different
/// dust depending on the order the survey chain was summed in. A real geometry
/// regression is millimetres at the very least.
const TOLERANCE_ABS: f64 = 1e-9;
/// Relative tolerance, on top of the absolute one.
const TOLERANCE_REL: f64 = 1e-12;

/// Fields no history move can be expected to restore, each an artefact rather
/// than state:
///
/// * `modified_at` - every undo/redo path calls `Project::touch()`.
/// * `file_path` - `serialize_project_snapshot` scrubs it, and
///   `restore_snapshot_into` carries the LIVE path across a restore.
/// * the id allocators - monotonic counters, and an ASYMMETRY rather than a bug:
///   a snapshot-pair undo restores them verbatim while a targeted undo only ever
///   raises them, so comparing them would fire on every mixed history.
fn drop_key(path: &str) -> bool {
    matches!(
        path,
        "modified_at"
            | "file_path"
            | "next_section_id"
            | "next_closure_id"
            | "sections.*.next_shot_id"
    )
}

/// `station_offsets` and `station_comments` serialize as a Vec of `(key, value)`
/// pairs built from a `HashMap`, so their ORDER is not stable across a restore.
/// Sorting them canonicalizes that without hiding a value - the same treatment
/// the harness gives `section_offsets` (whose JSON-object form serde_json already
/// sorts on parse).
fn sort_as_set(path: &str) -> bool {
    matches!(path, "station_offsets" | "station_comments")
}

/// Round to 12 significant digits, as the harness's canonical serializer does.
/// The fast path is string equality, so without this every re-derived coordinate
/// would take the slow tolerant path.
fn round_significant(value: f64) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }
    format!("{value:.11e}").parse::<f64>().unwrap_or(value)
}

/// Canonical, comparison-ready JSON for a serialized `Project`.
///
/// `id_agnostic` replaces every `Shot::id` with its POSITION. `Shot::id` is not
/// persisted - the loader renumbers to `section_id * FEATURE_ID_STRIDE +
/// sequence_order` - so once a run has crossed a save/reload, comparing ids
/// against a pre-save snapshot would report the renumbering as drift. Everything
/// else stays compared exactly, and a run that never saves never turns this on.
pub(super) fn canonicalize(
    value: &serde_json::Value,
    path: &str,
    id_agnostic: bool,
) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let child_base = if path.is_empty() {
                String::new()
            } else {
                format!("{path}.")
            };
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                let child_path = format!("{child_base}{key}");
                if drop_key(&child_path) {
                    continue;
                }
                let mut canonical = canonicalize(child, &child_path, id_agnostic);
                if sort_as_set(&child_path) {
                    if let serde_json::Value::Array(items) = &mut canonical {
                        items.sort_by_key(ToString::to_string);
                    }
                }
                out.insert(key.clone(), canonical);
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            let child_path = if path.is_empty() {
                "*".to_string()
            } else {
                format!("{path}.*")
            };
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let mut canonical = canonicalize(item, &child_path, id_agnostic);
                if id_agnostic && child_path == "sections.*.shots.*" {
                    if let serde_json::Value::Object(shot) = &mut canonical {
                        shot.insert("id".to_string(), serde_json::Value::from(index));
                    }
                }
                out.push(canonical);
            }
            serde_json::Value::Array(out)
        }
        serde_json::Value::Number(number) => number.as_f64().map_or_else(
            || value.clone(),
            |raw| {
                serde_json::Number::from_f64(round_significant(raw))
                    .map_or_else(|| value.clone(), serde_json::Value::Number)
            },
        ),
        other => other.clone(),
    }
}

/// One differing path between two canonical snapshots.
#[derive(Debug, Clone)]
pub(super) struct Diff {
    pub(super) path: String,
    pub(super) expected: String,
    pub(super) actual: String,
}

const ABSENT: &str = "<absent>";
/// Diffs collected before the walk gives up. Enough to classify and to read.
const MAX_DIFFS: usize = 60;
/// The walk collects ONE more than it reports.
///
/// That one extra entry is the whole difference between "there are exactly
/// `MAX_DIFFS` differences" and "there are at least `MAX_DIFFS` and the rest
/// were never looked at" - and the classifiers can only speak for what they
/// saw, so the caller has to be able to tell those apart. See `diff_snapshots`.
const WALK_DIFFS: usize = MAX_DIFFS + 1;

fn numbers_equal(a: f64, b: f64) -> bool {
    if a == b {
        return true;
    }
    if !a.is_finite() || !b.is_finite() {
        return false;
    }
    (a - b).abs() <= TOLERANCE_ABS + TOLERANCE_REL * a.abs().max(b.abs())
}

fn show(value: &serde_json::Value) -> String {
    let text = value.to_string();
    // CHARS, not bytes. `adversarial_tests::hostile_string` deliberately puts
    // multi-byte UTF-8 (U+FFFD, U+202E, U+200B) into shot comments, and a byte
    // slice at 120 lands mid-character and panics - from the helper that was
    // about to PRINT the finding, so the seed's actual defect is lost and
    // reported as an unrelated crash in the harness.
    let chars = text.chars().count();
    if chars > 120 {
        let head: String = text.chars().take(120).collect();
        format!("{head}...[{chars} chars]")
    } else {
        text
    }
}

fn diff_values(
    expected: &serde_json::Value,
    actual: &serde_json::Value,
    path: &str,
    out: &mut Vec<Diff>,
) {
    if out.len() >= WALK_DIFFS {
        return;
    }
    let label = if path.is_empty() { "<root>" } else { path };
    match (expected, actual) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
            keys.sort_unstable();
            keys.dedup();
            for key in keys {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match (a.get(key), b.get(key)) {
                    (Some(left), Some(right)) => diff_values(left, right, &child, out),
                    (left, right) => out.push(Diff {
                        path: child,
                        expected: left.map_or_else(|| ABSENT.to_string(), show),
                        actual: right.map_or_else(|| ABSENT.to_string(), show),
                    }),
                }
                if out.len() >= WALK_DIFFS {
                    return;
                }
            }
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            if a.len() != b.len() {
                out.push(Diff {
                    path: format!("{label}.length"),
                    expected: a.len().to_string(),
                    actual: b.len().to_string(),
                });
            }
            for index in 0..a.len().min(b.len()) {
                diff_values(&a[index], &b[index], &format!("{path}[{index}]"), out);
                if out.len() >= WALK_DIFFS {
                    return;
                }
            }
        }
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            let (Some(left), Some(right)) = (a.as_f64(), b.as_f64()) else {
                if a != b {
                    out.push(Diff {
                        path: label.to_string(),
                        expected: a.to_string(),
                        actual: b.to_string(),
                    });
                }
                return;
            };
            if !numbers_equal(left, right) {
                out.push(Diff {
                    path: label.to_string(),
                    expected: show(expected),
                    actual: show(actual),
                });
            }
        }
        (left, right) => {
            if left != right {
                out.push(Diff {
                    path: label.to_string(),
                    expected: show(left),
                    actual: show(right),
                });
            }
        }
    }
}

/// Every numeric path that differs AT ALL, with the size of the difference.
///
/// The tolerant comparison reports nothing for a difference under
/// `TOLERANCE_ABS`, which is right per comparison and wrong over a run: a depth
/// visited a hundred times can be walked past the tolerance one grain at a time.
/// This is what the accumulation oracle remembers between visits.
fn numeric_drift(expected: &str, actual: &str) -> BTreeMap<String, f64> {
    let (Ok(left), Ok(right)) = (
        serde_json::from_str::<serde_json::Value>(expected),
        serde_json::from_str::<serde_json::Value>(actual),
    ) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    collect_drift(&left, &right, "", &mut out);
    out
}

fn collect_drift(
    expected: &serde_json::Value,
    actual: &serde_json::Value,
    path: &str,
    out: &mut BTreeMap<String, f64>,
) {
    match (expected, actual) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            for (key, left) in a {
                if let Some(right) = b.get(key) {
                    let child = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    collect_drift(left, right, &child, out);
                }
            }
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            for (index, left) in a.iter().enumerate() {
                if let Some(right) = b.get(index) {
                    collect_drift(left, right, &format!("{path}[{index}]"), out);
                }
            }
        }
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            if let (Some(left), Some(right)) = (a.as_f64(), b.as_f64()) {
                let delta = (left - right).abs();
                if delta > 0.0 {
                    out.insert(path.to_string(), delta);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn diff_snapshots(expected: &str, actual: &str) -> Vec<Diff> {
    diff_snapshots_capped(expected, actual).0
}

/// The same walk, plus whether the cap HID differences.
///
/// Every caller that decides a mismatch is EXPLAINED needs the second half.
/// `classify` runs over the returned list, and "nothing unexplained is left" is
/// only a statement about that list - on a mismatch wider than the cap the walk
/// stops inside whichever root key it is in (they are visited in sorted order),
/// so damage in a later one is not merely unclassified, it was never seen.
/// Concluding "all explained" there is the vacuous green this whole module
/// exists to prevent.
pub(super) fn diff_snapshots_capped(expected: &str, actual: &str) -> (Vec<Diff>, bool) {
    let (Ok(left), Ok(right)) = (
        serde_json::from_str::<serde_json::Value>(expected),
        serde_json::from_str::<serde_json::Value>(actual),
    ) else {
        return (
            vec![Diff {
                path: "<unparseable>".to_string(),
                expected: expected.chars().take(80).collect(),
                actual: actual.chars().take(80).collect(),
            }],
            false,
        );
    };
    let mut out = Vec::new();
    diff_values(&left, &right, "", &mut out);
    let truncated = out.len() > MAX_DIFFS;
    out.truncate(MAX_DIFFS);
    (out, truncated)
}

pub(super) fn describe_diffs(diffs: &[Diff]) -> String {
    // Eight is enough to recognise a shape and short enough to read; a triage
    // run that needs the whole list sets `STALAGMIGHT_RUST_FUZZ_VERBOSE=1`
    // rather than editing this number, which is how the eight got chosen twice.
    let limit = if fuzz_verbose() { MAX_DIFFS } else { 8 };
    let shown = diffs.len().min(limit);
    let mut lines: Vec<String> = diffs[..shown]
        .iter()
        .map(|d| {
            format!(
                "    at {}\n      expected: {}\n      actual:   {}",
                d.path, d.expected, d.actual
            )
        })
        .collect();
    if diffs.len() > shown {
        lines.push(format!(
            "    ...and {} more differing path(s)",
            diffs.len() - shown
        ));
    }
    lines.join("\n")
}

// ===========================================================================
// The two derived-state oracles
// ===========================================================================

/// The app's OWN integrity verdict (`AppState::integrity_check`), normalized.
///
/// Ported from the harness tier, where this oracle earned its place by catching
/// a finding 7 manifestation ALONE: the state compared equal (the geometry paths
/// had been absorbed as reload-resolved across an epoch boundary) while
/// `check_project_integrity` reported a merge 5.00 m out. A verdict is part of
/// the state a history move is supposed to restore, and it is derived through a
/// different path than the snapshot - so it sees what the snapshot cannot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Verdict {
    save_blocked: bool,
    /// Violation messages, SORTED.
    ///
    /// The app sorts them by severity only (`commands.rs`), and that sort is
    /// stable - so within a severity the order is the order the checks pushed
    /// them, which for the `for_each_station_ref` traversal follows `HashMap`
    /// iteration and is not stable between runs. Sorting loses nothing the
    /// oracle wants (the SET of violations is the verdict) and is what makes it
    /// comparable at all.
    ///
    /// Values are kept VERBATIM otherwise, ids and distances included. Every id
    /// a message can carry - section id, station index, closure id, uuid -
    /// survives a save/reload unchanged, and the distances are quantized to a
    /// centimetre by the `{:.2}` in the messages themselves, so float dust
    /// cannot move one except exactly on a rounding boundary. Stripping them
    /// would hide "5.00 m apart" turning into "500.00 m apart", which is the
    /// kind of thing this exists to see.
    messages: Vec<String>,
}

impl Verdict {
    pub(super) fn describe(&self) -> String {
        format!(
            "save_blocked={} messages={:?}",
            self.save_blocked, self.messages
        )
    }
}

/// One merged pair, measured BOTH ways.
///
/// `check_merge_alignment` (models/project/integrity.rs) measures through
/// `resolved_station_offsets()`, which folds in closure adjustment;
/// `debug_assert_merge_alignment` (models/project/offsets.rs) measures
/// `compute_station_raw_coords`. Issue #413's finding 14 records that the two
/// DISAGREE about the same state - the assert panics at 6.97 m while the checker
/// reports nothing - and asks which is authoritative. This is the oracle that
/// mines that class instead of noting it.
///
/// The pair's two SECTION ids used to be carried here as well, for finding 20's
/// classifier to ask whether their merge group was datum-anchored. That
/// classifier is gone - every drifted merge fails now, anchored or not - so the
/// distance is the whole measurement again.
struct MergeMeasurement {
    label: String,
    raw: f64,
    resolved: f64,
}

/// Tolerance for the per-depth ADJUSTED-geometry comparison, in metres.
///
/// Deliberately not the raw oracle's 1e-9. The adjusted stations are the output
/// of a least-squares solve over the whole network, so an input difference the
/// raw oracle tolerates (1e-9 m) can come out of the solve amplified by the
/// condition number of the system. A micron is four orders of magnitude below
/// the smallest geometry change anyone can mean (a millimetre) and three above
/// what the raw comparison already lets through, which makes it tight enough to
/// catch a real regression and loose enough not to report the solver's own dust.
///
/// Accumulation past this bound over a long run is a FINDING, not noise - see
/// the horizon mode, which re-baselines expressly to detect it.
const ADJUSTED_TOLERANCE_M: f64 = 1e-6;
// There WAS a relative companion here, `ADJUSTED_TOLERANCE_REL = 1e-7`, and it
// existed for exactly one reason: the solve was not a function of the project,
// so a station 2.2 km from the origin could come back 28 microns (1.3e-8
// relative) from where the same depth had it. That is FINDING 21, and it is
// fixed - `adjust_least_squares` now numbers its unknowns from a SORTED sequence
// (`StationRef` is `Ord`) and sums over `BTreeMap`s, so re-solving one project
// returns the same bits every time (`the_adjusted_solve_is_a_function_of_the_project`
// in `open_undo_bugs_tests.rs`, no longer ignored). The constant's own doc
// comment said to delete it when that happened, so it is gone and the comparison
// is back to the absolute bound alone. Do not reintroduce it: a relative slack
// here would hide precisely the class of regression this oracle exists to catch.

/// The app's OWN merge-alignment tolerance, taken from the app rather than
/// copied: 5 cm, the distance past which `check_merge_alignment` calls a merged
/// pair broken.
const MERGE_ALIGNMENT_TOLERANCE_M: f64 = crate::models::Project::MERGE_ALIGNMENT_TOLERANCE_M;
/// Under this, a measure is calling the pair SOUND. The gap between the two
/// constants is deliberate: a pair sitting between 1 cm and 5 cm is not being
/// called sound by either measure, so a disagreement there is not a
/// contradiction and is not reported.
const RAW_RESOLVED_AGREEMENT_M: f64 = 0.01;

// ===========================================================================
// Known-bug classifiers
// ===========================================================================

/// A KNOWN OPEN BUG, tracked on <https://github.com/Stalagmight/Stalagmight/issues/413>.
///
/// These were NOT normalizations. The snapshots were still unequal and the run
/// still recorded the hit; classification existed so the gate could stay green
/// while eleven defects were open, and so a hit read as "finding 5 again" rather
/// than as a fresh mystery. Every matcher was narrowed by VALUE as well as by
/// path, because a path alone cannot tell "the anchors were lost" from "the
/// anchors changed", and only the first was the known bug.
///
/// **Delete a classifier when its bug is fixed.** The gate then fails on the next
/// hit, which is the regression guard the whole module exists to provide. Every
/// finding on the issue is fixed, so [`CLASSIFIERS`] is EMPTY and `classify`
/// answers `None` for every difference: nothing can bless a diff any more. The
/// type and the loop stay so the next open bug has somewhere to go.
struct Classifier {
    /// Finding id on issue #413.
    id: &'static str,
    matches: fn(&Diff, &MoveCtx) -> bool,
}

/// What produced the difference being classified.
///
/// The harness tier cannot see this (its bridge only knows the move's
/// DIRECTION), so its classifiers can only ever match on the path. Here the consumed
/// entries are a `get_undo_stack` read away, so a classifier can be narrowed to
/// the exact entry shape its bug needs instead of to a path alone.
///
/// It has been emptied out by the fixes rather than by design, and the fields
/// that went name the classifiers that read them: `entry` / `entries` (the
/// entries the move consumed, because a jump runs several restores in one move
/// and any of them can carry a finding's precondition) went with findings 1, 3,
/// 8 and 12's; `after_save` and `epoch` with finding 11's; `heal_reverted` with
/// the findings 1/11 cascade; `displaced` (the trajectory finding 3's RATCHET
/// had left in each section's `section_offsets` entry) with the cascade half of
/// finding 3; and `corrupted` / `section_ids` with finding 16's. `stale` is what
/// is left, and it is now read only by the failure note - the first classifier
/// that needs any of this again should put back what it needs.
#[derive(Clone, Copy)]
struct MoveCtx {
    /// Whether the shot ids the consumed entry addresses still address the
    /// shots they were recorded against - measured BEFORE the move ran.
    stale: StaleTargets,
}

/// What an in-session shot id an entry carries now addresses.
///
/// This is the fact findings 8, 12 and 13 are ABOUT: shot ids are not persisted
/// (the loader renumbers to `section_id * FEATURE_ID_STRIDE + sequence_order`),
/// undo entries reload as opaque JSON with no remapping, and the allocator hands
/// out ids from the same space - so a persisted id either still names its shot,
/// names a DIFFERENT one, or names nothing. The three outcomes are the three
/// findings, and knowing which one holds is what lets the classifiers below say
/// "finding 8" rather than "something about shots".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Staleness {
    /// The entry carries no id of this role, or the run has never reloaded.
    #[default]
    Absent,
    /// The id still addresses the shot it was recorded against.
    Fresh,
    /// The id addresses a DIFFERENT shot: findings 8 and 12's regime.
    Collides,
    /// The id addresses nothing: finding 13's regime.
    Missing,
}

/// The staleness of every id one entry carries, by role.
#[derive(Clone, Copy, Debug, Default)]
struct StaleTargets {
    /// The shot the entry's own restore names - the one a `CreateShot` undo
    /// deletes, or the one an `UpdateShot` / `SetShotKind` undo writes to.
    target: Staleness,
    /// The shot a `CreateShot` REDO anchors its re-insert to
    /// (`after_shot_id` / `before_shot_id`), which is a second, independent id
    /// on the same entry.
    anchor: Staleness,
}

impl StaleTargets {
    /// The more broken of two measurements, per role. A jump consumes several
    /// entries and one stale id among them is enough to explain the damage.
    fn worst(self, other: Self) -> Self {
        let pick = |a: Staleness, b: Staleness| match (a, b) {
            (Staleness::Missing, _) | (_, Staleness::Missing) => Staleness::Missing,
            (Staleness::Collides, _) | (_, Staleness::Collides) => Staleness::Collides,
            (Staleness::Fresh, _) | (_, Staleness::Fresh) => Staleness::Fresh,
            _ => Staleness::Absent,
        };
        Self {
            target: pick(self.target, other.target),
            anchor: pick(self.anchor, other.anchor),
        }
    }
}

// `MoveCtx` carried an `impl` block here, and five free helpers sat under it.
// All of them went with the classifiers that were their only callers:
//
// * `command_is` ("did the move consume an entry of this command type?") went
//   with findings 1, 8 and 12's classifiers;
// * `entry_predates_save` went with finding 11's;
// * `appended_into_empty` ("which section did a consumed `CreateShot` append the
//   FIRST shot into") and `is_finding_3_displacement` ("is this diff that
//   section sliding along the RATCHET's trajectory") went with FINDING 3's two
//   classifiers, and took `displacements_equal`, `offset_pair` and `offset_of`
//   with them;
// * `is_in_a_corrupted_section` went with finding 16's cascade, and took
//   `section_ids_of` and `section_of_path` with it.

/// `(section, element)` for a diff path inside `section_offsets`.
///
/// A whole key (`section_offsets.5`) is what an absent-vs-present difference
/// produces; an element (`section_offsets.5[0]`) is what a key that exists on
/// BOTH sides produces, because the differ recurses into the pair. The two
/// spellings are the same defect seen through a different starting state, and a
/// classifier that knew only the first silently stopped classifying as soon as
/// the section already had an offset.
fn offset_key_of(path: &str) -> Option<(SectionId, Option<usize>)> {
    let rest = path.strip_prefix("section_offsets.")?;
    match rest.split_once('[') {
        None => rest.parse().ok().map(|id| (id, None)),
        Some((id, index)) => {
            let index = index.strip_suffix(']')?.parse().ok()?;
            id.parse().ok().map(|id| (id, Some(index)))
        }
    }
}

/// What the entry pushed at one depth was.
#[derive(Clone, Debug, Default)]
struct EntryMeta {
    /// `UndoHistoryItem::command_type` of the entry that takes depth d-1 to d.
    command: String,
    /// Epoch the entry was pushed in.
    epoch: u32,
    // `appended_into_empty` - "this entry is a `CreateShot` that appended the
    // first shot into a previously EMPTY section" - was finding 3's exact
    // precondition and went with its classifiers.
    /// The shot this entry's own restore names by id, with the uuid that id
    /// addressed when the entry was pushed.
    target: Option<ShotTarget>,
    /// The shot a `CreateShot` entry's REDO anchors its re-insert to.
    anchor: Option<ShotTarget>,
}

/// A shot an undo entry addresses by its IN-SESSION id, recorded with the uuid
/// that id pointed at.
///
/// The uuid is the point. `Shot::id` is not persisted and `Shot::uuid` is, so
/// comparing "what does this id address now" against "what did it address then"
/// is a DIRECT measurement of the staleness findings 8, 12 and 13 describe -
/// rather than an inference from the damage afterwards.
#[derive(Clone, Debug)]
struct ShotTarget {
    section_id: SectionId,
    shot_id: ShotId,
    uuid: String,
}

// `is_null_or_false` and `is_absent_or_zero` lived here, read by the gauge-loss
// and orphan-zero-offset matchers respectively. Both went with their
// classifiers; they had no other callers.

/// Does `diff`'s path match `pattern`, where `#` stands for a run of digits?
///
/// Implemented by normalizing the path rather than by walking the pattern: every
/// array index and numeric map key becomes `#`, and the result is compared whole.
/// A prefix-walking version got this wrong for a trailing `#` (`section_offsets.5`
/// did not match `section_offsets.#`), which silently un-classified finding 2 -
/// the failure mode a clever matcher invites and a boring one does not.
fn path_is(diff: &Diff, pattern: &str) -> bool {
    let mut normalized = String::with_capacity(diff.path.len());
    let mut in_digits = false;
    for ch in diff.path.chars() {
        if ch.is_ascii_digit() {
            if !in_digits {
                normalized.push('#');
                in_digits = true;
            }
        } else {
            normalized.push(ch);
            in_digits = false;
        }
    }
    normalized == pattern
}

/// The classified set, in match order. EMPTY: every finding it named is fixed.
const CLASSIFIERS: &[Classifier] = &[
    // FINDINGS 1 and 11 were classified here, in four classifiers: the two
    // first-hand ones (a gauge anchor lost across a `create_section` entry, and
    // one lost across an entry recorded before a save), their shared cascade,
    // and the shot-uuid re-mint. All FIXED:
    //
    // * `UndoData::CreatedSectionSnapshot` carries a `SectionIdentity` alongside
    //   its `SectionData` - every shot uuid, the reverse parity and the gauge
    //   pair - and `Section::restore_with_identity` (models/section.rs) puts
    //   them back instead of minting through `Section::new`. The undo refreshes
    //   that identity from the live section immediately before deleting it, so
    //   anchors a save captured after the entry was pushed are carried too.
    // * `reapply_heals_after_restore` (core/state/project_io.rs) brings a
    //   project restored from a `ProjectSnapshotPair` up to the standard a saved
    //   one is held to, so a snapshot recorded before a save no longer reverts
    //   that save's heals when it is restored.
    //
    // The classifiers are deleted per the contract above: a gauge anchor, a
    // reverse-parity flag or a shot uuid differing across a history move - in
    // either direction, first-hand or cascaded - is now an unclassified failure
    // and fails the gate. `is_gauge_loss`, `is_null_or_false`,
    // `MoveCtx::entry_predates_save` and the `heal_reverted` latch went with
    // them; they had no other callers.
    //
    // FINDING 2 was classified here as "an orphan zero section offset" (absent
    // or inert within a nanometre on both sides). FIXED, and the classifier is
    // gone: a `section_offsets` key present on one side of a history move and
    // absent on the other now fails the gate, whatever its value.
    // `Project::write_section_offset` (models/project/offsets.rs) is THE write
    // point for `section_offsets` and drops the whole dust ball around zero, so
    // an adjustment that cancels leaves nothing behind - for EVERY writer
    // including a restore, which is the invariant that closed finding 27.
    // `is_absent_or_zero` went with it.
    //
    // FINDING 5 was classified here. FIXED: `SectionUpdates`' three nullable
    // fields carry `double_option` + `skip_serializing_if` (models/section.rs),
    // so `values_json` keeps the difference between "clear this field"
    // (`Some(None)`) and "leave it alone" (`None`). The classifier is deleted per
    // the contract above - a section name, surveyor or date going null on one
    // side of a history move is now an unclassified failure and fails the gate.
    //
    // FINDING 3 was classified here, twice: first-hand, as "a `section_offsets`
    // key gaining or losing real geometry on the move that consumed an append of
    // the FIRST shot into an empty section", and CASCADING (which is what finding
    // 15 turned out to be) as "that same section still carrying a value on the
    // ratchet's trajectory, several moves later".
    //
    // FIXED. Undo of `CreateShot` no longer reuses the general delete for the one
    // insert it is not the inverse of: `AppState::remove_created_shot`
    // (core/state/command_shot.rs) recognises the recorded append-into-an-empty-
    // section shape and calls `Project::remove_appended_first_shot`
    // (models/project/shots.rs), which removes the station the append CREATED and
    // leaves `section_offsets` alone - as offset-neutral in reverse as
    // `Project::add_shot` is forward, so there is nothing left to ratchet and
    // nothing left to cascade. Both classifiers are deleted per the
    // delete-when-fixed contract: a `section_offsets` key gaining or losing real
    // geometry across a history move is once again an unclassified failure that
    // fails the gate.
    //
    // FINDING 12 was classified here ("a stale in-session id edits the wrong
    // shot"), and FINDING 16's cascade with it ("a section holding one
    // `Shot::uuid` twice cannot match any snapshot recorded before the
    // duplication"). Both are FIXED at the source: every restore path addresses
    // its shot by `Shot::uuid` rather than by the id the entry happens to carry
    // (core/state/undo.rs, core/state/undo_restore.rs), so a persisted id going
    // stale can no longer reach a shot it was not recorded against. A field
    // changing on a shot an undo did not name, and a uuid appearing twice in one
    // section, are unclassified failures again.
];

/// Which classifier explains `diff`, if any.
fn classify(diff: &Diff, ctx: &MoveCtx) -> Option<&'static str> {
    CLASSIFIERS
        .iter()
        .find(|classifier| (classifier.matches)(diff, ctx))
        .map(|classifier| classifier.id)
}

/// Changes a SAVE makes to live state with no undo entry of its own
/// (`AppState::save_project`), subtracted only from comparisons that cross a
/// save. Deliberately narrow by direction: an anchor may only be GAINED, a uuid
/// filled in, an offset entry dropped.
fn attributable_to_save(diff: &Diff) -> bool {
    ((path_is(diff, "sections[#].gauge_shot_uuid")
        || path_is(diff, "sections[#].gauge_neighbour_uuid"))
        && diff.expected == "null"
        && diff.actual != "null")
        || (path_is(diff, "sections[#].shots[#].uuid")
            && (diff.expected == ABSENT || diff.expected == "null")
            && diff.actual != ABSENT)
        || (path_is(diff, "section_offsets.#") && diff.actual == ABSENT)
}

/// Derived geometry a RELOAD re-solves from scratch: `redrive_offsets_after_load`
/// ends in an unconditional `resolve_offsets()` ("Zero-trust: never honor a
/// persisted derivable offset"), so `section_offsets` and the station offsets
/// derived from them are RECOMPUTED rather than restored.
fn attributable_to_reload(diff: &Diff) -> bool {
    // BOTH spellings of an offset difference: the whole key when it is present
    // on one side only, and the element when it exists on both and the differ
    // recurses into the pair. The re-solve recomputes the whole map, so the two
    // are the same fact - and matching only the first left every value change
    // across a reload reading as unexplained.
    offset_key_of(&diff.path).is_some() || diff.path.starts_with("station_offsets")
}

// A block of DIFF SUBTRACTIONS lived here, each stripping one known bug's shape
// out of the comparison before anything else looked at it, because each of them
// re-indexed a list and buried every other difference of the same move under
// twenty derived paths. All of them are gone, per the delete-when-fixed
// contract:
//
// * FINDING 3's DATA-LOSS half - `diffs_without_station_zero_refs`, which
//   stripped both sides of every ref keyed to one section's station 0
//   (`names_station_zero` / `strip_station_zero_refs`), plus the cascade latch
//   for the sections it had already destroyed them on. The `pos == 0` branch of
//   `remove_shot_with_rewire` DROPS those refs, and an empty section has only
//   station 0, so a merge, zero datum or station comment anchored there was
//   destroyed by the undo of the append that created the section's first shot.
//   FIXED with the displacement half: that undo no longer routes through the
//   general delete at all (`Project::remove_appended_first_shot`,
//   models/project/shots.rs).
// * FINDING 10 - `diffs_without_orphan_conversion` and `converted_pair`, which
//   recognised a loop closure the LOAD had reclassified as a merge moving
//   between two lists and re-indexing both. FIXED at the source: the commands
//   that leave a closure orphaned convert it themselves
//   (`execute_delete_last_shot_and_section` and the appended-first-shot undo,
//   core/state/command_delete_split.rs and command_shot.rs), so the load has
//   nothing left to reclassify.
// * FINDINGS 4 and 17 - `LIST_REORDER_SIGNATURES`, `diffs_without_list_reorders`
//   and `sort_if_reordered`, which recognised `loop_closures` / `zero_datums`
//   holding the same members in a different order. Both are FIXED: the undo of
//   `RemoveLoopClosure` and of `RemoveZeroDatum` re-insert at the index the
//   forward `Vec::remove` took the entry out of, recorded on the entry
//   (core/undo.rs).
// * FINDINGS 8 and 16 - `shot_uuids`, `is_shot_identity_swap`,
//   `section_shot_uuids`, `strip_shot`, `diffs_without_misplaced_shot` and
//   `uuid_occurrences`, which recognised the wrong shot having been deleted, a
//   shot having come back in the wrong PLACE, and a section holding one
//   `Shot::uuid` twice. All FIXED by the same change: every restore path
//   addresses its shot by `Shot::uuid` instead of by the in-session id the
//   entry happens to carry, a load-time pass drops an entry whose target no
//   longer resolves, and a redo refuses to duplicate a uuid (core/undo.rs,
//   core/state/undo.rs). `shot_field_path` went with finding 12's classifier,
//   its last caller.
//
// A merge, datum or comment disappearing across a history move, either list
// coming back permuted, a closure that changed lists, a shot in the wrong place
// and a duplicated uuid are all unclassified failures again.

// ===========================================================================
// Synthetic starting project
// ===========================================================================

pub(super) const COLORS: &[&str] = &["#3388FF", "#FF8833", "#33CC66", "#CC33AA", "#FFCC00"];

/// A valid, unremarkable shot. Every value comes from an integer draw.
pub(super) fn random_shot(rng: &mut Rng) -> ShotData {
    let azimuth = rng.float(0.0, 359.9);
    let depth_in = rng.float(0.0, 30.0);
    ShotData {
        // `None` mints a fresh identity on insert, as every user-created shot
        // does; the uuid streams the identity oracles draw on come from the
        // project, not from the generator.
        uuid: None,
        azimuth_in: azimuth,
        azimuth_out: (azimuth + rng.float(-1.5, 1.5) + 360.0) % 360.0,
        length: rng.float(1.0, 12.0),
        depth_in,
        depth_out: (depth_in + rng.float(-3.0, 3.0)).max(0.0),
        pitch_in: rng.chance(40).then(|| rng.float(-60.0, 60.0)),
        pitch_out: rng.chance(40).then(|| rng.float(-60.0, 60.0)),
        left: rng.chance(70).then(|| rng.float(0.0, 5.0)),
        right: rng.chance(70).then(|| rng.float(0.0, 5.0)),
        up: rng.chance(70).then(|| rng.float(0.0, 5.0)),
        down: rng.chance(70).then(|| rng.float(0.0, 5.0)),
        comment: rng
            .chance(20)
            .then(|| format!("shot-{}", rng.range(0, 9999))),
        kind: *rng.pick(&[
            ShotKind::Normal,
            ShotKind::Normal,
            ShotKind::Jump,
            ShotKind::Reference,
        ]),
        marker_index: rng.chance(15).then(|| rng.range(0, 9) as u32),
        temperature: rng.chance(30).then(|| rng.float(4.0, 30.0)),
        credit: None,
    }
}

/// A starting project with the topology that made the naharon fixture productive:
/// sections of varied length INCLUDING empty and single-shot ones, station
/// merges, a zero datum, a genuine loop closure, station comments, and sections
/// with null metadata.
///
/// Built rather than loaded. The repo forbids committing fixtures and CI has
/// none, so the shapes that matter are constructed explicitly - which also means
/// each one is here on purpose and can be pointed at when a finding needs it.
pub(super) fn build_project(state: &AppState, rng: &mut Rng, closures: bool) -> Vec<SectionId> {
    let created = state.execute(Command::CreateProject {
        name: "RustFuzz".to_string(),
        location: None,
    });
    assert!(created.success, "CreateProject failed: {:?}", created.error);

    // Shot counts chosen to include the two shapes that have carried findings:
    // 0 (bug 3's empty section) and 1 (bug 8's single-shot divergence).
    let shape: &[usize] = &[3, 0, 1, 4, 2, 1, 3];
    let mut ids = Vec::new();
    for (index, count) in shape.iter().enumerate() {
        let shots: Vec<ShotData> = (0..*count).map(|_| random_shot(rng)).collect();
        // Null name/surveyor/date on some sections: finding 5 only exists for a
        // field that STARTS null, and the fractal fixture had none.
        let null_metadata = index % 3 == 0;
        let result = state.execute(Command::CreateSection {
            name: (!null_metadata).then(|| format!("sec-{index}")),
            surveyor: (!null_metadata).then(|| format!("surveyor-{}", rng.range(1, 9))),
            date: None,
            color: (*rng.pick(COLORS)).to_string(),
            direction: Some(if rng.chance(50) {
                SurveyDirection::In
            } else {
                SurveyDirection::Out
            }),
            shots,
        });
        assert!(result.success, "CreateSection failed: {:?}", result.error);
        ids.push(result.section_id.expect("CreateSection returned no id"));
    }

    // A zero datum anchors the first component, which is what makes closures and
    // several delete guards reachable at all.
    let datum = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(ids[0], 0),
        coordinates: GpsCoordinates::new(20.4, -87.3, 0.0).unwrap(),
    });
    assert!(datum.success, "SetZeroDatum failed: {:?}", datum.error);

    // A - bridge - B chain plus a closure straight from A to B: a genuine loop
    // with two independent paths, which is the only shape a closure may legally
    // describe.
    //
    // FINDING 14 lives here, which is why the gate does not build it. With this
    // chain in place and the datum at section 0, a later mid-section insert into
    // section 0 leaves the 0 <-> 3 merge 6.97 m out of alignment and the app's own
    // `#[cfg(test)]` assert panics inside `insert_shot_after`. The generators
    // still build merges, closures and datums during a run, so the gate keeps
    // exercising those commands - it just does not START from the shape that is
    // currently broken.
    if !closures {
        return ids;
    }
    for (left, right) in [(0_usize, 3_usize), (3, 4)] {
        let a = ids[left];
        let b = ids[right];
        let a_stations = shape[left] as u32;
        let merged = state.execute(Command::MergeStations {
            station_a: StationRef::new(a, a_stations),
            station_b: StationRef::new(b, 0),
        });
        assert!(
            merged.success,
            "setup MergeStations failed: {:?}",
            merged.error
        );
    }
    let closure = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(ids[0], 0),
        station_b: StationRef::new(ids[4], shape[4] as u32),
        closure_strategy: ClosureStrategy::Move,
    });
    assert!(
        closure.success,
        "setup CreateLoopClosure failed: {:?}",
        closure.error
    );

    for index in [0_usize, 5] {
        let commented = state.execute(Command::SetStationComment {
            station_ref: StationRef::new(ids[index], 0),
            comment: Some(format!("station-note-{index}")),
        });
        assert!(
            commented.success,
            "SetStationComment failed: {:?}",
            commented.error
        );
    }

    ids
}

// ===========================================================================
// Live-state targeting
// ===========================================================================

/// Everything the generators need to pick a target, read out of the live project
/// and SORTED. Sorting is what keeps a seed's trajectory stable: `HashMap`
/// iteration order is not, and neither is `Vec` order once a command reorders a
/// collection.
pub(super) struct Targets {
    /// `(section_id, [shot_id, ...])`, ascending.
    pub(super) sections: Vec<(SectionId, Vec<ShotId>)>,
    /// `(section_id, station_index)`, ascending.
    pub(super) stations: Vec<(SectionId, u32)>,
    /// Merged pairs, as resolved positional refs, ascending.
    pub(super) merges: Vec<((SectionId, u32), (SectionId, u32))>,
    /// Zero-datum stations, ascending.
    pub(super) datums: Vec<(SectionId, u32)>,
    /// Loop closure ids, ascending.
    pub(super) closures: Vec<u64>,
    /// `(section_id, shot_id, uuid)`, ascending, for every shot that has a uuid.
    ///
    /// Read here rather than through a second `get_project_state()` because the
    /// TRACE names shots by uuid: `Shot::id` is renumbered by the loader and
    /// re-issued by the allocator, so an id is not an identity a recorded move
    /// can carry, while `Shot::uuid` is persisted and survives both.
    pub(super) shot_uuids: Vec<(SectionId, ShotId, String)>,
}

impl Targets {
    /// The uuid the live shot `section_id`/`shot_id` carries, if it has one.
    pub(super) fn uuid_of(&self, section_id: SectionId, shot_id: ShotId) -> Option<String> {
        self.shot_uuids
            .iter()
            .find(|(section, shot, _)| *section == section_id && *shot == shot_id)
            .map(|(_, _, uuid)| uuid.clone())
    }

    /// Which live shot carries `uuid`, if any.
    pub(super) fn by_uuid(&self, uuid: &str) -> Option<(SectionId, ShotId)> {
        self.shot_uuids
            .iter()
            .find(|(_, _, candidate)| candidate == uuid)
            .map(|(section, shot, _)| (*section, *shot))
    }

    /// Shot ids of one live section, ascending, or `None` when the section is
    /// gone.
    pub(super) fn shots_of(&self, section_id: SectionId) -> Option<&[ShotId]> {
        self.sections
            .iter()
            .find(|(id, _)| *id == section_id)
            .map(|(_, shots)| shots.as_slice())
    }
}

pub(super) fn read_targets(state: &AppState) -> Targets {
    let project = state.get_project_state().expect("no project open");
    let mut sections: Vec<(SectionId, Vec<ShotId>)> = project
        .sections
        .iter()
        .map(|section| {
            let mut shots: Vec<ShotId> = section.shots.iter().map(|shot| shot.id).collect();
            shots.sort_unstable();
            (section.id, shots)
        })
        .collect();
    sections.sort_by_key(|entry| entry.0);

    let mut stations = Vec::new();
    for (id, shots) in &sections {
        for index in 0..=shots.len() {
            stations.push((*id, index as u32));
        }
    }

    let mut merges: Vec<((SectionId, u32), (SectionId, u32))> = project
        .station_merges
        .iter()
        .filter_map(|merge| {
            let a = project.resolve_key(&merge.station_a)?;
            let b = project.resolve_key(&merge.station_b)?;
            Some((
                (a.section_id, a.station_index),
                (b.section_id, b.station_index),
            ))
        })
        .collect();
    merges.sort_unstable();

    let mut datums: Vec<(SectionId, u32)> = project
        .zero_datums
        .iter()
        .filter_map(|datum| {
            let resolved = project.resolve_key(&datum.station_ref)?;
            Some((resolved.section_id, resolved.station_index))
        })
        .collect();
    datums.sort_unstable();

    let mut closures: Vec<u64> = project
        .loop_closures
        .iter()
        .map(|closure| closure.id)
        .collect();
    closures.sort_unstable();

    let mut shot_uuids: Vec<(SectionId, ShotId, String)> = project
        .sections
        .iter()
        .flat_map(|section| {
            section
                .shots
                .iter()
                .filter_map(move |shot| shot.uuid.clone().map(|uuid| (section.id, shot.id, uuid)))
        })
        .collect();
    shot_uuids.sort();

    Targets {
        sections,
        stations,
        merges,
        datums,
        closures,
        shot_uuids,
    }
}

pub(super) fn random_offset(rng: &mut Rng) -> Coordinates {
    let extreme = |rng: &mut Rng| *rng.pick(&[0.0, GEOMETRY_EXTREME, -GEOMETRY_EXTREME, 1e-9]);
    if rng.chance(15) {
        Coordinates::new(extreme(rng), extreme(rng), extreme(rng))
    } else {
        Coordinates::new(
            rng.float(-3.0, 3.0),
            rng.float(-3.0, 3.0),
            rng.float(-1.5, 1.5),
        )
    }
}

// ===========================================================================
// The replayable move trace
// ===========================================================================

/// A shot, named by the identity that survives BOTH a reload and the removal of
/// the moves around it.
///
/// `Shot::id` is neither. The loader renumbers it (`section_id *
/// FEATURE_ID_STRIDE + position`) and the allocator re-issues it, so a trace
/// that named shots by id would address a DIFFERENT shot the moment a shrink
/// step dropped a `create_shot` from in front of it. `Shot::uuid` is persisted,
/// so it survives the loader - but it is minted fresh per RUN
/// (`Uuid::new_v4`), so the recorded uuid and the replay's uuid for the same
/// shot are different strings that have to be PAIRED. [`Refs`] is that pairing.
///
/// The id travels alongside for the log, and as the fallback for the one shape
/// the uuid cannot cover: a shot that has none.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ShotRefT {
    pub(super) section: SectionId,
    pub(super) id: ShotId,
    pub(super) uuid: Option<String>,
}

/// A station, named positionally - which is what a station IS. There is no
/// stored station object to carry an identity, so a station reference is a
/// section plus an index, and it stops resolving when the section is short.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct StationRefT {
    pub(super) section: SectionId,
    pub(super) index: u32,
}

impl StationRefT {
    const fn new(section: SectionId, index: u32) -> Self {
        Self { section, index }
    }
}

/// One mutation, as a value: everything a generator drew, with every reference
/// symbolic rather than resolved.
///
/// This is what makes the engine replayable. A generator DRAWS an `Op` and
/// [`Fuzzer::apply_op`] executes it; a replay hands the recorded `Op` to the
/// SAME function, so there is no second execution path for a shrunk trace to
/// drift from - which is the property the whole shrinker rests on.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) enum Op {
    CreateSection {
        name: Option<String>,
        surveyor: Option<String>,
        color: String,
        shots: Vec<ShotData>,
    },
    CreateShot {
        section: SectionId,
        shot: Box<ShotData>,
        after: Option<ShotRefT>,
        before: Option<ShotRefT>,
    },
    UpdateShot {
        shot: ShotRefT,
        updates: Box<ShotUpdates>,
    },
    SetShotKind {
        shot: ShotRefT,
        kind: ShotKind,
    },
    UpdateSection {
        section: SectionId,
        updates: Box<SectionUpdates>,
    },
    DeleteShot {
        shot: ShotRefT,
    },
    DeleteSection {
        section: SectionId,
    },
    ReverseSection {
        section: SectionId,
        swap_left_right: bool,
    },
    Move {
        section: SectionId,
        offset: Coordinates,
        segment: bool,
    },
    MoveStation {
        station: StationRefT,
        offset: Coordinates,
    },
    StationComment {
        station: StationRefT,
        comment: Option<String>,
    },
    Merge {
        a: StationRefT,
        b: StationRefT,
    },
    Unmerge {
        a: StationRefT,
        b: StationRefT,
    },
    CreateClosure {
        a: StationRefT,
        b: StationRefT,
        strategy: ClosureStrategy,
    },
    RemoveClosure {
        closure: u64,
    },
    SetZeroDatum {
        station: StationRefT,
        coordinates: GpsCoordinates,
    },
    RemoveZeroDatum {
        station: StationRefT,
    },
    SplitShotAtDistance {
        shot: ShotRefT,
        distance: f64,
    },
    DeleteShotAndSplit {
        shot: ShotRefT,
    },
    /// Split Section Here (#335): at the shot's end station, or `distance`
    /// along it when `along` is set. `side` picks where the split station's
    /// comment (0 A, 1 B, 2 both) and zero datum (even A, odd B) go.
    SplitSection {
        shot: ShotRefT,
        along: bool,
        distance: f64,
        side: u8,
    },
    /// Combine Sections, routed through `combine_candidate` at an end
    /// `station`: combines its section with whatever section ends there,
    /// keeping the candidate's `b` section when `keep_b`. `pick` chooses the
    /// join comment and name (0 kept, 1 other, 2 both), `swap_lr` the walls of
    /// a section the combine reverses. A blocked candidate still runs, so the
    /// refusal is exercised too.
    CombineSections {
        station: StationRefT,
        keep_b: bool,
        swap_lr: bool,
        pick: u8,
    },
    DeleteLastShotAndSection {
        shot: ShotRefT,
    },
    DeleteShotRange {
        section: SectionId,
        from: ShotRefT,
        to: ShotRefT,
        split: bool,
    },
    Batch {
        commands: Vec<BatchOp>,
        poisoned: bool,
    },
}

/// The sub-commands `gen_batch` builds. A batch is one undo entry over several
/// commands, and each of them carries its own references, so each resolves (or
/// fails to) on its own.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) enum BatchOp {
    UpdateShotLength {
        shot: ShotRefT,
        length: f64,
    },
    MoveSection {
        section: SectionId,
        offset: Coordinates,
    },
    MoveStation {
        station: StationRefT,
        offset: Coordinates,
    },
    ReverseSection {
        section: SectionId,
        swap_left_right: bool,
    },
}

/// Everything one recorded move BROUGHT INTO EXISTENCE, in project order.
///
/// The shrinker's map from recorded ids to live ids is built from these: a
/// replayed move creates its own sections, shots and closures, and pairing what
/// the recorded run created with what the replay created is what lets a later
/// move still address the right thing after the moves before it changed.
///
/// A `redo` is in here for a reason rather than for symmetry: redo of a
/// `CreateShot` re-executes the command, which MINTS A NEW UUID (findings 1 and
/// 16 are about exactly that), and every later recorded move naming that shot
/// names the re-minted uuid.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Created {
    pub(super) sections: Vec<SectionId>,
    pub(super) shots: Vec<String>,
    pub(super) closures: Vec<u64>,
}

/// One move of a recorded run: what the fuzzer's main loop did, in the order it
/// did it.
///
/// The closing `finish()` walk is NOT in here, and neither is any prologue: both
/// are functions of the `Plan`, so a replay re-runs them rather than replaying
/// them, and a shrink step can never delete them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct TraceMove {
    pub(super) kind: MoveKind,
    /// What this move brought into existence - see [`Created`]. Filled in after
    /// the move ran, so a move that failed mid-flight carries an empty one.
    pub(super) created: Created,
    /// The generator's state immediately before this move. See [`Rng::state`].
    pub(super) rng: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) enum MoveKind {
    Mutate(Box<Op>),
    Undo(usize),
    Redo(usize),
    Cross,
}

impl TraceMove {
    /// One line, readable, for the minimized repro's summary.
    pub(super) fn describe(&self) -> String {
        match &self.kind {
            MoveKind::Mutate(op) => op.describe(),
            MoveKind::Undo(steps) => format!("undo x{steps}"),
            MoveKind::Redo(steps) => format!("redo x{steps}"),
            MoveKind::Cross => "save/close/reopen".to_string(),
        }
    }
}

impl Op {
    /// One line, readable, naming the RECORDED ids - which is what the trace
    /// carries and what a reader matches against the original run's log.
    pub(super) fn describe(&self) -> String {
        let station = |station: &StationRefT| format!("{}/{}", station.section, station.index);
        let shot = |shot: &ShotRefT| format!("{}:{}", shot.section, shot.id);
        match self {
            Self::CreateSection { name, shots, .. } => format!(
                "createSection(shots={},nullMeta={})",
                shots.len(),
                name.is_none()
            ),
            Self::CreateShot {
                section,
                after,
                before,
                ..
            } => {
                let how = match (after, before) {
                    (Some(anchor), _) => format!("after {}", shot(anchor)),
                    (None, Some(anchor)) => format!("before {}", shot(anchor)),
                    (None, None) => "append".to_string(),
                };
                format!("createShot(s={section},{how})")
            }
            Self::UpdateShot { shot: target, .. } => format!("updateShot({})", shot(target)),
            Self::SetShotKind { shot: target, kind } => {
                format!("setShotKind({},{kind:?})", shot(target))
            }
            Self::UpdateSection { section, .. } => format!("updateSection(s={section})"),
            Self::DeleteShot { shot: target } => format!("deleteShot({})", shot(target)),
            Self::DeleteSection { section } => format!("deleteSection(s={section})"),
            Self::ReverseSection {
                section,
                swap_left_right,
            } => format!("reverseSection(s={section},swapLR={swap_left_right})"),
            Self::Move {
                section, segment, ..
            } => format!(
                "{}(s={section})",
                if *segment {
                    "moveSegment"
                } else {
                    "moveSection"
                }
            ),
            Self::MoveStation { station: at, .. } => format!("moveStation({})", station(at)),
            Self::StationComment {
                station: at,
                comment,
            } => format!("stationComment({},{})", station(at), comment.is_some()),
            Self::Merge { a, b } => format!("mergeStations({},{})", station(a), station(b)),
            Self::Unmerge { a, b } => format!("unmergePair({},{})", station(a), station(b)),
            Self::CreateClosure { a, b, strategy } => format!(
                "createLoopClosure({},{},{strategy:?})",
                station(a),
                station(b)
            ),
            Self::RemoveClosure { closure } => format!("removeLoopClosure({closure})"),
            Self::SetZeroDatum { station: at, .. } => format!("setZeroDatum({})", station(at)),
            Self::RemoveZeroDatum { station: at } => format!("removeZeroDatum({})", station(at)),
            Self::SplitShotAtDistance {
                shot: target,
                distance,
            } => format!("splitShotAtDistance({},{distance})", shot(target)),
            Self::DeleteShotAndSplit { shot: target } => {
                format!("deleteShotAndSplit({})", shot(target))
            }
            Self::SplitSection {
                shot: target,
                along,
                distance,
                side,
            } => format!(
                "splitSection({},along={along},{distance},side={side})",
                shot(target)
            ),
            Self::CombineSections {
                station: at,
                keep_b,
                swap_lr,
                pick,
            } => format!(
                "combineSections({},keep_b={keep_b},swap_lr={swap_lr},pick={pick})",
                station(at)
            ),
            Self::DeleteLastShotAndSection { shot: target } => {
                format!("deleteLastShotAndSection({})", shot(target))
            }
            Self::DeleteShotRange {
                section,
                from,
                to,
                split,
            } => format!(
                "{}(s={section},{}..{})",
                if *split {
                    "deleteShotRangeSplit"
                } else {
                    "deleteShotRangeShift"
                },
                from.id,
                to.id
            ),
            Self::Batch { commands, poisoned } => {
                format!("batch(n={},poisoned={poisoned})", commands.len())
            }
        }
    }
}

/// Every section, its shots' uuids in project order, and every closure id.
///
/// Taken before and after a move: the difference is that move's [`Created`].
/// Project ORDER rather than sorted, because the pairing between the recorded
/// run's new entities and the replay's is positional and both runs build their
/// lists the same way.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Inventory {
    pub(super) sections: Vec<(SectionId, Vec<String>)>,
    pub(super) closures: Vec<u64>,
}

impl Inventory {
    /// Every shot uuid, in project order.
    fn shots(&self) -> Vec<&String> {
        self.sections
            .iter()
            .flat_map(|(_, uuids)| uuids.iter())
            .collect()
    }

    /// What `self` holds that `before` did not.
    fn created_since(&self, before: &Self) -> Created {
        let old_sections: BTreeSet<SectionId> = before.sections.iter().map(|(id, _)| *id).collect();
        let old_shots: BTreeSet<&String> = before.shots().into_iter().collect();
        let old_closures: BTreeSet<u64> = before.closures.iter().copied().collect();
        Created {
            sections: self
                .sections
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| !old_sections.contains(id))
                .collect(),
            shots: self
                .shots()
                .into_iter()
                .filter(|uuid| !old_shots.contains(*uuid))
                .cloned()
                .collect(),
            closures: self
                .closures
                .iter()
                .copied()
                .filter(|id| !old_closures.contains(id))
                .collect(),
        }
    }
}

/// What a recorded reference means in THIS run.
///
/// A recorded run is the identity: every reference already names a live entity,
/// so `apply_op` resolves to exactly what the generator drew and nothing is ever
/// skipped. That is the whole point - the recorded path and the replay path are
/// one function, and turning recording on cannot change what a seed does.
///
/// A REPLAY is a mapping, because the ids differ: the run allocates its own
/// section ids (dropping a `create_section` shifts every later one down), mints
/// its own uuids, and numbers its own closures. Each map is seeded from the
/// state the trace starts in and extended by every move's [`Created`].
///
/// A reference the maps cannot resolve - because the move that created its
/// target was SHRUNK AWAY - makes the move inapplicable, and an inapplicable
/// move is SKIPPED rather than fatal. ddmin needs a total replay function: a
/// candidate that panicked in the harness rather than in the app would be
/// indistinguishable from a candidate that reproduced the bug.
pub(super) struct Refs {
    identity: bool,
    sections: BTreeMap<SectionId, SectionId>,
    shots: BTreeMap<String, String>,
    closures: BTreeMap<u64, u64>,
    /// Moves skipped so far, and why - reported with the minimized repro.
    skipped: Vec<String>,
}

impl Refs {
    /// The recorded run's resolver: everything means itself.
    pub(super) fn identity() -> Self {
        Self {
            identity: true,
            sections: BTreeMap::new(),
            shots: BTreeMap::new(),
            closures: BTreeMap::new(),
            skipped: Vec::new(),
        }
    }

    /// A replay's resolver, seeded by pairing the state the recorded trace
    /// starts in against the state this run starts in.
    ///
    /// The two are the same project - same seed, same prologues, and the trace
    /// never contains a prologue move - so the pairing is positional and exact.
    /// It is CHECKED rather than assumed: a mismatch means the trace was
    /// recorded under a different plan and every later resolution would be
    /// quietly wrong.
    pub(super) fn replay(recorded: &Inventory, live: &Inventory) -> Result<Self, String> {
        if recorded.sections.len() != live.sections.len() {
            return Err(format!(
                "the trace starts in a project with {} section(s); this run starts with {} - \
                 the plan does not match the trace",
                recorded.sections.len(),
                live.sections.len()
            ));
        }
        let mut refs = Self {
            identity: false,
            sections: BTreeMap::new(),
            shots: BTreeMap::new(),
            closures: BTreeMap::new(),
            skipped: Vec::new(),
        };
        for ((recorded_id, recorded_shots), (live_id, live_shots)) in
            recorded.sections.iter().zip(&live.sections)
        {
            refs.sections.insert(*recorded_id, *live_id);
            if recorded_shots.len() != live_shots.len() {
                return Err(format!(
                    "the trace starts with {} shot(s) in section {recorded_id}; this run starts \
                     with {} - the plan does not match the trace",
                    recorded_shots.len(),
                    live_shots.len()
                ));
            }
            for (recorded_uuid, live_uuid) in recorded_shots.iter().zip(live_shots) {
                refs.shots.insert(recorded_uuid.clone(), live_uuid.clone());
            }
        }
        for (recorded_id, live_id) in recorded.closures.iter().zip(&live.closures) {
            refs.closures.insert(*recorded_id, *live_id);
        }
        Ok(refs)
    }

    /// Pair what the recorded move created with what the replayed move created.
    ///
    /// Only ENTITIES NEITHER SIDE ALREADY KNOWS are paired, in order. A move
    /// that restores something (an undo of a delete) brings back the very uuid
    /// it removed, which is already mapped and must not be re-paired against
    /// something else; a move that mints (a redo of a create) brings back a new
    /// one on both sides, and those two are each other's.
    fn learn(&mut self, recorded: &Created, live: &Created) {
        let fresh_sections: Vec<SectionId> = live
            .sections
            .iter()
            .copied()
            .filter(|id| !self.sections.values().any(|live_id| live_id == id))
            .collect();
        let unknown_sections: Vec<SectionId> = recorded
            .sections
            .iter()
            .copied()
            .filter(|id| !self.sections.contains_key(id))
            .collect();
        for (recorded_id, live_id) in unknown_sections.iter().zip(&fresh_sections) {
            self.sections.insert(*recorded_id, *live_id);
        }
        let fresh_shots: Vec<&String> = live
            .shots
            .iter()
            .filter(|uuid| !self.shots.values().any(|live_uuid| live_uuid == *uuid))
            .collect();
        let unknown_shots: Vec<&String> = recorded
            .shots
            .iter()
            .filter(|uuid| !self.shots.contains_key(*uuid))
            .collect();
        for (recorded_uuid, live_uuid) in unknown_shots.iter().zip(&fresh_shots) {
            self.shots
                .insert((*recorded_uuid).clone(), (*live_uuid).clone());
        }
        let fresh_closures: Vec<u64> = live
            .closures
            .iter()
            .copied()
            .filter(|id| !self.closures.values().any(|live_id| live_id == id))
            .collect();
        let unknown_closures: Vec<u64> = recorded
            .closures
            .iter()
            .copied()
            .filter(|id| !self.closures.contains_key(id))
            .collect();
        for (recorded_id, live_id) in unknown_closures.iter().zip(&fresh_closures) {
            self.closures.insert(*recorded_id, *live_id);
        }
    }

    /// The live section a recorded section id names, if it still exists.
    fn section(&self, recorded: SectionId, targets: &Targets) -> Option<SectionId> {
        if self.identity {
            return Some(recorded);
        }
        let live = *self.sections.get(&recorded)?;
        targets.shots_of(live).map(|_| live)
    }

    /// The live shot a recorded reference names, if it still exists.
    fn shot(&self, recorded: &ShotRefT, targets: &Targets) -> Option<(SectionId, ShotId)> {
        if self.identity {
            return Some((recorded.section, recorded.id));
        }
        let Some(uuid) = recorded.uuid.as_ref() else {
            // No uuid to pair on: fall back to the id inside the mapped
            // section, which is right exactly when the loader has not
            // renumbered it - and wrong loudly rather than quietly, because
            // a wrong target changes the signature and the step is rejected.
            let section = self.section(recorded.section, targets)?;
            return targets
                .shots_of(section)?
                .contains(&recorded.id)
                .then_some((section, recorded.id));
        };
        targets.by_uuid(self.shots.get(uuid)?)
    }

    /// The live station a recorded reference names, if the section is still that
    /// long.
    fn station(&self, recorded: StationRefT, targets: &Targets) -> Option<(SectionId, u32)> {
        let section = self.section(recorded.section, targets)?;
        if self.identity {
            return Some((section, recorded.index));
        }
        let stations = targets.shots_of(section)?.len() as u32;
        (recorded.index <= stations).then_some((section, recorded.index))
    }

    /// The live closure a recorded closure id names, if it still exists.
    fn closure(&self, recorded: u64, targets: &Targets) -> Option<u64> {
        if self.identity {
            return Some(recorded);
        }
        let live = *self.closures.get(&recorded)?;
        targets.closures.contains(&live).then_some(live)
    }

    /// Moves this replay could not apply, and why.
    pub(super) fn skipped(&self) -> &[String] {
        &self.skipped
    }

    fn skip(&mut self, what: &str) -> Option<Drawn> {
        self.skipped.push(what.to_string());
        None
    }
}

/// A recorded run, and everything a replay of it needs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Trace {
    pub(super) seed: u32,
    pub(super) plan: Plan,
    /// The project the trace starts in - after the plan's prologues, before the
    /// first recorded move. Pairing this against a replay's own starting state
    /// is what seeds [`Refs`].
    pub(super) prelude: Inventory,
    pub(super) moves: Vec<TraceMove>,
    /// TEST-ONLY, and `None` for every recorded run - see [`Fault`].
    #[serde(default)]
    pub(super) fault: Option<Fault>,
}

/// A failure INJECTED into a replay, so the shrinker has something to minimize.
///
/// The shrinker's own gate needs a failing run, and after issue #413 the app has
/// none: every finding is fixed, [`CLASSIFIERS`] is empty and every gate here is
/// clean. A gate that waited for a real bug would be permanently vacuous, and one
/// pinned to a finding goes red when the finding is FIXED - which is exactly what
/// happened to `the_shrinker_minimizes_a_classified_failure`, whose target was
/// "whatever this run classifies" and which had nothing left to classify.
///
/// So the failure is injected instead. [`run_trace`] fails the run the moment the
/// project holds `at_sections` sections, which is a property of the moves a
/// candidate KEPT: a subsequence reproduces exactly when it still creates enough
/// sections. That is a monotone predicate with a small, readable minimum, and it
/// exercises the whole shrink flow - record, replay, reference remapping,
/// signature match, the ddmin schedule, verify - without asking the app to be
/// broken.
///
/// It rides on the [`Trace`] rather than on the [`Plan`] because it is the
/// shrinker's business and not a mode of the fuzzer: `run_fuzz` cannot see it, so
/// no gate, survey, soak or corpus sweep can be affected by it.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct Fault {
    /// Fail the replay once the project holds this many sections.
    pub(super) at_sections: usize,
}

/// The recorder, per thread.
///
/// A thread-local rather than a field on the `Fuzzer` for one reason: a run that
/// FAILS unwinds, dropping the `Fuzzer` and with it every move it made - and the
/// move that failed is the one a shrinker most needs. This survives the unwind,
/// so `record_run` gets the trace whether the run returned or panicked.
///
/// Off by default and armed only by the shrinker, so no sweep pays for it: a run
/// that is not recording never builds an `Inventory` and never clones an `Op`.
struct Recording {
    active: bool,
    prelude: Inventory,
    moves: Vec<TraceMove>,
}

thread_local! {
    static RECORDER: std::cell::RefCell<Option<Recording>> =
        const { std::cell::RefCell::new(None) };
}

/// Arm the recorder for the next run on this thread, discarding anything a
/// previous run left.
pub(super) fn recorder_arm() {
    RECORDER.with_borrow_mut(|slot| {
        *slot = Some(Recording {
            active: false,
            prelude: Inventory::default(),
            moves: Vec::new(),
        });
    });
}

/// Is a recorder armed at all? Checked before an `Inventory` is built, so an
/// unarmed run pays one thread-local read per move and nothing else.
fn recorder_armed() -> bool {
    RECORDER.with_borrow(Option::is_some)
}

/// Is the recorder recording RIGHT NOW? False during the prologues and the
/// closing walk, which a replay re-runs from the plan rather than replaying.
fn recorder_active() -> bool {
    RECORDER.with_borrow(|slot| slot.as_ref().is_some_and(|recording| recording.active))
}

/// Start recording, from the state the run has reached.
fn recorder_begin(prelude: Inventory) {
    RECORDER.with_borrow_mut(|slot| {
        if let Some(recording) = slot.as_mut() {
            recording.active = true;
            recording.prelude = prelude;
        }
    });
}

fn recorder_end() {
    RECORDER.with_borrow_mut(|slot| {
        if let Some(recording) = slot.as_mut() {
            recording.active = false;
        }
    });
}

fn recorder_push(mv: TraceMove) {
    RECORDER.with_borrow_mut(|slot| {
        if let Some(recording) = slot.as_mut() {
            if recording.active {
                recording.moves.push(mv);
            }
        }
    });
}

fn recorder_seal(created: Created) {
    RECORDER.with_borrow_mut(|slot| {
        if let Some(recording) = slot.as_mut() {
            if recording.active {
                if let Some(last) = recording.moves.last_mut() {
                    last.created = created;
                }
            }
        }
    });
}

/// Take what was recorded, and disarm.
pub(super) fn recorder_take() -> Option<(Inventory, Vec<TraceMove>)> {
    RECORDER.with_borrow_mut(|slot| {
        slot.take()
            .map(|recording| (recording.prelude, recording.moves))
    })
}

// ===========================================================================
// Topology signature, for corpus guidance
// ===========================================================================

/// A count, bucketed.
///
/// Bucketing is the whole trick. An EXACT count makes every project novel -
/// 41 shots is not the same signature as 42 - so a corpus keyed on exact counts
/// stores everything and guides nothing. These classes are chosen so that
/// crossing one means the project acquired a structural possibility rather than
/// one more of something it already had.
const fn bucket(count: usize) -> u8 {
    match count {
        0 => 0,
        1 => 1,
        2 => 2,
        3..=4 => 3,
        5..=8 => 4,
        9..=16 => 5,
        17..=32 => 6,
        _ => 7,
    }
}

/// A compact, deterministic fingerprint of a project's SHAPE.
///
/// Counts and structural features only - no float appears anywhere in it, which
/// is what makes it stable across platforms and across the float dust findings
/// 21 and 23 are about. Two projects with the same signature can reach the same
/// generators; two with different signatures cannot necessarily.
///
/// The features are the ones this programme has actually needed. Findings 4 and
/// 10 were unreachable by random walk because they need a second closure and an
/// orphaned one; finding 3 needs an EMPTY section; findings 8, 12 and 13 need
/// ids that have diverged from positional order. Every one of those is a bit
/// here, so a corpus keyed on this can tell "I have been in a project like this
/// before" from "this is a shape I have never had".
pub(super) fn topology_signature(state: &AppState) -> String {
    let Some(project) = state.get_project_state() else {
        return "no-project".to_string();
    };
    let sections = project.sections.len();
    let empty = project
        .sections
        .iter()
        .filter(|section| section.shots.is_empty())
        .count();
    let single = project
        .sections
        .iter()
        .filter(|section| section.shots.len() == 1)
        .count();
    let shots: usize = project.sections.iter().map(|s| s.shots.len()).sum();

    // The merge graph's SHAPE, not its size: the largest degree tells a chain
    // (2) from a star (3+), and comparing the edge count against the node count
    // tells whether it holds a cycle.
    let mut degree: BTreeMap<SectionId, usize> = BTreeMap::new();
    let mut edges = 0;
    for merge in &project.station_merges {
        if merge.station_a.section_id == merge.station_b.section_id {
            continue;
        }
        edges += 1;
        *degree.entry(merge.station_a.section_id).or_insert(0) += 1;
        *degree.entry(merge.station_b.section_id).or_insert(0) += 1;
    }
    let max_degree = degree.values().copied().max().unwrap_or(0);
    let merged_nodes = degree.len();
    // More edges than nodes minus one means at least one cycle somewhere.
    let cyclic = u8::from(edges + 1 > merged_nodes && merged_nodes > 0);

    // The id-vs-position divergence findings 8, 12 and 13 need, as a bit.
    let diverged =
        u8::from(project.sections.iter().any(|section| {
            let base = section.id * crate::models::FEATURE_ID_STRIDE;
            section.shots.iter().enumerate().any(|(position, shot)| {
                shot.id != base + position as u32 && shot.id != position as u32
            })
        }));

    format!(
        "s{}e{}o{}h{}m{}d{}c{}y{}z{}g{}k{}v{}",
        bucket(sections),
        bucket(empty),
        bucket(single),
        bucket(shots),
        bucket(edges),
        bucket(max_degree),
        bucket(project.loop_closures.len()),
        cyclic,
        bucket(project.zero_datums.len()),
        bucket(project.section_offsets.len()),
        bucket(project.station_comments.len()),
        diverged,
    )
}

/// The topology signature at every prefix of a seed's MUTATE-ONLY walk - which
/// is the exact trajectory [`Plan::prologue`] replays, state by state.
///
/// `prologue_signatures(seed, closures, n)[i - 1]` is the signature of the
/// project a run with `prologue: Some((seed, i))` STARTS FROM, for every `i` in
/// `1..=n`. That is what lets a corpus be keyed on what an entry actually
/// carries rather than on what the run that suggested it happened to end with -
/// see `fuzz_rarity_tests`, which is the only caller and which measures the gap
/// between those two things.
///
/// Exact while no config is drawn: `start_run` builds the same `Fuzzer::new` in
/// that case, and every regime that uses this has `config_fuzz` and `combined`
/// off. The walk stops early for the same reason the prologue does - once the
/// app's own alignment assert has fired, nothing after it measures the app.
pub(super) fn prologue_signatures(seed: u32, closures: bool, moves: usize) -> Vec<String> {
    let mut fuzzer = Fuzzer::new(seed, closures);
    let mut signatures = Vec::with_capacity(moves);
    for _ in 0..moves {
        if fuzzer.stopped() {
            break;
        }
        fuzzer.step_mutate();
        signatures.push(topology_signature(&fuzzer.state));
    }
    signatures
}

// ===========================================================================
// The fuzzer
// ===========================================================================

/// How a run ended, for the caller to assert on.
#[derive(Debug, Default)]
pub(super) struct Outcome {
    mutations: usize,
    rejected: usize,
    noops: usize,
    undos: usize,
    redos: usize,
    jumps: usize,
    saves: usize,
    reloads: usize,
    max_depth: usize,
    /// Classified known-bug hits, by finding id.
    pub(super) hits: BTreeMap<&'static str, usize>,
    /// Comparisons the numeric tolerance absorbed.
    float_noise: usize,
    /// Comparisons explained entirely by a documented save/reload heal.
    attributed: usize,
    // `wedged` - "a known wedge stopped the walk early" - was set only where
    // `classify_move_failure` excused a refused history move, and both findings
    // it named (6 and 13) are fixed; a refused move fails the run now.
    // `drift_stop` was finding 20's terminal latch, and went the same way.
    /// True when the app's own `#[cfg(test)]` merge-alignment assert fired. The
    /// run stops there and `run_fuzz` turns it into a failure carrying the seed,
    /// the depth and the move log - see `catching_alignment`.
    alignment_panic: bool,
    /// Integrity verdicts compared. Counted so the oracle can be SHOWN to be
    /// doing work rather than passing vacuously.
    integrity_compared: usize,
    /// Adjusted-geometry states compared.
    adjusted_compared: usize,
    /// Wall time spent inside each derived-state oracle, in microseconds:
    /// (snapshot, integrity, adjusted). Reported per run so the cost of the two
    /// ports is a measured number rather than a guess.
    oracle_us: (u128, u128, u128),
    /// Undo entries the app FIFO-trimmed out of reach during the run.
    trimmed: usize,
    /// Largest collection sizes seen. Growth sanity for a long run.
    peak: Peak,
    /// The topology signature of the state the run ENDED in. Corpus guidance
    /// keys on this; nothing else reads it.
    signature: String,
    /// The undo-relevant config this run DREW, when the knob was on. Carried on
    /// the outcome so `summarize` can put it in the run header: a failing seed
    /// under the config knob is only a repro if the app it ran against is named.
    config: Option<DrawnConfig>,
}

/// The part of an [`Outcome`] a REPLAY has to reproduce exactly.
///
/// Not the whole thing, and the exclusions are the point: `oracle_us` is wall
/// time, and `float_noise` / `attributed` / the peaks are counts a shorter
/// candidate legitimately reaches differently. What is in here is what makes a
/// run the run it was - how much it did, how deep it went, what it hit, and the
/// topology it ended in.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Shape<'a> {
    pub(super) mutations: usize,
    pub(super) rejected: usize,
    pub(super) noops: usize,
    pub(super) undos: usize,
    pub(super) redos: usize,
    pub(super) max_depth: usize,
    pub(super) hits: &'a BTreeMap<&'static str, usize>,
    pub(super) signature: &'a str,
}

impl Outcome {
    pub(super) fn shape(&self) -> Shape<'_> {
        Shape {
            mutations: self.mutations,
            rejected: self.rejected,
            noops: self.noops,
            undos: self.undos,
            redos: self.redos,
            max_depth: self.max_depth,
            hits: &self.hits,
            signature: &self.signature,
        }
    }

    /// An outcome carrying nothing but classified hits, for a test that is about
    /// what a hit MEANS rather than about a run.
    pub(super) fn from_hits(hits: BTreeMap<&'static str, usize>) -> Self {
        Self {
            hits,
            ..Self::default()
        }
    }
}

/// The largest each unbounded-in-principle collection got during a run.
#[derive(Debug, Default, Clone, Copy)]
struct Peak {
    sections: usize,
    stations: usize,
    section_offsets: usize,
    station_offsets: usize,
    station_comments: usize,
    merges: usize,
    closures: usize,
}

/// Every difference between two adjusted-geometry states, outside
/// `ADJUSTED_TOLERANCE_M`.
pub(super) fn adjusted_diffs(expected: &str, actual: &str) -> Vec<String> {
    let parse = |text: &str| -> BTreeMap<String, [f64; 3]> {
        text.lines()
            .filter_map(|line| {
                let mut parts = line.split('|');
                let key = parts.next()?.to_string();
                let x = parts.next()?.parse().ok()?;
                let y = parts.next()?.parse().ok()?;
                let z = parts.next()?.parse().ok()?;
                Some((key, [x, y, z]))
            })
            .collect()
    };
    let (before, after) = (parse(expected), parse(actual));
    let mut out = Vec::new();
    for (key, left) in &before {
        match after.get(key) {
            None => out.push(format!("station {key} is GONE from the adjusted solve")),
            Some(right) => {
                let worst = (0..3)
                    .map(|axis| (left[axis] - right[axis]).abs())
                    .fold(0.0_f64, f64::max);
                // Absolute bound only. The relative term that used to be added
                // here scaled with the station's distance from the origin and
                // existed solely to absorb finding 21's solve-to-solve wander;
                // the solve is deterministic now, so it is gone.
                if worst > ADJUSTED_TOLERANCE_M {
                    out.push(format!(
                        "station {key} moved {worst:.6} m: {left:?} -> {right:?}"
                    ));
                }
            }
        }
    }
    for key in after.keys() {
        if !before.contains_key(key) {
            out.push(format!("station {key} APPEARED in the adjusted solve"));
        }
    }
    out.truncate(8);
    out
}

/// The three per-state oracles as FREE functions, so a driver that is not the
/// `Fuzzer` - the metamorphic twins in `metamorphic_undo_tests.rs` - measures a
/// state exactly as the fuzzer does rather than with a second implementation
/// that can drift from it.
///
/// The `Fuzzer` methods are thin wrappers that add the per-run timing the
/// summary line reports.
///
/// Canonical, comparison-ready snapshot of the live project.
pub(super) fn canonical_snapshot(state: &AppState, id_agnostic: bool) -> String {
    let raw = state
        .capture_project_snapshot()
        .expect("capture_project_snapshot failed");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("snapshot is not JSON");
    canonicalize(&parsed, "", id_agnostic).to_string()
}

/// The app's own integrity verdict, normalized. See `Verdict`.
pub(super) fn integrity_verdict(state: &AppState) -> Verdict {
    let result = state.integrity_check();
    let mut messages = result.messages;
    messages.sort();
    Verdict {
        save_blocked: result.save_blocked,
        messages,
    }
}

/// The ADJUSTED station geometry - what the map draws and every export writes -
/// canonicalized to a comparable string.
pub(super) fn adjusted_geometry_of(state: &AppState) -> String {
    let Some(stations) = state.get_adjusted_stations() else {
        return String::new();
    };
    let mut rows: Vec<String> = stations
        .iter()
        .map(|station| {
            format!(
                "{}:{}|{}|{}|{}",
                station.station_ref.section_id,
                station.station_ref.station_index,
                round_significant(station.coordinates.x),
                round_significant(station.coordinates.y),
                round_significant(station.coordinates.z),
            )
        })
        .collect();
    // Sorted: `get_adjusted_stations` builds its list by walking sections, and a
    // command that reorders sections would otherwise read as every station
    // having moved.
    rows.sort();
    rows.join("\n")
}

// ===========================================================================
// Undo-entry metadata: what the history dropdown SAYS an entry is
// ===========================================================================

/// The ENTRY-INTRINSIC half of a history item: `timestamp|command_type|description`.
///
/// Every one of the three is a pure function of the `UndoEntry` alone -
/// `Command::description()` and `get_command_type()` read the stored command and
/// nothing else, and `shot_display_position` is arithmetic on the ids the command
/// carries rather than a lookup in the live project (models/shot.rs:16). So this
/// triple may be pinned PER DEPTH for the life of a run: nothing a later command
/// does to the project can legitimately change what an already-pushed entry says.
///
/// `preview_data` is deliberately NOT in here - see [`Fuzzer::history_rows`].
fn entry_print(item: &UndoHistoryItem) -> String {
    format!(
        "{}|{}|{}",
        item.timestamp.clone().unwrap_or_default(),
        item.command_type,
        item.description
    )
}

/// EVERY field of a history item, including `preview_data`, as canonical JSON.
///
/// `UndoHistoryItem` is the IPC DTO the history dropdown renders
/// (`commands/history.rs`), so serializing it is exactly what the frontend
/// receives - `index`, `description`, `timestamp`, `commandType` and the whole
/// `previewData` tree of field changes.
fn history_item_text(item: &UndoHistoryItem) -> String {
    serde_json::to_string(item).unwrap_or_else(|error| format!("<unserializable: {error}>"))
}

/// One row of the history dropdown, next to the bytes the file carries for it.
///
/// The pair is the point: a rendering alone cannot say WHY it changed, and the
/// two known reasons are told apart by whether the bytes moved. See
/// [`Fuzzer::expect_history_rows`].
#[derive(Clone, Debug)]
struct HistoryRow {
    /// `undo[3]` / `redo[0]` - the row a user would be looking at.
    label: String,
    /// The whole `UndoHistoryItem`, as the frontend receives it.
    item: String,
    /// `serde_json::to_string(&entry.command)`, which is what a save writes.
    command: String,
    /// `serde_json::to_string(&entry.undo_data)`, likewise.
    undo_data: String,
}

// The RELOAD half of the entry-content oracle classified, once, and everything
// that made that possible lived here: `classify_history_row` and its two
// matchers, plus the `Preview` splitting they needed
// (`CLEARABLE_SECTION_FIELDS`, `is_cleared_section_field_loss`,
// `is_cleared_row`, `is_history_payload_float_dust`, `Preview`,
// `split_preview`, `rows_differ_only_in_the_last_shown_place` and
// `rendered_numbers_are_one_place_apart`). Both findings are FIXED and the whole
// block is deleted, so a history row that reads differently after a reload than
// it did before the save now fails the run outright:
//
// * FINDING 5, COMMAND side. `SectionUpdates` carried `name`, `surveyor` and
//   `date` as `Option<Option<T>>` with a plain `#[serde(default)]`, so
//   `Some(None)` ("clear it") and `None` ("leave it alone") both serialized to
//   `null` AND both read back as `None` - the persisted TEXT was identical and
//   its MEANING was not, which emptied the hover preview and made a reloaded
//   redo of "clear the name" silently leave the name alone. The three fields
//   carry `double_option` + `skip_serializing_if` now (models/section.rs), and
//   schema v17 drops pre-v17 history rather than reinterpret them.
// * FINDING 23, reaching the history dropdown. `serde_json` wrote an `f64`
//   through ryu's shortest round-tripping form and read it back one ULP low, so
//   a length rendered `13.57` before the save and `13.56` after it on an entry
//   nobody touched. The crate is built with the `float_roundtrip` feature now
//   (src-tauri/Cargo.toml), which is the switch that closes it.

struct Fuzzer {
    state: AppState,
    rng: Rng,
    seed: u32,
    /// `snapshots[d]` is the canonical project at undo depth `d`.
    snapshots: Vec<String>,
    /// `verdicts[d]` is the app's own integrity verdict at undo depth `d`.
    verdicts: Vec<Verdict>,
    /// `adjusted[d]` is the canonical ADJUSTED station geometry at depth `d` -
    /// what the map draws, solved rather than stored.
    adjusted: Vec<String>,
    /// Epoch each snapshot was recorded in; a save or a reload bumps the epoch.
    epochs: Vec<u32>,
    /// `entry_meta[d]` describes the entry that takes depth d-1 to d. Index 0 is
    /// a placeholder so the indexing matches `snapshots`.
    entry_meta: Vec<EntryMeta>,
    /// `entry_prints[d]` is [`entry_print`] of the entry that takes depth d-1 to
    /// d, READ OFF THE APP the moment it was pushed. Index 0 is a placeholder, as
    /// in `entry_meta`.
    ///
    /// The metadata oracle's model. Recorded once and never rewritten, because
    /// the triple it holds is a pure function of the entry: an entry whose
    /// description, command type or timestamp differs from this later has been
    /// REWRITTEN, whether by a history move, a push, or a trip through the file.
    entry_prints: Vec<String>,
    epoch: u32,
    cursor: usize,
    top: usize,
    /// The lowest depth the history can still reach.
    ///
    /// Zero until the app's `undo_history_limit` FIFO-trims the oldest entries
    /// (`UndoStack::enforce_depth_limit` -> `trim_to_count`, core/undo/budget.rs,
    /// which drops from the FRONT of both stacks on every push). From then on
    /// the model is a SLIDING WINDOW: depth `floor` is the deepest state the
    /// user can still get back to, everything below it is gone for good, and
    /// `undo_len` is `cursor - floor` rather than `cursor`.
    floor: usize,
    /// The app's own count limit, read from the app rather than assumed.
    history_limit: usize,
    /// The config this run DREW, when the config knob is on. `None` is "the
    /// app's defaults", which is every mode that existed before the knob.
    ///
    /// Read for two things only: the run header (so a failure names the app it
    /// happened in), and `crossing_would_truncate`, where a drawn limit makes
    /// the old measure meaningless rather than merely different.
    drawn: Option<DrawnConfig>,
    /// Per depth, the largest tolerated numeric difference seen at each path.
    ///
    /// The accumulation oracle's memory: a difference that stays under
    /// `TOLERANCE_ABS` is accepted every time it is seen, so a run that visits a
    /// depth a hundred times can walk it past the tolerance one dust grain at a
    /// time. Recording what was tolerated is what lets the failure be told apart
    /// from a fresh regression.
    drift: BTreeMap<usize, BTreeMap<String, f64>>,
    /// Human-readable move history, tailed into a failure payload.
    log: Vec<String>,
    /// The move that is RUNNING RIGHT NOW, named the way the trace names it -
    /// `None` between moves.
    ///
    /// A `log` line reports what a move DID to the stacks, so it can only be
    /// written once the move RETURNS. A move that never returns - the app's own
    /// `debug_assert_merge_alignment` unwinds straight out of the command - then
    /// left the payload's tail ending on the PREVIOUS move, with
    /// `catching_alignment`'s "^ ..." note pointing at that line. It reads as an
    /// attribution and it is one: it names the wrong move. That mis-read #464's
    /// defects B and C twice during triage, because the recorded TRACE carries
    /// the true last move while the log tail named the one before it.
    ///
    /// Recording the label BEFORE the move runs is what makes the two agree.
    /// Nothing else consumes it: it is rendered by `fail` and by
    /// `catching_alignment`, and the trace is untouched.
    in_flight: Option<String>,
    id_agnostic: bool,
    outcome: Outcome,
    /// Kept alive for the run: dropping it deletes the save file.
    temp: Option<TempDir>,
    /// May the generators create loop closures? See `Plan::closures`.
    closures: bool,
    /// Build finding 10's orphan-closure shape on the next boundary crossing.
    /// See `expose_orphan_closure` for why it cannot be built up front.
    orphan_pending: bool,
    /// The join a successful Split Section Here just made, `(first half, its
    /// end station)`, so the combine generator can put it back together.
    /// Written by `apply_op` (a function of the state, so a replay sets it
    /// the same way) and only READ by the draw.
    last_split_join: Option<(SectionId, u32)>,
    // Four CASCADE LATCHES lived here, and every one went with the classifier
    // that read it: `heal_reverted` (findings 1 and 11 had reverted the save's
    // gauge capture), `displaced` (the trajectory finding 3's RATCHET had walked
    // each section's offset along), `ref_dropped` (sections whose station-0 refs
    // finding 3 had destroyed) and `corrupted` (sections finding 16 had
    // duplicated a shot uuid in). `saved_anchors`, the gauge pairs the last save
    // captured, went with `heal_reverted`'s measurement.
    /// Set once a persisted in-session id has been measured stale in this run.
    ///
    /// A MEASUREMENT now, not a latch: findings 8, 12, 13 and 16 are fixed and
    /// the restore paths resolve their shot by `Shot::uuid`, so nothing branches
    /// on this. It survives because the fact that an id went stale is the
    /// precondition those defects needed, and the move log is where a reader
    /// wants to see it happen.
    stale_corruption: bool,
}

impl Fuzzer {
    fn new(seed: u32, closures: bool) -> Self {
        Self::new_with_config(seed, closures, None)
    }

    /// [`Self::new`], against an app configured the way this run drew it.
    ///
    /// The limit is armed through `UndoStack::set_history_limit` - the setter
    /// `AppState::reload_config` itself calls when the user changes the setting
    /// (core/state/config.rs) - and deliberately NOT by writing
    /// `Config::general.undo_history_limit`. That is not a shortcut: every save
    /// this module makes reaches `note_recent_project`, which calls
    /// `Config::save()`, so a value written into the config struct would be
    /// persisted into the real `config.toml` and would silently become the
    /// default for every later run in every later process. The stack owns the
    /// in-memory window (`use_count_limit` re-arms `Count(self.history_limit)`
    /// on every open and close, so the drawn value survives a crossing), and
    /// nothing re-reads the config entry during a run.
    ///
    /// The limit is then read BACK off the app, exactly as it always was: the
    /// model's authority on the window is the app, not what this asked for.
    fn new_with_config(seed: u32, closures: bool, drawn: Option<DrawnConfig>) -> Self {
        let state = AppState::new().unwrap();
        if let (Some(config), Ok(mut stack)) = (drawn, state.undo_stack.lock()) {
            stack.set_history_limit(config.history_limit);
        }
        let history_limit = state
            .undo_stack
            .lock()
            .map_or(DEFAULT_HISTORY_LIMIT, |stack| stack.history_limit());
        let mut rng = Rng::new(seed);
        build_project(&state, &mut rng, closures);
        let mut fuzzer = Self {
            state,
            rng,
            seed,
            snapshots: Vec::new(),
            verdicts: Vec::new(),
            adjusted: Vec::new(),
            epochs: Vec::new(),
            entry_meta: vec![EntryMeta::default()],
            entry_prints: vec![String::new()],
            epoch: 0,
            cursor: 0,
            top: 0,
            floor: 0,
            history_limit,
            drawn,
            drift: BTreeMap::new(),
            log: Vec::new(),
            in_flight: None,
            id_agnostic: false,
            outcome: Outcome {
                config: drawn,
                ..Outcome::default()
            },
            temp: None,
            closures,
            orphan_pending: false,
            last_split_join: None,
            stale_corruption: false,
        };
        // The builder's own commands are undoable, so the stacks are not empty.
        // Clear them: the model measures depth from a known floor, and the
        // builder's history is scaffolding rather than something to fuzz over.
        if let Ok(mut stack) = fuzzer.state.undo_stack.lock() {
            stack.clear();
        }
        assert_eq!(fuzzer.undo_len(), 0, "setup left undo entries behind");
        assert_eq!(fuzzer.redo_len(), 0, "setup left redo entries behind");
        let base = fuzzer.snapshot();
        fuzzer.snapshots.push(base);
        let base_verdict = fuzzer.verdict();
        fuzzer.verdicts.push(base_verdict);
        let base_adjusted = fuzzer.adjusted_geometry();
        fuzzer.adjusted.push(base_adjusted);
        fuzzer.epochs.push(0);
        fuzzer
    }

    fn undo_len(&self) -> usize {
        self.state.get_undo_stack().len()
    }

    fn redo_len(&self) -> usize {
        self.state.get_redo_stack().len()
    }

    /// Canonical snapshot of the live project.
    fn snapshot(&mut self) -> String {
        let started = std::time::Instant::now();
        let out = canonical_snapshot(&self.state, self.id_agnostic);
        self.outcome.oracle_us.0 += started.elapsed().as_micros();
        out
    }

    /// The app's own integrity verdict, normalized. See `Verdict`.
    fn verdict(&mut self) -> Verdict {
        let started = std::time::Instant::now();
        let out = integrity_verdict(&self.state);
        self.outcome.oracle_us.1 += started.elapsed().as_micros();
        out
    }

    /// The ADJUSTED station geometry - `AppState::get_adjusted_stations`, which
    /// is what the map draws and what every export writes - canonicalized to a
    /// comparable string.
    ///
    /// This is DERIVED state, solved from the project rather than stored in it,
    /// so it is a second, independent view of every history move. Two things it
    /// can see that the stored snapshot cannot: a solve whose result depends on
    /// something the snapshot drops, and a stale memo. The second is not
    /// hypothetical - issue #413's finding 7 originally claimed the missing
    /// `invalidate_adjusted_cache()` in `undo_to`/`redo_to` was part of the
    /// defect, and the correction argued it is unobservable because the cache
    /// keys on `modified_at`, which every undo touches. This comparison is that
    /// argument's test.
    fn adjusted_geometry(&mut self) -> String {
        let started = std::time::Instant::now();
        let out = adjusted_geometry_of(&self.state);
        self.outcome.oracle_us.2 += started.elapsed().as_micros();
        out
    }

    /// Every cross-section merge, measured raw AND resolved.
    fn merge_measurements(&self) -> Vec<MergeMeasurement> {
        let Some(project) = self.state.get_project_state() else {
            return Vec::new();
        };
        let adjusted: BTreeMap<(SectionId, u32), (f64, f64)> = self
            .state
            .get_adjusted_stations()
            .unwrap_or_default()
            .iter()
            .map(|station| {
                (
                    (
                        station.station_ref.section_id,
                        station.station_ref.station_index,
                    ),
                    (station.coordinates.x, station.coordinates.y),
                )
            })
            .collect();
        let mut out = Vec::new();
        for merge in &project.station_merges {
            if merge.station_a.section_id == merge.station_b.section_id {
                continue;
            }
            let (Some(a), Some(b)) = (
                project.resolve_key(&merge.station_a),
                project.resolve_key(&merge.station_b),
            ) else {
                continue;
            };
            let (Some(raw_a), Some(raw_b)) = (
                project.compute_station_raw_coords(a),
                project.compute_station_raw_coords(b),
            ) else {
                continue;
            };
            let (Some(res_a), Some(res_b)) = (
                adjusted.get(&(a.section_id, a.station_index)),
                adjusted.get(&(b.section_id, b.station_index)),
            ) else {
                continue;
            };
            out.push(MergeMeasurement {
                label: format!(
                    "{}:{} <-> {}:{}",
                    a.section_id, a.station_index, b.section_id, b.station_index
                ),
                raw: ((raw_a.x - raw_b.x).powi(2) + (raw_a.y - raw_b.y).powi(2)).sqrt(),
                resolved: ((res_a.0 - res_b.0).powi(2) + (res_a.1 - res_b.1).powi(2)).sqrt(),
            });
        }
        out
    }

    /// Re-express every recorded snapshot in id-agnostic form. Called once, on the
    /// first reload: `Shot::id` is not persisted, so from then on a byte-exact
    /// comparison against a pre-reload snapshot would report the renumbering as
    /// drift.
    /// The derived models need no equivalent: an integrity message names section
    /// ids, station indices, closure ids and uuids, and an adjusted station names
    /// a `StationRef` - none of which the loader renumbers. Shot ids appear in
    /// neither.
    fn enable_id_agnostic(&mut self) {
        if self.id_agnostic {
            return;
        }
        self.id_agnostic = true;
        for snapshot in &mut self.snapshots {
            let parsed: serde_json::Value =
                serde_json::from_str(snapshot).expect("recorded snapshot is not JSON");
            *snapshot = canonicalize(&parsed, "", true).to_string();
        }
    }

    fn note(&mut self, line: String) {
        // `STALAGMIGHT_RUST_FUZZ_TRACE=1` prints every move as it happens.
        // The tail in a `fail()` payload covers a failure the fuzzer detects, but
        // the app has assertions of its own - `debug_assert_merge_alignment` is
        // `#[cfg(test)]`, so it exists ONLY in this tier - and when one of those
        // panics it takes the process down before the payload is built. Streaming
        // the log is how those get attributed to a move.
        if fuzz_trace() {
            println!("  [seed {} depth {}] {line}", self.seed, self.cursor);
        }
        self.log.push(line);
    }

    /// Arm the in-flight label. Paired with [`Self::finished_move`] on every
    /// path a move can return by; see [`Self::in_flight`].
    fn starting_move(&mut self, label: String) {
        self.in_flight = Some(label);
    }

    /// Disarm it: the move returned, so its own log line is (or is about to be)
    /// the last one and there is nothing left in flight to name.
    fn finished_move(&mut self) {
        self.in_flight = None;
    }

    /// Move the in-flight label INTO the log, marked as never having returned.
    ///
    /// Called on the panic path only, and before the note that describes the
    /// panic - which is what makes that note's "^" point at the move that
    /// actually panicked instead of at the one before it.
    fn flush_in_flight(&mut self) {
        if let Some(label) = self.in_flight.take() {
            self.note(format!("{label} -> PANICKED (did not return)"));
        }
    }

    fn fail(&self, headline: &str, detail: &str) -> ! {
        let mut tail: Vec<String> = self
            .log
            .iter()
            .rev()
            .take(14)
            .rev()
            .map(|line| format!("    {line}"))
            .collect();
        // A move that has not RETURNED has no log line yet, so name it here:
        // without this the tail ends on the previous move and reads as if THAT
        // one is where the run stopped. See `Fuzzer::in_flight`.
        if let Some(label) = &self.in_flight {
            tail.push(format!("    {label} -> IN FLIGHT (had not returned)"));
        }
        panic!(
            "{headline}\n  seed={} depth={} top={} saves={} reloads={}{}\n{detail}\n  last {} move(s):\n{}",
            self.seed,
            self.cursor,
            self.top,
            self.outcome.saves,
            self.outcome.reloads,
            // The drawn config, when there is one: a seed alone is not a repro
            // under the config knob, since the same moves ran against a
            // differently configured app.
            self.drawn.map(DrawnConfig::describe).unwrap_or_default(),
            tail.len(),
            tail.join("\n"),
        );
    }

    /// Compare the live project against the snapshot recorded at `depth`.
    ///
    /// Returns normally when the states match, when the numeric tolerance absorbs
    /// the difference, or when EVERY difference is explained by a documented open
    /// bug or by a save/reload heal - in which case the hit is counted and the
    /// model resyncs, so one open defect does not cascade into every later
    /// comparison. Anything else panics with the full payload.
    fn expect_state(
        &mut self,
        depth: usize,
        what: &str,
        consumed: Vec<EntryMeta>,
        stale: StaleTargets,
    ) {
        let ctx_entry = consumed.first().cloned();
        let ctx = MoveCtx { stale };
        let actual = self.snapshot();
        let expected = self.snapshots[depth].clone();
        if expected == actual {
            self.expect_derived(depth, &format!("{what} (stored state EXACT)"), false);
            return;
        }
        let (diffs, truncated) = diff_snapshots_capped(&expected, &actual);
        // A mismatch wider than the cap is NEVER read as explained. Every
        // tolerate path below - attributable, accumulated - reads "nothing
        // unexplained is left" off a list that stops at MAX_DIFFS, and the walk
        // visits root keys in sorted order, so a wide `project.*` difference can
        // consume the whole budget before `stations` or `stats` is looked at
        // once. A rule can only speak for what it saw; this is where that is
        // enforced.
        if truncated {
            self.fail(
                &format!("{what} did not restore the state recorded at depth {depth}"),
                &format!(
                    "  the difference is WIDER than the diff cap ({MAX_DIFFS} path(s)): what \
                     follows is a PREFIX, and nothing past it was examined - so nothing can \
                     account for this mismatch, and nothing is allowed to try.\n{}",
                    describe_diffs(&diffs)
                ),
            );
        }
        if diffs.is_empty() {
            self.outcome.float_noise += 1;
            // Remember WHAT the tolerance absorbed, and how much. This is the
            // only record that a path was ever inexact here, and the
            // accumulation rule below needs it to tell "this has been creeping
            // for fifty visits" from "this broke just now".
            let measured = numeric_drift(&expected, &actual);
            if !measured.is_empty() {
                let seen = self.drift.entry(depth).or_default();
                for (path, delta) in measured {
                    let worst = seen.entry(path).or_insert(0.0);
                    *worst = worst.max(delta);
                }
            }
            // The stored state is equal WITHIN TOLERANCE, so the derived views
            // are still compared strictly - the whole point of having them is
            // that a solve can amplify what the raw comparison let through.
            let worst = numeric_drift(&expected, &actual)
                .values()
                .fold(0.0_f64, |acc, delta| acc.max(*delta));
            self.expect_derived(
                depth,
                &format!("{what} (stored state within tolerance, worst input delta {worst:e})"),
                false,
            );
            return;
        }

        // FIVE subtractions ran here, each stripping one known bug's shape out of
        // the diffs before anything else looked at them, because each re-indexed
        // a list and buried every other difference of the same move under twenty
        // derived paths: finding 10's orphan-closure conversion, findings 4 and
        // 17's list reorders, finding 3's destroyed station-0 refs and their
        // cascade, and finding 16's misplaced shot. All five are gone - see the
        // deleted helper block above the synthetic starting project for the fix
        // behind each - so a merge, datum or comment disappearing across a
        // history move, either list coming back permuted, a closure that changed
        // lists and a shot in the wrong place are unclassified failures again.

        // A KNOWN BUG gets first refusal, BEFORE the epoch attribution below.
        //
        // The ordering was settled while finding 3 was open, and is kept: the two
        // overlapped on `section_offsets`, a reload re-solves the whole map, and
        // attributing first read finding 3's displacement as "the reload re-solved
        // offsets" - which silently swallowed the hit AND resynced the model to a
        // state carrying it, so the redo back up to that depth failed later with
        // nothing to explain it (seed 4, persistence). Classifying first
        // attributes a difference to the defect that caused it, which is what
        // makes the later comparison readable.
        let mut classified: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut unknown = Vec::new();
        for diff in &diffs {
            match classify(diff, &ctx) {
                Some(id) => *classified.entry(id).or_insert(0) += 1,
                None => unknown.push(diff.clone()),
            }
        }

        // ONLY a comparison that crosses an epoch boundary may subtract the
        // documented undo-less changes a save or a reload makes. Within an epoch
        // the oracle stays strict, which is what keeps a geometry bug from hiding
        // behind "a reload re-solves offsets".
        let crosses_epoch = self.epochs[depth] < self.epoch;
        if crosses_epoch {
            let before = unknown.len();
            unknown.retain(|diff| {
                !(self.outcome.saves > 0 && attributable_to_save(diff))
                    && !(self.outcome.reloads > 0 && attributable_to_reload(diff))
            });
            self.outcome.attributed += before - unknown.len();
            if unknown.is_empty() && classified.is_empty() {
                self.resync(depth, actual);
                self.expect_derived(depth, what, true);
                return;
            }
        }

        // ACCUMULATION, checked before the entry-shaped classifiers because it is
        // about the HISTORY of a path rather than about this move. A difference
        // is accumulated dust when every unexplained path is numeric, that path
        // has been inexact at THIS depth before, and it has GROWN since - which
        // is what "the tolerance absorbed a little each time and the little
        // added up" looks like from the outside. A path that was exact until now
        // is a fresh regression and is not this.
        if !unknown.is_empty() {
            let history = self.drift.get(&depth).cloned().unwrap_or_default();
            let current = numeric_drift(&expected, &actual);
            let accumulated = unknown.iter().all(|diff| {
                match (history.get(&diff.path), current.get(&diff.path)) {
                    (Some(was), Some(now)) => *now > *was && *was > 0.0,
                    _ => false,
                }
            });
            if accumulated {
                *self
                    .outcome
                    .hits
                    .entry("finding-19-tolerated-float-dust-accumulated-past-tolerance")
                    .or_insert(0) += 1;
                self.note(format!(
                    "  ^ accumulation at depth {depth}: {} path(s) crept past the tolerance",
                    unknown.len()
                ));
                // Re-baseline, so the NEXT accumulation is measured from here
                // rather than reported again every visit.
                self.drift.remove(&depth);
                self.resync(depth, actual);
                self.expect_derived(depth, what, true);
                return;
            }
        }

        if unknown.is_empty() && !classified.is_empty() {
            for id in classified.keys() {
                *self.outcome.hits.entry(id).or_insert(0) += 1;
            }
            // The move log is what a reader reconstructs a failure from, and a
            // classified hit is a MOVE with consequences - it resyncs the model.
            // A log that showed only the moves would make a later failure look
            // spontaneous, which is exactly how finding 15 came to be filed
            // separately from finding 3.
            let ids: Vec<&str> = classified.keys().copied().collect();
            self.note(format!(
                "  ^ classified at depth {depth}: {}",
                ids.join(", ")
            ));
            self.resync(depth, actual);
            self.expect_derived(depth, what, true);
            return;
        }

        // THREE structural signatures were checked here, over what the path rules
        // could not explain: finding 16's duplicated shot uuid, finding 8's
        // wrong-shot delete, and finding 8's cascade. All three are FIXED with
        // findings 8/12/13/16 - the restore paths address shots by `Shot::uuid` -
        // so an identity swap is an unexplained failure again, and the
        // `stale_corruption` latch that armed them is now a MEASUREMENT the move
        // log prints and nothing branches on.
        let entry_note = ctx_entry.as_ref().map_or_else(
            || "  consumed entry: (none - not a history move)".to_string(),
            |entry| {
                format!(
                    "  consumed entry: {} (pushed in epoch {}, run epoch {})\n  \
                     persisted ids: target={:?} anchor={:?}",
                    entry.command, entry.epoch, self.epoch, ctx.stale.target, ctx.stale.anchor,
                )
            },
        );
        self.fail(
            &format!("{what} did not restore the state recorded at depth {depth}"),
            &(format!(
                "  {} unexplained difference(s){}:\n{}",
                unknown.len(),
                if classified.is_empty() {
                    String::new()
                } else {
                    format!(
                        " (plus classified: {:?})",
                        classified.keys().collect::<Vec<_>>()
                    )
                },
                describe_diffs(&unknown),
            ) + "\n"
                + entry_note.as_str()),
        );
    }

    // Two latch helpers lived here and both went with the classifiers that read
    // them. `anchors_reverted` / `record_saved_anchors` measured whether a
    // restore had reverted the gauge pair a save captured, which armed findings
    // 1 and 11's cascade. `record_displacement` extended the trajectory finding
    // 3 had walked a section's offset along, so its cascade could recognise the
    // SAME ratchet later and nothing else. Every one of those findings is fixed.

    /// The two DERIVED-state oracles, run after the stored-state comparison has
    /// settled, and the consistency rule that makes them usable at all.
    ///
    /// `resynced` is the whole subtlety. When a classified known bug explains a
    /// difference, the model accepts the live state as the expectation at that
    /// depth - and the derived models MUST accept it at the same moment. A
    /// verdict model that kept the pre-bug expectation would report the same
    /// defect again at every later depth as a fresh integrity failure, which is
    /// chasing phantoms rather than finding bugs. The harness tier resolves this
    /// the same way (`undoRedoFuzz.ts`, `resolveMismatch` re-records the
    /// integrity verdict alongside the state).
    ///
    /// When the stored state MATCHED, both are compared strictly. That case is
    /// the one worth having: a verdict or a solved geometry that differs while
    /// the stored project agrees is either a checker that is not a function of
    /// the state, or a divergence the snapshot cannot see. At the harness tier
    /// exactly that caught a finding 7 manifestation nothing else did.
    fn expect_derived(&mut self, depth: usize, what: &str, resynced: bool) {
        let verdict = self.verdict();
        let adjusted = self.adjusted_geometry();
        if resynced {
            if depth < self.verdicts.len() {
                self.verdicts[depth] = verdict;
            }
            if depth < self.adjusted.len() {
                self.adjusted[depth] = adjusted;
            }
            return;
        }
        if let Some(expected) = self.verdicts.get(depth).cloned() {
            self.outcome.integrity_compared += 1;
            if expected != verdict {
                self.fail(
                    &format!(
                        "{what} restored the state but NOT the integrity verdict recorded at depth {depth}"
                    ),
                    &format!(
                        "  expected: {}\n  actual:   {}",
                        expected.describe(),
                        verdict.describe()
                    ),
                );
            }
        }
        if let Some(expected) = self.adjusted.get(depth).cloned() {
            self.outcome.adjusted_compared += 1;
            let diffs = adjusted_diffs(&expected, &adjusted);
            if !diffs.is_empty() {
                self.fail(
                    &format!(
                        "{what} restored the stored state but NOT the ADJUSTED geometry recorded at depth {depth}"
                    ),
                    &format!("  {}", diffs.join("\n  ")),
                );
            }
        }
    }

    /// RAW versus RESOLVED, on the same merges, in the same state.
    ///
    /// The app measures merge alignment two ways and issue #413's finding 14
    /// records that they disagree. This asserts they agree about the only thing
    /// either is used to decide: whether a merged pair is aligned. A pair that
    /// one measure calls sound (under a centimetre) while the other calls broken
    /// (past the app's own 5 cm `MERGE_ALIGNMENT_TOLERANCE_M`) is a contradiction
    /// no consumer can resolve - `debug_assert_merge_alignment` panics a test
    /// build on it while `check_project_integrity` tells the user nothing.
    ///
    /// Deliberately NOT "the two distances are equal": they are not supposed to
    /// be. The resolved measure folds in closure adjustment, which legitimately
    /// moves stations. Only the VERDICTS have to agree.
    ///
    /// A disagreement was a TOLERATED classified hit
    /// (`finding-18-raw-and-resolved-merge-alignment-disagree`) for as long as
    /// finding 14 was open, because finding 14's write-up is where the claim
    /// that the two measures disagree came from. Finding 14 is fixed, and the
    /// classifier is gone with it - but the CHECK stays and now FAILS, because
    /// the id never recorded a single hit in any mode, before the fix or after.
    /// Finding 14's disagreement lived INSIDE one command - the assert fires
    /// mid-`insert_shot_after` and the epilogue re-solves before it returns - so
    /// a per-move check could never see it. Anything this fires on from here is
    /// a persistent contradiction nobody has an explanation for, which is a
    /// failure and not a hit.
    fn expect_raw_and_resolved_agree(&mut self, what: &str) {
        let disagreements: Vec<String> = self
            .merge_measurements()
            .into_iter()
            .filter(|m| {
                (m.raw >= MERGE_ALIGNMENT_TOLERANCE_M && m.resolved < RAW_RESOLVED_AGREEMENT_M)
                    || (m.resolved >= MERGE_ALIGNMENT_TOLERANCE_M
                        && m.raw < RAW_RESOLVED_AGREEMENT_M)
            })
            .map(|m| {
                format!(
                    "merged {} is {:.6} m apart RAW and {:.6} m apart RESOLVED",
                    m.label, m.raw, m.resolved
                )
            })
            .collect();
        if disagreements.is_empty() {
            return;
        }
        self.fail(
            &format!(
                "{what} left raw and resolved disagreeing about {} merge(s)",
                disagreements.len()
            ),
            &format!("  {}", disagreements.join("\n  ")),
        );
    }

    /// Accept the live state as the model's expectation at `depth`. Only ever
    /// reached for a difference that was ENTIRELY explained.
    fn resync(&mut self, depth: usize, actual: String) {
        self.snapshots[depth] = actual;
        self.epochs[depth] = self.epoch;
    }

    /// ABSOLUTE well-formedness, checked after every move.
    ///
    /// Independent of the roundtrip comparison, and here for a reason the harness
    /// tier learned the hard way: `Project::debug_assert_merge_alignment` is
    /// `#[cfg(test)]`, so in THIS tier a misaligned merge eventually panics from
    /// inside whatever model function next touches it - a command away from the
    /// one that caused it, with no move log and no seed in the message. Checking
    /// alignment after every move turns that into an attributed finding.
    ///
    /// `check_merge_alignment` is the app's own checker
    /// (`models/project/integrity.rs`), so this asserts the app's own contract
    /// rather than a re-invented one.
    fn expect_merges_aligned(&mut self, what: &str) {
        // Measured BOTH ways, in one pass, so the failure payload can say what
        // the other checker thought of the same state. Issue #413's finding 14
        // asserts the two disagree; this is where that claim becomes a number.
        let broken: Vec<MergeMeasurement> = self
            .merge_measurements()
            .into_iter()
            .filter(|m| m.raw >= RAW_RESOLVED_AGREEMENT_M)
            .collect();
        if broken.is_empty() {
            return;
        }
        let messages: Vec<String> = broken
            .iter()
            .map(|m| {
                format!(
                    "merged {} is {:.6} m apart RAW ({:.6} m RESOLVED{})",
                    m.label,
                    m.raw,
                    m.resolved,
                    if m.resolved < RAW_RESOLVED_AGREEMENT_M {
                        " - which calls the same pair SOUND"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        // EVERY drifted pair fails, with no band and no precondition.
        //
        // FINDING 20 was classified here, by MECHANISM: a pair under the app's 5
        // cm reporting tolerance in a merge group holding no resolvable zero
        // datum was the exact state in which both repairs declined -
        // `realign_after_shot_edit` returned early for want of an anchor, and
        // the epilogue's `realign_merges` re-solved only past the 5 cm report.
        // Both halves are FIXED: the realign now anchors an unanchored group on
        // the edited section (models/project/mutations.rs) and the heal gate is
        // `Project::MERGE_ALIGNMENT_INVARIANT_M`, the same 1 cm the app's own
        // assert uses (models/project/offsets.rs). There is no longer a band
        // between "the app asserts this cannot exist" and "the app declines to
        // repair it", so there is nothing left for a classifier to name -
        // `group_is_unanchored` and the `drift_stop` latch went with it.
        self.fail(
            &format!("{what} left a station merge out of alignment"),
            &format!("  {}", messages.join("\n  ")),
        );
    }

    fn expect_stacks(&self, what: &str) {
        // Under the sliding window the app holds `cursor - floor` undo entries,
        // not `cursor`: everything below the floor was FIFO-trimmed. The redo
        // stack is bounded by the same limit (`enforce_depth_limit` trims BOTH).
        let (want_undo, want_redo) = (
            self.cursor - self.floor,
            (self.top - self.cursor).min(self.history_limit),
        );
        if self.undo_len() != want_undo || self.redo_len() != want_redo {
            self.fail(
                &format!("{what} left the history stacks in an unexpected shape"),
                &format!(
                    "  expected undo={} redo={}, got undo={} redo={}",
                    want_undo,
                    want_redo,
                    self.undo_len(),
                    self.redo_len()
                ),
            );
        }
        self.expect_entry_prints(what);
    }

    /// The ENTRY-CONTENT oracle, within-run half: every entry on either stack
    /// still SAYS what it said when it was pushed, at the depth it belongs to.
    ///
    /// The stacks are a map from depth to entry - undo item `i` is the entry that
    /// takes depth `cursor - i` to `cursor - i + 1`, redo item `j` the one that
    /// takes `cursor + j` to `cursor + j + 1` - and `entry_prints` is what the app
    /// said each of those was at push time. So this catches three things a length
    /// check and a project comparison both miss: an entry whose metadata was
    /// REWRITTEN by an unrelated move, two entries that swapped places while their
    /// effects stayed put, and a restore that put the right state back under the
    /// wrong label.
    ///
    /// Only the entry-intrinsic triple, deliberately: see [`entry_print`] for why
    /// that is pinnable and `preview_data` is not.
    fn expect_entry_prints(&self, what: &str) {
        let mut wrong: Vec<String> = Vec::new();
        let mut compare = |side: &str, depth: usize, actual: &UndoHistoryItem| {
            let expected = self.entry_prints.get(depth);
            let actual = entry_print(actual);
            if expected.is_some_and(|print| *print == actual) {
                return;
            }
            wrong.push(format!(
                "  {side} entry for depth {depth}\n    expected: {}\n    actual:   {actual}",
                expected.map_or("<no entry recorded at that depth>", String::as_str),
            ));
        };
        for (index, item) in self.state.get_undo_stack().iter().enumerate() {
            compare("undo", self.cursor.saturating_sub(index), item);
        }
        for (index, item) in self.state.get_redo_stack().iter().enumerate() {
            compare("redo", self.cursor + 1 + index, item);
        }
        if wrong.is_empty() {
            return;
        }
        let shown = if fuzz_verbose() { wrong.len() } else { 4 };
        self.fail(
            &format!("{what} left an undo entry describing itself differently"),
            &format!(
                "  {} entr{} what the app said at push time:\n{}",
                wrong.len(),
                if wrong.len() == 1 {
                    "y no longer matches"
                } else {
                    "ies no longer match"
                },
                wrong
                    .iter()
                    .take(shown)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        );
    }

    /// Everything the history dropdown would render, both stacks, in stack order.
    ///
    /// Labelled by side and index so a difference names the row a user would be
    /// looking at.
    fn history_rows(&self) -> Vec<HistoryRow> {
        let undo = self.state.get_undo_stack();
        let redo = self.state.get_redo_stack();
        // The bytes the save is about to write, or that the load just read, for
        // the same entries in the same order. Read straight off the stack so the
        // two halves of a row cannot drift apart: `serialize_entry` is what
        // `rows_to_persist` calls (core/undo/persistence.rs).
        let payloads: Vec<(String, String)> = self.state.undo_stack.lock().map_or_else(
            |_| Vec::new(),
            |stack| {
                stack
                    .undo_entries()
                    .chain(stack.redo_entries())
                    .map(|entry| {
                        (
                            serde_json::to_string(&entry.command).unwrap_or_default(),
                            serde_json::to_string(&entry.undo_data).unwrap_or_default(),
                        )
                    })
                    .collect()
            },
        );
        undo.iter()
            .enumerate()
            .map(|(index, item)| (format!("undo[{index}]"), item))
            .chain(
                redo.iter()
                    .enumerate()
                    .map(|(index, item)| (format!("redo[{index}]"), item)),
            )
            .enumerate()
            .map(|(position, (label, item))| {
                let (command, undo_data) = payloads.get(position).cloned().unwrap_or_default();
                HistoryRow {
                    label,
                    item: history_item_text(item),
                    command,
                    undo_data,
                }
            })
            .collect()
    }

    /// The ENTRY-CONTENT oracle, PERSISTENCE half: a save/close/reopen restores
    /// the history dropdown itself, not merely the entries' effects.
    ///
    /// Compared field for field over the WHOLE `UndoHistoryItem` - index,
    /// description, timestamp, command type and the entire `preview_data` tree -
    /// on both stacks, with NOTHING masked. Three reasons nothing needs to be:
    ///
    /// * The persisted row is the entry's own `command` and `undo_data` JSON plus
    ///   an RFC 3339 timestamp (`UndoStack::serialize_entry_with_timestamp`), and
    ///   the loader rebuilds the entry from exactly those - so description,
    ///   command type and preview all re-derive from bytes that made the round
    ///   trip. `command_bytes` is `#[serde(skip_serializing)]` and feeds nothing
    ///   the dropdown shows.
    /// * The timestamp is NOT wall-clock noise here, unlike the metamorphic
    ///   engine's `merged_at`: it is not re-taken on load, it is parsed back from
    ///   the column, and `UndoStack::next_stamp` depends on it staying exact
    ///   (#303's push cutoff is a comparison against it). Masking it would erase
    ///   the field a persistence bug is most likely to round off. Measured: over
    ///   the four gates and a 40-seed survey, not one timestamp moved.
    /// * The loader's shot-id renumbering, which `canonicalize` has to normalize
    ///   for the project snapshot, cannot reach here: an entry's ids travel INSIDE
    ///   its serialized command, so they come back exactly as they went in. That
    ///   they are then STALE against the reloaded project is findings 8/12/13, and
    ///   is a defect in what the entry DOES, not in what it says.
    ///
    /// The baseline is read BEFORE the save rather than after it, so a save-time
    /// mutation of the stacks is inside the oracle rather than under it.
    ///
    /// Each row still carries the BYTES the file would hold for it as well as
    /// the rendering, and that pair is now purely diagnostic: it used to tell
    /// the two KNOWN causes of a difference apart - identical bytes with a
    /// different rendering could only be a lossy PARSE (finding 5), and
    /// differing bytes within the module's numeric tolerance were the history
    /// payload's float dust (finding 23) - and both are fixed, so a row that
    /// reads differently after a reload FAILS whatever its bytes did. The
    /// payloads go in the failure message, which is where a reader wants them.
    fn expect_history_rows(&mut self, before: &[HistoryRow]) {
        let after = self.history_rows();
        if after.len() != before.len() {
            // `expect_stacks` has already pinned both lengths by the time this
            // runs, so reaching here means the two stacks disagree in a way the
            // length model does not see. Report it rather than index past it.
            self.fail(
                "the reload restored a different NUMBER of history items than the save wrote",
                &format!("  before {} item(s), after {}", before.len(), after.len()),
            );
        }
        let mut unexplained: Vec<String> = Vec::new();
        for (was, now) in before.iter().zip(&after) {
            if was.item == now.item {
                continue;
            }
            unexplained.push(format!(
                "  {}\n    before save:  {}\n    after reload: {}\n    \
                 command before: {}\n    command after:  {}\n    \
                 undo_data before: {}\n    undo_data after:  {}",
                was.label,
                was.item,
                now.item,
                was.command,
                now.command,
                was.undo_data,
                now.undo_data,
            ));
        }
        if !unexplained.is_empty() {
            let shown = if fuzz_verbose() { unexplained.len() } else { 3 };
            self.fail(
                "the reload restored the history entries with different METADATA",
                &format!(
                    "  {} history item(s) differ:\n{}",
                    unexplained.len(),
                    unexplained
                        .iter()
                        .take(shown)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            );
        }
    }
}

/// One drawn mutation: what to run, and a label for the move log.
struct Drawn {
    label: String,
    result: CommandResult,
    // `appended_into_empty` - "this was a `CreateShot` that appended the FIRST
    // shot into an empty section", which only the generator could know - was
    // finding 3's precondition and went with its classifiers.
    /// The shot this command's entry will address by id when it is restored.
    target: Option<ShotTarget>,
    /// The shot a `CreateShot`'s redo will anchor its re-insert to.
    anchor: Option<ShotTarget>,
}

/// What one `Op` did to the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepOutcome {
    /// The command ran (whether the app accepted it or refused it).
    Applied,
    /// A reference no longer resolves, because the move that created its target
    /// is not in this candidate trace. Only ever reachable in a REPLAY.
    Skipped,
    /// The app's own merge-alignment assert fired. Nothing after it measures the
    /// app - see [`Fuzzer::catching_alignment`].
    Stopped,
}

impl Drawn {
    fn new(label: String, result: CommandResult) -> Self {
        Self {
            label,
            result,
            target: None,
            anchor: None,
        }
    }
}

impl Fuzzer {
    /// Draw one mutation. Returns `None` when nothing was buildable (the project
    /// momentarily has no target of the drawn kind).
    ///
    /// DRAWS ONLY. What the generators return is an [`Op`] - a value, with every
    /// reference symbolic - and [`Self::apply_op`] is what executes it. The split
    /// is what makes a run replayable: the recorded `Op` goes back through the
    /// same `apply_op`, so a shrunk trace is executed by the same code that
    /// executed the original.
    fn draw_op(&mut self, targets: &Targets) -> Option<Op> {
        // Weights keep the cheap structural edits dominant while still reaching
        // the graph operations, which is where the interesting undo bugs live.
        // The delete family is held down so a long run does not empty the project.
        let kind = self.rng.below(100);
        match kind {
            0..=9 => self.gen_create_section(),
            10..=21 => self.gen_create_shot(targets),
            22..=35 => self.gen_update_shot(targets),
            36..=43 => self.gen_set_shot_kind(targets),
            44..=50 => self.gen_update_section(targets),
            51..=55 => self.gen_delete_shot(targets),
            56..=58 => self.gen_delete_section(targets),
            59..=63 => self.gen_reverse_section(targets),
            64..=69 => self.gen_move(targets),
            70..=74 => self.gen_move_station(targets),
            75..=79 => self.gen_station_comment(targets),
            80..=83 => self.gen_merge(targets),
            84..=86 => self.gen_unmerge(targets),
            87..=89 => self.gen_closure(targets),
            90..=91 => self.gen_zero_datum(targets),
            92..=95 => self.gen_split_family(targets),
            _ => self.gen_batch(targets),
        }
    }

    /// A live shot, as a reference the trace can carry.
    fn shot_ref(targets: &Targets, section_id: SectionId, shot_id: ShotId) -> ShotRefT {
        ShotRefT {
            section: section_id,
            id: shot_id,
            uuid: targets.uuid_of(section_id, shot_id),
        }
    }

    fn gen_create_section(&mut self) -> Option<Op> {
        // Includes the EMPTY section deliberately: finding 3 is literally "append
        // the first shot into an empty section", and a uniform generator reaches
        // that shape rarely.
        let count = self.rng.below(4);
        let shots: Vec<ShotData> = (0..count).map(|_| random_shot(&mut self.rng)).collect();
        let null_metadata = self.rng.chance(30);
        Some(Op::CreateSection {
            name: (!null_metadata).then(|| format!("fuzz-{}", self.rng.range(1000, 9999))),
            surveyor: (!null_metadata).then(|| "fuzz".to_string()),
            color: (*self.rng.pick(COLORS)).to_string(),
            shots,
        })
    }

    fn gen_create_shot(&mut self, targets: &Targets) -> Option<Op> {
        if targets.sections.is_empty() {
            return None;
        }
        let index = self.rng.below(targets.sections.len());
        let (section_id, shots) = targets.sections[index].clone();
        let shot = random_shot(&mut self.rng);
        // A mid-section insert is what makes shot ids diverge from positional
        // order, which is the precondition for findings 8, 12 and 13.
        let (after, before) = if shots.is_empty() {
            (None, None)
        } else {
            match self.rng.below(3) {
                0 => (Some(shots[self.rng.below(shots.len())]), None),
                1 => (None, Some(shots[self.rng.below(shots.len())])),
                _ => (None, None),
            }
        };
        Some(Op::CreateShot {
            section: section_id,
            shot: Box::new(shot),
            after: after.map(|shot_id| Self::shot_ref(targets, section_id, shot_id)),
            before: before.map(|shot_id| Self::shot_ref(targets, section_id, shot_id)),
        })
    }

    /// `(section, shot, uuid)` for a live shot, or `None` when the id addresses
    /// nothing.
    fn shot_target(&self, section_id: SectionId, shot_id: ShotId) -> Option<ShotTarget> {
        let uuid = self.uuid_of(section_id, shot_id)?;
        Some(ShotTarget {
            section_id,
            shot_id,
            uuid,
        })
    }

    /// The uuid the id `shot_id` addresses in `section_id` RIGHT NOW.
    fn uuid_of(&self, section_id: SectionId, shot_id: ShotId) -> Option<String> {
        self.state
            .get_project_state()?
            .sections
            .iter()
            .find(|section| section.id == section_id)?
            .shots
            .iter()
            .find(|shot| shot.id == shot_id)?
            .uuid
            .clone()
    }

    /// A shot picked from a section that has one, with its section.
    fn pick_shot(&mut self, targets: &Targets) -> Option<(SectionId, ShotId)> {
        let populated: Vec<&(SectionId, Vec<ShotId>)> = targets
            .sections
            .iter()
            .filter(|(_, shots)| !shots.is_empty())
            .collect();
        if populated.is_empty() {
            return None;
        }
        let (section_id, shots) = populated[self.rng.below(populated.len())];
        Some((*section_id, shots[self.rng.below(shots.len())]))
    }

    fn gen_update_shot(&mut self, targets: &Targets) -> Option<Op> {
        let (section_id, shot_id) = self.pick_shot(targets)?;
        let mut updates = ShotUpdates::default();
        // Each field independently, so both the "set" and the "clear" arms of the
        // nullable ones are exercised.
        if self.rng.chance(50) {
            updates.length = Some(self.rng.float(0.5, 20.0));
        }
        if self.rng.chance(40) {
            updates.azimuth_in = Some(self.rng.float(0.0, 359.9));
        }
        if self.rng.chance(30) {
            updates.depth_out = Some(self.rng.float(0.0, 40.0));
        }
        if self.rng.chance(30) {
            updates.pitch_in = Some(self.rng.chance(50).then(|| self.rng.float(-60.0, 60.0)));
        }
        if self.rng.chance(30) {
            updates.left = Some(self.rng.chance(50).then(|| self.rng.float(0.0, 5.0)));
        }
        if self.rng.chance(25) {
            updates.comment = Some(
                self.rng
                    .chance(50)
                    .then(|| format!("c{}", self.rng.range(0, 999))),
            );
        }
        Some(Op::UpdateShot {
            shot: Self::shot_ref(targets, section_id, shot_id),
            updates: Box::new(updates),
        })
    }

    fn gen_set_shot_kind(&mut self, targets: &Targets) -> Option<Op> {
        let (section_id, shot_id) = self.pick_shot(targets)?;
        let kind = *self
            .rng
            .pick(&[ShotKind::Normal, ShotKind::Jump, ShotKind::Reference]);
        Some(Op::SetShotKind {
            shot: Self::shot_ref(targets, section_id, shot_id),
            kind,
        })
    }

    fn gen_update_section(&mut self, targets: &Targets) -> Option<Op> {
        if targets.sections.is_empty() {
            return None;
        }
        let section_id = targets.sections[self.rng.below(targets.sections.len())].0;
        let mut updates = SectionUpdates::default();
        if self.rng.chance(45) {
            updates.name = Some(
                self.rng
                    .chance(60)
                    .then(|| format!("r{}", self.rng.range(0, 999))),
            );
        }
        if self.rng.chance(45) {
            updates.surveyor = Some(
                self.rng
                    .chance(60)
                    .then(|| format!("s{}", self.rng.range(0, 9))),
            );
        }
        if self.rng.chance(40) {
            updates.color = Some((*self.rng.pick(COLORS)).to_string());
        }
        if self.rng.chance(30) {
            updates.quality = Some(self.rng.range(1, 5) as f64);
        }
        Some(Op::UpdateSection {
            section: section_id,
            updates: Box::new(updates),
        })
    }

    fn gen_delete_shot(&mut self, targets: &Targets) -> Option<Op> {
        let (section_id, shot_id) = self.pick_shot(targets)?;
        Some(Op::DeleteShot {
            shot: Self::shot_ref(targets, section_id, shot_id),
        })
    }

    fn gen_delete_section(&mut self, targets: &Targets) -> Option<Op> {
        // Keep at least two sections so cross-section merges stay reachable.
        if targets.sections.len() <= 2 {
            return None;
        }
        let section_id = targets.sections[self.rng.below(targets.sections.len())].0;
        Some(Op::DeleteSection {
            section: section_id,
        })
    }

    fn gen_reverse_section(&mut self, targets: &Targets) -> Option<Op> {
        if targets.sections.is_empty() {
            return None;
        }
        let section_id = targets.sections[self.rng.below(targets.sections.len())].0;
        let swap = self.rng.chance(50);
        Some(Op::ReverseSection {
            section: section_id,
            swap_left_right: swap,
        })
    }

    fn gen_move(&mut self, targets: &Targets) -> Option<Op> {
        if targets.sections.is_empty() {
            return None;
        }
        let section_id = targets.sections[self.rng.below(targets.sections.len())].0;
        let offset = random_offset(&mut self.rng);
        let segment = self.rng.chance(50);
        Some(Op::Move {
            section: section_id,
            offset,
            segment,
        })
    }

    fn gen_move_station(&mut self, targets: &Targets) -> Option<Op> {
        if targets.stations.is_empty() {
            return None;
        }
        let (section_id, index) = targets.stations[self.rng.below(targets.stations.len())];
        Some(Op::MoveStation {
            station: StationRefT::new(section_id, index),
            offset: random_offset(&mut self.rng),
        })
    }

    fn gen_station_comment(&mut self, targets: &Targets) -> Option<Op> {
        if targets.stations.is_empty() {
            return None;
        }
        let (section_id, index) = targets.stations[self.rng.below(targets.stations.len())];
        let comment = self
            .rng
            .chance(70)
            .then(|| format!("note-{}", self.rng.range(0, 9999)));
        Some(Op::StationComment {
            station: StationRefT::new(section_id, index),
            comment,
        })
    }

    /// Two stations in DIFFERENT sections: merging or closing within one section
    /// is rejected up front, so cross-section pairs are the ones that exercise the
    /// undo path.
    fn pick_cross_pair(
        &mut self,
        targets: &Targets,
    ) -> Option<((SectionId, u32), (SectionId, u32))> {
        if targets.sections.len() < 2 || targets.stations.len() < 2 {
            return None;
        }
        let a = targets.stations[self.rng.below(targets.stations.len())];
        let candidates: Vec<(SectionId, u32)> = targets
            .stations
            .iter()
            .copied()
            .filter(|station| station.0 != a.0)
            .collect();
        if candidates.is_empty() {
            return None;
        }
        Some((a, candidates[self.rng.below(candidates.len())]))
    }

    fn gen_merge(&mut self, targets: &Targets) -> Option<Op> {
        let (a, b) = self.pick_cross_pair(targets)?;
        Some(Op::Merge {
            a: StationRefT::new(a.0, a.1),
            b: StationRefT::new(b.0, b.1),
        })
    }

    fn gen_unmerge(&mut self, targets: &Targets) -> Option<Op> {
        if targets.merges.is_empty() {
            return None;
        }
        let (a, b) = targets.merges[self.rng.below(targets.merges.len())];
        Some(Op::Unmerge {
            a: StationRefT::new(a.0, a.1),
            b: StationRefT::new(b.0, b.1),
        })
    }

    fn gen_closure(&mut self, targets: &Targets) -> Option<Op> {
        if !self.closures {
            return None;
        }
        if !targets.closures.is_empty() && self.rng.chance(40) {
            let closure_id = targets.closures[self.rng.below(targets.closures.len())];
            return Some(Op::RemoveClosure {
                closure: closure_id,
            });
        }
        let (a, b) = self.pick_cross_pair(targets)?;
        // `ProportionalByLength` / `EqualDistribution` are rejected outright by
        // `create_loop_closure`, so they are drawn only occasionally: a rejected
        // command is a case the model checks, just not an interesting one to spend
        // most draws on.
        let strategy = *self.rng.pick(&[
            ClosureStrategy::Move,
            ClosureStrategy::Move,
            ClosureStrategy::LeastSquaresWeighted,
            ClosureStrategy::ProportionalByLength,
        ]);
        Some(Op::CreateClosure {
            a: StationRefT::new(a.0, a.1),
            b: StationRefT::new(b.0, b.1),
            strategy,
        })
    }

    fn gen_zero_datum(&mut self, targets: &Targets) -> Option<Op> {
        if !targets.datums.is_empty() && self.rng.chance(50) {
            let (section_id, index) = targets.datums[self.rng.below(targets.datums.len())];
            return Some(Op::RemoveZeroDatum {
                station: StationRefT::new(section_id, index),
            });
        }
        if targets.stations.is_empty() {
            return None;
        }
        let (section_id, index) = targets.stations[self.rng.below(targets.stations.len())];
        let gps = GpsCoordinates::new(
            self.rng.float(20.2, 20.5),
            self.rng.float(-87.5, -87.2),
            self.rng.float(0.0, 5.0),
        )
        .ok()?;
        Some(Op::SetZeroDatum {
            station: StationRefT::new(section_id, index),
            coordinates: gps,
        })
    }

    /// Combine Sections at a section end: the join the last Split Section Here
    /// made, most of the time while there is one (split-then-combine is the
    /// inverse pair), otherwise either end of a random section.
    fn gen_combine_sections(&mut self, targets: &Targets) -> Option<Op> {
        let (section_id, index) = match self.last_split_join {
            Some(join) if self.rng.chance(60) => join,
            _ => {
                if targets.sections.is_empty() {
                    return None;
                }
                let (id, shots) = &targets.sections[self.rng.below(targets.sections.len())];
                let index = if self.rng.chance(50) {
                    0
                } else {
                    shots.len() as u32
                };
                (*id, index)
            }
        };
        Some(Op::CombineSections {
            station: StationRefT::new(section_id, index),
            keep_b: self.rng.chance(30),
            swap_lr: self.rng.chance(50),
            pick: self.rng.below(3) as u8,
        })
    }

    /// The delete/split family, which are `AppState` entry points rather than
    /// `Command` variants and so never see `execute()`'s epilogue - which is
    /// exactly what finding 10 is about.
    fn gen_split_family(&mut self, targets: &Targets) -> Option<Op> {
        let (section_id, shot_id) = self.pick_shot(targets)?;
        let shots = targets
            .sections
            .iter()
            .find(|(id, _)| *id == section_id)
            .map(|(_, shots)| shots.clone())
            .unwrap_or_default();
        match self.rng.below(7) {
            0 => {
                // Distances AT the boundaries are the interesting ones: a split
                // there degenerates into a zero-length shot.
                let distance = if self.rng.chance(20) {
                    *self.rng.pick(&[0.0, 1e-9, GEOMETRY_EXTREME])
                } else {
                    self.rng.float(0.1, 8.0)
                };
                Some(Op::SplitShotAtDistance {
                    shot: Self::shot_ref(targets, section_id, shot_id),
                    distance,
                })
            }
            1 => Some(Op::DeleteShotAndSplit {
                shot: Self::shot_ref(targets, section_id, shot_id),
            }),
            2 => Some(Op::DeleteLastShotAndSection {
                shot: Self::shot_ref(targets, section_id, shot_id),
            }),
            3 => Some(Op::SplitSection {
                shot: Self::shot_ref(targets, section_id, shot_id),
                along: self.rng.chance(50),
                distance: if self.rng.chance(20) {
                    *self.rng.pick(&[0.0, 1e-9, 0.005])
                } else {
                    self.rng.float(0.1, 8.0)
                },
                side: self.rng.below(6) as u8,
            }),
            4 => self.gen_combine_sections(targets),
            _ => {
                if shots.len() < 2 {
                    return None;
                }
                let first = self.rng.below(shots.len());
                let second = self.rng.below(shots.len());
                let (lo, hi) = (first.min(second), first.max(second));
                let split = self.rng.chance(50);
                Some(Op::DeleteShotRange {
                    section: section_id,
                    from: Self::shot_ref(targets, section_id, shots[lo]),
                    to: Self::shot_ref(targets, section_id, shots[hi]),
                    split,
                })
            }
        }
    }

    /// A batch is ONE undo entry covering several sub-commands, with all-or-nothing
    /// rollback - the only way to reach `execute_batch`'s snapshot-pair path and
    /// its rollback guard.
    fn gen_batch(&mut self, targets: &Targets) -> Option<Op> {
        let count = self.rng.range(2, 4);
        let mut commands = Vec::new();
        for _ in 0..count {
            let built = match self.rng.below(4) {
                0 => {
                    self.pick_shot(targets)
                        .map(|(section_id, shot_id)| BatchOp::UpdateShotLength {
                            shot: Self::shot_ref(targets, section_id, shot_id),
                            length: self.rng.float(1.0, 15.0),
                        })
                }
                1 => (!targets.sections.is_empty()).then(|| {
                    let index = self.rng.below(targets.sections.len());
                    BatchOp::MoveSection {
                        section: targets.sections[index].0,
                        offset: random_offset(&mut self.rng),
                    }
                }),
                2 => (!targets.stations.is_empty()).then(|| {
                    let index = self.rng.below(targets.stations.len());
                    let (section_id, station) = targets.stations[index];
                    BatchOp::MoveStation {
                        station: StationRefT::new(section_id, station),
                        offset: random_offset(&mut self.rng),
                    }
                }),
                _ => (!targets.sections.is_empty()).then(|| {
                    let index = self.rng.below(targets.sections.len());
                    BatchOp::ReverseSection {
                        section: targets.sections[index].0,
                        swap_left_right: self.rng.chance(50),
                    }
                }),
            };
            if let Some(command) = built {
                commands.push(command);
            }
        }
        if commands.is_empty() {
            return None;
        }
        // The case the rollback exists for: the last sub-command names a section
        // id that cannot exist, so the batch must fail, leave state untouched and
        // push nothing.
        let poisoned = self.rng.chance(20);
        Some(Op::Batch { commands, poisoned })
    }

    // -----------------------------------------------------------------------
    // The one execution path
    // -----------------------------------------------------------------------

    /// Execute one [`Op`] and gather the metadata the classifiers key on.
    ///
    /// EVERY mutation goes through here: a drawn one with [`Refs::identity`],
    /// which resolves every reference to itself and can never skip, and a
    /// replayed one with a mapping resolver, which can. That is what makes a
    /// shrunk trace a faithful repro rather than an approximation - there is no
    /// second implementation of any command for it to differ from.
    ///
    /// `None` means INAPPLICABLE: a reference the resolver could not map,
    /// because the move that created its target is no longer in the trace. The
    /// move is skipped and the run continues, which is what ddmin needs - a
    /// candidate that killed the harness would be indistinguishable from one
    /// that reproduced the bug.
    fn apply_op(&mut self, op: &Op, refs: &mut Refs, targets: &Targets) -> Option<Drawn> {
        match op {
            Op::CreateSection {
                name,
                surveyor,
                color,
                shots,
            } => Some(Drawn::new(
                format!(
                    "createSection(shots={},nullMeta={})",
                    shots.len(),
                    name.is_none()
                ),
                self.state.execute(Command::CreateSection {
                    name: name.clone(),
                    surveyor: surveyor.clone(),
                    date: None,
                    color: color.clone(),
                    direction: None,
                    shots: shots.clone(),
                }),
            )),
            Op::CreateShot {
                section,
                shot,
                after,
                before,
            } => {
                let Some(section_id) = refs.section(*section, targets) else {
                    return refs.skip(&op.describe());
                };
                let anchor = match (after, before) {
                    (Some(anchor), _) | (None, Some(anchor)) => match refs.shot(anchor, targets) {
                        Some((_, shot_id)) => Some(shot_id),
                        None => return refs.skip(&op.describe()),
                    },
                    (None, None) => None,
                };
                let (after_shot_id, before_shot_id) = if after.is_some() {
                    (anchor, None)
                } else {
                    (None, anchor)
                };
                let live = targets.shots_of(section_id).unwrap_or_default();
                let how = if after_shot_id.is_some() {
                    "after"
                } else if before_shot_id.is_some() {
                    "before"
                } else {
                    "append"
                };
                // The anchor's identity is read BEFORE the insert, because that
                // is the shot the persisted `after_shot_id` / `before_shot_id`
                // means. After a reload the same id may address a different one -
                // which is what makes the REDO re-insert somewhere else.
                let anchor_target =
                    anchor.and_then(|shot_id| self.shot_target(section_id, shot_id));
                let mut drawn = Drawn::new(
                    format!("createShot(s={section_id},n={},{how})", live.len()),
                    self.state.execute(Command::CreateShot {
                        section_id,
                        shot: (**shot).clone(),
                        after_shot_id,
                        before_shot_id,
                    }),
                );
                drawn.anchor = anchor_target;
                // And the shot the UNDO will delete by id, read after the insert.
                drawn.target = drawn
                    .result
                    .shot_id
                    .and_then(|shot_id| self.shot_target(section_id, shot_id));
                Some(drawn)
            }
            Op::UpdateShot { shot, updates } => {
                let Some((section_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&op.describe());
                };
                let target = self.shot_target(section_id, shot_id);
                let mut drawn = Drawn::new(
                    format!("updateShot(s={section_id},shot={shot_id})"),
                    self.state.execute(Command::UpdateShot {
                        section_id,
                        shot_id,
                        updates: (**updates).clone(),
                    }),
                );
                // `UndoData::PreviousValues { section_id, shot_id, .. }` - the
                // undo calls `update_shot` with this id, and `redo_inner`'s
                // catch-all re-executes the command with it too. Finding 12's
                // two halves.
                drawn.target = target;
                Some(drawn)
            }
            Op::SetShotKind { shot, kind } => {
                let Some((section_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&op.describe());
                };
                let target = self.shot_target(section_id, shot_id);
                let mut drawn = Drawn::new(
                    format!("setShotKind(s={section_id},shot={shot_id},{kind:?})"),
                    self.state.execute(Command::SetShotKind {
                        section_id,
                        shot_id,
                        kind: *kind,
                    }),
                );
                // `UndoData::InverseCommand(Command::SetShotKind { shot_id, .. })`,
                // which names the shot by its in-session id on BOTH the undo and
                // the redo.
                drawn.target = target;
                Some(drawn)
            }
            Op::UpdateSection { section, updates } => {
                let Some(section_id) = refs.section(*section, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("updateSection(s={section_id})"),
                    self.state.execute(Command::UpdateSectionMetadata {
                        section_id,
                        updates: (**updates).clone(),
                    }),
                ))
            }
            Op::DeleteShot { shot } => {
                let Some((section_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("deleteShot(s={section_id},shot={shot_id})"),
                    self.state.execute(Command::DeleteShot {
                        section_id,
                        shot_id,
                    }),
                ))
            }
            Op::DeleteSection { section } => {
                let Some(section_id) = refs.section(*section, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("deleteSection(s={section_id})"),
                    self.state.execute(Command::DeleteSection { section_id }),
                ))
            }
            Op::ReverseSection {
                section,
                swap_left_right,
            } => {
                let Some(section_id) = refs.section(*section, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("reverseSection(s={section_id},swapLR={swap_left_right})"),
                    self.state.execute(Command::ReverseSection {
                        section_id,
                        swap_left_right: *swap_left_right,
                    }),
                ))
            }
            Op::Move {
                section,
                offset,
                segment,
            } => {
                let Some(section_id) = refs.section(*section, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!(
                        "{}(s={section_id})",
                        if *segment {
                            "moveSegment"
                        } else {
                            "moveSection"
                        }
                    ),
                    self.state.execute(if *segment {
                        Command::MoveSegment {
                            section_id,
                            offset: *offset,
                        }
                    } else {
                        Command::MoveSection {
                            section_id,
                            offset: *offset,
                        }
                    }),
                ))
            }
            Op::MoveStation { station, offset } => {
                let Some((section_id, index)) = refs.station(*station, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("moveStation({section_id}/{index})"),
                    self.state.execute(Command::MoveStation {
                        station_ref: StationRef::new(section_id, index),
                        offset: *offset,
                    }),
                ))
            }
            Op::StationComment { station, comment } => {
                let Some((section_id, index)) = refs.station(*station, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("stationComment({section_id}/{index},{})", comment.is_some()),
                    self.state.execute(Command::SetStationComment {
                        station_ref: StationRef::new(section_id, index),
                        comment: comment.clone(),
                    }),
                ))
            }
            Op::Merge { a, b } => {
                let (Some(left), Some(right)) =
                    (refs.station(*a, targets), refs.station(*b, targets))
                else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!(
                        "mergeStations({}/{},{}/{})",
                        left.0, left.1, right.0, right.1
                    ),
                    self.state.execute(Command::MergeStations {
                        station_a: StationRef::new(left.0, left.1),
                        station_b: StationRef::new(right.0, right.1),
                    }),
                ))
            }
            Op::Unmerge { a, b } => {
                let (Some(left), Some(right)) =
                    (refs.station(*a, targets), refs.station(*b, targets))
                else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("unmergePair({}/{},{}/{})", left.0, left.1, right.0, right.1),
                    self.state.execute(Command::UnmergeStationPair {
                        station_a: StationRef::new(left.0, left.1),
                        station_b: StationRef::new(right.0, right.1),
                    }),
                ))
            }
            Op::CreateClosure { a, b, strategy } => {
                let (Some(left), Some(right)) =
                    (refs.station(*a, targets), refs.station(*b, targets))
                else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!(
                        "createLoopClosure({}/{},{}/{})",
                        left.0, left.1, right.0, right.1
                    ),
                    self.state.execute(Command::CreateLoopClosure {
                        station_a: StationRef::new(left.0, left.1),
                        station_b: StationRef::new(right.0, right.1),
                        closure_strategy: *strategy,
                    }),
                ))
            }
            Op::RemoveClosure { closure } => {
                let Some(closure_id) = refs.closure(*closure, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("removeLoopClosure({closure_id})"),
                    self.state
                        .execute(Command::RemoveLoopClosure { closure_id }),
                ))
            }
            Op::SetZeroDatum {
                station,
                coordinates,
            } => {
                let Some((section_id, index)) = refs.station(*station, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("setZeroDatum({section_id}/{index})"),
                    self.state.execute(Command::SetZeroDatum {
                        station_ref: StationRef::new(section_id, index),
                        coordinates: *coordinates,
                    }),
                ))
            }
            Op::RemoveZeroDatum { station } => {
                let Some((section_id, index)) = refs.station(*station, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("removeZeroDatum({section_id}/{index})"),
                    self.state.execute(Command::RemoveZeroDatum {
                        station_ref: StationRef::new(section_id, index),
                    }),
                ))
            }
            Op::SplitShotAtDistance { shot, distance } => {
                let Some((section_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("splitShotAtDistance(s={section_id},shot={shot_id})"),
                    self.state
                        .execute_split_shot_at_distance(section_id, shot_id, *distance),
                ))
            }
            Op::DeleteShotAndSplit { shot } => {
                let Some((section_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("deleteShotAndSplit(s={section_id},shot={shot_id})"),
                    self.state.execute_delete_shot_and_split(
                        section_id,
                        shot_id,
                        "split-a".to_string(),
                        "split-b".to_string(),
                    ),
                ))
            }
            Op::SplitSection {
                shot,
                along,
                distance,
                side,
            } => {
                let Some((section_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&op.describe());
                };
                let at = if *along {
                    crate::models::SplitAt::Shot {
                        shot_id,
                        distance: *distance,
                    }
                } else {
                    let position = self
                        .state
                        .get_project_state()
                        .and_then(|p| {
                            p.get_section(section_id)?
                                .shots
                                .iter()
                                .position(|s| s.id == shot_id)
                        })
                        .unwrap_or(0);
                    crate::models::SplitAt::Station {
                        station_index: position as u32 + 1,
                    }
                };
                let pick = |n: u8| match n {
                    0 => crate::models::SplitSide::A,
                    1 => crate::models::SplitSide::B,
                    _ => crate::models::SplitSide::Both,
                };
                let options = crate::models::SplitSectionOptions {
                    name_a: "split-a".to_string(),
                    name_b: "split-b".to_string(),
                    color_a: "#111111".to_string(),
                    color_b: "#222222".to_string(),
                    comment_side: pick(side % 3),
                    datum_side: pick(side % 2),
                };
                let result = self.state.execute_split_section(section_id, at, options);
                if result.success {
                    self.last_split_join = self.state.get_project_state().and_then(|p| {
                        let first = p.get_section(section_id)?;
                        Some((section_id, first.shots.len() as u32))
                    });
                }
                Some(Drawn::new(
                    format!("splitSection(s={section_id},at={at:?})"),
                    result,
                ))
            }
            Op::CombineSections {
                station,
                keep_b,
                swap_lr,
                pick,
            } => {
                let Some((section_id, index)) = refs.station(*station, targets) else {
                    return refs.skip(&op.describe());
                };
                // At a junction, any partner: the fuzz picks the first.
                let Some(candidate) = self
                    .state
                    .combine_candidates(StationRef::new(section_id, index))
                    .ok()
                    .and_then(|candidates| candidates.into_iter().next())
                else {
                    return refs.skip(&op.describe());
                };
                let (kept, other) = if *keep_b {
                    (candidate.section_b, candidate.section_a)
                } else {
                    (candidate.section_a, candidate.section_b)
                };
                let pick = match pick % 3 {
                    0 => crate::models::CombinePick::Kept,
                    1 => crate::models::CombinePick::Other,
                    _ => crate::models::CombinePick::Both,
                };
                let options = crate::models::CombineSectionsOptions {
                    name: "combined".to_string(),
                    color: "#333333".to_string(),
                    swap_lr: *swap_lr,
                    comment_pick: pick,
                    name_pick: pick,
                    join: None,
                };
                Some(Drawn::new(
                    format!("combineSections(kept={kept},other={other})"),
                    self.state.execute_combine_sections(kept, other, options),
                ))
            }
            Op::DeleteLastShotAndSection { shot } => {
                let Some((section_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!("deleteLastShotAndSection(s={section_id},shot={shot_id})"),
                    self.state
                        .execute_delete_last_shot_and_section(section_id, shot_id),
                ))
            }
            Op::DeleteShotRange {
                section,
                from,
                to,
                split,
            } => {
                let Some(section_id) = refs.section(*section, targets) else {
                    return refs.skip(&op.describe());
                };
                let (Some((_, lo)), Some((_, hi))) =
                    (refs.shot(from, targets), refs.shot(to, targets))
                else {
                    return refs.skip(&op.describe());
                };
                Some(Drawn::new(
                    format!(
                        "{}(s={section_id},{lo}..{hi})",
                        if *split {
                            "deleteShotRangeSplit"
                        } else {
                            "deleteShotRangeShift"
                        }
                    ),
                    if *split {
                        self.state.execute_delete_shot_range_split(
                            section_id,
                            lo,
                            hi,
                            "range-a".to_string(),
                            "range-b".to_string(),
                        )
                    } else {
                        self.state
                            .execute_delete_shot_range_shift(section_id, lo, hi)
                    },
                ))
            }
            Op::Batch { commands, poisoned } => {
                // A sub-command whose target is gone is DROPPED rather than
                // failing the batch: a batch is one undo entry over several
                // commands, and the shape that matters (an entry covering more
                // than one edit) survives losing one of them.
                let mut built = Vec::new();
                for command in commands {
                    match command {
                        BatchOp::UpdateShotLength { shot, length } => {
                            if let Some((section_id, shot_id)) = refs.shot(shot, targets) {
                                built.push(Command::UpdateShot {
                                    section_id,
                                    shot_id,
                                    updates: ShotUpdates {
                                        length: Some(*length),
                                        ..ShotUpdates::default()
                                    },
                                });
                            }
                        }
                        BatchOp::MoveSection { section, offset } => {
                            if let Some(section_id) = refs.section(*section, targets) {
                                built.push(Command::MoveSection {
                                    section_id,
                                    offset: *offset,
                                });
                            }
                        }
                        BatchOp::MoveStation { station, offset } => {
                            if let Some((section_id, index)) = refs.station(*station, targets) {
                                built.push(Command::MoveStation {
                                    station_ref: StationRef::new(section_id, index),
                                    offset: *offset,
                                });
                            }
                        }
                        BatchOp::ReverseSection {
                            section,
                            swap_left_right,
                        } => {
                            if let Some(section_id) = refs.section(*section, targets) {
                                built.push(Command::ReverseSection {
                                    section_id,
                                    swap_left_right: *swap_left_right,
                                });
                            }
                        }
                    }
                }
                if built.is_empty() {
                    return refs.skip(&op.describe());
                }
                if *poisoned {
                    built.push(Command::MoveSection {
                        section_id: 999_999,
                        offset: Coordinates::new(1.0, 1.0, 1.0),
                    });
                }
                Some(Drawn::new(
                    format!("batch(n={},poisoned={poisoned})", built.len()),
                    self.state.execute(Command::Batch { commands: built }),
                ))
            }
        }
    }
}

/// The persisted window: only the newest `undo_persist_limit` entries of each
/// stack survive a save, so a deeper history would come back truncated and the
/// model would be invalid. Matches `config::defaults`.
const UNDO_PERSIST_LIMIT: usize = 20;
/// Headroom kept below the in-memory history limit so FIFO trimming can never
/// fire in the ordinary modes. HORIZON mode deliberately goes past it - see
/// `Plan::horizon`.
const MAX_MODEL_DEPTH: usize = 40;
/// Fallback when the app's own limit cannot be read (a poisoned lock, which
/// would fail the run for other reasons first). Matches `config::defaults`.
const DEFAULT_HISTORY_LIMIT: usize = 100;
/// How often a history move is taken as a multi-step `undo_to`/`redo_to` jump,
/// when `Plan::jumps` is on. Raised from 10 while hunting finding 7, which only
/// exists on that path: a jump is the only move that skips `reconcile_geometry`.
const JUMP_PERCENT: u32 = 40;

impl Fuzzer {
    /// One mutation, with the full case analysis on what the stacks did.
    fn step_mutate(&mut self) {
        let targets = read_targets(&self.state);
        let Some(op) = self.draw_op(&targets) else {
            return;
        };
        self.step_apply(&op, &mut Refs::identity(), &targets);
    }

    /// Execute one `Op` through the model: the oracles, the stack case analysis
    /// and the recorder, shared by a DRAWN move and a REPLAYED one.
    fn step_apply(&mut self, op: &Op, refs: &mut Refs, targets: &Targets) -> StepOutcome {
        let before_undo = self.undo_len();
        let before_redo = self.redo_len();
        let before_newest = self.newest_entry();
        let allocators = self.allocators();
        // Recorded BEFORE the command runs, because the command that trips the
        // app's own merge-alignment assert is the one a shrinker most needs and
        // that assert unwinds straight out of `apply_op`.
        let mark = self.recorder_mark(MoveKind::Mutate(Box::new(op.clone())));
        // The LABEL, armed before the command runs, for exactly the reason the
        // trace move above is recorded before it: `record_mutation` writes this
        // move's log line only once `apply_op` RETURNS. See `Fuzzer::in_flight`.
        self.starting_move(op.describe());
        let applied = self.catching_alignment(|fuzzer| fuzzer.apply_op(op, refs, targets));
        self.finished_move();
        let Some(applied) = applied else {
            return StepOutcome::Stopped;
        };
        self.recorder_seal(mark);
        let Some(drawn) = applied else {
            return StepOutcome::Skipped;
        };
        let label = drawn.label.clone();
        self.record_mutation(drawn, before_undo, before_redo, before_newest);
        self.expect_allocators_advanced(&allocators, &label);
        self.record_growth();
        StepOutcome::Applied
    }

    /// Every section, its shots' uuids in project order, and every closure id.
    /// Built only while recording or replaying - see [`Inventory`].
    fn inventory(&self) -> Inventory {
        let Some(project) = self.state.get_project_state() else {
            return Inventory::default();
        };
        Inventory {
            sections: project
                .sections
                .iter()
                .map(|section| {
                    (
                        section.id,
                        section
                            .shots
                            .iter()
                            .filter_map(|shot| shot.uuid.clone())
                            .collect(),
                    )
                })
                .collect(),
            closures: project
                .loop_closures
                .iter()
                .map(|closure| closure.id)
                .collect(),
        }
    }

    /// Push a move onto the recording, and remember the state it starts from.
    /// `None` - and no cost at all - when nothing is recording.
    fn recorder_mark(&self, kind: MoveKind) -> Option<Inventory> {
        if !recorder_active() {
            return None;
        }
        let before = self.inventory();
        recorder_push(TraceMove {
            kind,
            created: Created::default(),
            rng: self.rng.state(),
        });
        Some(before)
    }

    /// Fill in what the move just made brought into existence.
    fn recorder_seal(&self, mark: Option<Inventory>) {
        if let Some(before) = mark {
            recorder_seal(self.inventory().created_since(&before));
        }
    }

    /// The bookkeeping half of `step_mutate`, split out so the exposure prologue
    /// can push its own commands through the SAME model updates and the SAME
    /// per-move oracles rather than a parallel path.
    fn record_mutation(
        &mut self,
        drawn: Drawn,
        before_undo: usize,
        before_redo: usize,
        before_newest: Option<String>,
    ) {
        let accepted = drawn.result.success;
        // Not a length delta. AT THE HISTORY LIMIT a push and its FIFO trim
        // cancel out - the stack was 100 entries and is still 100 - so a length
        // delta reads a real push as a no-op, which is how the horizon run first
        // went wrong: the model stopped advancing, kept comparing against the
        // wrong depth, and reported an app bug that was its own. The newest
        // entry's identity is the signal that survives trimming.
        let newest = self.newest_entry();
        let pushed = if self.undo_len() > before_undo {
            self.undo_len() - before_undo
        } else if self.undo_len() == before_undo
            && before_undo >= self.history_limit
            && newest.is_some()
            && newest != before_newest
        {
            1
        } else {
            0
        };
        self.note(format!(
            "{} -> {} (undo {}->{}, redo {}->{})",
            drawn.label,
            if accepted { "ok" } else { "rejected" },
            before_undo,
            self.undo_len(),
            before_redo,
            self.redo_len()
        ));

        if !accepted {
            self.outcome.rejected += 1;
            // A rejected command must change neither the stacks nor the state.
            // That assertion is what catches a command that mutates and THEN
            // fails, which is the worst shape a validation bug can take.
            if self.undo_len() != before_undo || self.redo_len() != before_redo {
                self.fail(
                    "a REJECTED command changed the history stacks",
                    &format!("  error: {:?}", drawn.result.error),
                );
            }
            self.expect_state(
                self.cursor,
                "a rejected command",
                Vec::new(),
                StaleTargets::default(),
            );
            return;
        }

        if pushed == 1 {
            if self.redo_len() != 0 {
                self.fail(
                    "a successful command did not clear the redo stack",
                    &format!("  after {}", drawn.label),
                );
            }
            self.cursor += 1;
            self.top = self.cursor;
            self.snapshots.truncate(self.cursor);
            self.verdicts.truncate(self.cursor);
            self.adjusted.truncate(self.cursor);
            self.epochs.truncate(self.cursor);
            self.entry_meta.truncate(self.cursor);
            self.entry_prints.truncate(self.cursor);
            let snapshot = self.snapshot();
            self.snapshots.push(snapshot);
            let verdict = self.verdict();
            self.verdicts.push(verdict);
            let adjusted = self.adjusted_geometry();
            self.adjusted.push(adjusted);
            self.epochs.push(self.epoch);
            // Read the entry BACK off the stack rather than inferring it from the
            // command that was drawn: the app is the authority on what it pushed,
            // and several entry points push an entry labelled by a different
            // command than the one the caller named.
            let newest = self.state.get_undo_stack();
            let newest = newest.first();
            let command = newest
                .map(|item| item.command_type.clone())
                .unwrap_or_default();
            // The metadata oracle's baseline for this depth, taken from the SAME
            // read: what the app says this entry is, at the instant it pushed it.
            let print = newest.map(entry_print).unwrap_or_default();
            self.entry_prints.push(print);
            self.entry_meta.push(EntryMeta {
                command,
                epoch: self.epoch,
                target: drawn.target,
                anchor: drawn.anchor,
            });
            self.outcome.mutations += 1;
            self.outcome.max_depth = self.outcome.max_depth.max(self.cursor);
            self.absorb_trim(&drawn.label);
            self.expect_merges_aligned(&drawn.label);
            self.expect_raw_and_resolved_agree(&drawn.label);
            return;
        }
        if pushed > 1 {
            self.fail(
                &format!("one command pushed {pushed} undo entries (expected 0 or 1)"),
                &format!("  after {}", drawn.label),
            );
        }

        // Succeeded without pushing an entry: a no-op command. It must not have
        // touched the project; it MAY have cleared the redo stack, which truncates
        // the model's forward branch.
        self.outcome.noops += 1;
        if self.redo_len() == 0 {
            self.top = self.cursor;
            self.snapshots.truncate(self.cursor + 1);
            self.verdicts.truncate(self.cursor + 1);
            self.adjusted.truncate(self.cursor + 1);
            self.epochs.truncate(self.cursor + 1);
            self.entry_meta.truncate(self.cursor + 1);
            self.entry_prints.truncate(self.cursor + 1);
        } else if self.redo_len() != before_redo {
            self.fail(
                "a no-op command changed the redo stack without clearing it",
                &format!("  after {}", drawn.label),
            );
        }
        self.expect_state(
            self.cursor,
            "a no-op command",
            Vec::new(),
            StaleTargets::default(),
        );
    }

    /// Run one command through the model exactly as a drawn mutation, for the
    /// prologues that need a specific shape rather than a random one.
    ///
    /// `before` is the stack shape from BEFORE the command ran, which the caller
    /// has to capture: `record_mutation` tells a push from a no-op by the
    /// difference, and reading it here would make every forced command look like
    /// a no-op.
    fn forced(&mut self, drawn: Drawn, before: (usize, usize, Option<String>)) {
        self.record_mutation(drawn, before.0, before.1, before.2);
    }

    /// The current `(undo, redo)` stack depths, and the newest entry's identity.
    fn stacks(&self) -> (usize, usize, Option<String>) {
        (self.undo_len(), self.redo_len(), self.newest_entry())
    }

    /// A fingerprint of the newest undo entry, or `None` when the stack is empty.
    ///
    /// `(timestamp, command_type, description)` - enough to tell a fresh push
    /// from the same entry still sitting on top, which a stack LENGTH cannot do
    /// once the FIFO trim is active.
    fn newest_entry(&self) -> Option<String> {
        self.state.get_undo_stack().first().map(entry_print)
    }

    /// Build the state findings 8, 12 and 13 need, instead of waiting for a seed
    /// to stumble into it.
    ///
    /// The three defects all require a shot id that means something DIFFERENT
    /// after a reload, and an entry carrying such an id that a walk actually
    /// reaches. A random stream reaches both by luck: it has to insert
    /// mid-section (so the ids stop matching positional order), then push an
    /// id-bearing entry, then save, then unwind to it without crossing a
    /// `ProjectSnapshotPair` on the way - and a snapshot-pair entry is a WALL,
    /// because `Shot::id` is serialized inside a project snapshot, so restoring
    /// one puts the whole project back into the pre-save id space and every
    /// stale id below it becomes valid again.
    ///
    /// So this builds the shape directly, in the order the harness's
    /// `undo-persistence-fuzz` arrived at for the same reason
    /// (`e2e/agent/scenarios/undoPersistenceCoverage.ts`, "why a
    /// ProjectSnapshotPair entry is a wall"): a fresh section, the insert that
    /// makes ids diverge, then EVERY id-bearing entry contiguous at the top, so
    /// the reload lands inside that block and the unwind starts on it.
    ///
    /// The section is FRESH - disconnected, no datum, in no loop - on purpose:
    /// `delete_shot`'s loop guard would otherwise refuse the undo of the insert
    /// (finding 6) and wedge the history before any of this could be observed.
    fn expose_stale_ids(&mut self) {
        let shots: Vec<ShotData> = (0..3).map(|_| random_shot(&mut self.rng)).collect();
        let before = self.stacks();
        let created = self.state.execute(Command::CreateSection {
            name: Some("exposed".to_string()),
            surveyor: None,
            date: None,
            color: (*self.rng.pick(COLORS)).to_string(),
            direction: None,
            shots,
        });
        let Some(section_id) = created.section_id else {
            self.fail(
                "the exposure prologue could not create its section",
                &format!("  error: {:?}", created.error),
            );
        };
        self.forced(
            Drawn::new(
                format!("exposure:createSection(s={section_id},shots=3)"),
                created,
            ),
            before,
        );

        // The insert that makes ids diverge from positional order. BEFORE the
        // first shot, so every id moves by one slot when the loader renumbers.
        let first = self.live_shot_ids(section_id);
        let Some(&first_shot) = first.first() else {
            self.fail("the exposure prologue's section has no shots", "");
        };
        let anchor = self.shot_target(section_id, first_shot);
        let shot = random_shot(&mut self.rng);
        let before = self.stacks();
        let inserted = self.state.execute(Command::CreateShot {
            section_id,
            shot,
            after_shot_id: None,
            before_shot_id: Some(first_shot),
        });
        let mut drawn = Drawn::new(
            format!("exposure:createShot(s={section_id},before)"),
            inserted,
        );
        drawn.anchor = anchor;
        drawn.target = drawn
            .result
            .shot_id
            .and_then(|shot_id| self.shot_target(section_id, shot_id));
        self.forced(drawn, before);

        // The divergence is the whole precondition, so it is CHECKED rather than
        // assumed: a section whose ids still equal `section * STRIDE + position`
        // survives the reload unchanged and none of this proves anything.
        let colliding = self.colliding_shots(section_id);
        assert!(
            !colliding.is_empty(),
            "the exposure prologue produced no colliding shot id in section {section_id}: ids {:?}",
            self.live_shot_ids(section_id)
        );

        // Every id-bearing edit variant, contiguous, on top: `PreviousValues`
        // (update_shot) and `InverseCommand` (set_shot_kind), each aimed at a
        // shot whose id will address a DIFFERENT one after the renumbering.
        for &shot_id in colliding.iter().take(2) {
            let target = self.shot_target(section_id, shot_id);
            let before = self.stacks();
            let edited = self.state.execute(Command::UpdateShot {
                section_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(self.rng.float(20.0, 40.0)),
                    ..ShotUpdates::default()
                },
            });
            let mut drawn = Drawn::new(
                format!("exposure:updateShot(s={section_id},shot={shot_id})"),
                edited,
            );
            drawn.target = target;
            self.forced(drawn, before);
        }
        for &shot_id in colliding.iter().take(2) {
            let target = self.shot_target(section_id, shot_id);
            let before = self.stacks();
            let marked = self.state.execute(Command::SetShotKind {
                section_id,
                shot_id,
                // Never `Normal`: every shot here starts Normal, so setting it
                // would be a forward no-op with a no-op inverse, and an undo
                // landing on the WRONG shot would be unobservable.
                kind: ShotKind::Jump,
            });
            let mut drawn = Drawn::new(
                format!("exposure:setShotKind(s={section_id},shot={shot_id})"),
                marked,
            );
            drawn.target = target;
            self.forced(drawn, before);
        }
    }

    /// Build the two closure shapes findings 4 and 10 need.
    ///
    /// Both are reachable in principle from the generators and neither is
    /// reachable in practice: a 30-seed x 60-move sweep with `closures` on
    /// produced no `RemoveLoopClosure` of a non-last closure at all, because a
    /// second closure needs a second independent path between the same two
    /// sections and `create_loop_closure` refuses everything else. So the shapes
    /// are constructed, and then left to the ordinary walk to exercise.
    ///
    /// GROUP 1, finding 4: two sections joined by a merge, two closures between
    /// them, and the FIRST one removed. Its undo is `loop_closures.push()`
    /// (core/state/undo.rs) rather than an insert at the recorded index, so the
    /// order the walk restores is not the order it recorded - and that order is
    /// persisted, indexed by the frontend and traversed by `resolve_offsets`.
    ///
    /// GROUP 2, finding 10: A - bridge - B chained by two merges plus a closure
    /// straight from A to B, with the bridge then deleted through
    /// `execute_delete_last_shot_and_section` - one of the two delete entry
    /// points that never calls `convert_orphan_closures_to_merges`. The closure
    /// is now the sole connection, so it is "really" a merge; memory keeps it as
    /// a closure, the save persists that, and the LOADER converts it.
    fn expose_closure_shapes(&mut self) {
        let group_one = self.build_merged_pair("closure-order-a", "closure-order-b", 2);
        if let Some((a, b)) = group_one {
            let mut ids = Vec::new();
            for (station_a, station_b) in [((a, 0), (b, 1)), ((a, 1), (b, 1))] {
                let before = self.stacks();
                let created = self.state.execute(Command::CreateLoopClosure {
                    station_a: StationRef::new(station_a.0, station_a.1),
                    station_b: StationRef::new(station_b.0, station_b.1),
                    closure_strategy: ClosureStrategy::Move,
                });
                if let Some(closure) = self
                    .state
                    .get_project_state()
                    .and_then(|project| project.loop_closures.last().map(|c| c.id))
                {
                    if created.success {
                        ids.push(closure);
                    }
                }
                self.forced(
                    Drawn::new(
                        format!(
                            "closures:createLoopClosure({}/{})",
                            station_a.0, station_b.0
                        ),
                        created,
                    ),
                    before,
                );
            }
            // The FIRST of two, so the undo's `push` lands it in the wrong place.
            // Removing the last one would restore correctly and prove nothing.
            if ids.len() == 2 {
                let before = self.stacks();
                let removed = self
                    .state
                    .execute(Command::RemoveLoopClosure { closure_id: ids[0] });
                self.forced(
                    Drawn::new(format!("closures:removeLoopClosure({})", ids[0]), removed),
                    before,
                );
            }
        }
    }

    /// GROUP 2, finding 10, built immediately before the save rather than at the
    /// start of the run.
    ///
    /// The orphan has to still BE an orphan when the file is written: any later
    /// command flagged `can_orphan_closures` runs the conversion from
    /// `execute()`'s epilogue and heals it, which is what made this fire in 1
    /// seed out of 10 when it was built up front.
    fn expose_orphan_closure(&mut self) {
        let a = self.forced_section("orphan-a", 1);
        let bridge = self.forced_section("orphan-bridge", 1);
        let b = self.forced_section("orphan-b", 1);
        let (Some(a), Some(bridge), Some(b)) = (a, bridge, b) else {
            return;
        };
        let before = self.stacks();
        let datum = self.state.execute(Command::SetZeroDatum {
            station_ref: StationRef::new(a, 0),
            coordinates: GpsCoordinates::new(21.1, -86.9, 0.0).unwrap(),
        });
        self.forced(
            Drawn::new(format!("closures:setZeroDatum({a}/0)"), datum),
            before,
        );
        for (left, right) in [((a, 1), (bridge, 0)), ((bridge, 1), (b, 0))] {
            let before = self.stacks();
            let merged = self.state.execute(Command::MergeStations {
                station_a: StationRef::new(left.0, left.1),
                station_b: StationRef::new(right.0, right.1),
            });
            self.forced(
                Drawn::new(format!("closures:merge({}/{})", left.0, right.0), merged),
                before,
            );
        }
        let before = self.stacks();
        let closure = self.state.execute(Command::CreateLoopClosure {
            station_a: StationRef::new(a, 0),
            station_b: StationRef::new(b, 1),
            closure_strategy: ClosureStrategy::Move,
        });
        let closed = closure.success;
        self.forced(
            Drawn::new(format!("closures:createLoopClosure({a}/{b})"), closure),
            before,
        );
        if !closed {
            return;
        }
        // The delete path that does NOT convert. `execute_delete_last_shot_and_section`
        // is an `AppState` entry point rather than a `Command`, so it never sees
        // `execute()`'s `can_orphan_closures` epilogue either.
        let Some(&bridge_shot) = self.live_shot_ids(bridge).first() else {
            return;
        };
        let before = self.stacks();
        let deleted = self
            .state
            .execute_delete_last_shot_and_section(bridge, bridge_shot);
        self.forced(
            Drawn::new(
                format!("closures:deleteLastShotAndSection(s={bridge})"),
                deleted,
            ),
            before,
        );
    }

    /// A section with `shots` random shots, run through the model. Returns its id.
    fn forced_section(&mut self, name: &str, shots: usize) -> Option<SectionId> {
        let shots: Vec<ShotData> = (0..shots).map(|_| random_shot(&mut self.rng)).collect();
        let before = self.stacks();
        let created = self.state.execute(Command::CreateSection {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: (*self.rng.pick(COLORS)).to_string(),
            direction: None,
            shots,
        });
        let section_id = created.section_id;
        self.forced(
            Drawn::new(format!("closures:createSection({name})"), created),
            before,
        );
        section_id
    }

    /// Two sections joined by a merge, with a zero datum on the first so a loop
    /// closure between them is legal. Returns `(a, b)`.
    fn build_merged_pair(
        &mut self,
        name_a: &str,
        name_b: &str,
        shots_a: usize,
    ) -> Option<(SectionId, SectionId)> {
        let a = self.forced_section(name_a, shots_a)?;
        let b = self.forced_section(name_b, 1)?;
        let before = self.stacks();
        let datum = self.state.execute(Command::SetZeroDatum {
            station_ref: StationRef::new(a, 0),
            coordinates: GpsCoordinates::new(20.9, -87.1, 0.0).unwrap(),
        });
        self.forced(
            Drawn::new(format!("closures:setZeroDatum({a}/0)"), datum),
            before,
        );
        let before = self.stacks();
        let merged = self.state.execute(Command::MergeStations {
            station_a: StationRef::new(a, shots_a as u32),
            station_b: StationRef::new(b, 0),
        });
        let ok = merged.success;
        self.forced(
            Drawn::new(format!("closures:merge({a}/{b})"), merged),
            before,
        );
        ok.then_some((a, b))
    }

    /// Live shot ids of one section, in positional order.
    fn live_shot_ids(&self, section_id: SectionId) -> Vec<ShotId> {
        self.state
            .get_project_state()
            .map(|project| {
                project
                    .sections
                    .iter()
                    .find(|section| section.id == section_id)
                    .map(|section| section.shots.iter().map(|shot| shot.id).collect())
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    /// Shots whose id will address a DIFFERENT, still-valid position once the
    /// loader has renumbered the section.
    ///
    /// The loader rewrites position `i` to `section_id * FEATURE_ID_STRIDE + i`
    /// (persistence/sqlite/read.rs), so a persisted id `ids[i]` will address
    /// position `ids[i] - section_id * STRIDE` after a reload. It is a COLLISION
    /// exactly when that number is a different, still-valid position - which is
    /// findings 8 and 12's regime, as opposed to finding 13's (out of range).
    fn colliding_shots(&self, section_id: SectionId) -> Vec<ShotId> {
        let ids = self.live_shot_ids(section_id);
        let base = section_id * crate::models::FEATURE_ID_STRIDE;
        ids.iter()
            .enumerate()
            .filter(|(position, id)| {
                let lands = id.saturating_sub(base) as usize;
                lands != *position && lands < ids.len()
            })
            .map(|(_, id)| *id)
            .collect()
    }

    /// Run `body`, turning the app's OWN merge-alignment assert from an
    /// unattributable process panic into a FAILURE that carries the seed and the
    /// move log.
    ///
    /// `Project::debug_assert_merge_alignment` (models/project/offsets.rs) has a
    /// `#[cfg(test)]` assert arm, so it exists in this tier and in no other. It
    /// fires at the END of the command it is checking - which is why this cannot
    /// be pre-empted by checking alignment before the move: the command that
    /// breaks the invariant is the command that asserts it.
    /// `expect_merges_aligned` proves the state was clean immediately before.
    ///
    /// It CLASSIFIED, once. Findings 14 (`insert_shot_after`) and its mirror
    /// (`remove_shot_with_rewire`) were open, so an assert from either site was a
    /// counted, tolerated hit that merely stopped the run. Both are FIXED - the
    /// insert and the delete now decide which side of the edit is anchored over
    /// the rigid merge forest and compensate only when the tail alone is
    /// (models/project/shots.rs) - so per the delete-when-fixed contract the
    /// tolerance is gone: ANY merge-alignment assert, from ANY call site, fails
    /// the run again.
    ///
    /// What this still does is ATTRIBUTE it. The payload is READ and anything
    /// that is not that assert is re-raised verbatim with `resume_unwind`, so no
    /// other panic is absorbed or reshaped. An alignment assert instead unwinds
    /// no further than here, is recorded in the move log, and latches
    /// `alignment_panic` - which stops the walk (the unwind went through
    /// `AppState`'s project mutex, so it is poisoned and several call sites now
    /// read "no project open") and which `run_fuzz` turns into a `fail` carrying
    /// the seed, the depth and the last fourteen moves. A bare process panic
    /// carries none of those.
    ///
    /// The assert itself is untouched. Disabling it would delete this tier's
    /// only view of the defect.
    fn catching_alignment<T>(&mut self, body: impl FnOnce(&mut Self) -> T) -> Option<T> {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(self)));
        match outcome {
            Ok(value) => Some(value),
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_default();
                if !message.contains("Merge alignment violated after") {
                    std::panic::resume_unwind(payload);
                }
                self.outcome.alignment_panic = true;
                // FIRST: the move that panicked has no log line of its own yet,
                // and the note below points at whatever line is last.
                self.flush_in_flight();
                self.note(format!(
                    "  ^ the app's own merge-alignment assert fired: {}",
                    message.lines().next().unwrap_or_default()
                ));
                None
            }
        }
    }

    /// Move the model's floor up when the app FIFO-trimmed the oldest entries,
    /// and assert it trimmed exactly as much as the limit requires.
    ///
    /// `UndoStack::enforce_depth_limit` drops from the FRONT of the undo stack
    /// while `len > history_limit` (core/undo/budget.rs). So after a push the
    /// app holds at most `history_limit` entries, and the model's reachable
    /// floor is `cursor - history_limit`. Everything below that is unreachable
    /// FOREVER - not "not currently reachable" - so the model drops those
    /// depths' expectations rather than keeping stale ones it can never check.
    ///
    /// The assertion is exact in both directions: too little trimming is an
    /// unbounded history, too much is a user's edits disappearing early.
    fn absorb_trim(&mut self, what: &str) {
        let reachable = self.cursor - self.floor;
        let expected_undo = reachable.min(self.history_limit);
        let actual_undo = self.undo_len();
        if actual_undo != expected_undo {
            self.fail(
                &format!("{what} left the undo stack the wrong length for the history limit"),
                &format!(
                    "  limit={} reachable={reachable} expected undo={expected_undo}, got {actual_undo}",
                    self.history_limit
                ),
            );
        }
        if reachable > self.history_limit {
            let new_floor = self.cursor - self.history_limit;
            // The trimmed depths can never be observed again, so their
            // expectations are dropped rather than left to rot. `snapshots` is
            // indexed by ABSOLUTE depth, so the entries stay in place and only
            // the floor moves - which keeps every other index meaningful.
            self.outcome.trimmed += new_floor - self.floor;
            self.floor = new_floor;
        }
    }

    /// Allocators may not go BACKWARDS across a forward command.
    ///
    /// The roundtrip oracle drops them deliberately: a snapshot-pair undo
    /// restores them verbatim while a targeted undo only ever raises them, so
    /// comparing them per depth would fire on every mixed history. That leaves
    /// the one thing about them that IS an invariant unchecked - a forward
    /// command that lowers an allocator hands the same id out twice - which is
    /// what this checks, per mutation, against the value from immediately
    /// before it.
    fn expect_allocators_advanced(&mut self, before: &BTreeMap<String, u32>, what: &str) {
        let after = self.allocators();
        let mut regressions = Vec::new();
        for (key, old) in before {
            if let Some(new) = after.get(key) {
                if new < old {
                    regressions.push(format!("{key}: {old} -> {new}"));
                }
            }
        }
        if regressions.is_empty() {
            return;
        }
        self.fail(
            &format!("{what} REWOUND an id allocator, so an id can now be issued twice"),
            &format!("  {}", regressions.join("\n  ")),
        );
    }

    /// Every id allocator in the project, by name.
    fn allocators(&self) -> BTreeMap<String, u32> {
        let mut out = BTreeMap::new();
        let Some(project) = self.state.get_project_state() else {
            return out;
        };
        out.insert("next_section_id".to_string(), project.next_section_id);
        out.insert(
            "next_closure_id".to_string(),
            u32::try_from(project.next_closure_id).unwrap_or(u32::MAX),
        );
        for section in &project.sections {
            out.insert(
                format!("section {}.next_shot_id", section.id),
                section.next_shot_id,
            );
        }
        out
    }

    /// The sizes worth watching over a long run: an unbounded collection is a
    /// leak whether or not any single move looks wrong.
    fn record_growth(&mut self) {
        let Some(project) = self.state.get_project_state() else {
            return;
        };
        let stations: usize = project
            .sections
            .iter()
            .map(|section| section.shots.len() + 1)
            .sum();
        let peak = &mut self.outcome.peak;
        peak.sections = peak.sections.max(project.sections.len());
        peak.stations = peak.stations.max(stations);
        peak.section_offsets = peak.section_offsets.max(project.section_offsets.len());
        peak.station_offsets = peak.station_offsets.max(project.station_offsets.len());
        peak.station_comments = peak.station_comments.max(project.station_comments.len());
        peak.merges = peak.merges.max(project.station_merges.len());
        peak.closures = peak.closures.max(project.loop_closures.len());
    }

    // `classify_move_failure` lived here: "did a failed history move fail for a
    // KNOWN, documented reason?", the one place a WEDGED history could be
    // excused. It is empty and therefore gone, and a failed history move is now
    // ALWAYS an outright failure of the run.
    //
    // * FINDING 6 was "the user-facing `delete_shot` refuses the undo of a
    //   permitted `CreateShot`", by any of its three guards (the shift-blocked
    //   loop/datum pair, and the frozen-shot check that runs before them). FIXED:
    //   the undo removes the created shot through `AppState::remove_created_shot`
    //   (core/state/command_shot.rs), which consults no user-intent guard, so a
    //   permitted insert can always be undone.
    // * FINDING 13 was "a stale persisted shot id misses and wedges the history",
    //   in both spellings the app words it. FIXED with findings 8/12/16: an entry
    //   addresses its shot by `Shot::uuid`, and a load-time pass drops an entry
    //   whose target no longer resolves instead of leaving it to wedge the walk
    //   (core/undo.rs, core/undo/persistence.rs).

    /// What the ids on `entry` address in the LIVE project right now.
    ///
    /// This is the whole of findings 8, 12 and 13 reduced to one measurement.
    /// `Staleness::Absent` when the run has never reloaded, because an id that
    /// has not crossed the loader's renumbering cannot be stale - and saying so
    /// keeps the classifiers from ever explaining an in-memory difference with a
    /// persistence defect.
    fn measure_staleness(&self, entry: Option<&EntryMeta>) -> StaleTargets {
        if self.outcome.reloads == 0 {
            return StaleTargets::default();
        }
        let Some(entry) = entry else {
            return StaleTargets::default();
        };
        let measure = |target: Option<&ShotTarget>| -> Staleness {
            let Some(target) = target else {
                return Staleness::Absent;
            };
            match self.uuid_of(target.section_id, target.shot_id) {
                None => Staleness::Missing,
                Some(uuid) if uuid == target.uuid => Staleness::Fresh,
                Some(_) => Staleness::Collides,
            }
        };
        StaleTargets {
            target: measure(entry.target.as_ref()),
            anchor: measure(entry.anchor.as_ref()),
        }
    }

    /// Has something happened that makes everything after it measure the
    /// harness rather than the app? See `catching_alignment`.
    const fn stopped(&self) -> bool {
        self.outcome.alignment_panic
    }

    /// The shrinker's INJECTED failure - see [`Fault`]. Reached only from
    /// [`run_trace`], and only when the trace carries one.
    ///
    /// The headline is fixed text with the numbers in the detail, so
    /// `failure_signature` buckets every firing of it as ONE defect however deep
    /// it happens - which is what the shrinker's signature matching needs.
    fn check_injected_fault(&mut self, fault: Fault) {
        let sections = self
            .state
            .get_project_state()
            .map_or(0, |project| project.sections.len());
        if sections >= fault.at_sections {
            self.fail(
                "INJECTED FAULT: the project reached the section count this trace fails at",
                &format!(
                    "  sections={sections}, at_sections={}\n  this is the shrinker's own \
                     fault injection, not a defect in the app",
                    fault.at_sections
                ),
            );
        }
    }

    /// One history move, recorded. Only the main loop's history moves are in a
    /// trace: the closing `finish()` walk is a function of the plan, so a replay
    /// re-runs it and a shrink step can never delete it.
    fn step_recorded_history(&mut self, back: bool, steps: usize) -> bool {
        let mark = self.recorder_mark(if back {
            MoveKind::Undo(steps)
        } else {
            MoveKind::Redo(steps)
        });
        let alive = self.step_history(back, steps);
        self.recorder_seal(mark);
        alive
    }

    /// One history move. `steps` > 1 goes through `undo_to`/`redo_to`, the
    /// history dropdown's multi-step path, verified against the SAME snapshots
    /// single steps recorded - an equivalence that is not free, because
    /// `undo_to`/`redo_to` loop the `*_inner` variants and call neither
    /// `invalidate_adjusted_cache()` nor `reconcile_geometry()` (finding 7).
    ///
    /// Returns false when a known wedge stopped it.
    fn step_history(&mut self, back: bool, steps: usize) -> bool {
        // `floor`, not 0: everything below it was FIFO-trimmed and the app has
        // no entry left to undo.
        let reach = if back {
            self.cursor - self.floor
        } else {
            self.top - self.cursor
        };
        if reach == 0 || steps == 0 {
            return true;
        }
        let steps = steps.min(reach);
        let jumped = steps > 1;
        // The entry this move will consume. For an undo that is the one at the
        // current depth; for a redo, the one just above it. A jump consumes
        // several - the FIRST one is what the classifiers key on, since it is the
        // one whose restore produced the state being compared.
        // Every entry this move consumes, in restore order. For an undo those are
        // the entries at the current depth downwards; for a redo, the ones just
        // above it upwards.
        let consumed: Vec<EntryMeta> = if back {
            (0..steps)
                .filter_map(|step| self.entry_meta.get(self.cursor - step).cloned())
                .collect()
        } else {
            (1..=steps)
                .filter_map(|step| self.entry_meta.get(self.cursor + step).cloned())
                .collect()
        };
        // Measured NOW, before the move: the ids the entry carries are about to
        // be resolved against THIS project, and the move itself changes which
        // shot each one addresses.
        // Over EVERY entry the move will consume, worst case per role. For the
        // entries after the first this is measured against the pre-move project
        // rather than against the intermediate state each restore will actually
        // see - an approximation, and one that can only ever say "an id on this
        // move is stale", which is exactly what the classifiers ask it.
        let stale = consumed
            .iter()
            .map(|entry| self.measure_staleness(Some(entry)))
            .fold(StaleTargets::default(), StaleTargets::worst);
        // MEASUREMENT, kept after findings 8/12/13/16 were fixed: an in-session
        // id an entry still carries no longer decides anything (the restore
        // paths address the shot by `Shot::uuid`), but the fact that the id has
        // gone stale is exactly the precondition those defects needed, so the
        // move log records the first time it happens in a run.
        if matches!(stale.target, Staleness::Collides | Staleness::Missing)
            || matches!(stale.anchor, Staleness::Collides | Staleness::Missing)
        {
            if !self.stale_corruption {
                self.note(format!(
                    "  ^ a persisted id on this entry is STALE (target={:?}, anchor={:?}): the \
                     restore resolves its shot by uuid instead",
                    stale.target, stale.anchor
                ));
            }
            self.stale_corruption = true;
        }
        // Built BEFORE the move, so a move that panics can be named in the
        // failure tail - the notes below are all written after it returns. See
        // `Fuzzer::in_flight`.
        let label = format!(
            "{}{} x{steps}",
            if back { "undo" } else { "redo" },
            if jumped { "To" } else { "" }
        );
        self.starting_move(label.clone());
        let result = self.catching_alignment(|fuzzer| {
            if back {
                if jumped {
                    fuzzer.state.undo_to(steps - 1)
                } else {
                    fuzzer.state.undo()
                }
            } else if jumped {
                fuzzer.state.redo_to(steps - 1)
            } else {
                fuzzer.state.redo()
            }
        });
        self.finished_move();
        let Some(result) = result else {
            return false;
        };

        if !result.success {
            let error = result.error.clone().unwrap_or_default();
            self.note(format!("{label} -> FAILED [{error}]"));
            // No excuse is left: a refused history move leaves its entry on the
            // stack (`undo_inner`'s epilogue pushes it back), so the history is
            // WEDGED and every entry beneath it is unreachable. See the deleted
            // `classify_move_failure` above for the two findings that used to be
            // excused here.
            self.fail(
                &format!("{label} at depth {} failed outright", self.cursor),
                &format!("  error: {error}"),
            );
        }

        self.cursor = if back {
            self.cursor - steps
        } else {
            self.cursor + steps
        };
        self.note(format!("{label} -> depth {}", self.cursor));
        if back {
            self.outcome.undos += 1;
        } else {
            self.outcome.redos += 1;
        }
        if jumped {
            self.outcome.jumps += 1;
        }
        self.expect_stacks(&label);
        if jumped {
            self.probe_jump_equivalence(back, steps, self.cursor);
        }
        self.expect_state(self.cursor, &label, consumed, stale);
        self.expect_merges_aligned(&label);
        self.expect_raw_and_resolved_agree(&label);
        true
    }

    /// FINDING 7, decided by PROVING the inequality rather than by recognising a
    /// shape: run the same distance again one step at a time and see whether the
    /// two paths agree.
    ///
    /// `undo()` and `redo()` end in `reconcile_geometry()`, which runs
    /// `realign_merges()`; `undo_to()` and `redo_to()` validate the index and
    /// then loop the `*_inner` variants, calling it NEITHER per step nor at the
    /// end (core/state/undo.rs). The contract that breaks is exactly
    /// "`undo_to(N)` == N x `undo()`", so this measures that contract directly:
    /// the jump is replayed forwards, then walked back down in single steps, and
    /// if the single-step path lands on the state the model recorded while the
    /// jump did not, the two paths are not equal. That is the finding, proven on
    /// the spot rather than inferred.
    ///
    /// Only ever entered for a jump that has ALREADY diverged, and it attributes
    /// nothing unless the single-step walk succeeds AND lands exactly on the
    /// recorded state; anything else is left to the ordinary classification,
    /// with the log recording that the replay happened.
    fn probe_jump_equivalence(&mut self, back: bool, steps: usize, depth: usize) {
        if self.snapshot() == self.snapshots[depth] {
            return;
        }
        // Undo what the jump did, one step at a time in the opposite direction,
        // then redo it one step at a time. Both use the plain `undo`/`redo`
        // entry points, which is the whole point - they carry the epilogue.
        let (rewind, replay): (
            fn(&AppState) -> CommandResult,
            fn(&AppState) -> CommandResult,
        ) = if back {
            (AppState::redo, AppState::undo)
        } else {
            (AppState::undo, AppState::redo)
        };
        for _ in 0..steps {
            if !rewind(&self.state).success {
                return;
            }
        }
        for _ in 0..steps {
            if !replay(&self.state).success {
                return;
            }
        }
        self.note(format!(
            "  ^ replayed the same {steps} step(s) singly to test undo_to == N x undo"
        ));
        if self.snapshot() == self.snapshots[depth] {
            *self
                .outcome
                .hits
                .entry("finding-7-a-jump-is-not-n-single-steps")
                .or_insert(0) += 1;
        }
    }

    /// Would a save/close/reopen here come back holding FEWER entries than the
    /// model still expects to reach?
    ///
    /// Only the newest `undo_persist_limit` rows of EACH stack survive the file
    /// (`UndoStack::rows_to_persist`), so a crossing above that window returns a
    /// history the model's snapshots no longer describe. The run skips such a
    /// crossing rather than modelling the loss - which is what the one
    /// comparison this replaces did.
    ///
    /// Two measures, and the second is why this is a method now. With the app's
    /// own limit the model's whole history is in memory, so `top` - undo entries
    /// plus redo entries - IS the count that has to fit, and the first branch is
    /// byte-for-byte the test that was here. With a DRAWN limit it is not:
    /// everything below `floor` was FIFO-trimmed out of the app, so `top` counts
    /// depths that are no longer entries at all, and a limit of 3 would refuse
    /// every crossing past move 20 while the app is holding three rows. The
    /// window-aware measure is therefore per STACK, because `undo_persist_limit`
    /// is applied per stack.
    fn crossing_would_truncate(&self) -> bool {
        if self.drawn.is_none() {
            return self.top > UNDO_PERSIST_LIMIT;
        }
        let undo = (self.cursor - self.floor).min(self.history_limit);
        let redo = (self.top - self.cursor).min(self.history_limit);
        undo > UNDO_PERSIST_LIMIT || redo > UNDO_PERSIST_LIMIT
    }

    /// Save to a temp path, close, reopen. Undo history is PERSISTED - both stacks
    /// serialize into the `.stalx` and restore on open - so nothing below a save
    /// point is exercised until a run crosses this boundary. Findings 8, 12 and 13
    /// all live on the other side of it.
    fn cross_boundary(&mut self) -> bool {
        if self.orphan_pending {
            self.orphan_pending = false;
            self.expose_orphan_closure();
        }
        if self.top == 0 || self.crossing_would_truncate() {
            return false;
        }
        // The ENTRY-CONTENT oracle's baseline, read BEFORE the save so that the
        // save is inside the comparison too. See `expect_history_rows`.
        let items_before = self.history_rows();
        let temp = self.temp.get_or_insert_with(|| TempDir::new().unwrap());
        let path: PathBuf = temp.path().join("fuzz.stalx");

        // The save is about to make undo-less changes, so everything recorded so
        // far belongs to the previous epoch.
        self.epoch += 1;
        // Same rule as a mutation and a history move: the crossing is a MOVE,
        // its log lines are written after each half returns, and a panic inside
        // either half would otherwise leave the tail naming the move before it.
        self.starting_move("save".to_string());
        let saved = self.state.save_project(Some(path.clone()));
        self.finished_move();
        if !saved.success {
            self.fail(
                "save_project failed",
                &format!("  error: {:?}", saved.error),
            );
        }
        self.outcome.saves += 1;
        self.note(format!("save -> ok (depth {})", self.cursor));
        self.expect_stacks("save");
        self.expect_state(self.cursor, "save", Vec::new(), StaleTargets::default());

        self.starting_move("close/reopen".to_string());
        let closed = self.state.execute(Command::CloseProject);
        if !closed.success {
            self.fail(
                "CloseProject failed",
                &format!("  error: {:?}", closed.error),
            );
        }
        let opened = self.state.execute(Command::OpenProject { path });
        self.finished_move();
        if !opened.success {
            self.fail(
                "OpenProject failed",
                &format!("  error: {:?}", opened.error),
            );
        }
        // Switch BEFORE re-observing, so both sides of the comparison are in the
        // same form.
        self.enable_id_agnostic();
        self.epoch += 1;
        self.outcome.reloads += 1;
        self.note(format!("reload -> depth {}", self.cursor));
        self.expect_stacks("reload");
        self.expect_history_rows(&items_before);
        self.expect_state(self.cursor, "reload", Vec::new(), StaleTargets::default());
        true
    }

    /// The closing guarantee: unwind every entry to depth 0, then replay every
    /// entry back to the top, verifying the recorded snapshot at each depth in
    /// BOTH directions.
    fn finish(&mut self) {
        let summit = self.top;
        while self.cursor > self.floor {
            if !self.step_history(true, 1) {
                return;
            }
        }
        while self.cursor < summit {
            if !self.step_history(false, 1) {
                return;
            }
        }
    }
}

/// What a run is allowed to do.
///
/// Serializable because a recorded [`Trace`] carries it: a trace is only
/// replayable against the plan it was recorded under, since the plan decides the
/// starting project and the prologues the trace does NOT contain.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct Plan {
    pub(super) iterations: usize,
    /// Cross the save/close/reopen boundary once, part way through.
    pub(super) persistence: bool,
    /// Draw multi-step `undo_to`/`redo_to` jumps as well as single steps.
    ///
    /// OFF for the gate, ON for the soak, and the reason is finding 7, not taste.
    /// `undo_to`/`redo_to` loop `undo_inner`/`redo_inner` and call NEITHER
    /// `invalidate_adjusted_cache()` NOR `reconcile_geometry()`, which
    /// `undo()`/`redo()` both do - so `realign_merges()` never runs after a jump
    /// and a merge can be left METRES out of alignment. In this tier that does not
    /// surface as a snapshot mismatch the fuzzer can classify: it surfaces later,
    /// as the app's OWN `debug_assert_merge_alignment` (models/project/offsets.rs,
    /// `#[cfg(test)]` and therefore an oracle only this tier has) panicking inside
    /// whatever command next touches the merge. A panic from inside the app cannot
    /// be attributed to a finding from outside it, so a gate that drew jumps would
    /// be red for a KNOWN bug with an unreadable message.
    ///
    /// Turn this on with finding 7 and the gate goes red - which is the point:
    /// delete this flag when it is fixed.
    pub(super) jumps: bool,
    /// Put a genuine LOOP CLOSURE in the starting project, and let the generators
    /// create more.
    ///
    /// OFF for the gate, ON for the soak, because of FINDING 14 - which this tier
    /// found, and which no other tier can see. With a closure present,
    /// `insert_shot_after` leaves a downstream merge METRES out of alignment, and
    /// the app's own `#[cfg(test)]` `debug_assert_merge_alignment`
    /// (models/project/offsets.rs) panics the process from inside the insert. It
    /// is the INSERT that breaks it, not a leftover: this module re-checks raw
    /// merge alignment after every move with the same measure the assert uses,
    /// and that check is clean on the move immediately before.
    ///
    /// A panic from inside the app cannot be classified from outside it, so a gate
    /// that built closures would be red for a known finding with an unreadable
    /// message. Delete this flag when finding 14 is fixed; the gate then covers
    /// `ClosureInfo` again, which is the coverage it currently costs.
    pub(super) closures: bool,
    /// Build the id-vs-position divergence findings 8, 12 and 13 need, and put
    /// every id-bearing entry contiguous on top, before crossing the save
    /// boundary. See `Fuzzer::expose_stale_ids`.
    ///
    /// OFF for the gate. Not because it is slow but because it is DELIBERATE
    /// corruption: with it on a run reaches the data-loss findings by
    /// construction, and its classified-hit counts then say nothing about
    /// whether the model has acquired a NEW way to be wrong, which is the only
    /// thing the gate is for.
    pub(super) divergence: bool,
    /// Build the two closure shapes findings 4 and 10 need before fuzzing. See
    /// `Fuzzer::expose_closure_shapes`. Implies `closures`.
    ///
    /// OFF for the gate, for the same reason as `divergence` and for
    /// `closures`'s reason too: the shapes are built to reach known defects, so
    /// the hit counts say nothing about new ones.
    pub(super) closure_shapes: bool,
    /// Replay `(seed, moves)` as a PROLOGUE before this run's own stream.
    ///
    /// Corpus guidance, and the reason it costs nothing: every run in this
    /// module is a pure function of its seed, so "the state that run reached
    /// after n moves" is fully described by the pair `(seed, n)`. No project
    /// has to be serialized, stored or reloaded to replay a shape - which is
    /// what makes a corpus affordable inside a test binary.
    pub(super) prologue: Option<(u32, usize)>,
    /// How many times a persistence run crosses save/close/reopen.
    ///
    /// One is the historical behaviour and what the gate uses. MORE than one is
    /// what stacks heals on heals: `capture_missing_gauge_anchors` runs per
    /// save, the loader re-solves offsets per open, and the orphan-closure
    /// conversion happens on load - so every crossing bumps the epoch again and
    /// a snapshot recorded two epochs back is compared against a project that
    /// has been healed twice. Nothing exercised that: `cross_boundary` was
    /// called at most twice per run and only ever once in the gate.
    pub(super) crossings: usize,
    /// Run PAST the app's `undo_history_limit`, so the FIFO trim fires and the
    /// model becomes a sliding window.
    ///
    /// OFF for the gate because it needs thousands of moves to be worth
    /// anything: the trim only starts at depth 100, and the accumulation oracle
    /// it exists alongside needs a depth to be revisited dozens of times before
    /// tolerated dust can add up to anything.
    pub(super) horizon: bool,
    /// Draw this run's undo-relevant CONFIG - see [`DrawnConfig`] - instead of
    /// taking the app's defaults.
    ///
    /// OFF everywhere it was not asked for, and the draw comes from a SECOND
    /// generator (`seed ^ CONFIG_STREAM`), so a run's move stream is
    /// byte-identical with it on and off and every documented repro seed keeps
    /// reproducing. What changes is the shape of the history the same moves walk
    /// over: at `history_limit` 1 every push trims, which is the FIFO window
    /// under maximum pressure at 60 moves rather than at 10,000.
    ///
    /// `#[serde(default)]` here and on `combined` because a [`Trace`] carries
    /// its `Plan`: a trace file written before these fields existed still
    /// replays, and the default IS the behaviour it was recorded under.
    #[serde(default)]
    pub(super) config_fuzz: bool,
    /// The COMBINED regime: horizon, closures, closure shapes, jumps and several
    /// crossings AT ONCE, rather than one constructed shape at a time.
    ///
    /// Implies [`Self::config_fuzz`] (see `start_run`), which is how it gets the
    /// second of the three resolutions in the module header under "The COMBINED
    /// regime" - `crossing_would_truncate` keys on the drawn config rather than
    /// on this flag. The only other thing in the engine that branches on it is
    /// `run_fuzz`'s climb test, the first resolution; every other mode flag
    /// means here exactly what it means alone.
    #[serde(default)]
    pub(super) combined: bool,
}

// ===========================================================================
// The CONFIG SPACE
// ===========================================================================

/// The offset the CONFIG stream is seeded at: `seed ^ CONFIG_STREAM`.
///
/// ADDITIVE, which is the whole design constraint rather than a detail. Every
/// row of this file's identification matrix names a seed, and a run's moves are
/// a pure function of that seed's own `Rng` - so a new draw taken from THAT
/// generator would shift every later value and renumber every documented repro.
/// This takes its values from a second generator seeded off the same number, so
/// the primary stream is byte-identical whether or not config is drawn, and the
/// only thing the knob changes is the app the same moves run against.
const CONFIG_STREAM: u32 = 0x00C0_F167;

/// The undo-relevant configuration one run drew.
///
/// # What is in it, and what is not
///
/// `undo_history_limit` is the whole of it, and the exclusions were measured
/// rather than assumed. The config has exactly two undo-relevant entries
/// (`config/types.rs`, `GeneralConfig`), and the second one cannot be drawn:
///
/// * `undo_persist_limit` is read from `Config::general` at SAVE time
///   (`history_rows_to_persist`, core/state/project_io.rs), so drawing it means
///   writing the config struct - and `save_project` calls
///   `note_recent_project`, which calls `Config::save()`. A drawn value would
///   therefore be written into the developer's real `config.toml` and become the
///   default for every later run, in this process and every other one. (That the
///   fuzzer's saves already write that file is visible in its
///   `recent_projects`, which is full of `/tmp/.tmpXXXX/fuzz.stalx`.) The model
///   keeps [`UNDO_PERSIST_LIMIT`] instead, which is what the app's default is.
/// * The BYTE budget (`DepthLimit::Bytes`, [#303]) is not config at all: it is
///   armed only for a SpeleoDB-linked project in Full mode (`undo_policy_for`
///   returns `UndoPolicy::local()` without a link), and every project this
///   module builds is local. Its eviction is also by estimated size rather than
///   by count, which the FIFO oracle in `absorb_trim` cannot predict, so
///   reaching it would need a second oracle rather than a second draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DrawnConfig {
    /// `general.undo_history_limit`: how many entries each in-memory stack keeps
    /// before `UndoStack::enforce_depth_limit` FIFO-trims the oldest.
    history_limit: usize,
}

impl DrawnConfig {
    /// The run-header form, appended to every per-seed summary line and to a
    /// failure payload, so a failure names the app it happened in.
    fn describe(self) -> String {
        format!(", undo_history_limit={}", self.history_limit)
    }
}

/// The ladder the history limit is drawn from, and why each rung is on it.
///
/// | rung | what it is |
/// |---|---|
/// | 1 | every push trims. The one value at which a stack LENGTH cannot see a push at all - see `record_mutation`'s newest-entry fingerprint |
/// | 2, 3, 5, 8, 13 | the window is a handful of entries: a walk reaches the floor constantly and the trim fires on most moves |
/// | 20 | the persisted window (`UNDO_PERSIST_LIMIT`), where the in-memory and on-disk limits coincide |
/// | 40 | [`MAX_MODEL_DEPTH`], the headroom the ordinary modes were built to stay under |
/// | 100 | the app's own default, so the drawn regime CONTAINS the undrawn one |
/// | `iterations + 1` | larger than the run can ever reach: no trim is possible, which is the control |
fn config_history_ladder(iterations: usize) -> [usize; 10] {
    [
        1,
        2,
        3,
        5,
        8,
        13,
        UNDO_PERSIST_LIMIT,
        MAX_MODEL_DEPTH,
        DEFAULT_HISTORY_LIMIT,
        iterations.saturating_add(1),
    ]
}

/// What this run's config stream draws, or `None` when the knob is off.
///
/// A pure function of `(seed, plan)` like everything else here, which is what
/// lets a recorded trace be replayed against the same app: `run_trace` re-runs
/// `start_run`, so a shrunk candidate is measured under the limit its original
/// ran under rather than under the default.
fn draw_config(seed: u32, plan: Plan) -> Option<DrawnConfig> {
    if !plan.config_fuzz && !plan.combined {
        return None;
    }
    let mut rng = Rng::new(seed ^ CONFIG_STREAM);
    let ladder = config_history_ladder(plan.iterations);
    Some(DrawnConfig {
        history_limit: ladder[rng.below(ladder.len())],
    })
}

/// Everything a run does BEFORE its own move stream: the starting project, the
/// corpus prologue and the two exposure prologues.
///
/// Split out because a REPLAY re-runs all of it rather than replaying it. All of
/// it is a pure function of `(seed, Plan)`, so a shrunk trace starts from the
/// same project the recorded one did - and no shrink step can delete a prologue
/// move, which is what keeps `Plan::divergence`'s constructed shape intact while
/// the moves that walk over it are minimized.
fn start_run(seed: u32, plan: Plan) -> Fuzzer {
    // A corpus prologue rebuilds the shape a previous run reached, then hands
    // over to THIS run's stream. The project is built from the prologue's seed
    // (the builder is part of the shape) and the RNG is re-seeded at the
    // handover, so the continuation is fresh randomness over an inherited
    // topology rather than a longer replay of the same one.
    // The CONFIG the run is drawn, before the app it runs against is built: the
    // limit has to be armed before the first push, and `Fuzzer::new` reads the
    // app's own value back afterwards rather than trusting what it set.
    //
    // `combined` implies `config_fuzz` for the same kind of reason
    // `closure_shapes` implies `closures`: without a drawn limit the combined
    // regime is EMPTY at its most interesting point - see the module header,
    // "The COMBINED regime".
    let mut fuzzer = Fuzzer::new_with_config(
        plan.prologue
            .map_or(seed, |(prologue_seed, _)| prologue_seed),
        plan.closures || plan.closure_shapes,
        draw_config(seed, plan),
    );
    if let Some((_, moves)) = plan.prologue {
        for _ in 0..moves {
            if fuzzer.stopped() {
                break;
            }
            fuzzer.step_mutate();
        }
        fuzzer.rng = Rng::new(seed);
    }
    // The exposure prologue runs FIRST and crosses the boundary itself, so the
    // block of id-bearing entries it builds is the newest thing on the stack
    // when the reload lands and the closing unwind starts on it.
    if plan.divergence {
        fuzzer.expose_stale_ids();
        fuzzer.cross_boundary();
    }
    if plan.closure_shapes {
        fuzzer.expose_closure_shapes();
        fuzzer.orphan_pending = plan.persistence;
    }
    fuzzer
}

/// The closing guarantee, and the one crossing a persistence run owes: run
/// AFTER the move stream, drawn or replayed.
fn finish_run(mut fuzzer: Fuzzer, plan: Plan) -> Outcome {
    // The app's own merge-alignment assert is a FAILURE again. It was a
    // classified, tolerated hit while finding 14 and its `remove_shot_with_rewire`
    // mirror were open; both are fixed, so per the delete-when-fixed contract the
    // tolerance is gone and the only thing `catching_alignment` still buys is the
    // seed, the depth and the move log this failure carries.
    if fuzzer.outcome.alignment_panic {
        fuzzer.fail(
            "the app's own merge-alignment assert fired",
            "  see the last move in the log below for the assert's own message",
        );
    }
    fuzzer.outcome.signature = topology_signature(&fuzzer.state);
    // Cross the boundary at least once per persistence run, so the closing walk
    // below runs against a RESTORED stack rather than an in-memory one. (This
    // was skipped for a WEDGED run and for a merge-drift stop, neither of which
    // exists any more - a refused history move and a drifted merge both fail the
    // run.)
    if plan.persistence && fuzzer.outcome.reloads == 0 {
        fuzzer.cross_boundary();
    }
    fuzzer.finish();
    fuzzer.outcome
}

/// Which iterations of the main loop cross the save/close/reopen boundary.
fn boundary_iterations(plan: Plan) -> Vec<usize> {
    let crossings = plan.crossings.max(1);
    if plan.persistence && !plan.divergence {
        (0..crossings)
            .map(|index| plan.iterations / 3 + index * (plan.iterations * 2 / 3) / crossings)
            .collect()
    } else {
        Vec::new()
    }
}

pub(super) fn run_fuzz(seed: u32, plan: Plan) -> Outcome {
    let mut fuzzer = start_run(seed, plan);
    // Recording starts HERE, after the prologues: what a trace holds is the move
    // stream and nothing else.
    if recorder_armed() {
        recorder_begin(fuzzer.inventory());
    }
    // Crossing at a third of the way in leaves entries both below and above the
    // boundary, so the closing walk covers restored UNDO entries and restored
    // REDO entries alike. With `crossings` > 1 the rest are spread over the
    // remaining run, so each one lands on a stack the previous crossing already
    // restored - which is the case heals-on-heals needs.
    let boundaries = boundary_iterations(plan);

    // HORIZON mode deliberately pushes past the app's own history limit so the
    // FIFO trim fires and the sliding window gets fuzzed; every other mode stays
    // under it, which is what keeps `snapshots[0]` valid for the closing walk.
    let depth_cap = if plan.horizon {
        fuzzer.history_limit * 2
    } else {
        MAX_MODEL_DEPTH
    };
    for iteration in 0..plan.iterations {
        // Nothing after the app's own assert measures the app: it unwound
        // through the project mutex, so the lock is poisoned and half the call
        // sites now read "no project open".
        if fuzzer.outcome.alignment_panic {
            break;
        }
        if boundaries.contains(&iteration) {
            let mark = fuzzer.recorder_mark(MoveKind::Cross);
            fuzzer.cross_boundary();
            fuzzer.recorder_seal(mark);
        }
        // 60 / 25 / 15 mutate / undo / redo, re-rolled when the stack a move
        // needs is empty. Same mix as the harness.
        let roll = fuzzer.rng.below(100);
        // 60 / 25 / 15 mutate / undo / redo, EXCEPT while a horizon run is still
        // climbing: at that mix the depth random-walks around 25 and never
        // reaches the history limit at all (measured - 335 mutations, depth 23),
        // because every jump knocks the cursor down and the next mutation
        // truncates everything above it. 85 / 8 / 7 climbs while still walking
        // the history on the way up. ONE draw either way, so no other mode's
        // stream moves.
        // RESOLUTION 1 of the combined regime. `cursor - floor` is the number of
        // entries the app is holding, and `absorb_trim` pins that at exactly
        // `history_limit` from the first trim onwards - which is below
        // `depth_cap` (twice the limit) forever, so a horizon run stays
        // "climbing" for its whole life and NEVER draws a jump, whatever
        // `plan.jumps` says. The combined regime ends the climb where it was
        // meant to end: when the window starts sliding, which is what `floor`
        // moving off zero means. Byte-identical for `combined: false`, which is
        // every horizon run recorded in this file's open observations.
        let sliding = plan.combined && fuzzer.floor > 0;
        let climbing = plan.horizon && !sliding && fuzzer.cursor - fuzzer.floor < depth_cap;
        let mut back = if climbing {
            (85..93).contains(&roll)
        } else {
            (60..85).contains(&roll)
        };
        let mut forward = roll >= if climbing { 93 } else { 85 };
        if forward && fuzzer.cursor >= fuzzer.top {
            forward = false;
            back = fuzzer.cursor > fuzzer.floor;
        }
        if back && fuzzer.cursor == fuzzer.floor {
            back = false;
        }
        if !back && !forward {
            if fuzzer.cursor - fuzzer.floor >= depth_cap {
                if !fuzzer.step_recorded_history(true, 1) {
                    break;
                }
                continue;
            }
            fuzzer.step_mutate();
            if fuzzer.outcome.alignment_panic {
                break;
            }
            continue;
        }
        // About one history move in ten is taken as a multi-step jump.
        let reach = if back {
            fuzzer.cursor
        } else {
            fuzzer.top - fuzzer.cursor
        };
        // One history move in ten is a jump by default. `plan.jumps` is checked
        // FIRST so a run without jumps draws nothing here and its stream is
        // unchanged by this knob - the gate's pinned seeds depend on that.
        // No jumps while a horizon run is CLIMBING: a jump lands the cursor near
        // the floor and the next mutation truncates everything above it, which
        // caps the depth well below the history limit (measured - 909 mutations
        // and 57 jumps got to depth 63, never to 100). Jumps resume once the
        // window is sliding, which is where they are interesting anyway.
        let steps = if plan.jumps && !climbing && reach > 1 && fuzzer.rng.chance(JUMP_PERCENT) {
            1 + fuzzer.rng.below(reach)
        } else {
            1
        };
        if !fuzzer.step_recorded_history(back, steps) {
            break;
        }
        if fuzzer.outcome.alignment_panic {
            break;
        }
    }

    recorder_end();
    finish_run(fuzzer, plan)
}

/// Replay a recorded trace, or any subsequence of one, through the SAME engine.
///
/// This is the shrinker's probe. Everything outside the move stream - the
/// starting project, the prologues, the closing unwind/rewind walk - is re-run
/// from the plan, and every oracle is the one a drawn run uses, because the
/// moves go through `step_apply` and `step_history` exactly as drawn moves do.
///
/// A move whose target no longer exists is SKIPPED (see [`Refs`]); a history
/// move deeper than the candidate's history is clamped by `step_history`, which
/// already does that for the drawn path.
pub(super) fn run_trace(trace: &Trace, moves: &[TraceMove]) -> (Outcome, Vec<String>) {
    let mut fuzzer = start_run(trace.seed, trace.plan);
    let mut refs = match Refs::replay(&trace.prelude, &fuzzer.inventory()) {
        Ok(refs) => refs,
        Err(message) => panic!("cannot replay this trace: {message}"),
    };
    for mv in moves {
        if fuzzer.stopped() {
            break;
        }
        // The generator, put back where the recorded run had it. Nothing in a
        // replay draws except a crossing that builds finding 10's shape - see
        // `Rng::state`.
        fuzzer.rng.restore(mv.rng);
        let before = fuzzer.inventory();
        let alive = match &mv.kind {
            MoveKind::Mutate(op) => {
                let targets = read_targets(&fuzzer.state);
                fuzzer.step_apply(op, &mut refs, &targets) != StepOutcome::Stopped
            }
            MoveKind::Undo(steps) => fuzzer.step_history(true, *steps),
            MoveKind::Redo(steps) => fuzzer.step_history(false, *steps),
            MoveKind::Cross => {
                fuzzer.cross_boundary();
                true
            }
        };
        refs.learn(&mv.created, &fuzzer.inventory().created_since(&before));
        // The shrinker's own fault, checked after the move that could have
        // caused it. A recorded trace never carries one; only the shrink gate
        // sets it - see [`Fault`].
        if let Some(fault) = trace.fault {
            fuzzer.check_injected_fault(fault);
        }
        if !alive {
            break;
        }
    }
    let skipped = refs.skipped().to_vec();
    (finish_run(fuzzer, trace.plan), skipped)
}

// ===========================================================================
// The CI gate
// ===========================================================================

/// Seeds the gate runs, in both modes.
///
/// PINNED, and pinned deliberately rather than arbitrarily: `fuzz_seed_survey`
/// (below) reports which seeds are currently clean, and these are clean at 100
/// moves, so they have headroom above the 60 the gate uses.
///
/// RE-PINNED once the classifiers could name what they were hitting. The list
/// was six seeds when 32 of the first 40 reached an unclassified finding within
/// 100 moves; both of the findings responsible turned out to be one already on
/// the issue (finding 3, which nothing classified in its ratcheted or
/// data-loss form) plus its cascade, and with those named 35 of the first 40 are
/// clean at 100. Thirteen of them are gated, which is still well under a second
/// per mode.
///
/// What that does NOT mean is that the gate now covers those findings: it covers
/// SEQUENCES that do not reach them. The modes that construct a known defect
/// (`Plan::divergence`, `Plan::closure_shapes`, `jumps`, `closures`) stay out of
/// the gate on purpose - see the matrix in the module header. Re-run the survey
/// and widen this list as findings are FIXED.
const GATE_SEEDS: &[u32] = &[1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 19, 26];
/// Moves per gate seed. 60 keeps the whole gate well inside a second while still
/// reaching depth 20+, crossing the persistence boundary and walking the closing
/// unwind/rewind over every entry.
pub(super) const GATE_ITERATIONS: usize = 60;

/// Every run of digits as a single `#`.
///
/// The one normalization every part of a signature goes through: it is what
/// keeps "at depth 23" and "at depth 7" one bucket rather than twenty, and it is
/// why a distance, a coordinate or a section id cannot split a bucket even where
/// one reaches the signature.
fn collapse_digits(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_digits = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            if !in_digits {
                out.push('#');
                in_digits = true;
            }
        } else {
            out.push(ch);
            in_digits = false;
        }
    }
    out
}

/// Discriminators a payload carries BELOW its headline, as
/// `(marker, terminator, label)`.
///
/// A [`Fuzzer::fail`] headline is FIXED TEXT - "the app's own merge-alignment
/// assert fired" says WHICH INVARIANT broke and nothing about WHERE - so every
/// defect that trips one app assert shares one signature. Issue #464 filed three
/// such defects at once, and shrinking the second one drifted onto the third:
/// ddmin accepted a candidate that reproduced "an alignment assert" rather than
/// THE alignment assert, and every later step then minimized for the wrong bug.
///
/// The assert's own message is in the move-log tail and it names the CALL SITE
/// that asserted ("Merge alignment violated after `remove_shot_with_rewire`:").
/// That token is the discriminator, and it is stable in the only sense that
/// matters here: it is a compile-time `&'static str` literal passed to
/// `Project::debug_assert_merge_alignment` (models/project/offsets.rs), the same
/// text on every run, on every machine, at every geometry.
///
/// What is deliberately NOT admitted is the rest of that message - the endpoints
/// and the distance ("... are 1111.324000m apart"). Those move with the geometry
/// that a shrink step is REMOVING MOVES from, so a signature carrying them would
/// reject the very candidates ddmin exists to accept, and a shrink would
/// terminate at the original trace every time.
const DISCRIMINATORS: &[(&str, char, &str)] = &[("Merge alignment violated after ", ':', "after")];

/// The first discriminator this payload carries, if any.
///
/// FIRST occurrence only, and one entry only: a payload's tail is a WINDOW over
/// the move log, so admitting every occurrence would put the window's contents
/// into the bucket key and make the signature depend on how deep the run got.
fn discriminator(message: &str) -> Option<String> {
    DISCRIMINATORS
        .iter()
        .find_map(|(marker, terminator, label)| {
            let rest = message.split_once(marker)?.1;
            // Bounded twice: at the terminator the message puts after the token,
            // and at the end of the line either way.
            let line = rest.lines().next().unwrap_or(rest);
            let token = line.split(*terminator).next().unwrap_or(line).trim();
            if token.is_empty() {
                return None;
            }
            Some(format!("{label}={}", collapse_digits(token)))
        })
}

/// Bucket a failure by its SHAPE rather than by its text.
///
/// The headline alone buckets by depth ("did not restore the state recorded at
/// depth 23"), which splits one defect across twenty buckets and hides how many
/// distinct things a sweep actually found. Digits are collapsed to `#` and the
/// first differing path is appended, so two seeds that lost the same field at
/// different depths land in the same bucket - which is what a triage read wants.
///
/// The headline alone also buckets TOO COARSELY in the other direction, once a
/// headline names an invariant rather than a call site: see [`DISCRIMINATORS`]
/// for the token that splits those back apart and for why it is only that token.
pub(super) fn failure_signature(message: &str) -> String {
    let headline = collapse_digits(message.lines().next().unwrap_or("<empty>"));
    let first_path = message
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("at "))
        .map(collapse_digits);
    let mut signature = headline;
    if let Some(token) = discriminator(message) {
        signature.push_str(" | ");
        signature.push_str(&token);
    }
    if let Some(path) = first_path {
        signature.push_str(" | ");
        signature.push_str(&path);
    }
    signature
}

pub(super) fn summarize(label: &str, seed: u32, outcome: &Outcome) -> String {
    let hits = outcome
        .hits
        .iter()
        .map(|(id, count)| format!("{id}={count}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{label} seed={seed}: {} mutations, {} rejected, {} no-ops, {} undos, {} redos, \
         {} jumps, {} saves, {} reloads, depth<={}, float-noise={}, attributed={}, \
         integrity-compared={}, adjusted-compared={}, oracle-us=(snap {} integrity {} adjusted {}), \
         trimmed={}, peak={:?}{}{}{}",
        outcome.mutations,
        outcome.rejected,
        outcome.noops,
        outcome.undos,
        outcome.redos,
        outcome.jumps,
        outcome.saves,
        outcome.reloads,
        outcome.max_depth,
        outcome.float_noise,
        outcome.attributed,
        outcome.integrity_compared,
        outcome.adjusted_compared,
        outcome.oracle_us.0,
        outcome.oracle_us.1,
        outcome.oracle_us.2,
        outcome.trimmed,
        outcome.peak,
        // Empty unless the config knob drew something, so every line this module
        // printed before the knob existed is unchanged to the byte.
        outcome.config.map(DrawnConfig::describe).unwrap_or_default(),
        if outcome.alignment_panic {
            " ALIGNMENT-PANIC"
        } else {
            ""
        },
        if hits.is_empty() {
            String::new()
        } else {
            format!("; classified hits: {hits}")
        }
    )
}

// ===========================================================================
// The env knobs the ENGINE reads, and the seed-parallel runner
// ===========================================================================

/// `STALAGMIGHT_RUST_FUZZ_VERBOSE`, read ONCE per process.
///
/// Both engine-level knobs are LATCHED rather than re-read, and the reason is
/// the parallel sweeps below. `std::env::var` is neither free nor race-free on
/// a hot path: it takes the process-wide environment lock on every call, and
/// these two sit inside `describe_diffs` (per differing path) and `note` (per
/// MOVE), so a soak makes millions of them - from every worker at once. Other
/// tests in this same binary call `set_var`, which against a concurrent reader
/// is a data race in the C sense whatever the edition says about the signature.
///
/// Latching also buys the property the determinism contract actually needs: a
/// run cannot change mode half way through, so what a seed does is fixed before
/// the first move rather than re-decided at every diff.
fn fuzz_verbose() -> bool {
    static VERBOSE: OnceLock<bool> = OnceLock::new();
    *VERBOSE.get_or_init(|| std::env::var("STALAGMIGHT_RUST_FUZZ_VERBOSE").is_ok())
}

/// `STALAGMIGHT_RUST_FUZZ_TRACE`, read ONCE per process. See [`fuzz_verbose`]
/// for why it is latched.
fn fuzz_trace() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("STALAGMIGHT_RUST_FUZZ_TRACE").is_ok())
}

/// A caught panic's message, or a marker when the payload is not a string.
///
/// The same three lines were written out at every `catch_unwind` in this
/// module; a sweep that silently reported `<non-string panic>` because one of
/// them forgot the `&str` arm would be a failure lost, which is the one thing
/// the sweeps must not do.
pub(super) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_else(|| "<non-string panic>".to_string())
}

/// Cores left alone. A sweep that saturates the machine measures the scheduler
/// as much as the model, and the last core is what the test harness's own
/// thread, the tracing writer and whatever else is on the box are living on.
const THREAD_HEADROOM: usize = 1;

/// Stack per worker. A spawned thread's default is 2 MiB, which is a quarter of
/// what libtest gives the thread a `#[test]` runs on, and this engine recurses
/// through `canonicalize`/`diff_values` over a whole serialized project. Sizing
/// the workers UP is free (the pages are mapped on demand) and removes the one
/// way a parallel run could fail where the sequential one does not.
const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

/// How many workers a sweep runs its seeds on: `STALAGMIGHT_RUST_FUZZ_THREADS`,
/// else the machine's parallelism less [`THREAD_HEADROOM`], never more than
/// there are jobs and never less than one.
///
/// `TRACE=1` pins one worker unless the knob overrides it, because the trace is
/// a STREAM: twelve seeds interleaving their moves into one stdout is not a
/// move log, and reading that log in order is the entire point of the mode.
pub(super) fn fuzz_threads(jobs: usize) -> usize {
    let requested = std::env::var("STALAGMIGHT_RUST_FUZZ_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let threads = match requested {
        Some(count) => count,
        None if fuzz_trace() => 1,
        None => std::thread::available_parallelism()
            .map_or(1, |cores| cores.get().saturating_sub(THREAD_HEADROOM)),
    };
    threads.clamp(1, jobs.max(1))
}

/// Run `job` once per entry of `jobs` across a pool of `threads` workers, and
/// return the results IN INPUT ORDER whatever order they finished in.
///
/// # Why this is sound
///
/// A run is a pure function of `(seed, Plan)`: `Rng` is integer-only and lives
/// in the `Fuzzer`, every `AppState` is built by the run that uses it, and a
/// persistence run saves into its OWN `TempDir`. Seeds share no mutable state,
/// so a seed's `Outcome` is the same value whichever worker computed it. The
/// only process-global the engine touched was the environment, and
/// [`fuzz_verbose`]/[`fuzz_trace`] latch that before any worker starts.
///
/// # Why the order matters more than the speed
///
/// Every caller merges these results SEQUENTIALLY, in this vector's order, so
/// the printed output is a function of the seeds and not of the scheduler.
/// `STALAGMIGHT_RUST_FUZZ_THREADS=1` is therefore not a fallback path: it is
/// this same code with one worker, and any difference between it and eight
/// workers is a bug in this function rather than a mode of the fuzzer. No
/// caller prints the worker count for the same reason - two runs at different
/// thread counts have to diff clean.
///
/// The one number that is NOT a pure function of the seed is `summarize`'s
/// `oracle-us` triple. It is wall time, so it inflates under contention;
/// measure the oracle cost at one thread.
///
/// # Failures
///
/// A panic is caught PER JOB and returned as its message, so one failing seed
/// neither aborts its siblings nor loses its attribution - the caller pairs it
/// back with the input that produced it and reports it after every job is in.
/// `AssertUnwindSafe` is honest here for the reason it is honest in
/// `fuzz_seed_survey`: a panicking run's `AppState` is dropped immediately and
/// never observed again.
pub(super) fn run_seed_jobs<J: Sync, R: Send>(
    jobs: &[J],
    threads: usize,
    job: impl Fn(&J) -> R + Sync,
) -> Vec<Result<R, String>> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Result<R, String>)>> =
        Mutex::new(Vec::with_capacity(jobs.len()));
    std::thread::scope(|scope| {
        for worker in 0..threads {
            std::thread::Builder::new()
                .name(format!("fuzz-worker-{worker}"))
                .stack_size(WORKER_STACK_BYTES)
                .spawn_scoped(scope, || {
                    loop {
                        // Index-at-a-time rather than a static split: seeds cost
                        // wildly different amounts (a wedged run stops early, a
                        // persistence run does real SQLite I/O), so a static
                        // split would leave the sweep waiting on one worker.
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = jobs.get(index) else {
                            break;
                        };
                        let result =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(item)))
                                .map_err(|payload| panic_message(&*payload));
                        results
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push((index, result));
                    }
                })
                .expect("failed to spawn a fuzz worker thread");
        }
    });
    let mut collected = results.into_inner().unwrap_or_else(PoisonError::into_inner);
    collected.sort_by_key(|(index, _)| *index);
    collected.into_iter().map(|(_, result)| result).collect()
}

/// THE GATE. Not `#[ignore]`d, so it runs on every `cargo test` and in CI.
///
/// It passes when every difference the fuzzer sees is either equal, inside the
/// numeric tolerance, a documented save/reload heal, or one of the classified
/// open bugs on <https://github.com/Stalagmight/Stalagmight/issues/413>. ANY
/// unexplained divergence panics with the seed, the depth and the first differing
/// paths.
///
/// So this is a REGRESSION guard rather than a bug hunt: the hunt happens in the
/// soak below and in the agent harness. What it guarantees is that the undo model
/// does not acquire a NEW way to be wrong without someone noticing.
#[test]
fn undo_redo_fuzz_gate() {
    let _config = isolated_config();
    let mut total: BTreeMap<&'static str, usize> = BTreeMap::new();
    for &seed in GATE_SEEDS {
        let outcome = run_fuzz(
            seed,
            Plan {
                iterations: GATE_ITERATIONS,
                persistence: false,
                crossings: 1,
                prologue: None,
                jumps: false,
                closures: false,
                divergence: false,
                closure_shapes: false,
                horizon: false,
                config_fuzz: false,
                combined: false,
            },
        );
        println!("{}", summarize("in-memory", seed, &outcome));
        assert!(
            outcome.mutations > 10,
            "seed {seed} barely mutated ({} mutations); the generators are not reaching the project",
            outcome.mutations
        );
        assert!(
            outcome.undos > 3 && outcome.redos > 3,
            "seed {seed} barely walked the history (undos={} redos={})",
            outcome.undos,
            outcome.redos
        );
        for (id, count) in outcome.hits {
            *total.entry(id).or_insert(0) += count;
        }
    }
    println!("gate (in-memory) classified totals: {total:?}");
}

/// The persistence half of the gate: every seed crosses save/close/reopen and
/// then walks the RESTORED stacks, including the restored REDO entries.
///
/// Split from the in-memory gate because it is the slower half (each crossing is
/// a real SQLite write and read) and because a failure here has a different
/// meaning: it is about what survives the file, not about the model.
#[test]
fn undo_redo_fuzz_gate_persistence() {
    let _config = isolated_config();
    let mut total: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut crossings = 0;
    for &seed in GATE_SEEDS {
        let outcome = run_fuzz(
            seed,
            Plan {
                iterations: GATE_ITERATIONS,
                persistence: true,
                crossings: 1,
                prologue: None,
                jumps: false,
                closures: false,
                divergence: false,
                closure_shapes: false,
                horizon: false,
                config_fuzz: false,
                combined: false,
            },
        );
        println!("{}", summarize("persistence", seed, &outcome));
        crossings += outcome.reloads;
        for (id, count) in outcome.hits {
            *total.entry(id).or_insert(0) += count;
        }
    }
    assert!(
        crossings >= GATE_SEEDS.len(),
        "the persistence gate never crossed the boundary ({crossings} reloads)"
    );
    println!("gate (persistence) classified totals: {total:?}");
}

/// The MULTI-CROSSING half of the persistence gate: several save/close/reopen
/// boundaries per run, so the run walks stacks that a previous crossing already
/// restored.
///
/// One crossing exercises "does the file carry the history". Several exercise
/// what only stacking can: `capture_missing_gauge_anchors` runs on every save,
/// the loader re-solves offsets on every open, the orphan-closure conversion
/// happens on load, and each crossing bumps the epoch again - so a snapshot
/// recorded two epochs back is compared against a project that has been healed
/// twice, and the save/reload attribution has to hold across BOTH. Nothing
/// exercised that: `cross_boundary` was reachable at most twice per run and the
/// gate only ever crossed once.
/// Runs `GATE_SEEDS`, the same list as its two siblings.
///
/// It ran a shorter, separately-pinned list while seed 10 failed here and
/// nowhere else. That observation was finding 12 cascading, and it no longer
/// reproduces now that the restore paths address shots by uuid - so the
/// exclusion and its list are deleted and the multi-crossing gate covers the
/// same seeds as the in-memory and persistence halves.
const MULTI_CROSSING_SEEDS: &[u32] = GATE_SEEDS;

#[test]
fn undo_redo_fuzz_gate_multi_crossing() {
    let _config = isolated_config();
    let mut total: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut reloads = 0;
    for &seed in MULTI_CROSSING_SEEDS {
        let outcome = run_fuzz(
            seed,
            Plan {
                iterations: GATE_ITERATIONS,
                persistence: true,
                crossings: 3,
                prologue: None,
                jumps: false,
                closures: false,
                divergence: false,
                closure_shapes: false,
                horizon: false,
                config_fuzz: false,
                combined: false,
            },
        );
        println!("{}", summarize("multi-crossing", seed, &outcome));
        reloads += outcome.reloads;
        for (id, count) in outcome.hits {
            *total.entry(id).or_insert(0) += count;
        }
    }
    // A run that crossed once per seed proves nothing this test does not
    // already get from `undo_redo_fuzz_gate_persistence`.
    assert!(
        reloads > MULTI_CROSSING_SEEDS.len(),
        "the multi-crossing gate crossed {reloads} time(s) over {} seed(s) - no seed crossed \
         more than once, so nothing stacked",
        MULTI_CROSSING_SEEDS.len()
    );
    println!("gate (multi-crossing) classified totals: {total:?}");
}

// ===========================================================================
// The COMBINED gate
// ===========================================================================

/// Crossings per combined run. Three for the same reason
/// `undo_redo_fuzz_gate_multi_crossing` uses three - heals stacking on heals -
/// and now on a history the FIFO trim is actively sliding.
const COMBINED_CROSSINGS: usize = 3;
/// Moves per combined seed. Enough that the drawn window slides for most of the
/// run (a seed drawing a limit of 3 trims from move ~5 on) while the whole gate
/// stays in the same second-or-two band as the other three.
const COMBINED_ITERATIONS: usize = 300;

/// The COMBINED regime's plan: every mode that constructs a shape, at once.
///
/// `persistence` is not a parameter because multi-crossing IS half of what this
/// regime is; `divergence` is the one deliberate omission, and it is not
/// squeamishness: `boundary_iterations` returns NO boundaries when divergence is
/// on (its exposure prologue crosses on its own schedule instead), so combining
/// it with `crossings: 3` would silently delete the multi-crossing half. It also
/// builds its stale ids by construction, which is the reason the header gives
/// for keeping it out of every gate.
const fn combined_plan(iterations: usize) -> Plan {
    Plan {
        iterations,
        persistence: true,
        crossings: COMBINED_CROSSINGS,
        prologue: None,
        jumps: true,
        closures: true,
        divergence: false,
        closure_shapes: true,
        horizon: true,
        config_fuzz: true,
        combined: true,
    }
}

/// The combined gate's seeds, MINED rather than chosen: `fuzz_seed_survey` under
/// `COMBINED=1` at 40 seeds x 300 moves, of which 30 are clean, and these two are
/// the pair whose classified hits cover the most ground while exercising every
/// part of the regime at once.
///
/// | seed | drawn limit | what only it brings |
/// |---|---|---|
/// | 15 | 1 | the FIFO window under maximum pressure: 126 entries trimmed, every push trimming, and the newest-entry fingerprint the only way a push is visible at all |
/// | 21 | 3 | 30 jumps and `finding-7-a-jump-is-not-n-single-steps`, which is reachable ONLY because the combined regime lets a horizon run jump - see resolution 1 |
///
/// Both cross three times, both trim, both are clean, and between them they used
/// to reach the eight ids `COMBINED_CLASSIFIED` pinned. Seed 19 (limit 3, 52
/// trimmed, 28 jumps) was the runner-up and added findings 17 and 4 at the cost
/// of a third of the gate's runtime; seed 3 is richer still and was kept out
/// because its in-memory half failed on what turned out to be finding 26.
///
/// The pair is KEPT after the fixes rather than re-mined, because what made it
/// the best pair is not the ids: it is that seed 15 drives the FIFO window at a
/// drawn limit of 1 (126 entries trimmed, every push trimming) and seed 21 draws
/// 30 jumps over a sliding window. Those are properties of the REGIME, they are
/// asserted below one by one, and they are what the gate is now for.
const COMBINED_SEEDS: &[u32] = &[15, 21];

// `COMBINED_CLASSIFIED` pinned the SET of ids the combined regime had to REACH -
// findings 1, 10, 2, 3 (both halves), 4, 5 and 7 - so that a regime which
// stopped combining could not go quietly green. Its own doc said what to do when
// a finding was fixed: drop the id and say so. All eight are fixed, so the list
// is empty and therefore deleted, and the pin is INVERTED: the assertion below
// is that the combined regime reaches NOTHING at all. What still proves the
// regime is combining are the three structural assertions - it trimmed, it
// crossed more than once per seed, and it jumped - which no fix can satisfy
// vacuously.

/// THE COMBINED GATE: horizon, closures, closure shapes, jumps, several
/// crossings and a drawn `undo_history_limit`, all in one run.
///
/// Every other gate holds one variable at a time, which is what makes them
/// readable - and what leaves the interactions unguarded. This one exists
/// because three of the interactions are not the sum of their parts:
///
/// * a horizon run's FIFO window and a save/reload crossing had never met (the
///   crossing was refused above `undo_persist_limit` and the horizon depth is
///   hundreds), so nothing had ever reloaded a history the trim was sliding;
/// * a horizon run had never drawn a jump, because its climb test can never
///   finish - see resolution 1 in the module header;
/// * the closure shapes findings 4 and 10 need had never been walked over by a
///   trimmed history, where the entry a walk reaches is one epoch old and the
///   entry below it no longer exists.
///
/// It runs in the same second-or-two band as the other three because the drawn
/// limit is what makes the horizon regime CHEAP: at a limit of 1 or 3 the window
/// slides from the fifth move, so 300 moves reach a state that used to take ten
/// thousand.
#[test]
fn undo_redo_fuzz_gate_combined() {
    let _config = isolated_config();
    let mut total: BTreeMap<&'static str, usize> = BTreeMap::new();
    let (mut reloads, mut trimmed, mut jumps) = (0, 0, 0);
    for &seed in COMBINED_SEEDS {
        let outcome = run_fuzz(seed, combined_plan(COMBINED_ITERATIONS));
        println!("{}", summarize("combined", seed, &outcome));
        reloads += outcome.reloads;
        trimmed += outcome.trimmed;
        jumps += outcome.jumps;
        assert!(
            outcome.mutations > 50,
            "seed {seed} barely mutated ({} mutations) - the combined regime stopped reaching \
             the project",
            outcome.mutations
        );
        for (id, count) in outcome.hits {
            *total.entry(id).or_insert(0) += count;
        }
    }
    // Each of the three, asserted rather than assumed: a combined run that did
    // not trim is a horizon run that never got going, one that did not cross is
    // the multi-crossing half missing, and one that never jumped is resolution 1
    // regressed - and all three would still pass a hit-count check.
    assert!(
        trimmed > 0,
        "no combined seed reached the drawn history limit ({trimmed} entries trimmed) - the \
         sliding window went unfuzzed"
    );
    assert!(
        reloads > COMBINED_SEEDS.len(),
        "the combined gate crossed {reloads} time(s) over {} seed(s) - no seed crossed more \
         than once, so nothing stacked",
        COMBINED_SEEDS.len()
    );
    assert!(
        jumps > 0,
        "the combined gate drew no multi-step jump - a horizon run that cannot jump is the \
         defect resolution 1 fixed"
    );
    // And the pin, inverted now that every finding it named is fixed: the
    // hardest regime this module has reaches NOTHING. `outcome.hits` is the only
    // channel a tolerated difference has left - `classify` answers `None` for
    // every diff, so the map can only ever hold the accumulation id - and an
    // empty map is the statement that the combined regime found nothing even
    // that far short of a failure.
    assert!(
        total.is_empty(),
        "the combined regime tolerated something: {total:?}. Every #413 finding is fixed and \
         `CLASSIFIERS` is empty, so a hit here is either a NEW open bug (name it, classify it, \
         and pin it) or tolerated float dust that has crept past the tolerance"
    );
    println!("gate (combined) classified totals: {total:?}");
}

/// Determinism is the property every other guarantee rests on: without it a
/// failing seed is not a repro and the shrink loop in the harness has nothing to
/// stand on.
///
/// Both gated regimes are in here, and the second one is not a formality: the
/// COMBINED plan draws its `undo_history_limit` from a second generator, saves
/// and reloads three times, and walks a history the FIFO trim is sliding under
/// it - three more chances for a run to depend on something that is not its
/// seed. Its classified totals are pinned by this comparison rather than by a
/// literal: two runs of the same seed have to agree exactly, id for id and count
/// for count.
#[test]
fn the_fuzzer_is_deterministic() {
    let _config = isolated_config();
    // The GATES' seeds, not arbitrary ones: a seed that reaches an open finding
    // panics rather than returning an `Outcome`, and this test is about replay
    // fidelity, not about which seeds are clean (`fuzz_seed_survey` answers that).
    let plain = Plan {
        iterations: GATE_ITERATIONS,
        persistence: false,
        crossings: 1,
        prologue: None,
        jumps: false,
        closures: false,
        divergence: false,
        closure_shapes: false,
        horizon: false,
        config_fuzz: false,
        combined: false,
    };
    let runs = GATE_SEEDS.iter().map(|&seed| (seed, plain)).chain(
        COMBINED_SEEDS
            .iter()
            .map(|&seed| (seed, combined_plan(COMBINED_ITERATIONS))),
    );
    for (seed, plan) in runs {
        let first = run_fuzz(seed, plan);
        let second = run_fuzz(seed, plan);
        assert_eq!(
            (
                first.mutations,
                first.rejected,
                first.noops,
                first.undos,
                first.redos,
                first.max_depth
            ),
            (
                second.mutations,
                second.rejected,
                second.noops,
                second.undos,
                second.redos,
                second.max_depth
            ),
            "seed {seed} did not replay identically (combined={})",
            plan.combined
        );
        assert_eq!(
            first.hits, second.hits,
            "seed {seed} classified differently on replay (combined={})",
            plan.combined
        );
        assert_eq!(
            first.config, second.config,
            "seed {seed} drew a different config on replay"
        );
    }
}

// ===========================================================================
// RED tests pinned to a SEED
// ===========================================================================
//
// Two findings have no reduction that reproduces them - both were measured, and
// both attempts are recorded in `open_undo_bugs_tests.rs`. What they do have is
// a deterministic seed, which is a repro in every sense that matters: it is
// synthetic (no fixture is read or derived from), it is byte-identical on every
// machine (`Rng` is integer-only and no float makes a decision anywhere in this
// module), and it fails today for the documented reason.
//
// They were `#[ignore]`d like every other red test, on the rule DELETE THE
// IGNORE WHEN THE BUG IS FIXED. Both are fixed and neither is ignored: each is
// now the regression guard for the seed that found it.

/// The diff cap REPORTS itself.
///
/// Every tolerate path in `expect_state` reads "nothing unexplained is left" off
/// the returned list, so the difference between "exactly `MAX_DIFFS`
/// differences" and "at least that many, and the rest were never walked" is the
/// difference between a sound green and a vacuous one. The walk collects
/// `MAX_DIFFS + 1` so the caller can tell them apart; this pins both directions,
/// because an off-by-one either way silently disarms the guard.
#[test]
fn the_diff_cap_says_when_it_hid_something() {
    let _config = isolated_config();
    let object_with = |count: usize, from: usize| -> String {
        let fields: Vec<String> = (0..count)
            .map(|i| format!("\"k{i}\":{}", i + from))
            .collect();
        format!("{{{}}}", fields.join(","))
    };

    // Fewer differences than the cap: complete, and it says so.
    let (few, truncated) = diff_snapshots_capped(&object_with(3, 0), &object_with(3, 100));
    assert_eq!(few.len(), 3);
    assert!(!truncated, "3 differences is not a truncation");

    // EXACTLY the cap: still complete. This is the boundary an off-by-one turns
    // into a false alarm, which would make every wide-but-classified mismatch
    // fail and the guard get relaxed again.
    let (exact, truncated) =
        diff_snapshots_capped(&object_with(MAX_DIFFS, 0), &object_with(MAX_DIFFS, 100_000));
    assert_eq!(exact.len(), MAX_DIFFS);
    assert!(
        !truncated,
        "exactly {MAX_DIFFS} differences fit in the cap and must not read as truncated"
    );

    // One more than the cap: the list is a prefix, and nothing may be concluded
    // from it. The returned list is still capped, so callers that only report
    // are unaffected.
    let (over, truncated) = diff_snapshots_capped(
        &object_with(MAX_DIFFS + 1, 0),
        &object_with(MAX_DIFFS + 1, 100_000),
    );
    assert_eq!(over.len(), MAX_DIFFS, "the reported list stays capped");
    assert!(
        truncated,
        "one difference past the cap must report as truncated"
    );
}

/// A move that does NOT RETURN is the move the failure names.
///
/// The tail is built from `log`, and a move's log line is written after it
/// returns, so before `in_flight` existed a panicking move left the tail ending
/// on the PREVIOUS move - with `catching_alignment`'s "^ ..." note pointing at
/// it, which reads as an attribution and is one. It mis-read issue #464's
/// defects B and C twice.
///
/// Both halves are pinned here, on a synthetic alignment panic raised through
/// `catching_alignment` itself: the log line the flush writes, and what
/// `failure_signature` then makes of the payload `fail` builds - which is where
/// the two follow-ups meet, since the call site the tail now carries is the
/// discriminator ddmin matches on.
#[test]
fn a_move_that_panics_is_the_move_the_failure_tail_names() {
    let _config = isolated_config();
    let mut fuzzer = Fuzzer::new(1, false);
    fuzzer.note("createShot(s=8,append) -> ok (undo 0->1, redo 0->0)".to_string());
    fuzzer.starting_move("deleteShot(8:800002)".to_string());

    // FIRST render path: an oracle that fails while a move is still running.
    // The tail has no line for that move yet, so `fail` names it itself.
    let mid_move = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fuzzer.fail("something failed mid-move", "  detail")
    }))
    .expect_err("`fail` returned");
    let mid_move = panic_message(&*mid_move);
    assert!(
        mid_move.contains("deleteShot(8:800002) -> IN FLIGHT"),
        "a failure raised mid-move does not name the move that was running:\n{mid_move}"
    );
    assert!(
        mid_move
            .lines()
            .last()
            .is_some_and(|line| line.contains("deleteShot(8:800002)")),
        "the in-flight move is not the LAST line of the tail, so the tail still reads as if the \
         move before it is where the run stopped:\n{mid_move}"
    );

    // SECOND render path: the move panics outright, and the flush puts it in
    // the log so the note that follows attributes the assert to it.
    let absorbed = fuzzer.catching_alignment(|_| {
        panic!(
            "Merge alignment violated after remove_shot_with_rewire: stations A and B are \
             16.700000m apart (threshold 0.01m). A=(1.000000, 2.000000), B=(0.000000, 0.000000)"
        )
    });
    assert!(
        absorbed.is_none(),
        "the alignment assert was not absorbed, so nothing below is being measured"
    );
    assert!(fuzzer.outcome.alignment_panic);

    // The panicking move has a line of its own, and the "^ ..." note is UNDER
    // it rather than under the move before it.
    let flushed = fuzzer
        .log
        .iter()
        .position(|line| line.contains("deleteShot(8:800002)"))
        .expect("the in-flight move was never written to the log");
    assert!(
        fuzzer.log[flushed].contains("PANICKED"),
        "the in-flight move's line does not say it never returned: {}",
        fuzzer.log[flushed]
    );
    assert!(
        fuzzer.log[flushed + 1].contains("^ the app's own merge-alignment assert fired"),
        "the assert note is not directly under the move that panicked: {:?}",
        fuzzer.log
    );

    // ... and the payload the run then fails with carries that move's assert,
    // so the signature ddmin matches on names the call site.
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fuzzer.fail(
            "the app's own merge-alignment assert fired",
            "  see the last move in the log below for the assert's own message",
        )
    }))
    .expect_err("`fail` returned");
    let message = panic_message(&*payload);
    assert!(
        message.contains("deleteShot(8:800002) -> PANICKED"),
        "the failure tail does not name the move that panicked:\n{message}"
    );
    assert_eq!(
        failure_signature(&message),
        "the app's own merge-alignment assert fired | after=remove_shot_with_rewire",
        "the call site did not reach the signature"
    );
}

/// FINDING 14, red: an insert leaves a merged partner metres out of alignment,
/// and the app's own `#[cfg(test)]` assert says so from inside
/// `insert_shot_after`.
///
/// The fuzzer catches that panic, verifies the message is that assert, and stops
/// the run - so the finding shows up here as `alignment_panic` rather than as a
/// process abort.
///
/// FIXED, and kept as the REGRESSION GUARD: this seed was red on
/// `Merge alignment violated after insert_shot_after: ... 3.965451m apart`. The
/// run itself now fails on an alignment assert (`finish_run`), so this assertion
/// is the second line of defence rather than the first - and it is the one that
/// names the seed.
#[test]
fn finding_14_no_insert_leaves_a_merge_out_of_alignment() {
    let _config = isolated_config();
    let outcome = run_fuzz(
        6,
        Plan {
            iterations: 60,
            persistence: false,
            crossings: 1,
            prologue: None,
            jumps: false,
            closures: true,
            divergence: false,
            closure_shapes: false,
            horizon: false,
            config_fuzz: false,
            combined: false,
        },
    );
    assert!(
        !outcome.alignment_panic,
        "seed 6 x60 with closures tripped the app's own merge-alignment assert: {}",
        summarize("finding-14", 6, &outcome)
    );
}

/// BUG 7, red: `undo_to(N)` is not N x `undo()`.
///
/// `undo()`/`redo()` end in `reconcile_geometry()`; `undo_to()`/`redo_to()` loop
/// the `*_inner` variants and call it neither per step nor at the end
/// (core/state/undo.rs). The fuzzer proves the inequality rather than
/// recognising it: on a jump that diverges it replays the same distance in
/// single steps and only records the finding when the single-step path lands
/// exactly where the model says it should.
///
/// This is the first pin bug 7 has ever had. `open_undo_bugs_tests.rs` records
/// the four synthetic reductions that were measured and do NOT diverge, and why:
/// `realign_merges` is `resolve_offsets` behind an alignment gate, and every
/// command's own epilogue has already run it, so a persistently misaligned merge
/// is not reachable through the command API in a small project. (That gate was 5
/// cm when the reductions were measured and is `MERGE_ALIGNMENT_INVARIANT_M`
/// now, #413 finding 20 - tighter, so the argument only got stronger.)
///
/// FIXED, and kept as the REGRESSION GUARD: `undo_to`/`redo_to` reconcile per
/// step now (core/state/undo.rs), so the jump path and the single-step path end
/// in the same epilogue.
#[test]
fn bug_7_a_jump_is_n_single_steps() {
    let _config = isolated_config();
    let outcome = run_fuzz(
        6,
        Plan {
            iterations: 150,
            persistence: true,
            crossings: 1,
            prologue: None,
            jumps: true,
            closures: true,
            divergence: false,
            closure_shapes: true,
            horizon: false,
            config_fuzz: false,
            combined: false,
        },
    );
    assert_eq!(
        outcome.hits.get("finding-7-a-jump-is-not-n-single-steps"),
        None,
        "a multi-step undo_to/redo_to landed somewhere the same number of single steps does not: {}",
        summarize("bug-7", 6, &outcome)
    );
}

/// Which seeds are currently clean, and what the rest fail on.
///
/// The triage tool for choosing (and re-choosing) `GATE_SEEDS`: it catches each
/// run's panic instead of propagating it, so one bad seed does not hide the
/// twenty after it. `catch_unwind` is used with `AssertUnwindSafe` because a
/// panicking run's `AppState` is dropped immediately and never observed again.
///
/// The seeds run in PARALLEL (`run_seed_jobs`, `STALAGMIGHT_RUST_FUZZ_THREADS`)
/// and are merged in seed order, so the summary lines, the clean list and the
/// failure buckets are byte-identical to a one-thread run. The exception is
/// each line's `oracle-us` triple, which is wall time and inflates under
/// contention: measure that at `THREADS=1`.
#[test]
#[ignore = "survey: prints per-seed status, used to choose GATE_SEEDS"]
fn fuzz_seed_survey() {
    let _config = isolated_config();
    let seeds: u32 = std::env::var("STALAGMIGHT_RUST_FUZZ_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(40);
    let iterations: usize = std::env::var("STALAGMIGHT_RUST_FUZZ_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(GATE_ITERATIONS);
    let jumps = std::env::var("STALAGMIGHT_RUST_FUZZ_JUMPS").is_ok();
    let closures = std::env::var("STALAGMIGHT_RUST_FUZZ_CLOSURES").is_ok();
    let divergence = std::env::var("STALAGMIGHT_RUST_FUZZ_DIVERGENCE").is_ok();
    let closure_shapes = std::env::var("STALAGMIGHT_RUST_FUZZ_CLOSURE_SHAPES").is_ok();
    let horizon = std::env::var("STALAGMIGHT_RUST_FUZZ_HORIZON").is_ok();
    let config_fuzz = std::env::var("STALAGMIGHT_RUST_FUZZ_CONFIG").is_ok();
    // `COMBINED=1` is a shorthand rather than a seventh independent flag: it IS
    // the other modes at once, so it turns them on rather than sitting beside
    // them. Spelling them out as well is harmless (they are already true).
    let combined = std::env::var("STALAGMIGHT_RUST_FUZZ_COMBINED").is_ok();
    let crossings: usize = std::env::var("STALAGMIGHT_RUST_FUZZ_CROSSINGS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(if combined { COMBINED_CROSSINGS } else { 1 });
    let verbose = fuzz_verbose();
    // Seed-major, mode-minor: the same order the sweep used when it ran the
    // seeds one at a time, and the order every line below is merged in.
    let jobs: Vec<(u32, bool)> = (1..=seeds)
        .flat_map(|seed| [(seed, false), (seed, true)])
        .collect();
    let results = run_seed_jobs(&jobs, fuzz_threads(jobs.len()), |&(seed, persistence)| {
        run_fuzz(
            seed,
            Plan {
                iterations,
                persistence,
                crossings,
                prologue: None,
                jumps: jumps || combined,
                closures: closures || combined,
                divergence,
                closure_shapes: closure_shapes || combined,
                horizon: horizon || combined,
                config_fuzz,
                combined,
            },
        )
    });
    let mut clean = Vec::new();
    let mut failures: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for (&(seed, persistence), outcome) in jobs.iter().zip(results) {
        match outcome {
            Ok(result) => {
                if !persistence {
                    clean.push(seed);
                }
                println!(
                    "{}",
                    summarize(
                        if persistence { "persist" } else { "memory " },
                        seed,
                        &result
                    )
                );
            }
            Err(message) => {
                if verbose {
                    println!(
                        "--- seed {seed} ({}) panicked ---\n{message}",
                        if persistence { "persist" } else { "memory" }
                    );
                }
                failures
                    .entry(failure_signature(&message))
                    .or_default()
                    .push(seed);
                clean.retain(|&s| s != seed);
            }
        }
    }
    println!("clean seeds ({}): {clean:?}", clean.len());
    for (headline, seeds) in &failures {
        println!("FAILING {} seed(s) on: {headline}", seeds.len());
        println!("  seeds: {seeds:?}");
    }
}

/// CORPUS-GUIDED sweeping, and the measurement that says whether it is worth
/// anything.
///
/// # The claim
///
/// A random walk under-reaches topology space. That is not a hypothesis here -
/// findings 4 and 10 had to have their shapes HAND-BUILT because a 30-seed
/// sweep produced none, and the metamorphic sweep still skips a fifth of its
/// `move-out-and-back` draws on a guard the random project keeps tripping. So:
/// remember the shapes a sweep reaches, and start later runs from one.
///
/// # Why it costs nothing
///
/// Every run in this module is a pure function of its seed, so "the state seed
/// 7 was in after 40 moves" is fully described by the pair `(7, 40)`. A corpus
/// entry is two integers. No project is serialized, no file is written, and
/// nothing has to be kept compatible across a change to the generators - a
/// stale corpus simply replays to a different shape and is re-keyed on the
/// spot.
///
/// # The measurement, and its answer: NO
///
/// This runs the SAME budget twice, once with guidance and once without, and
/// reports distinct signatures reached by each. Measured at 80 runs x 60 moves:
///
/// ```text
///   UNGUIDED: 78 distinct signature(s), 8 classifier id(s)
///   GUIDED:   73 distinct signature(s), 8 classifier id(s), corpus grew to 73
///   reached ONLY with guidance: 34
///   reached ONLY without:       39
/// ```
///
/// **Guidance is slightly WORSE, and the diagnosis is in the numbers.** 78 of
/// 80 unguided runs land on a distinct signature, and the corpus admitted 73 of
/// 80 - so "novel" carries almost no information at this granularity. The
/// corpus admits nearly everything, and "guided" therefore just means "half the
/// budget spent replaying a prefix instead of drawing a fresh seed", which
/// costs diversity rather than buying it. The classifier id sets are identical,
/// so it reaches no finding the random walk does not.
///
/// The union of the two (34 + 39 signatures reached by only one) is much larger
/// than either, which says the two explore differently and neither dominates -
/// the random walk is simply not the bottleneck this was built to relieve.
///
/// What WOULD make a corpus pay, and is left for whoever picks this up: a
/// coarser signature (the current one splits on eight buckets across twelve
/// features, which is far finer than the generators can exploit), or admission
/// keyed on a RARE feature rather than on novelty - the shapes findings 4 and
/// 10 needed are rare, not merely new, and `c`/`y`/`v` in the signature are
/// exactly those bits. Guidance keyed on "has a closure" or "ids have diverged"
/// would compound the shapes that matter instead of every shape at all.
///
/// # The second answer, also NO
///
/// That rare-feature alternative was picked up and measured -
/// `fuzz_rarity_tests.rs`, six repetitions of this budget against three
/// admission predicates - and it fails too, in three ways this comment could not
/// have known:
///
/// * Keyed on a run's END state it is 11% FAITHFUL in the plain regime: the
///   prologue replays `(seed, n)` as n mutations, the admitted run walked a
///   different trajectory, and eight of nine admitted entries deliver a state
///   carrying none of the bits they were admitted for.
/// * Keyed on the prologue's OWN trajectory, which is exact by construction, it
///   reaches the rare classified findings 41% sooner counted in RUNS, 17% sooner
///   counted in MOVES, and not at all counted in WALL TIME. The lead was the
///   metric: a continuation run is not one run's worth of compute.
/// * The accounting above is part of the first answer too. This test gives a
///   guided run its prologue for FREE, so "the same budget" was 9665 moves and
///   31.6 s against 6780 and 17.6 s. Guidance lost here while being subsidised
///   by 80%.
///
/// Two negative results from different directions now say the same thing: the
/// random walk is not this tier's bottleneck, and a shape worth having is
/// cheaper to CONSTRUCT (`Plan::closure_shapes`, `Plan::divergence`) than to
/// rediscover.
///
/// The mechanism is kept because the measurement is the deliverable and because
/// `Plan::prologue` costs two integers; it is NOT wired into any gate, and this
/// comment is the reason.
///
/// ```text
/// STALAGMIGHT_RUST_FUZZ_CORPUS_BUDGET=120 STALAGMIGHT_RUST_FUZZ_ITERATIONS=60 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib fuzz_corpus_sweep -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measurement: compares signature coverage with corpus guidance on and off"]
fn fuzz_corpus_sweep() {
    let _config = isolated_config();
    let budget: usize = std::env::var("STALAGMIGHT_RUST_FUZZ_CORPUS_BUDGET")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(80);
    let iterations: usize = std::env::var("STALAGMIGHT_RUST_FUZZ_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(GATE_ITERATIONS);

    let base = |prologue: Option<(u32, usize)>| Plan {
        iterations,
        persistence: false,
        crossings: 1,
        prologue,
        jumps: false,
        closures: false,
        divergence: false,
        closure_shapes: false,
        horizon: false,
        config_fuzz: false,
        combined: false,
    };

    // ---- UNGUIDED: `budget` fresh seeds, nothing remembered. ----
    let mut unguided: BTreeSet<String> = BTreeSet::new();
    let mut unguided_hits: BTreeMap<&'static str, usize> = BTreeMap::new();
    for seed in 1..=budget as u32 {
        if let Ok(outcome) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_fuzz(seed, base(None))))
        {
            unguided.insert(outcome.signature);
            for (id, count) in outcome.hits {
                *unguided_hits.entry(id).or_insert(0) += count;
            }
        }
    }

    // ---- GUIDED: the same budget, but every other run starts from a shape a
    // previous run reached and only then draws its own stream. ----
    let mut guided: BTreeSet<String> = BTreeSet::new();
    let mut guided_hits: BTreeMap<&'static str, usize> = BTreeMap::new();
    // `(seed, moves)` pairs whose signature was novel when it was recorded.
    let mut corpus: Vec<(u32, usize)> = Vec::new();
    let mut picker = Rng::new(0x00C0_FFEE);
    for step in 0..budget {
        let seed = step as u32 + 1;
        // Alternate: a fresh seed keeps feeding the corpus new material, a
        // corpus entry compounds what is already there. All-corpus would
        // converge on one lineage and reach less than the random walk does.
        let prologue = if corpus.is_empty() || step % 2 == 0 {
            None
        } else {
            Some(corpus[picker.below(corpus.len())])
        };
        let Ok(outcome) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_fuzz(seed, base(prologue))
        })) else {
            continue;
        };
        let novel = guided.insert(outcome.signature.clone());
        for (id, count) in outcome.hits {
            *guided_hits.entry(id).or_insert(0) += count;
        }
        // A run that reached a shape nothing had reached becomes corpus
        // material - at a PREFIX of its length, so a later run inherits the
        // topology and still has most of its own budget to explore with.
        if novel {
            corpus.push((
                prologue.map_or(seed, |(prologue_seed, _)| prologue_seed),
                iterations / 2,
            ));
        }
    }

    println!("corpus sweep: budget {budget} run(s) x {iterations} moves, each way");
    println!(
        "  UNGUIDED: {} distinct signature(s), {} classifier id(s)",
        unguided.len(),
        unguided_hits.len()
    );
    println!(
        "  GUIDED:   {} distinct signature(s), {} classifier id(s), corpus grew to {}",
        guided.len(),
        guided_hits.len(),
        corpus.len()
    );
    let only_guided: Vec<&String> = guided.difference(&unguided).collect();
    let only_unguided: Vec<&String> = unguided.difference(&guided).collect();
    println!(
        "  reached ONLY with guidance ({}): {only_guided:?}",
        only_guided.len()
    );
    println!(
        "  reached ONLY without ({}): {only_unguided:?}",
        only_unguided.len()
    );
    println!(
        "  unguided classifier ids: {:?}",
        unguided_hits.keys().collect::<Vec<_>>()
    );
    println!(
        "  guided classifier ids:   {:?}",
        guided_hits.keys().collect::<Vec<_>>()
    );
}

/// THE HORIZON RUN: thousands of moves, past the app's own history limit.
///
/// Three things only a long run can reach, and all three are about time rather
/// than about any single move:
///
/// 1. **The FIFO trim.** `UndoStack::enforce_depth_limit` drops the oldest entry
///    from BOTH stacks once either passes `undo_history_limit`
///    (core/undo/budget.rs). Nothing shorter than ~100 pushes reaches it, so the
///    sliding window was completely unfuzzed. The model follows it exactly:
///    `absorb_trim` asserts the app trimmed neither too little (an unbounded
///    history) nor too much (a user's edits disappearing early), moves the
///    reachable `floor` up, and every walk from then on treats the floor as the
///    bottom - including the closing unwind, which still verifies every depth
///    that is still reachable.
/// 2. **Accumulation.** A difference under `TOLERANCE_ABS` is accepted every
///    time it is seen, so a depth visited a hundred times can be walked past the
///    tolerance one grain at a time. The run remembers what it tolerated per
///    depth per path, and a later failure whose every path has been inexact here
///    before AND has grown is classified as accumulation rather than reported as
///    a fresh regression - then re-baselined, so the next one is measured from
///    there.
/// 3. **Growth.** Id allocators may not be rewound by a forward command (that
///    hands the same id out twice) and the peak size of every
///    unbounded-in-principle collection is reported.
///
/// The seeds run in PARALLEL like the other two sweeps, which is what makes
/// "minutes per seed" affordable at all. The one thing to watch that the
/// shorter sweeps do not have: a horizon run holds a snapshot per reachable
/// depth for thousands of moves, so peak memory is per-worker rather than
/// per-run. `STALAGMIGHT_RUST_FUZZ_THREADS` is the dial if a long horizon
/// sweep starts swapping.
///
/// ```text
/// STALAGMIGHT_RUST_FUZZ_HORIZON=1 STALAGMIGHT_RUST_FUZZ_SEEDS=4 \
///   STALAGMIGHT_RUST_FUZZ_ITERATIONS=10000 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib undo_redo_fuzz_horizon -- --ignored --nocapture
/// ```
#[test]
#[ignore = "horizon: thousands of moves per seed - run deliberately with STALAGMIGHT_RUST_FUZZ_SEEDS/ITERATIONS"]
fn undo_redo_fuzz_horizon() {
    let _config = isolated_config();
    let seeds: u32 = std::env::var("STALAGMIGHT_RUST_FUZZ_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2);
    let iterations: usize = std::env::var("STALAGMIGHT_RUST_FUZZ_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(10_000);
    let persistence = std::env::var("STALAGMIGHT_RUST_FUZZ_PERSISTENCE").is_ok();
    let mut total: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut trimmed = 0;
    let mut failures = Vec::new();
    let jobs: Vec<u32> = (1..=seeds).collect();
    // Caught per seed, and RE-RAISED at the end. A long run is expensive enough
    // that stopping the whole sweep on the first failure wastes the other seeds'
    // evidence; hiding the failure would be worse, so every one is reported and
    // the test still fails.
    let results = run_seed_jobs(&jobs, fuzz_threads(jobs.len()), |&seed| {
        run_fuzz(
            seed,
            Plan {
                iterations,
                persistence,
                crossings: 1,
                prologue: None,
                jumps: true,
                closures: false,
                divergence: false,
                closure_shapes: false,
                horizon: true,
                config_fuzz: false,
                combined: false,
            },
        )
    });
    for (&seed, result) in jobs.iter().zip(results) {
        match result {
            Ok(outcome) => {
                println!("{}", summarize("horizon", seed, &outcome));
                trimmed += outcome.trimmed;
                for (id, count) in outcome.hits {
                    *total.entry(id).or_insert(0) += count;
                }
            }
            Err(message) => {
                println!("horizon seed={seed} FAILED:\n{message}");
                failures.push(seed);
            }
        }
    }
    // A horizon run that never trimmed proves nothing about the window, which is
    // the main thing it exists to exercise.
    assert!(
        trimmed > 0,
        "no run reached the history limit ({trimmed} entries trimmed over {seeds} seed(s) x \
         {iterations} moves); the sliding window went unfuzzed"
    );
    println!("horizon: {trimmed} entry(ies) trimmed; classified totals: {total:?}");
    assert!(
        failures.is_empty(),
        "horizon seed(s) {failures:?} failed - see the payloads above"
    );
}

/// The SOAK: the same engine, at a scale a CI gate cannot afford.
///
/// `#[ignore]`d, and driven by the environment so a sweep needs no recompile:
///
/// ```text
/// STALAGMIGHT_RUST_FUZZ_SEEDS=200 STALAGMIGHT_RUST_FUZZ_ITERATIONS=1000 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib undo_redo_fuzz_soak -- --ignored --nocapture
/// ```
///
/// Every seed runs BOTH modes. A soak failure is a finding: minimize it by
/// re-running the seed with a smaller iteration count (the run is deterministic,
/// so a prefix that still fails is a valid smaller repro), then pin it in
/// `open_undo_bugs_tests.rs`.
///
/// The seeds run in PARALLEL - `run_seed_jobs`, sized by
/// `STALAGMIGHT_RUST_FUZZ_THREADS` - and are merged in seed order, so the two
/// printed lines are byte-identical to a one-thread run and a soak is worth
/// running at a scale that used to mean leaving it overnight.
///
/// A panicking seed is caught, attributed and RE-RAISED at the end rather than
/// taking the sweep down with it, which is the horizon run's rule and for the
/// same reason: at soak scale the other seeds' evidence costs too much to
/// throw away over the first failure.
#[test]
#[ignore = "soak: minutes, not seconds - run deliberately with STALAGMIGHT_RUST_FUZZ_SEEDS/ITERATIONS"]
fn undo_redo_fuzz_soak() {
    let _config = isolated_config();
    let seeds: u32 = std::env::var("STALAGMIGHT_RUST_FUZZ_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(50);
    let iterations: usize = std::env::var("STALAGMIGHT_RUST_FUZZ_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1000);
    let mut total: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    // Seed-major, mode-minor: the order the soak ran in when it ran one seed at
    // a time, and the order the totals are merged in.
    let jobs: Vec<(u32, bool)> = (1..=seeds)
        .flat_map(|seed| [(seed, false), (seed, true)])
        .collect();
    let results = run_seed_jobs(&jobs, fuzz_threads(jobs.len()), |&(seed, persistence)| {
        run_fuzz(
            seed,
            Plan {
                iterations,
                persistence,
                // The soak crosses several times: heals stack on heals, and
                // a snapshot two epochs back is compared against a project
                // that has been through the loader twice.
                crossings: 3,
                prologue: None,
                // The soak DOES draw jumps and build closures: findings 7 and
                // 14 are real, and this is the tier that can afford to keep
                // hitting them.
                jumps: true,
                closures: true,
                divergence: persistence,
                closure_shapes: true,
                horizon: false,
                config_fuzz: false,
                combined: false,
            },
        )
    });
    for (&(seed, persistence), result) in jobs.iter().zip(results) {
        match result {
            Ok(outcome) => {
                for (id, count) in outcome.hits {
                    *total.entry(id).or_insert(0) += count;
                }
            }
            Err(message) => {
                println!(
                    "soak seed={seed} ({}) FAILED:\n{message}",
                    if persistence { "persist" } else { "memory" }
                );
                failures.push(seed);
            }
        }
    }
    println!("soak: {seeds} seed(s) x {iterations} iterations x 2 modes");
    println!("soak classified totals: {total:?}");
    assert!(
        failures.is_empty(),
        "soak seed(s) {failures:?} failed - see the payloads above"
    );
}

/// Is the adjusted solve a FUNCTION of the project, or does it wander?
///
/// The horizon run's adjusted-geometry oracle fired on a state whose stored
/// project roundtripped byte for byte, with a station 28 microns from where the
/// same depth had it - at kilometre-scale coordinates, 3e-8 relative. Either the
/// solve is not deterministic or something outside the compared state feeds it,
/// and the difference between those two answers decides whether the oracle's
/// tolerance is a measurement or an excuse.
///
/// This measures BOTH paths, and the split is the whole point. The first version
/// of it measured only the first and reported "zero spread", which was used to
/// argue the solve was deterministic and the input must have differed. That
/// conclusion did not follow: a project with no eligible loop closure never
/// enters `adjust_least_squares` at all (`plan_adjustment_anchors`,
/// models/project/coordinates.rs), so the measurement had never touched the code
/// that wanders. FINDING 21 is the second column.
#[test]
#[ignore = "measurement: prints the solve-to-solve spread, used to justify ADJUSTED_TOLERANCE_M"]
fn measure_adjusted_solve_determinism() {
    let _config = isolated_config();
    // PATH ONE: no closure, so the "solve" is a deterministic coordinate walk.
    // Closures are also excluded because the starting topology with one is
    // finding 14's and the app's own assert would stop this before it measured
    // anything.
    let mut fuzzer = Fuzzer::new(1, false);
    for _ in 0..40 {
        if fuzzer.catching_alignment(Fuzzer::step_mutate).is_none() {
            break;
        }
    }
    let first = fuzzer.adjusted_geometry();
    let mut worst: f64 = 0.0;
    let mut differing = 0;
    for _ in 0..8 {
        fuzzer.state.invalidate_adjusted_cache();
        let again = fuzzer.adjusted_geometry();
        let diffs = adjusted_diffs(&first, &again);
        if !diffs.is_empty() {
            println!("solve differs: {diffs:?}");
        }
        if again != first {
            differing += 1;
        }
        for (a, b) in first.lines().zip(again.lines()) {
            let (pa, pb): (Vec<&str>, Vec<&str>) = (a.split('|').collect(), b.split('|').collect());
            for axis in 1..4 {
                if let (Ok(x), Ok(y)) = (
                    pa.get(axis).unwrap_or(&"0").parse::<f64>(),
                    pb.get(axis).unwrap_or(&"0").parse::<f64>(),
                ) {
                    worst = worst.max((x - y).abs());
                }
            }
        }
    }
    println!("no closure: {differing} of 8 re-solves differ; worst difference {worst:e} m");

    // PATH TWO: a least-squares closure, which is what
    // `plan_adjustment_anchors` requires before it enters
    // `adjust_least_squares` at all. `Move` is not enough - it is applied
    // directly - so the strategy is named explicitly rather than drawn.
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "SolveDeterminism".to_string(),
                location: None,
            })
            .success
    );
    let mut rng = Rng::new(1);
    let section = |name: &str, count: usize, rng: &mut Rng| -> SectionId {
        let shots: Vec<ShotData> = (0..count).map(|_| random_shot(rng)).collect();
        state
            .execute(Command::CreateSection {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: COLORS[0].to_string(),
                direction: None,
                shots,
            })
            .section_id
            .expect("CreateSection returned no id")
    };
    let a = section("lsq-a", 3, &mut rng);
    let b = section("lsq-b", 2, &mut rng);
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(a, 0),
                coordinates: GpsCoordinates::new(20.4, -87.3, 0.0).unwrap(),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(a, 3),
                station_b: StationRef::new(b, 0),
            })
            .success
    );
    let closed = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 2),
        closure_strategy: ClosureStrategy::LeastSquaresWeighted,
    });
    assert!(closed.success, "CreateLoopClosure: {:?}", closed.error);

    let base = adjusted_geometry_of(&state);
    let mut lsq_worst: f64 = 0.0;
    let mut lsq_differing = 0;
    for _ in 0..64 {
        state.invalidate_adjusted_cache();
        let again = adjusted_geometry_of(&state);
        if again == base {
            continue;
        }
        lsq_differing += 1;
        for (left, right) in base.lines().zip(again.lines()) {
            let (pa, pb): (Vec<&str>, Vec<&str>) =
                (left.split('|').collect(), right.split('|').collect());
            for axis in 1..4 {
                if let (Ok(x), Ok(y)) = (
                    pa.get(axis).unwrap_or(&"0").parse::<f64>(),
                    pb.get(axis).unwrap_or(&"0").parse::<f64>(),
                ) {
                    lsq_worst = lsq_worst.max((x - y).abs());
                }
            }
        }
    }
    println!(
        "least-squares closure: {lsq_differing} of 64 re-solves differ; worst difference \
         {lsq_worst:e} m (FINDING 21)"
    );
}
