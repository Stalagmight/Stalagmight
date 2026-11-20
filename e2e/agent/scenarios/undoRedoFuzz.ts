/**
 * undo-redo-fuzz: a seeded random-action undo/redo fuzzer.
 *
 * Loads a real `.stalx` fixture, then walks a randomized stream of REAL
 * mutating commands (through the projectStore actions, so the frontend store
 * stays in step with the backend) interleaved with random undo/redo moves, and
 * asserts after every single move that the project state is EXACTLY the state
 * previously recorded at that undo-stack depth.
 *
 * ## The starting point
 *
 * `STALAGMIGHT_FUZZ_FIXTURE` picks any `.stalx` in `e2e/fixtures/` (basename
 * only), defaulting to `DEFAULT_FIXTURE`. Whatever project is open is closed
 * first: the model measures depth from a known floor, so the scenario always
 * opens its own copy.
 *
 * That copy has its persisted undo history DELETED before the load
 * (`loadFixtureWithoutUndoHistory` -> `stripUndoHistory`), because tracked
 * fixtures ship with one - 20 rows in the fractal fixture, 2 in naharon - and
 * fuzzing on top of somebody else's entries both burns headroom below the
 * 100-entry limit and puts un-modelled state under `snapshots[0]`. The scenario
 * then asserts both stacks are empty as a HARD expectation.
 *
 * ## The model
 *
 * `snapshots[d]` is the expected canonical `get_project_state` at undo depth
 * `d` (`snapshots[0]` = the state right after the fixture opened). `cursor` is
 * the current depth; the ground truth for it is the backend's own
 * `get_undo_stack` / `get_redo_stack` lengths, re-read after every move, so the
 * model can never quietly drift away from the app.
 *
 * The snapshot array lives IN THE WEBVIEW and each step is a single fused
 * `executeAsync` - call the action, observe, canonicalize, compare - so a
 * ~60 KB (fractal) / ~277 KB (naharon) payload no longer crosses the bridge
 * twice per iteration. See `undoRedoFuzzBridge.ts` for the contract and
 * `fuzzOracleCore.ts` for the serializer, which is one self-contained function
 * injected into the page and called directly by Node, never two copies. Node
 * keeps `cursor`/`top` mirrors, re-derives them from the same rules, and asserts
 * the page agrees. Full canonical strings are fetched only when a comparison
 * has ALREADY failed.
 *
 *   - MUTATE: draw an action, run it, then reconcile with what the stacks did.
 *     A rejected command must change neither the stacks nor the state (that
 *     assertion is what catches a command that mutates and then fails). A
 *     command that succeeds without pushing an entry (`ReverseLine` on a
 *     shot-less line, `UnmergeStations` on an unmerged station) must leave
 *     the state alone; it may still clear the redo stack, which truncates the
 *     model's forward branch.
 *   - UNDO / REDO: the state must equal `snapshots[cursor +/- 1]` exactly, and
 *     both stack lengths must match the model.
 *
 * At the end it unwinds all the way to depth 0 and replays back to the top,
 * verifying every intermediate snapshot in both directions - the "full
 * roundtrip at every depth" guarantee.
 *
 * ## What "exactly" means
 *
 * `e2e/agent/lib/fuzzOracleCore.ts` owns the canonical serializer and documents
 * every field it drops (all of them monotonic counters or `touch()`/`file_path`
 * artefacts). Map key order is canonicalized rather than dropped, and floats are
 * rounded to 12 significant digits. Shot uuids, `reverse_parity`, the gauge
 * anchors, `merged_at`, all closure metrics and all geometry ARE compared. When
 * two canonical strings still disagree, `projectSnapshot.ts` re-compares them
 * with a 1e-9 absolute tolerance and classifies the result.
 *
 * ## The app bugs this found - ALL FIXED, and this scenario is now STRICT
 *
 * Tracked at https://github.com/Stalagmight/Stalagmight/issues/413, one comment
 * per finding with mechanism and fix direction.
 *
 * Nothing here is open. The last one to close was finding 11's residue
 * (b9d6b14b), which this scenario found after the earlier waves and which is
 * written up under entry 11 - read it before concluding that a gauge-anchor
 * failure is new. No classifier was ever re-added for it: the oracle stayed
 * strict and the runs stayed red until the app was fixed, which is the whole
 * discipline this file is built on.
 *
 * EVERY classifier and every known-bug tolerance is GONE, per the
 * delete-when-fixed contract the fix waves follow: a mismatch of any shape, at
 * any depth, is an outright failure again, and `STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS=1`
 * has nothing left to allow (the variable is still read, and still named in the
 * `reproduce:` line, so a stale invocation is visible rather than silently
 * meaningless). The only differences this scenario still ATTRIBUTES are the two
 * documented, intended families a save or a reload produces
 * (`SAVE_HEALING_MUTATIONS`, `RELOAD_RESOLVED_GEOMETRY`), and only across an
 * epoch boundary.
 *
 * The table below is the RECORD, not a to-do list. Every repro line is kept
 * verbatim and stays runnable: each one is now a REGRESSION CHECK that must pass
 * clean. Do not shorten them - dropping the stream selector or the fixture makes
 * a line stop exercising what it was minimized to exercise. Each entry names the
 * commit that fixed it (`git show <hash>`), which is where the mechanism, the
 * fix and its Rust-tier regression test live.
 *
 * The template every line below plugs into:
 *
 *     STALAGMIGHT_FUZZ_FIXTURE=<fractal_plant_iter3|naharon>.stalx \
 *     pnpm agent:scenario undo-redo-fuzz --seed N --iterations M --release
 *
 * 4, 5 and 6 need `STALAGMIGHT_FUZZ_FIXTURE=naharon.stalx`: they need loop
 * closures, zero datums and null line metadata present AT LOAD, and the
 * fractal fixture has none of those (all 32 of its lines have a non-null
 * surveyor; naharon has 13 null ones). Fixture diversity is what found them.
 *
 * 1. FIXED (94896857). `--seed 1 --iterations 40` on fractal - this
 *    is what the `agent:scenarios` smoke entry hits. Create a line, undo,
 *    redo: every shot uuid was re-minted. SECOND MANIFESTATION, only
 *    observable once runs save (`naharon`, seed 23 x60): the same
 *    `restore_line` path also reset `reverse_parity` and BOTH gauge anchors
 *    (`Line::new`), so a redo silently discarded the least-squares gauge pair
 *    a save had captured.
 *    `UndoData::CreatedLineSnapshot` stored a `LineData`, which has no
 *    uuid field, so redo went `restore_line` -> `Line::new_with_id` ->
 *    `Shot::new` -> `Uuid::new_v4()`. Fixed by giving the entry a
 *    `LineIdentity` (shot uuids, parity, both anchors) that
 *    `Line::restore_with_identity` puts back, with the undo refreshing that
 *    identity from the live line so save-captured anchors survive; legacy
 *    entries still mint.
 * 2. FIXED (94896857 exact-zero half, a67dd1b5 float-dust half, f16c96a8 the
 *    whole state class).
 *    `STALAGMIGHT_FUZZ_STREAM=v1 ... --seed 11 --iterations 150` on fractal
 *    (not re-found under v2) - undoing a shot INSERT left
 *    behind an all-zero `line_offsets` entry the line did not have.
 *    `adjust_line_offset` created the key while compensating; the undo put
 *    the value back to 0 but nothing removed the key. The end state is that a
 *    dust-zero entry is UNREPRESENTABLE: `write_line_offset` is the single
 *    normalizing writer (a value within `Project::OFFSET_DUST_M` of the origin is
 *    removed rather than written), and two boundaries enforce it on maps this app
 *    did not write - the loader after its zero-trust re-solve, and snapshot
 *    restore for legacy undo payloads. No write-local rule could satisfy both
 *    identities at once (+d/-d from ABSENT must end absent, from PRESENT-ZERO
 *    must end present-zero), which is why the state itself was removed from the
 *    model rather than the writes being tuned again - see finding 27 in
 *    `undoPersistenceFuzz.ts` for the other end of the same argument.
 * 3. FIXED (72595cc6). `STALAGMIGHT_FUZZ_STREAM=v1 ... --seed 2 --iterations 36`
 *    on fractal (not re-found under v2; failed at #36, depth 11) - appending the
 *    FIRST shot into an EMPTY line and undoing displaced the whole line by
 *    that shot's horizontal vector (1.13 m in the repro), and the displacement
 *    RATCHETED on every further undo/redo cycle. `Project::add_shot` (the append
 *    path) is offset-neutral, but the undo routed through
 *    `remove_shot_with_rewire`, whose `pos == 0` branch unconditionally called
 *    `adjust_line_offset`. The empty-line append now has its exact inverse
 *    (`remove_appended_first_shot`): offsets untouched, station-0 refs kept.
 *    (Finding 15 was this bug re-measured, and is fixed with it.)
 * 4. FIXED (94896857). `STALAGMIGHT_FUZZ_STREAM=v1 ... --seed 4 --iterations 19`
 *    on naharon (not re-found under v2) - undoing a loop-closure removal
 *    put the closure back at the END of `project.loop_closures` instead of its
 *    original index (`undo.rs` pushed; the removal used `Vec::remove(pos)`).
 *    Order-only, but the order is persisted, indexed by the frontend, and feeds
 *    `resolve_offsets`' adjacency lists. `ClosureInfo` (and `ZeroDatumInfo`, the
 *    same shape, finding 17) now capture their index and reinsert at it, with a
 *    push fallback for legacy entries.
 * 5. FIXED (b9c9d649). `--seed 16 --iterations 40` on naharon - undoing
 *    `updateLine` COULD NOT restore a `name` / `surveyor` / `date` that was
 *    previously null; the new value silently survived the undo. The capture was
 *    right (`undo_capture.rs` builds `Some(None)`), but `LineUpdates` had only
 *    `#[serde(default)]` on those three fields, so `Some(None)` and `None` BOTH
 *    serialized to `null` in `values_json` and both read back as `None` = "leave
 *    it alone". They now carry `deserialize_with = "double_option"` +
 *    `skip_serializing_if`, exactly as `ShotUpdates` always did; schema v17 drops
 *    pre-v17 undo history because the meaning of an explicit null changed.
 * 6. FIXED (72595cc6). `--seed 1 --iterations 40` on naharon - undo FAILED
 *    OUTRIGHT: "This shot is part of a loop ... Both sides are anchored to a zero
 *    datum. Use split instead." Undo of `CreateShot` ran the user-facing
 *    `delete_shot`, guards and all, so an insert into an anchored loop was an
 *    action that COULD NOT be undone - and `undo_inner` pushed the entry back, so
 *    it failed the same way forever and everything beneath it was unreachable.
 *    Since jumps were added this seed reached it through `undoTo(2)` and failed
 *    at "step 2 of 3" - the multi-step path applied the first undo, then wedged
 *    PARTWAY. Fixed by routing the undo through the guard-free
 *    `AppState::remove_created_shot`: an inverse consults no user-intent guard,
 *    while `Command::DeleteShot` keeps every guard (pinned by its own test).
 * 7. FIXED (94896857). `--seed 12 --iterations 40` on fractal - a jump through
 *    the history dropdown's path landed on a DIFFERENT state than the same depth
 *    reached by single undos: `line_offsets` out by 2.4 m, and the stations
 *    with it.
 *    `--seed 15 --iterations 40` on fractal is the same bug reached through the
 *    INTEGRITY oracle instead - the state compared equal (the geometry paths
 *    were absorbed as reload-resolved across a save boundary) while
 *    `check_project_integrity` reported a merge 5.00 m apart. Both lines are kept
 *    because they exercise two different oracles.
 *
 *    `undo()`/`redo()` call `invalidate_adjusted_cache()` before and
 *    `reconcile_geometry()` after. `undo_to()`/`redo_to()` looped
 *    `undo_inner`/`redo_inner` directly and called NEITHER, so `realign_merges()`
 *    never ran for a jump and the closure-adjusted cache was left stale on top.
 *    They now run `mark_remote_dirty` + `reconcile_geometry` ONCE after the loop,
 *    and a synthetic jump-equivalence contract test pins N single steps == one
 *    N-wide jump. (The missing `invalidate_adjusted_cache()` was measured NOT to
 *    be observable - the adjusted cache is keyed on `modified_at`, which every
 *    undo bumps via `touch()` - so only the missing `reconcile_geometry()` was
 *    ever the defect.)
 *
 * 8. FIXED (b9c9d649). DATA CORRUPTION. `--seed 14 --iterations 6` on fractal -
 *    after a save/reload, an `undo` DELETED THE WRONG SHOT. Six iterations: a
 *    `createShot(before=)` into a one-shot line makes the ids diverge from
 *    positional order, the save/reload renumbers, and the undo hit the wrong
 *    slot. The shot the user still had was gone and one they deleted was back -
 *    silent, and it persisted on the next save.
 *
 *    The chain: shots are RENUMBERED on load to
 *    `line_id * FEATURE_ID_STRIDE + sequence_order` (read.rs); undo entries
 *    are persisted and reloaded as OPAQUE JSON (`load_undo_stack`) with NO
 *    remapping anywhere on the restore path; and
 *    `UndoData::CreatedShotSnapshot { line_id, shot_id }` undid by calling
 *    `delete_shot(line_id, shot_id)`. Because the in-session allocator
 *    (`take_shot_id`) hands out ids from the SAME `line_id * STRIDE + n` space
 *    the loader renumbers into, a stale id did not miss - it COLLIDED with
 *    whatever shot now occupied that slot. A line only diverges from positional
 *    order after a mid-line insert or delete, which is why this needed a fuzzer
 *    to reach.
 *
 *    Fixed by addressing shots by `Shot::uuid` in the persisted entries (additive
 *    serde fields, in-session id kept only as the legacy fallback), plus a
 *    load-time applicability pass that DROPS an entry whose target no longer
 *    resolves rather than wedging the history, and a redo that refuses to
 *    duplicate a shot uuid. Findings 12, 13 and 16 are the same root cause and
 *    the same fix; see `undoPersistenceFuzz.ts`.
 *
 *    Note what did NOT fire while this was open, and correctly: the absolute
 *    invariant suite. The resulting state was perfectly well-formed - right shot
 *    count, unique ids, no dangling refs. Only the ROUNDTRIP oracle could see
 *    that it was the wrong state, which is exactly what that oracle is for.
 *
 * `STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS=1` is now INERT. It used to let a run
 * continue past a mismatch that was entirely a classified asymmetry (1, 2 and 4),
 * logging it, counting the `knownAsymmetry` metric and resyncing the model. There
 * are no classifiers left, so it tolerates nothing; the plumbing stays because
 * the day a new open bug needs the same treatment, the escape hatch and its
 * "deliberately not the default" rule should be re-derived from the same place
 * rather than reinvented.
 *
 * ## The persistence boundary
 *
 * Undo history is PERSISTED - both stacks serialize into the .stalx and restore
 * on open - so nothing below a save point is exercised until a run crosses it.
 * Runs now save to a temp path, close and reopen, at least once and up to three
 * times, drawn from a THIRD independent stream (`seed ^ 0x85ebca6b`) and taken
 * IN ADDITION to each iteration's normal move so the primary numbers do not
 * shift. Skipped while the history is deeper than `undo_persist_limit`, since
 * only the newest entries survive the write.
 *
 * Two documented families of undo-less state change live here, and BOTH are
 * subtracted only from comparisons that cross an epoch boundary (a save or a
 * reload bumps the epoch); within an epoch the oracle stays strict, which is what
 * kept bugs 3 and 7 visible and is now the whole of what this scenario tolerates:
 *
 *   - `SAVE_HEALING_MUTATIONS` - `save_project` mints missing shot uuids, dedupes
 *     within-line duplicates, captures missing gauge anchors, drops orphan
 *     `line_offsets` and sanitizes non-finite values, all with no undo entry.
 *   - `RELOAD_RESOLVED_GEOMETRY` - `redrive_offsets_after_load` ends in an
 *     unconditional `resolve_offsets()` ("Zero-trust: never honor a persisted
 *     derivable offset", offsets.rs:186-188), so a reload RECOMPUTES
 *     `line_offsets` and the station coordinates derived from them rather than
 *     restoring them.
 *
 * That subtraction stays honest by two guards `checkSnapshot` keeps: the slow
 * re-compare collects at most `DIFF_LIMIT` paths, and a CAPPED list can only
 * support a negative conclusion - so "everything was attributable" is refused
 * outright on a truncated diff, and a truncated failure says so in the message
 * rather than reading as a complete one. The MAGNITUDE of what the attribution
 * did absorb is printed with it, because both families are bounded by shape and
 * not by size.
 *
 * `Shot::id` is not persisted either - the loader renumbers to
 * `line_id * FEATURE_ID_STRIDE + sequence_order` (read.rs:372) - so from the
 * first reload the model compares id-agnostically (shot ids replaced by position,
 * everything else still exact). Line ids ARE persisted and are compared.
 *
 * ## Two oracles, not one
 *
 * 1. ROUNDTRIP - the snapshot-per-depth comparison above, including the
 *    single-step/jump equivalence.
 * 2. ABSOLUTE INVARIANTS (`checkProjectInvariants`, run in-page on every
 *    observed state). The roundtrip only sees drift BETWEEN states: a command
 *    that corrupts the project and an undo that faithfully restores the
 *    corruption match at every depth. Only an absolute check fires on that.
 *
 * A third, the INTEGRITY DELTA (`check_project_integrity`'s verdict recorded
 * per depth and restored by every history move), is retired with that command.
 * On the station graph the app runs `Project::validate` itself after every
 * edit, import and file-decoded history step and rolls back (with an error
 * log, which the sweep reports) whatever would break it, so the verdict was
 * empty at every depth; and the save and reload are strict now
 * (`SAVE_HEALING_MUTATIONS` / `RELOAD_RESOLVED_GEOMETRY` are empty), so the
 * roundtrip sees what the delta once caught behind a re-baseline (bug 7's
 * second line, below). Repair, which the delta measured, went with it.
 *
 * ## Generator streams: v1 repro lines, v2 default
 *
 * The action space above is the v2 stream. It necessarily DRAWS DIFFERENTLY from
 * the v1 stream every repro line in this header was minimized against, so those
 * lines do not replay verbatim under the default.
 *
 * `STALAGMIGHT_FUZZ_STREAM=v1` restores the original generator set, weights and
 * value ranges exactly - the v2-only helpers short-circuit without consuming a
 * draw - and reproduces the legacy stream byte for byte (verified: fractal seed 4
 * x60 gives the same 32 mutations, 37 undos, 20 redos, 2 jumps, depth 15 and
 * 60671-byte snapshot it always has). EVERY repro line numbered 1-8 above runs
 * verbatim with that variable set.
 *
 * V2 REGRESSION TABLE, mined from a 60-cell sweep (both fixtures, seeds 1-30,
 * x40, release) back when it yielded 18 failures. Every line runs STRICT - there
 * is no tolerance flag to pass - and every one is expected CLEAN, verified on
 * b9d6b14b + f16c96a8:
 *
 *   bug 1  fractal seed 1  x40       (the smoke entry; the shrink of this very
 *                                    seed is what identified finding 11's
 *                                    residue after bug 1 itself was fixed)
 *   bug 2  -- v1 escape --           not re-found in the sweep
 *   bug 3  -- v1 escape --           not re-found in the sweep
 *   bug 4  -- v1 escape --           not re-found in the sweep
 *   bug 5  naharon seed 16 x40       lines[20].surveyor null -> kept
 *   bug 6  naharon seed 1  x40       "undo at depth 2 failed outright"
 *          naharon seed 21 x60       the same wedge reached through undoTo; its
 *                                    6-move sequence later became the clearest
 *                                    statement of finding 11's residue (entry 11)
 *   bug 7  fractal seed 12 x40       undoTo(0) 2.4 m line_offsets divergence
 *          fractal seed 15 x40       same bug via the INTEGRITY oracle instead:
 *                                    undoTo skipped reconcile_geometry, so
 *                                    realign_merges never ran and the reloaded
 *                                    project reported "Merged stations 6:0 and
 *                                    3:2 are 5.00 m apart". The state compare
 *                                    had absorbed the geometry paths as
 *                                    reload-resolved across the epoch, so ONLY
 *                                    the integrity delta caught it - which is
 *                                    the argument for having three oracles.
 *   bug 8  fractal seed 14 x6        createShot(before=) into a 1-shot line,
 *                                    save/reload, undo -> the WRONG shot was
 *                                    deleted. Six iterations, the cleanest
 *                                    repro of the worst bug.
 *   bug 1  naharon seed 23 x60       bug 1's gauge-anchor half, needing a save
 *   fnd 10 naharon seed 26 x40       closure reclassified as a merge on reload
 *   fnd 11 fractal seed 4  x60       a pre-save snapshot pair reverting a heal.
 *                                    This seed stayed clean while the residue was
 *                                    open, because its snapshot pairs re-create
 *                                    nothing and the carry-forward covered them -
 *                                    which is exactly where the boundary between
 *                                    the two halves of finding 11 ran.
 *
 * Fresh-seed spread, x40, both fixtures, strict: fractal 31, 47, 63 and naharon
 * 31, 47, 63 all clean. (47 and 63 on fractal and 31 and 63 on naharon were red
 * on finding 11's residue before b9d6b14b - they are kept in the list as its
 * extra regression cover.) Nothing else was found, on any seed, in either stream.
 *
 * Bugs 2, 3 and 4 were not re-found across those 60 cells. Their v1 lines above
 * remain exact and executable with `STALAGMIGHT_FUZZ_STREAM=v1`, which
 * reproduces the legacy stream byte for byte - so no repro was lost, and the
 * fallback was a stream selector rather than a weakened assertion.
 *
 * ## v3: the import group, opt-in
 *
 * `STALAGMIGHT_FUZZ_STREAM=v3` is v2 PLUS the fifteen generators in
 * `undoRedoFuzzImportActions.ts`: synthetic `ImportLineConfig` batches across
 * all six placement variants, the null-in-a-required-f64 family, the DMP
 * wizard's `existing_shot_updates`, the four file importers on files built by
 * `surveyFileSpecs.ts`, and the cheap read-only probes. About 22% of a balanced
 * run's draws; see that file's header for what each one is for.
 *
 * v1 AND v2 DRAW IDENTICALLY, which is the property the table above depends on.
 * The group is filtered out before the weighted name array is built and appended
 * at its END when it is not, so every index `ctx.pick` reads still holds what it
 * held; the `import-heavy` swarm profile is likewise appended only under the v3
 * gate, because merging it into `SWARM_PROFILES` would have changed the divisor
 * every v2 seed picks its profile with. Both are pinned by
 * `e2e/agent/lib/undoRedoFuzzImportActions.test.ts`, and both were re-measured
 * against the pre-v3 tree: v1 fractal seed 4 x60 and v2 fractal seeds 1 x40 /
 * 4 x60 give metric-for-metric identical runs before and after.
 *
 * A v3 run legitimately produces `log-error` findings from the oracle sweep - a
 * malformed file that the parser refuses, or the deliberate missing-file probe,
 * is the app reporting a problem properly. That is a finding at severity
 * `error`, which does not fail a run; only an unexplained one is worth reading.
 *
 * A FOURTH oracle applies to v3 payloads only. `mustBeRejected` names the two
 * shapes that cannot be deserialized at all - a required `f64` arriving `null`,
 * and `date: ""` (issue #478, here on the route that BYPASSES the wizard's
 * normalisation) - and a run FAILS if the app accepts one. It is a pure function
 * of the recorded args rather than a flag on the drawn action, which is what
 * lets a replayed trace assert exactly what the recording did.
 *
 * ## A v3 OBSERVATION that is not a finding
 *
 * `execute_import_lines` refuses a non-finite value in an
 * `existing_shot_updates` patch by name, before any mutation, and has a Rust
 * test for it (`test_execute_import_lines_rejects_non_finite_shot_update`,
 * "attacker-controllable input ... must hold the same all_finite invariant as
 * update_shot"). NOTHING THAT CROSSED JSON CAN REACH IT. Every `ShotUpdates`
 * field is `Option<f64>` (or the double-option `Option<Option<f64>>`) with
 * `#[serde(default)]` (models/shot.rs), `JSON.stringify` spells `NaN` and
 * `Infinity` as `null` on the real `invoke` exactly as on this bridge, and an
 * explicit `null` on a plain `Option<f64>` deserializes to `None` - the
 * double-option's "leave this field alone". So the import SUCCEEDS, having
 * quietly dropped that one field of that one patch.
 *
 * Measured, not reasoned: naharon seed 59 x60 under v3 is what found it, by
 * failing an oracle that had claimed the refusal. It is not reported as an app
 * defect because the wizard's own guard covers the shots it interpolates FROM,
 * so no shipped route produces the value; what it means is that the backend
 * check is a Rust-API defence and cannot be cited as protecting the frontend.
 * If a caller ever needs a non-finite patch field rejected rather than dropped,
 * `ShotUpdates` has to distinguish "absent" from "explicitly null" for the
 * non-nullable numbers, exactly as finding 5 made `LineUpdates` do.
 *
 * V3 SWEEP, at the wave that introduced it. 8 cells (both fixtures, seeds 1-4,
 * x40) plus 14 more on the seven seeds whose swarm profile is `import-heavy`
 * (14, 15, 21, 23, 26, 56, 59, both fixtures, x60), plus fractal seeds 1 x80 and
 * 5 x80 with `STALAGMIGHT_FUZZ_IMPORT_MALFORMED_P=0.8`. All green. The one RED
 * the sweep produced was the oracle's, not the app's - naharon seed 59 x60,
 * written up in the observation above - and it is why `mustBeRejected` is now
 * two claims rather than three. No app defect was found; the group is coverage
 * that did not exist rather than a bug hunt that came back empty, and the seeds
 * above are its regression cover.
 *
 * ## v3 file inputs: what a trace carries, and what it rebuilds
 *
 * A file-backed import cannot record its path: the recording machine's absolute
 * path means nothing to a replayer, and a trace is meant to be attachable to a
 * bug report. So a file-backed move records TWO steps - a `genFile` carrying the
 * generator SPEC and a session-relative `rel`, then the `mutate` whose `args[0]`
 * is that same `rel` - and the replayer rebuilds the file from the spec into its
 * own directory before resolving `args[0]` against it. `surveyFileWriters.ts` is
 * a pure function of the spec, so the rebuilt bytes are identical; the trace
 * carries no fixture (which the repo forbids) and no absolute path.
 *
 * `FILE_ARG_ACTIONS` (`lib/fuzzTrace.ts`) is what says "argument zero is a
 * path", and `readTrace` refuses a trace that points one of those actions
 * outside the session directory - the same rule every other v2 `rel` has, and
 * for the same reason: a trace is untrusted input, and these arguments reach the
 * filesystem instead of the model.
 *
 * The generated files live under the run's own temp directory, removed in the
 * same `finally` as the save/reload copies. Nothing is lost by deleting them -
 * the trace regenerates them - and `pnpm agent:fuzz-shrink` replays this
 * scenario hundreds of times per shrink, so leaving them behind is not an option.
 *
 * Verified end to end: v3 fractal seed 1 x80 recorded with
 * `STALAGMIGHT_FUZZ_TRACE`, then replayed with `--trace`, reproduces the run
 * metric for metric (34 mutations, 31 undos, 19 redos, 18 rejected, 2 no-ops,
 * depth 15, 66037-byte snapshot) with all five `genFile` steps re-materialized.
 *
 * ## What v3 does NOT reach, and why
 *
 * `suggest_orphan_placements` and `suggest_all_import_placements` are not
 * `projectStore` actions - `App.tsx`, the three import dialogs and the DMP
 * wizard invoke them directly - and this scenario dispatches store actions by
 * name. Reaching them would take either an IPC-by-name path in the bridge, which
 * gives a recorded trace a much larger surface for two read-only probes, or a
 * store action with no product caller, which is dead product code kept alive by
 * a test. `import-dialog-fuzz` drives the dialogs that call them, and the
 * counters that say whether it reaches them for real - rather than only on their
 * empty return - are `screen:wiz-suggest` and `screen:orphan-placement`. See the
 * fuller note in `undoRedoFuzzImportActions.ts`, which records what the corpus
 * campaign actually measured.
 *
 * `suggest_import_placement` is a store action and IS reached, including its
 * non-empty return: `createMotifLine` plants a run and `probeSuggestMotif`
 * asks for a placement using a jittered copy of it. Both are appended at the END
 * of the v3 group, so v1 and v2 arrays are unchanged; the metric-for-metric v3
 * replay record above predates them.
 *
 * There is no Rust-tier analogue of this group in
 * `src-tauri/src/core/state/fuzz_undo_tests.rs`, and adding one is a design
 * problem rather than a transcription. `Refs::learn` pairs what a recorded move
 * created against what the replayed move created POSITIONALLY, with `zip` and no
 * length check - which is sound only because every existing `Op` creates a fixed
 * number of entities (at most one line, one shot, one closure; `BatchOp` has
 * no creating variant at all). An import creates an arbitrary number in one
 * move, and `execute_import_lines` drops lines whose placement does not
 * resolve - so a replay that places one fewer would pair a PREFIX, silently, and
 * the shrinker would then minimize toward the mis-pairing instead of toward the
 * bug. Whoever adds it has to give `learn` a checked, arity-aware pairing first.
 *
 * ALL OF THEM ALSO REPRODUCE IN THE RUST TIER, classified by finding id rather
 * than merely hit, and about a thousand times cheaper per sequence - which is
 * where they are now pinned as green regression tests. See the BUG
 * IDENTIFICATION MATRIX in the header of
 * `src-tauri/src/core/state/fuzz_undo_tests.rs` for the mode, seed and move
 * count per finding. Two things there matter to this file:
 *
 *   - BUG 7 IS PINNED. It had no unit test anywhere ("left uncovered
 *     deliberately rather than covered by a test that passes"), because every
 *     synthetic reduction was measured not to diverge. The Rust fuzzer PROVES
 *     the inequality instead of recognising a shape: on a jump that diverges it
 *     replays the same distance in single steps and records the finding only
 *     when the single-step path lands where the model says and the jump did not.
 *     `bug_7_a_jump_is_n_single_steps` is un-ignored and green since 94896857.
 *   - FINDING 15 WAS FINDING 3. Its displacement was never put back, so the
 *     leftover was still there several undos later when the consumed entry was
 *     unrelated - which is what 15 described. Fixed with 3 (72595cc6).
 *
 * 9. RESOLVED - not an app bug. The micron-scale drift seen at fractal seed 12
 *    was the fuzzer's OWN 1e6-metre boundary offsets: absolute error tracked the
 *    injected magnitude (1e6 -> 4.56e-6 m, 1e5 -> 1.59e-7, 1e4 -> 1.7e-9,
 *    1e3 -> below tolerance, all at the same step), the signature of float
 *    cancellation, and the shot fields roundtripped exactly. Fixed by bounding
 *    the INPUT (`GEOMETRY_EXTREME`, undoRedoFuzzActions.ts) rather than by
 *    loosening the oracle, which would have blinded it to real sub-centimetre
 *    drift.
 * 11. FIXED (94896857, completed by b9d6b14b).
 *    A save HEALS live state with no undo entry
 *    (`ensure_shot_uuids`, `ensure_unique_shot_uuids`,
 *    `capture_missing_gauge_anchors`, the orphan `line_offsets` retain -
 *    core/state/project_io.rs). A `ProjectSnapshotPair` captured BEFORE that
 *    save held the pre-heal project in BOTH halves and neither was refreshed,
 *    so undoing or redoing an UNRELATED command recorded before the save
 *    silently reverted the heal. The heals are now factored into
 *    `apply_save_heals`, which every snapshot restore re-applies (the fill-in
 *    half of it), so a restored project meets a saved one's standard.
 *
 *    The real damage is not the lost anchors (the next save re-captures them):
 *    it is that the project at a given undo DEPTH now depends on the ROUTE
 *    taken to reach it. Reach depth N by saving there and it has the anchors;
 *    reach the same depth N by undoing a pre-save entry and it does not. That
 *    is the fuzzer's roundtrip oracle doing exactly its job - a depth whose
 *    state is not well-defined breaks the promise the history feature makes.
 *
 *    Fuzzer: `--seed 4 --iterations 60` on fractal (a save at depth 1
 *    followed by a redo/undo of a `moveStation` recorded before it).
 *    Unit test: `undo_of_a_pre_save_entry_keeps_the_saves_gauge_anchors` -
 *    four commands, fully synthetic, un-ignored and green.
 *
 *    NOT an oracle gap, then or now. The model's save attribution deliberately
 *    allows null -> value ACROSS a save epoch (the heal itself); this was
 *    value -> null WITHIN the post-save epoch, which is the app losing state
 *    it had, and it was right to fail.
 *
 *    THE RESIDUE, found by this scenario after wave 2 and closed by b9d6b14b.
 *    94896857 re-applied the heals to a restored project as a CARRY-FORWARD:
 *    `reapply_heals_after_restore` (core/state/project_io.rs) copies an anchor
 *    onto a restored line only from a LIVE line with the same id that
 *    already has one. That could not cover a snapshot that RE-CREATES a line
 *    the live project does not currently have - nothing to carry forward from, so
 *    the anchors the save captured were lost. (The `CreateLine` path never had
 *    the problem: its entry carries a `LineIdentity` refreshed from the live
 *    line at undo time, finding 1. It was specifically the snapshot-pair path.)
 *    Fixed by making snapshot pairs SELF-REFRESHING WALLS: an undo re-records the
 *    entry's `after` half from the live project before restoring `before`, and a
 *    redo does the mirror, so pair undo/redo are exact inverses by construction
 *    rather than by re-healing afterwards.
 *
 *    Kept as the regression check, ddmin'd from fractal seed 1 x40 by
 *    `pnpm agent:fuzz-shrink` (43 steps -> 2, 1-minimal, no classifier involved):
 *
 *        1. executeBatch(revSec+crtSec+updShot)   <- pushes a ProjectSnapshotPair
 *                                                    whose `after` holds a NEW line
 *        2. save                                  <- captures that line's gauge pair
 *        (then the replayer's own unwind/rewind: undo to 0, redo back)
 *
 *        redo did not restore the state recorded at depth 2
 *          at project.lines[32].gauge_neighbour_uuid
 *            expected: "3178f6da-..."   actual: null
 *
 *    Six-move variant on naharon (seed 21 x60), a different command reaching the
 *    same place - `deleteShotRangeSplit` also produces a snapshot pair that
 *    re-creates a line: reverseLine, deleteShotRangeSplit, save+reload,
 *    updateShot, undo, undo, redo -> `lines[37].gauge_shot_uuid` null.
 *
 *    It was RED at fractal seeds 1, 12, 15, 47, 63 (x40) and naharon seeds 1, 21,
 *    31, 63 - roughly half of all seeds, because "a snapshot pair, then a save"
 *    is a common shape. All nine re-run CLEAN on b9d6b14b + f16c96a8.
 * 10. FIXED (b9c9d649). A loop closure BECAME a station merge across
 *    save/reload: naharon seed 26 x40 gave `loop_closures.length` 5 -> 4 with
 *    `station_merges.length` 34 -> 35. `redrive_offsets_after_load` calls
 *    `convert_orphan_closures_to_merges` unconditionally
 *    (models/project/offsets.rs), which normalizes a closure whose endpoints are
 *    no longer independently connected into the merge it really is. That is
 *    INTENDED - the app's stated invariant is that a closure exists only for a
 *    true loop - but it meant save+reopen was not state-preserving, because the
 *    same conversion did not run after every command that can orphan a closure.
 *    The resolution was to normalize EAGERLY rather than tolerate:
 *    `execute_delete_last_shot_and_line` converts orphan closures and
 *    reconciles, and the range-shift path already converted per shot. naharon
 *    seed 14 x40 showed the same count drop on a plain UNDO and is covered by
 *    the same fix.
 *
 * ## Trace record / replay
 *
 * Every run records its RESOLVED move list - the generator draws already
 * collapsed into concrete `projectStore` actions and args - and writes it to the
 * session artifacts dir on FAILURE, naming the path in the message.
 * `STALAGMIGHT_FUZZ_TRACE=path` also writes one on success.
 *
 * `pnpm agent:scenario undo-redo-fuzz --trace <file>` replays it. Replay is a
 * MODE of this scenario, not a separate one: only the source of the decisions
 * changes, so the oracles, the model and the failure payload are the same code.
 * `pnpm agent:fuzz-shrink <trace>` then ddmins it down to a 1-minimal move list.
 * See `e2e/agent/lib/fuzzTrace.ts` and docs/agent-harness.md.
 *
 * Recording is OBSERVE-ONLY - it draws nothing, reads no app state and runs after
 * the decision it records - so every repro seed above is unaffected by it. Both
 * faithfulness lines (bug 1 at seed 1 x40 strict, bug 8 at seed 14 x6 tolerant)
 * are re-verified with recording active.
 *
 * ## Determinism
 *
 * Every draw comes from `ctx.rng()`/`randInt`/`pick`, and every target id is
 * read back out of the live project and sorted before it is picked, so
 * `run_scenario("undo-redo-fuzz", seed, iterations)` replays byte-for-byte.
 *
 * The generators read a `TargetingDigest` rather than the raw project state.
 * The digest carries exactly the fields they read, each list in payload order,
 * so every sort and every pick sees the same values in the same sequence - the
 * draw stream, and every repro seed above, is unchanged by the move in-page.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { mulberry32, type Scenario, type ScenarioContext } from './framework';
import { DEFAULT_FIXTURE, disableAutoSave, loadFixtureWithoutUndoHistory } from './common';
import {
  LOOP_CLOSURES_PATH,
  RELOAD_RESOLVED_GEOMETRY,
  SAVE_HEALING_MUTATIONS,
  describeShotSetDelta,
  shotSetDelta,
  classifyDiffs,
  describeDiffs,
  diffSnapshots,
  maxNumericDelta,
  isListPermutationOnly,
  type KnownAsymmetry,
  type SnapshotDiff,
} from '../lib/projectSnapshot';
import type { TargetingDigest } from '../lib/fuzzOracleCore';
import {
  streamV2,
  streamV3,
  drawAction,
  swarmProfileFor,
  weightedNamesFor,
  type FuzzAction,
} from './undoRedoFuzzActions';
import { mustBeRejected } from './undoRedoFuzzImportActions';
import { materializeSurveyFile, type GeneratedFileSpec } from '../lib/surveyFileSpecs';
import {
  FILE_ARG_ACTIONS,
  TraceRecorder,
  defaultTracePath,
  readTrace,
  writeTrace,
  type FuzzTrace,
  type TraceStep,
} from '../lib/fuzzTrace';
import {
  REPLAYABLE_MUTATIONS,
  bumpEpoch,
  enableIdAgnosticModel,
  fetchMismatch,
  initModel,
  injectOracle,
  observeOnly,
  resyncModel,
  runStep,
  type StepResponse,
} from './undoRedoFuzzBridge';

/**
 * EVERY env knob below is read at CALL time, never frozen into a module-level
 * const.
 *
 * Two runners set these variables from inside their own `main()`, long after
 * this module was imported: `pnpm agent:scenario --trace` parses its flags
 * there, and `pnpm agent:fuzz-shrink` restores the env the RECORDING ran under
 * there. A module-level const would capture whatever shell the runner was
 * started from instead - so a trace recorded against one fixture would silently
 * replay against another, and the shrink would report the trace as green. Same
 * rule, and the same reason, as `agentBuildMode()` in `e2e/lib/env.ts`.
 */

/**
 * INERT, and kept deliberately. Every issue-413 finding this scenario reproduced
 * is fixed, every classifier is deleted, and there is nothing left for this flag
 * to tolerate - a mismatch of any shape fails the run.
 *
 * It is still READ, and still reported in the `reproduce:` line, for two
 * reasons: a repro line copied from the issue history still runs (it just no
 * longer changes the verdict), and a failure message that names the flag makes
 * "this run was tolerant" impossible to assume when it was not. If a future
 * open bug ever needs the same escape hatch, re-derive it here - including its
 * rule, which is that it must never be the default, because a green default
 * would mean the oracle had been weakened.
 */
function allowKnownBugs(): boolean {
  return process.env.STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS === '1';
}

/**
 * Replay a recorded trace instead of drawing moves (`STALAGMIGHT_FUZZ_REPLAY`,
 * or `--trace` on `pnpm agent:scenario`, which sets it).
 *
 * Replay is a MODE of this scenario, never a separate one. Everything after the
 * move is chosen - the fused step, the three oracles, the model bookkeeping, the
 * classification and the failure payload - is the same code on both paths, so a
 * replayed failure is the same failure and not a lookalike. Only the source of
 * the decisions changes.
 */
function replayTracePath(): string {
  return process.env.STALAGMIGHT_FUZZ_REPLAY ?? '';
}

/** Force-write the trace of a run that PASSED, to this path. */
function traceOutPath(): string {
  return process.env.STALAGMIGHT_FUZZ_TRACE ?? '';
}

/**
 * Which `.stalx` under `e2e/fixtures/` to fuzz. Basename only - anything else is
 * flattened by `fixtureSourcePath`'s `path.basename`, which is the containment
 * guard and stays the only one.
 */
function fuzzFixture(): string {
  return process.env.STALAGMIGHT_FUZZ_FIXTURE || DEFAULT_FIXTURE;
}

/** The stream label a run reports, in traces, metrics and the repro line. */
function streamName(): 'v1' | 'v2' | 'v3' {
  if (streamV3()) return 'v3';
  return streamV2() ? 'v2' : 'v1';
}

/** The `STALAGMIGHT_FUZZ_STREAM=` clause a repro line needs, if any. */
function streamClause(): string {
  const name = streamName();
  return name === 'v2' ? '' : ` with STALAGMIGHT_FUZZ_STREAM=${name}`;
}

/** Move mix. Undo/redo are re-rolled when the stack they need is empty. */
const P_MUTATE = 0.6;
const P_UNDO = 0.25;

/**
 * Share of history moves taken as a MULTI-step jump through
 * `undoTo`/`redoTo` (the history dropdown's path) instead of a single step.
 *
 * Drawn from a SEPARATE PRNG (`mulberry32(seed ^ JUMP_STREAM_SALT)`), never from
 * `ctx.rng()`: an extra draw on the primary stream would shift every subsequent
 * decision and invalidate every recorded repro. The primary stream still decides
 * mutate-vs-undo-vs-redo, every generator and every value; this stream only
 * decides whether a history move is taken in one hop and how wide.
 */
const P_JUMP = 0.1;
const JUMP_STREAM_SALT = 0x9e3779b9;

/**
 * Share of iterations that take a save/close/reopen round trip, drawn from the
 * same secondary stream as jumps (never from `ctx.rng()`).
 *
 * Undo history is PERSISTED - both stacks serialize into the .stalx and restore
 * on open - so nothing below the save point is exercised until a run crosses
 * that boundary. Kept low because a round trip costs a full save plus a full
 * reopen, and capped per run.
 */
const P_SAVE = 0.05;
const MAX_SAVE_RELOADS = 3;
/**
 * Its OWN salt, not the jump stream's. Each optional feature draws from an
 * independent stream so adding one cannot move another's decisions - sharing a
 * stream would have made "add saves" silently change which jumps a seed takes.
 */
const SAVE_STREAM_SALT = 0x85ebca6b;

/** Diffs shown in a failure message (more are counted, not printed). */
const SHOWN_DIFFS = 6;
/**
 * Differences the slow re-compare collects before it gives up.
 *
 * A cap is necessary (a whole-project divergence would otherwise build a list
 * the size of the project), but a capped list can only ever support a NEGATIVE
 * conclusion. `checkSnapshot` asks for one more than this and refuses to
 * tolerate anything when the extra one comes back.
 */
const DIFF_LIMIT = 200;
/** Actions shown in the failure message's history tail. */
const HISTORY_TAIL = 12;
/**
 * `STALAGMIGHT_FUZZ_DUMP_HISTORY=1` prints the WHOLE action history on failure
 * instead of the tail. The tail is right for reading a failure; the full
 * sequence is what you need to transcribe a repro into a Rust test.
 */
function dumpFullHistory(): boolean {
  return process.env.STALAGMIGHT_FUZZ_DUMP_HISTORY === '1';
}
/** Headroom kept below the undo history limit so FIFO trimming can never fire. */
const DEPTH_HEADROOM = 5;

interface SnapshotCheck {
  equal: boolean;
  /** Equal only once floating-point tolerance was applied. */
  floatNoise: boolean;
  /** Save/reload changes subtracted before reporting the rest (empty = none). */
  attributed: KnownAsymmetry[];
  /** Rendered diff block (empty when equal). */
  text: string;
}

const EQUAL: SnapshotCheck = {
  equal: true,
  floatNoise: false,
  attributed: [],
  text: '',
};

/**
 * The SLOW path, reached only after the page has already reported a mismatch:
 * re-compare with numeric tolerance, subtract what a save or a reload is
 * documented to change, and render whatever is left.
 *
 * There is no third outcome any more. This used to end in a known-open-bug
 * classification that `STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS=1` could walk past;
 * every one of those bugs is fixed, so anything that survives the attribution
 * subtraction is a failure.
 */
function checkSnapshot(expected: string, actual: string, attributable: readonly KnownAsymmetry[]): SnapshotCheck {
  if (expected === actual) return EQUAL;
  // Geometry is re-derived rather than copied on every undo/redo, so a
  // coordinate whose true value is 0 can come back as different femtometre dust.
  //
  // ONE MORE than we report, so `truncated` can tell "exactly SHOWN_DIFF_LIMIT
  // differences" from "at least that many, and the rest were never looked at".
  // The distinction decides whether the attribution may speak for this mismatch
  // at all: the walk visits root keys in sorted order, so a wide `project.*`
  // difference can spend the whole budget before `stations` or `stats` is
  // examined once, and "every path was a documented save/reload change" would
  // then be a statement about a prefix rather than about the mismatch.
  const all = diffSnapshots(expected, actual, DIFF_LIMIT + 1);
  if (all.length === 0) return { ...EQUAL, floatNoise: true };
  const truncated = all.length > DIFF_LIMIT;
  if (truncated) all.length = DIFF_LIMIT;

  // SUBTRACT the paths a save or a reload is documented to change before
  // classifying the rest. Without this a real bug found after a save reads as a
  // pile of gauge-anchor noise with the actual divergence buried in it.
  let diffs = all;
  let attributed: KnownAsymmetry[] = [];
  let absorbed: SnapshotDiff[] = [];
  if (attributable.length > 0) {
    const split = classifyDiffs(all, attributable);
    if (split.known.length > 0) {
      diffs = split.unknown;
      attributed = split.asymmetries;
      absorbed = split.known.map((hit) => hit.diff);
    }
  }
  if (diffs.length === 0) {
    // The ATTRIBUTED shape is what the caller RESYNCS on, so it may only be
    // returned for a complete list. Every path we saw was attributable, but on
    // a truncated diff the ones past the cap were never looked at.
    if (truncated) {
      return {
        equal: false,
        floatNoise: false,
        attributed,
        text:
          `  every path in the first ${DIFF_LIMIT} was a documented save/reload change, but the\n` +
          '  diff was CAPPED there - so this is a statement about a PREFIX, not about the mismatch.',
      };
    }
    // The absorbed MAGNITUDE goes on the record. The re-derived-geometry
    // families are bounded by shape, not by size (see RELOAD_RESOLVED_GEOMETRY),
    // which is only defensible while the number is visible.
    const worst = maxNumericDelta(absorbed);
    return {
      equal: false,
      floatNoise: false,
      attributed,
      text: attributed
        .map((entry) => `  ATTRIBUTED [${entry.id}]: ${entry.description}`)
        .concat(worst === null ? [] : [`  largest absorbed numeric change: ${worst.toExponential(3)}`])
        .join('\n'),
    };
  }

  const lines = [describeDiffs(diffs.slice(0, SHOWN_DIFFS))];
  if (diffs.length > SHOWN_DIFFS) lines.push(`  ...and ${diffs.length - SHOWN_DIFFS} more differing path(s)`);
  if (truncated) {
    lines.push(
      `  THE DIFF WAS CAPPED at ${DIFF_LIMIT} path(s): everything above is a PREFIX and nothing`,
      '  past it was examined, so no attribution can speak for this mismatch and none is allowed to.',
    );
  }
  // A shot list that differs field-by-field cannot say whether the same shots
  // moved or a DIFFERENT shot was removed. uuid survives a save/reload, so it
  // answers that - and the answer is the difference between drift and data loss.
  if (diffs.some((diff) => /^project\.lines\[\d+\]\.shots\b/.test(diff.path))) {
    const deltas = shotSetDelta(expected, actual);
    if (deltas.length > 0) {
      lines.push('  SHOT IDENTITY DELTA (by uuid, which survives save/reload):');
      lines.push(describeShotSetDelta(deltas));
    } else {
      lines.push('  (shot identity by uuid is IDENTICAL on both sides - the shots moved, none was lost)');
    }
  }
  for (const entry of attributed) {
    lines.push(`  (plus attributable change(s) [${entry.id}], subtracted before this diff)`);
  }

  // A shifted list reads as dozens of unrelated value diffs at every index past
  // the move, so say so in one sentence. DIAGNOSTIC ONLY: the run fails either
  // way. This used to be finding 4's classifier (undo of RemoveLoopClosure
  // pushed instead of reinserting); the classifier is deleted with the bug, and
  // what is kept is the READING of a failure that ever looks like it again.
  // Still gated on `!truncated`, because a capped diff cannot support that
  // sentence either - it would be describing a prefix.
  if (!truncated && attributed.length === 0 && isListPermutationOnly(expected, actual, LOOP_CLOSURES_PATH)) {
    lines.push('  (every difference above is explained by project.loop_closures being REORDERED:');
    lines.push('   same closures, same ids, same metrics, different positions)');
  }

  return {
    equal: false,
    floatNoise: false,
    attributed,
    text: lines.join('\n'),
  };
}

export const undoRedoFuzz: Scenario = {
  name: 'undo-redo-fuzz',
  description:
    'Seeded random mutations interleaved with undo/redo walks; asserts an exact ' +
    'canonical state roundtrip at every history depth, compared in-page.',
  defaultIterations: 40,
  async run(ctx, opts) {
    // The temp dir the save/reload boundary writes into is held HERE, one
    // level above the run body, so a FAILING run cleans it up too. That is
    // not a corner case even now that a clean run is the expectation: `pnpm
    // agent:fuzz-shrink` replays this scenario hundreds of times per shrink
    // with most candidates failing - each one used to leave a saved copy of
    // the fixture project behind in /tmp.
    const saveDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-fuzz-save-'));
    try {
      await runFuzz(ctx, opts, saveDir);
    } finally {
      try {
        fs.rmSync(saveDir, { recursive: true, force: true });
      } catch {
        // Best-effort: a leftover temp dir is not a scenario failure.
      }
    }
  },
};

/**
 * The run body. A free function rather than the `run` member so its caller can
 * own the temp dir in a `try`/`finally` - see `undoRedoFuzz.run`.
 */
async function runFuzz(
  ctx: ScenarioContext,
  { iterations }: { iterations: number },
  saveDir: string,
): Promise<void> {
  // Resolved once per RUN, not once per process - see the env-knob rule above.
  const fixture = fuzzFixture();
  const { stripped } = await loadFixtureWithoutUndoHistory(ctx, fixture);
  const strippedRows = Object.values(stripped.cleared).reduce((sum, n) => sum + n, 0);
  ctx.setMetric(`fixture:${fixture}`, 1);
  ctx.setMetric('strippedUndoRows', strippedRows);

  if (!(await disableAutoSave(ctx))) {
    throw new Error(
      'could not disable auto-save; a save mutates live state with no undo entry and would ' +
        'desync the model (see disableAutoSave in scenarios/common.ts)',
    );
  }

  // `saveAfterImport` (v3) branches on `uiStore.importContext`: null or
  // 'existing-project' only marks the frontend dirty, but 'new-project' SAVES
  // the open project - undo-lessly, with the healing mutations a save applies -
  // and this scenario would then be comparing against a model that never saw
  // it. Nothing here opens an import wizard, so it must be null; asserting that
  // once turns an assumption the generator relies on into a checked fact.
  // `readStore` answers `{ value, truncated }` for a dot path, never the bare
  // value - reading it as the value itself compares an object against null and
  // fails on a project that is perfectly fine.
  const importContext = (await ctx.readStore('ui', 'importContext')) as { value?: unknown };
  if (importContext.value !== null && importContext.value !== undefined) {
    throw new Error(
      `uiStore.importContext opened as ${JSON.stringify(importContext.value)}; the v3 import ` +
        'group assumes null, because saveAfterImport() saves the project when it is "new-project"',
    );
  }

  const historyLimit = await ctx.eval<number>(`
    const config = window.__E2E_TEST__.projectStore.getState().config;
    return config && config.general && typeof config.general.undo_history_limit === 'number'
      ? config.general.undo_history_limit
      : 100;
  `);

  await injectOracle(ctx);
  const init = await initModel(ctx);

  // Hard expectation, not a tolerance: the fixture's persisted history was
  // deleted from the temp copy before the load, so the app must come up with
  // both stacks empty. Anything else means the strip did not take (a table
  // this build persists that `UNDO_HISTORY_TABLES` does not list, say), and
  // every depth in the model would be measured from the wrong floor.
  const baseUndo = init.undoLen;
  if (baseUndo !== 0 || init.redoLen !== 0) {
    throw new Error(
      `${fixture} opened with inherited history after stripping ` +
        `${strippedRows} row(s): undoLen=${baseUndo} redoLen=${init.redoLen} ` +
        `(cleared ${JSON.stringify(stripped.cleared)}, absent ${JSON.stringify(stripped.missing)})`,
    );
  }

  if (init.invariants.length > 0) {
    throw new Error(
      `${fixture} violates project invariants BEFORE any fuzzing:\n` +
        init.invariants.map((violation) => `  INVARIANT: ${violation}`).join('\n'),
    );
  }

  // Never push so many entries that the stack's FIFO trimming fires - that
  // would silently invalidate snapshots[0] and every depth above it. baseUndo
  // is 0 by the assertion above; it stays in the arithmetic so the bound still
  // reads as "headroom from wherever the stack actually starts".
  const maxDepth = Math.max(1, Math.min(iterations, historyLimit - baseUndo - DEPTH_HEADROOM));

  /** Non-null when this run is replaying a recorded trace instead of drawing. */
  const replayPath = replayTracePath();
  const traceOut = traceOutPath();
  const replay: FuzzTrace | null = replayPath ? readTrace(replayPath) : null;
  if (replay && replay.scenario !== 'undo-redo-fuzz') {
    throw new Error(
      `trace ${replayPath} was recorded by ${replay.scenario}; run it with that scenario`,
    );
  }
  // The recorded steps carry RESOLVED line and shot ids, so replaying them
  // against a different project drives moves at ids that do not exist there:
  // an invented failure, or a green run, either way not the recorded bug.
  if (replay && replay.fixture !== fixture) {
    throw new Error(
      `trace ${replayPath} was recorded on ${replay.fixture} but this run is on ${fixture}; ` +
        `its steps carry ids resolved against that project. Re-run with ` +
        `STALAGMIGHT_FUZZ_FIXTURE=${replay.fixture} (pnpm agent:scenario --trace restores it for you).`,
    );
  }

  const history: string[] = [];
  let cursor = 0;
  /** Node's mirror of the page's `snapshots.length - 1`. */
  let top = 0;
  let deepest = 0;
  let maxBytes = init.bytes;
  let undoLen = init.undoLen;
  let redoLen = init.redoLen;
  let digest: TargetingDigest = init.digest;
  const jumpRng = mulberry32((ctx.seed ^ JUMP_STREAM_SALT) >>> 0);
  // Per-run generator weighting, derived from the seed on its own stream.
  const swarm = swarmProfileFor(ctx.seed);
  const weightedNames = weightedNamesFor(swarm);
  ctx.setMetric(`swarm:${swarm.name}`, 1);
  ctx.setMetric(`stream:${streamName()}`, 1);
  const saveRng = mulberry32((ctx.seed ^ SAVE_STREAM_SALT) >>> 0);
  let savesDone = 0;
  let reloadsDone = 0;
  const savePath = path.join(saveDir, 'fuzz-roundtrip.stalx');
  // Both stacks persist, but only the newest `undo_persist_limit` entries do.
  // Crossing the boundary with a deeper history would silently truncate the
  // restored stack and invalidate the model, so the move is skipped instead.
  const persistLimit = await ctx.eval<number>(`
    const config = window.__E2E_TEST__.projectStore.getState().config;
    return config && config.general && typeof config.general.undo_persist_limit === 'number'
      ? config.general.undo_persist_limit
      : 20;
  `);

  /**
   * Where the v3 import group's generated input files live.
   *
   * A subdirectory of the run's own temp dir, which `undoRedoFuzz.run` removes
   * in a `finally` - so a failing run and every one of `pnpm agent:fuzz-shrink`'s
   * hundreds of candidates clean up after themselves, exactly as the save/reload
   * copies already do. Nothing is lost by deleting them: the trace carries the
   * SPEC, and `surveyFileWriters.ts` is a pure function of it, so a replay
   * rebuilds byte-identical files.
   */
  const filesRoot = saveDir;

  /**
   * A session-relative `rel` as an absolute path, refusing anything that leaves
   * the run's own directory.
   *
   * `readTrace` already applies the same rule to every file-bearing step, so on
   * the replay path this is belt and braces - but the GENERATOR path never goes
   * through `readTrace`, and a containment rule that only one of two callers
   * enforces is a convention rather than a boundary.
   */
  const resolveSessionPath = (rel: string): string => {
    const root = path.resolve(filesRoot);
    const target = path.resolve(root, rel);
    if (target !== root && !target.startsWith(root + path.sep)) {
      throw new Error(`generated file path ${JSON.stringify(rel)} escapes ${root}`);
    }
    return target;
  };

  /** Write a generated spec at `rel`, returning the absolute path it landed on. */
  const materializeFile = (spec: GeneratedFileSpec, rel: string): string => {
    const target = resolveSessionPath(rel);
    // Renamed to the rel's basename rather than trusting `spec.name`: they agree
    // for anything a generator produced, and when a hand-edited trace disagrees
    // the file must still land where the mutation is about to look for it.
    const named: GeneratedFileSpec = { ...spec, name: path.basename(target) };
    materializeSurveyFile(path.dirname(target), named);
    ctx.metric('generatedFiles');
    return target;
  };

  /**
   * The args a file-taking action is DISPATCHED with: `args[0]` swapped from the
   * recorded session-relative path to this process's absolute one.
   *
   * The trace keeps the relative form, which is what makes a v3 trace portable
   * and publishable - see `FILE_ARG_ACTIONS` in `lib/fuzzTrace.ts`.
   */
  const resolveFileArgs = (action: string, args: readonly unknown[]): unknown[] => {
    if (!FILE_ARG_ACTIONS.has(action)) return [...args];
    const rel = args[0];
    if (typeof rel !== 'string' || rel === '') {
      throw new Error(
        `${action} takes an input file path as args[0], got ${JSON.stringify(rel ?? null)}`,
      );
    }
    return [resolveSessionPath(rel), ...args.slice(1)];
  };

  const note = (line: string): void => {
    history.push(line);
  };

  /**
   * The move that is RUNNING RIGHT NOW - `null` between moves.
   *
   * A `note` line reports what a move DID to the stacks, so it can only be
   * written once the move RETURNS. A move that throws instead - the step never
   * comes back, or an assertion inside it fires - then left the tail ending on
   * the PREVIOUS move, which reads as an attribution and is one: it names the
   * wrong move. The same gotcha in the Rust tier mis-read two of issue #464's
   * defects during triage.
   *
   * Armed before each step and disarmed after, and rendered by `fail` only, so
   * a run that does not fail behaves exactly as before and the trace is
   * untouched.
   */
  let inFlight: string | null = null;
  const startingMove = (label: string): void => {
    inFlight = label;
  };
  const finishedMove = (): void => {
    inFlight = null;
  };

  /**
   * The resolved move list for this run. Observe-only: `record` is called AFTER
   * the decision it describes, draws nothing and reads no app state, so a run
   * with recording on takes exactly the trajectory it took without.
   *
   * The closing unwind/rewind is deliberately NOT recorded - it is a function
   * of the depth the run ends at, and a replay regenerates it. See
   * `e2e/agent/lib/fuzzTrace.ts`.
   */
  const recorder = new TraceRecorder();

  const recordMove = (direction: 'undo' | 'redo', jumpSteps: number | null): void => {
    if (jumpSteps === null) {
      recorder.record(direction === 'undo' ? { kind: 'undo' } : { kind: 'redo' });
      return;
    }
    recorder.record(
      direction === 'undo'
        ? { kind: 'undoTo', steps: jumpSteps }
        : { kind: 'redoTo', steps: jumpSteps },
    );
  };

  /** Write the trace, best-effort: a failed write must not mask the failure. */
  const dumpTrace = (failure: { message: string } | null, explicitPath: string): string | null => {
    const target =
      explicitPath ||
      (ctx.artifactsDir ? defaultTracePath(ctx.artifactsDir, 'undo-redo-fuzz', ctx.seed) : '');
    if (!target) return null;
    try {
      return writeTrace(
        target,
        recorder.build({
          scenario: 'undo-redo-fuzz',
          seed: ctx.seed,
          iterations,
          fixture,
          stream: streamName(),
          failure,
        }),
      );
    } catch {
      return null;
    }
  };

  // Annotated on the CONST, not just the arrow: that is what makes TypeScript
  // treat a `fail(...)` call as never-returning, so a caller ending in one does
  // not need a dead `return` after it. Same form as undoPersistenceFuzz.
  const fail: (headline: string, detail?: string) => never = (headline, detail = '') => {
    const tail = (dumpFullHistory() ? history : history.slice(-HISTORY_TAIL)).map(
      (line) => `    ${line}`,
    );
    // A move that has not RETURNED has no history line yet, so name it here -
    // without this the tail ends on the move BEFORE the one that failed.
    if (inFlight !== null) tail.push(`    ${inFlight} -> IN FLIGHT (had not returned)`);
    const message = [
      headline,
      `  reproduce: run_scenario("undo-redo-fuzz", seed=${ctx.seed}, iterations=${iterations})` +
        `${fixture === DEFAULT_FIXTURE ? '' : ` with STALAGMIGHT_FUZZ_FIXTURE=${fixture}`}` +
        `${streamClause()}` +
        `${allowKnownBugs() ? ' with STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS=1 (INERT: nothing left to allow)' : ''}` +
        `${replayPath ? ` (REPLAY of ${replayPath})` : ''}`,
      `  fixture=${fixture} stream=${streamName()} swarm=${swarm.name} ` +
        `depth=${cursor} modelTop=${top} baseUndo=${baseUndo}`,
      ...(detail ? [detail] : []),
      `  last ${tail.length} action(s):`,
      ...tail,
    ].join('\n');
    // The trace carries the message VERBATIM, so a shrink compares signatures
    // taken from the same text a human reads. The path is appended afterwards,
    // which is why it is not part of what was recorded.
    const written = dumpTrace({ message }, traceOut);
    throw new Error(written ? `${message}\n  trace: ${written}` : message);
  };

  /** One fused round trip, timed. */
  const step = async (
    kind: 'mutate' | 'undo' | 'redo' | 'undoTo' | 'redoTo' | 'save' | 'reload',
    action: string,
    actionArgs: unknown[],
    steps = 1,
  ): Promise<StepResponse> => {
    const started = Date.now();
    const res = await runStep(ctx, {
      kind,
      action,
      args: actionArgs,
      cursor,
      undo0: undoLen,
      steps,
    });
    ctx.metric('fusedStepMs', Date.now() - started);
    // Attribution, so the next round of tuning aims at evidence: the store
    // action's own work, the three read-only IPC calls, the in-page
    // canonicalization, and (by subtraction from fusedStepMs) the bridge.
    ctx.metric('actionMs', res.timing.actionMs);
    ctx.metric('observeMs', res.timing.observeMs);
    ctx.metric('canonicalMs', res.timing.canonicalMs);
    ctx.metric('steps');
    return res;
  };

  /** The page applied the same bookkeeping rules; prove it landed in the same place. */
  const assertModel = (res: StepResponse, headline: string): void => {
    if (res.cursorAfter !== cursor || res.top !== top) {
      fail(
        headline,
        `  page model says cursor=${res.cursorAfter} top=${res.top}, Node says cursor=${cursor} top=${top}`,
      );
    }
  };

  /**
   * ABSOLUTE well-formedness, checked on every observed state. Independent of
   * the roundtrip comparison: a forward command that corrupts the project and
   * an undo that faithfully restores the corruption match at every depth, so
   * only this fires.
   */
  const assertInvariants = (res: StepResponse, what: string): void => {
    if (res.invariants.length === 0) return;
    fail(
      `project invariant(s) violated after ${what}`,
      res.invariants.map((violation) => `  INVARIANT: ${violation}`).join('\n'),
    );
  };

  const assertStacks = (headline: string, res: StepResponse): void => {
    const wantUndo = baseUndo + cursor;
    const wantRedo = top - cursor;
    if (res.undoLen !== wantUndo || res.redoLen !== wantRedo) {
      fail(
        headline,
        `  expected undoLen=${wantUndo} redoLen=${wantRedo}, got undoLen=${res.undoLen} redoLen=${res.redoLen}`,
      );
    }
  };

  /**
   * Resolve a mismatch the page reported. Fetches both canonical strings (the
   * only time they cross the bridge) and either RE-BASELINES a documented
   * save/reload change or fails with the full diff. Those two outcomes are all
   * there are - no known-bug tolerance survives.
   *
   * Returns true when the model was RESYNCED: every expectation the page
   * computed for this step was computed against the snapshot the resync
   * replaced.
   */
  const resolveMismatch = async (headline: string, res: StepResponse): Promise<boolean> => {
    const pair = await fetchMismatch(ctx);
    // ONLY a comparison that crosses an epoch boundary may subtract the
    // documented undo-less changes. Within an epoch the oracle stays strict -
    // otherwise "a reload re-solves offsets" would permanently hide every
    // line_offsets bug the fuzzer exists to find (bugs 3 and 7).
    const attributable = res.crossesEpoch
      ? [
          ...(savesDone > 0 ? SAVE_HEALING_MUTATIONS : []),
          ...(reloadsDone > 0 ? RELOAD_RESOLVED_GEOMETRY : []),
        ]
      : [];
    const check = checkSnapshot(pair.expected ?? '', pair.actual ?? '', attributable);
    if (check.equal) {
      // Counted, never resynced: keeping the ORIGINAL expectation means the
      // tolerance can never ratchet, only ever absorb the same dust again.
      if (check.floatNoise) ctx.metric('floatNoise');
      return false;
    }
    // Every difference was a documented save heal or reload re-solve: record
    // the new baseline and carry on. Nothing else is tolerated here.
    if (check.attributed.length > 0 && check.text.startsWith('  ATTRIBUTED')) {
      ctx.metric('resyncs');
      for (const entry of check.attributed) ctx.metric(`resync:${entry.id}`);
      top = await resyncModel(ctx, pair.index);
      return true;
    }
    // No known-bug branch: every issue-413 finding is fixed and every
    // classifier deleted, so anything the attribution did not explain fails.
    fail(headline, check.text);
  };

  const record = (res: StepResponse): void => {
    undoLen = res.undoLen;
    redoLen = res.redoLen;
    digest = res.digest;
    maxBytes = Math.max(maxBytes, res.bytes);
    deepest = Math.max(deepest, cursor);
  };

  /**
   * One history move. `jumpSteps` takes it through `undoTo`/`redoTo` - the
   * history dropdown's multi-step path - instead of a single `undo`/`redo`.
   *
   * The oracle is deliberately the SAME snapshot array either way: the state a
   * jump lands on must equal the one single steps recorded at that depth. That
   * equivalence is not free - `undo_to`/`redo_to` loop `undo_inner`/`redo_inner`
   * and call NEITHER `invalidate_adjusted_cache()` NOR `reconcile_geometry()`,
   * both of which `undo()`/`redo()` do (verified in
   * src-tauri/src/core/state/undo.rs) - so a divergence here is a real product
   * bug reachable from the dropdown.
   */
  const walk = async (
    direction: 'undo' | 'redo',
    tag: string,
    jumpSteps: number | null = null,
  ): Promise<void> => {
    const steps = jumpSteps ?? 1;
    const back = direction === 'undo';
    const jumped = jumpSteps !== null;
    const action = jumped ? (back ? 'undoTo' : 'redoTo') : direction;
    const label = jumped ? `${action}(${steps - 1})[${steps} step(s)]` : direction;
    const beforeUndo = undoLen;
    const beforeRedo = redoLen;
    startingMove(`${tag} ${label}`);
    const res = await step(
      jumped ? (back ? 'undoTo' : 'redoTo') : direction,
      action,
      jumped ? [steps - 1] : [],
      steps,
    );
    finishedMove();
    if (!res.actionOk || res.commandSuccess === false) {
      note(`${tag} ${label} -> FAILED`);
      fail(
        `${label} at depth ${cursor} failed outright`,
        `  error: ${res.commandError ?? res.actionError ?? 'unknown'}`,
      );
    }
    cursor += back ? -steps : steps;
    note(
      `${tag} ${label} -> depth ${cursor} (undo ${beforeUndo}->${res.undoLen}, redo ${beforeRedo}->${res.redoLen})`,
    );
    assertInvariants(res, label);
    assertModel(res, `${label} left the page model where Node did not expect it`);
    assertStacks(`${label} left the history stacks in an unexpected shape`, res);
    if (res.verdict !== 'equal') {
      await resolveMismatch(`${label} did not restore the state recorded at depth ${cursor}`, res);
    }
    record(res);
    ctx.metric(direction === 'undo' ? 'undos' : 'redos');
    if (jumped) {
      ctx.metric('jumps');
      ctx.metric(`jump:${action}`);
      ctx.metric('jumpSteps', steps);
    }
  };

  /**
   * Save, close, reopen: the persistence boundary.
   *
   * Undo history lives in the .stalx (both stacks, sequence renumbered), so
   * until a run crosses this boundary nothing below it is exercised at all.
   * Three separate oracles run here:
   *
   *   1. SAVE IS ATTRIBUTABLE. `save_project` heals live state with no undo
   *      entry - `ensure_shot_uuids`, `ensure_unique_shot_uuids`,
   *      `capture_missing_gauge_anchors`, the orphan `line_offsets` retain,
   *      non-finite sanitize. Every path that changes must match
   *      `SAVE_HEALING_MUTATIONS` by path AND direction; anything else fails.
   *   2. THE STACKS SURVIVE. Both lengths must come back exactly.
   *   3. THE RESTORED STACK IS THE SAME HISTORY. From here the model compares
   *      id-agnostically (the loader renumbers `Shot::id` to
   *      `line_id * FEATURE_ID_STRIDE + sequence_order`), and the run's
   *      final unwind/rewind then walks the whole RESTORED stack against the
   *      snapshots recorded before the save.
   */
  const saveAndReload = async (tag: string): Promise<void> => {
    // The save is about to make undo-less changes, so everything recorded so
    // far belongs to the previous epoch.
    await bumpEpoch(ctx);
    startingMove(`${tag} save`);
    const saveRes = await step('save', 'saveProject', [savePath]);
    finishedMove();
    if (!saveRes.actionOk || saveRes.commandSuccess === false) {
      note(`${tag} save -> FAILED`);
      fail('saveProject failed', `  error: ${saveRes.commandError ?? saveRes.actionError ?? 'unknown'}`);
    }
    note(`${tag} save -> ok (depth ${cursor}, undo ${saveRes.undoLen}, redo ${saveRes.redoLen})`);
    assertInvariants(saveRes, 'save');
    assertStacks('save changed the history stacks', saveRes);
    savesDone += 1;
    if (saveRes.verdict !== 'equal') {
      await resolveMismatch(
        'save mutated the project in a way SAVE_HEALING_MUTATIONS does not account for',
        saveRes,
      );
    }
    record(saveRes);
    ctx.metric('saves');

    startingMove(`${tag} close/reopen`);
    const closed = (await ctx.callAction('project', 'closeProject', [])) as {
      ok: boolean;
      error?: string;
      result?: { success?: boolean; error?: string };
    };
    if (!closed.ok || closed.result?.success === false) {
      fail('closeProject failed before reopen', `  error: ${closed.result?.error ?? closed.error ?? 'unknown'}`);
    }
    const opened = (await ctx.callAction('project', 'loadProject', [savePath])) as {
      ok: boolean;
      error?: string;
      result?: { success?: boolean; error?: string };
    };
    if (!opened.ok || opened.result?.success === false) {
      fail('reopening the saved project failed', `  error: ${opened.result?.error ?? opened.error ?? 'unknown'}`);
    }
    finishedMove();

    // Switch BEFORE re-observing so both sides of the comparison are in the
    // same form. Idempotent, so later reloads are a no-op.
    await enableIdAgnosticModel(ctx);
    await bumpEpoch(ctx);
    reloadsDone += 1;
    const res = await observeOnly(ctx, cursor);
    note(`${tag} reload -> depth ${cursor} (undo ${res.undoLen}, redo ${res.redoLen})`);
    assertInvariants(res, 'reload');
    assertStacks('the persisted history did not come back intact', res);
    if (res.verdict !== 'equal') {
      await resolveMismatch('reopening the saved project did not restore the state it was saved at', res);
    }
    record(res);
    ctx.metric('reloads');
  };

  /** Should this iteration cross the save/reload boundary? */
  const wantSaveReload = (): boolean => {
    if (savesDone >= MAX_SAVE_RELOADS) return false;
    // Only the newest `undo_persist_limit` entries survive the write, so a
    // deeper history would come back truncated - skip rather than model it.
    if (top > persistLimit) {
      ctx.metric('savesSkippedDeepHistory');
      return false;
    }
    return saveRng() < P_SAVE;
  };

  /**
   * How wide a jump to take, or null for a single step. Drawn from the
   * SEPARATE stream so the primary sequence is untouched.
   */
  const drawJump = (direction: 'undo' | 'redo'): number | null => {
    const reach = direction === 'undo' ? cursor : top - cursor;
    if (reach < 1) return null;
    if (jumpRng() >= P_JUMP) return null;
    // 1..reach. A 1-step jump is still worth taking: undo_to(0) reaches the
    // same depth through a different backend path than undo().
    return 1 + Math.floor(jumpRng() * reach);
  };

  /**
   * Apply ONE mutation and run the whole case analysis on what the stacks did.
   *
   * Lifted out of the main loop unchanged so that REPLAY can drive it with a
   * recorded action instead of a drawn one. The generator path still hands it
   * exactly what `drawAction` returned, in the same place in the sequence, so
   * no draw moved and every recorded repro seed is unaffected.
   */
  const applyMutation = async (action: FuzzAction, tag: string): Promise<void> => {
    // The FILE comes first, in the trace and on disk: a replay reads the steps
    // in order, so the step that writes the input has to precede the step that
    // opens it. On the replay path the `genFile` step was already recorded and
    // materialized by `applyTraceStep`, and `action.file` is absent - the two
    // paths converge here with the same relative path in `args[0]`.
    if (action.file) {
      recorder.record({
        kind: 'genFile',
        spec: action.file.spec,
        rel: action.file.rel,
        label: action.label,
      });
      materializeFile(action.file.spec, action.file.rel);
    }
    recorder.record({
      kind: 'mutate',
      action: action.action,
      args: action.args,
      label: action.label,
    });
    /**
     * Payloads that CANNOT legally be accepted, re-derived from the args rather
     * than carried on the action - so a replayed trace asserts exactly what the
     * recording did (see `mustBeRejected`). Null for every v1/v2 action.
     */
    const refusalRequired = mustBeRejected(action.action, action.args);
    const beforeUndo = undoLen;
    const beforeRedo = redoLen;
    startingMove(`${tag} ${action.label}`);
    const res = await step('mutate', action.action, resolveFileArgs(action.action, action.args));
    finishedMove();
    const accepted = res.actionOk && res.commandSuccess !== false;
    const pushed = res.undoLen - beforeUndo;
    note(
      `${tag} ${action.label} -> ${accepted ? 'ok' : 'rejected'} ` +
        `(undo ${beforeUndo}->${res.undoLen}, redo ${beforeRedo}->${res.redoLen})`,
    );

    if (refusalRequired !== null) {
      // Counted either way, so the oracle can be shown to be doing work rather
      // than never reaching a payload it has an opinion about.
      ctx.metric('contractRefusals');
      if (accepted) {
        fail(
          `${action.action} ACCEPTED a payload that cannot be represented`,
          `  ${refusalRequired}\n` +
            '  The frontend guard (src/utils/importPayloadGuard.ts) refuses this shape before it\n' +
            '  is sent; a route that bypasses the wizard must still be refused BELOW it.',
        );
      }
    }

    if (!accepted) {
      ctx.metric('rejected');
      ctx.metric(`rejected:${action.action}`);
      if (pushed !== 0 || res.redoLen !== beforeRedo) {
        fail(
          'a REJECTED command changed the history stacks',
          `  error: ${res.commandError ?? res.actionError ?? 'unknown'}\n` +
            `  undo ${beforeUndo}->${res.undoLen}, redo ${beforeRedo}->${res.redoLen}`,
        );
      }
      assertInvariants(res, `a rejected ${action.action}`);
      assertModel(res, 'a rejected command moved the page model');
      if (res.verdict !== 'equal') {
        await resolveMismatch(
          `a REJECTED command (${action.action}) mutated the project: ` +
            `${res.commandError ?? res.actionError ?? 'unknown'}`,
          res,
        );
      }
      record(res);
      return;
    }

    if (pushed === 1) {
      if (res.redoLen !== 0) {
        fail(
          'a successful command did not clear the redo stack',
          `  redo ${beforeRedo}->${res.redoLen} after ${action.label}`,
        );
      }
      cursor += 1;
      top = cursor;
      assertInvariants(res, action.label);
      assertModel(res, 'the page recorded the new snapshot at a different depth than Node expected');
      if (res.verdict !== 'recorded') {
        fail(`the page did not record a snapshot for a pushed entry (verdict=${res.verdict})`);
      }
      record(res);
      ctx.metric('mutations');
      ctx.metric(`op:${action.action}`);
      return;
    }

    if (pushed !== 0) {
      fail(
        `a single command pushed ${pushed} undo entries (expected 0 or 1)`,
        `  after ${action.label}`,
      );
    }

    // Succeeded without pushing an entry: a no-op command. It must not have
    // touched the project; it MAY have cleared the redo stack.
    ctx.metric('noops');
    ctx.metric(`noop:${action.action}`);
    if (res.undoLen >= historyLimit - 1) {
      fail(
        'the undo stack hit its history limit; FIFO trimming invalidates the model',
        `  undoLen=${res.undoLen} limit=${historyLimit}`,
      );
    }
    if (res.redoLen === 0) top = cursor;
    else if (res.redoLen !== beforeRedo) {
      fail(
        'a no-op command changed the redo stack without clearing it',
        `  redo ${beforeRedo}->${res.redoLen} after ${action.label}`,
      );
    }
    assertInvariants(res, `a no-op ${action.action}`);
    assertModel(res, 'a no-op command moved the page model');
    if (res.verdict !== 'equal') {
      await resolveMismatch(`a no-op command (${action.action}) changed the project state`, res);
    }
    record(res);
  };

  /**
   * Apply one recorded move.
   *
   * A history move whose precondition no longer holds is SKIPPED rather than
   * failed, and a jump wider than the reach is CLAMPED to it. That rule is what
   * makes an arbitrary subsequence of a trace runnable, which is what the
   * shrinker needs: removing a mutation leaves the moves around it pointing at
   * depths that no longer exist. Skipping asserts nothing, so it can never turn
   * a failing candidate green by hiding something - it just declines to
   * pretend a move was possible. Every skip is counted.
   */
  const applyTraceStep = async (traceStep: TraceStep, tag: string): Promise<void> => {
    if (traceStep.kind === 'mutate') {
      if (cursor >= maxDepth) {
        ctx.metric('replaySkipped:mutate');
        return;
      }
      // A trace supplies both the action NAME and its arguments, so the replay
      // path is narrower than the dispatch path: only the mutations a generator
      // can produce, never a scenario-driven control action like `saveProject`
      // (which takes a path this file would then be choosing).
      if (!REPLAYABLE_MUTATIONS.has(traceStep.action)) {
        throw new Error(
          `trace step names ${traceStep.action}, which is not a replayable mutation. ` +
            `A recorded trace may only replay the actions the generators produce.`,
        );
      }
      await applyMutation(
        { action: traceStep.action, args: traceStep.args, label: traceStep.label },
        tag,
      );
      return;
    }
    if (traceStep.kind === 'repair') {
      // Repair left with the integrity banner: it never changed a project on
      // the station graph (every edit validates), so an old trace's Repair
      // replays as nothing, as `undo-persistence-fuzz` replays it.
      ctx.metric('replaySkipped:repair');
      return;
    }
    if (traceStep.kind === 'save') {
      // Only the newest `undo_persist_limit` entries survive the write, so a
      // deeper history would come back truncated - skipped, exactly as
      // `wantSaveReload` skips it on the generator path.
      //
      // NOT skipped at depth 0. The generator has no such guard, so a run can
      // and does take the boundary on its first iteration; refusing it here
      // made the replay of such a trace do no reload at all, which means no
      // id-agnostic model and no RELOAD_RESOLVED_GEOMETRY - so the recorded
      // failure did not reproduce and `pnpm agent:fuzz-shrink` reported the
      // bug as fixed.
      if (top > persistLimit) {
        ctx.metric('replaySkipped:save');
        return;
      }
      recorder.record({ kind: 'save' });
      await saveAndReload(tag);
      return;
    }
    if (traceStep.kind === 'genFile') {
      // The v3 import group's input file, rebuilt from the recorded SPEC. This
      // is the whole replay contract for file-backed imports: the trace carries
      // no bytes and no absolute path, `surveyFileWriters.ts` is a pure function
      // of the spec, and the mutation that follows names the same session-
      // relative `rel` this writes - which `resolveFileArgs` then resolves
      // against THIS run's directory.
      //
      // A shrink is allowed to drop this step and keep the mutation: the file is
      // then simply absent, the import fails, and that is a legitimate (if
      // different) run rather than a broken one - so ddmin keeps the step
      // whenever removing it stops the failure reproducing.
      const spec = traceStep.spec as GeneratedFileSpec | null;
      if (spec === null || typeof spec !== 'object' || typeof spec.format !== 'string') {
        throw new Error(`trace step ${tag} has a genFile spec with no format to rebuild from`);
      }
      recorder.record(traceStep);
      materializeFile(spec, traceStep.rel);
      ctx.metric('replayGenFiles');
      return;
    }
    // Schema v2 added the rest of the import/export step vocabulary (`ui`,
    // `export`, ...). This scenario drives the store, not the UI, so it cannot
    // run them - and it must say so rather than let one fall through into the
    // history-move arm below, which is the exact bug the `kind` allow-list in
    // `validateStep` was added for.
    if (
      traceStep.kind !== 'undo' &&
      traceStep.kind !== 'redo' &&
      traceStep.kind !== 'undoTo' &&
      traceStep.kind !== 'redoTo'
    ) {
      throw new Error(
        `trace step kind ${traceStep.kind} is not one undo-redo-fuzz replays; ` +
          `it belongs to a scenario that drives the import/export UI.`,
      );
    }
    const back = traceStep.kind === 'undo' || traceStep.kind === 'undoTo';
    const reach = back ? cursor : top - cursor;
    if (reach < 1) {
      ctx.metric(`replaySkipped:${traceStep.kind}`);
      return;
    }
    const direction = back ? 'undo' : 'redo';
    if (traceStep.kind === 'undo' || traceStep.kind === 'redo') {
      recordMove(direction, null);
      await walk(direction, tag);
      return;
    }
    const steps = Math.min(traceStep.steps, reach);
    if (steps !== traceStep.steps) ctx.metric('replayClamped:jump');
    recordMove(direction, steps);
    await walk(direction, tag, steps);
  };

  if (replay) {
    ctx.setMetric('replaySteps', replay.steps.length);
    for (let index = 0; index < replay.steps.length; index++) {
      await applyTraceStep(replay.steps[index] as TraceStep, `T${index + 1}`);
    }
  }

  // Zero when replaying: the trace supplies every move, and drawing more would
  // make the replay something other than the run it came from.
  const generatorIterations = replay ? 0 : iterations;
  for (let iteration = 1; iteration <= generatorIterations; iteration++) {
    const tag = `#${iteration}`;
    // Drawn from the SECONDARY stream, and taken IN ADDITION to this
    // iteration's normal move rather than instead of it: consuming the
    // iteration would leave the run with fewer mutations than the same seed
    // produced before, and the point of the secondary stream is that adding a
    // feature does not move the primary numbers.
    if (wantSaveReload()) {
      recorder.record({ kind: 'save' });
      await saveAndReload(tag);
    }
    const roll = ctx.rng();
    let move: 'mutate' | 'undo' | 'redo' =
      roll < P_MUTATE ? 'mutate' : roll < P_MUTATE + P_UNDO ? 'undo' : 'redo';
    if (move === 'redo' && cursor >= top) move = cursor > 0 ? 'undo' : 'mutate';
    if (move === 'undo' && cursor === 0) move = 'mutate';
    if (move === 'mutate' && cursor >= maxDepth) move = 'undo';

    if (move !== 'mutate') {
      const jump = drawJump(move);
      recordMove(move, jump);
      await walk(move, tag, jump);
      continue;
    }
    await applyMutation(drawAction(ctx, digest, weightedNames), tag);
  }

  // Cross the persistence boundary at least once per run, so the final
  // unwind/rewind below walks a RESTORED stack rather than an in-memory one.
  // Also skipped on a replay, and for the same reason: a shrink is allowed to
  // remove the boundary crossing, and re-adding one here would make that
  // candidate untestable.
  if (!replay && savesDone === 0 && top > 0 && top <= persistLimit) {
    recorder.record({ kind: 'save' });
    await saveAndReload('boundary');
  }

  // Full roundtrip: unwind every entry, then replay every entry, verifying
  // the recorded snapshot at each depth in both directions.
  const summit = top;
  while (cursor > 0) await walk('undo', 'unwind');
  while (cursor < summit) await walk('redo', 'rewind');

  if (cursor !== summit) fail(`roundtrip ended at depth ${cursor}, expected ${summit}`);

  // A run that PASSED only writes a trace when asked to by name. Dropping one
  // per green run would fill the artifacts dir with files nobody reads; naming
  // a path is the signal that this particular run is wanted (recording a
  // baseline, or capturing a trace to shrink against a future regression).
  if (traceOut) {
    const written = dumpTrace(null, traceOut);
    if (written) ctx.log(`trace written to ${written} (${recorder.length} step(s))`);
  }
  ctx.setMetric('traceSteps', recorder.length);
  ctx.setMetric('modelDepth', summit);
  ctx.setMetric('maxDepth', deepest);
  ctx.setMetric('snapshotBytesMax', maxBytes);
  ctx.setMetric('historyLimit', historyLimit);
  ctx.log(
    `${replay ? `REPLAY of ${replayPath} (${replay.steps.length} recorded step(s)) ` : ''}` +
    `fixture=${fixture} stream=${streamName()} swarm=${swarm.name} ` +
      `(stripped ${strippedRows} inherited undo row(s)) seed=${ctx.seed}: ` +
      `${ctx.metrics.mutations ?? 0} mutations, ${ctx.metrics.undos ?? 0} undos, ` +
      `${ctx.metrics.redos ?? 0} redos, ${ctx.metrics.rejected ?? 0} rejected, ` +
      `${ctx.metrics.noops ?? 0} no-ops; max depth ${deepest}; snapshot <=${maxBytes}B`,
  );
}
