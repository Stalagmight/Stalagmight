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
 *     command that succeeds without pushing an entry (`ReverseSection` on a
 *     shot-less section, `UnmergeStations` on an unmerged station) must leave
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
 * closures, zero datums and null section metadata present AT LOAD, and the
 * fractal fixture has none of those (all 32 of its sections have a non-null
 * surveyor; naharon has 13 null ones). Fixture diversity is what found them.
 *
 * 1. FIXED (94896857). `--seed 1 --iterations 40` on fractal - this
 *    is what the `agent:scenarios` smoke entry hits. Create a section, undo,
 *    redo: every shot uuid was re-minted. SECOND MANIFESTATION, only
 *    observable once runs save (`naharon`, seed 23 x60): the same
 *    `restore_section` path also reset `reverse_parity` and BOTH gauge anchors
 *    (`Section::new`), so a redo silently discarded the least-squares gauge pair
 *    a save had captured.
 *    `UndoData::CreatedSectionSnapshot` stored a `SectionData`, which has no
 *    uuid field, so redo went `restore_section` -> `Section::new_with_id` ->
 *    `Shot::new` -> `Uuid::new_v4()`. Fixed by giving the entry a
 *    `SectionIdentity` (shot uuids, parity, both anchors) that
 *    `Section::restore_with_identity` puts back, with the undo refreshing that
 *    identity from the live section so save-captured anchors survive; legacy
 *    entries still mint.
 * 2. FIXED (94896857 exact-zero half, a67dd1b5 float-dust half, f16c96a8 the
 *    whole state class).
 *    `STALAGMIGHT_FUZZ_STREAM=v1 ... --seed 11 --iterations 150` on fractal
 *    (not re-found under v2) - undoing a shot INSERT left
 *    behind an all-zero `section_offsets` entry the section did not have.
 *    `adjust_section_offset` created the key while compensating; the undo put
 *    the value back to 0 but nothing removed the key. The end state is that a
 *    dust-zero entry is UNREPRESENTABLE: `write_section_offset` is the single
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
 *    FIRST shot into an EMPTY section and undoing displaced the whole section by
 *    that shot's horizontal vector (1.13 m in the repro), and the displacement
 *    RATCHETED on every further undo/redo cycle. `Project::add_shot` (the append
 *    path) is offset-neutral, but the undo routed through
 *    `remove_shot_with_rewire`, whose `pos == 0` branch unconditionally called
 *    `adjust_section_offset`. The empty-section append now has its exact inverse
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
 *    `updateSection` COULD NOT restore a `name` / `surveyor` / `date` that was
 *    previously null; the new value silently survived the undo. The capture was
 *    right (`undo_capture.rs` builds `Some(None)`), but `SectionUpdates` had only
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
 *    reached by single undos: `section_offsets` out by 2.4 m, and the stations
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
 *    `createShot(before=)` into a one-shot section makes the ids diverge from
 *    positional order, the save/reload renumbers, and the undo hit the wrong
 *    slot. The shot the user still had was gone and one they deleted was back -
 *    silent, and it persisted on the next save.
 *
 *    The chain: shots are RENUMBERED on load to
 *    `section_id * FEATURE_ID_STRIDE + sequence_order` (read.rs); undo entries
 *    are persisted and reloaded as OPAQUE JSON (`load_undo_stack`) with NO
 *    remapping anywhere on the restore path; and
 *    `UndoData::CreatedShotSnapshot { section_id, shot_id }` undid by calling
 *    `delete_shot(section_id, shot_id)`. Because the in-session allocator
 *    (`take_shot_id`) hands out ids from the SAME `section_id * STRIDE + n` space
 *    the loader renumbers into, a stale id did not miss - it COLLIDED with
 *    whatever shot now occupied that slot. A section only diverges from positional
 *    order after a mid-section insert or delete, which is why this needed a fuzzer
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
 *     within-section duplicates, captures missing gauge anchors, drops orphan
 *     `section_offsets`, stamps a creation time on a section that has none and
 *     sanitizes non-finite values, all with no undo entry.
 *   - `RELOAD_RESOLVED_GEOMETRY` - `redrive_offsets_after_load` ends in an
 *     unconditional `resolve_offsets()` ("Zero-trust: never honor a persisted
 *     derivable offset", offsets.rs:186-188), so a reload RECOMPUTES
 *     `section_offsets` and the station coordinates derived from them rather than
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
 * `section_id * FEATURE_ID_STRIDE + sequence_order` (read.rs:372) - so from the
 * first reload the model compares id-agnostically (shot ids replaced by position,
 * everything else still exact). Section ids ARE persisted and are compared.
 *
 * ## Three oracles, not one
 *
 * 1. ROUNDTRIP - the snapshot-per-depth comparison above, including the
 *    single-step/jump equivalence.
 * 2. ABSOLUTE INVARIANTS (`checkProjectInvariants`, run in-page on every
 *    observed state). The roundtrip only sees drift BETWEEN states: a command
 *    that corrupts the project and an undo that faithfully restores the
 *    corruption match at every depth. Only an absolute check fires on that.
 * 3. INTEGRITY DELTA - `check_project_integrity`'s own verdict is recorded per
 *    depth alongside the snapshot and must be restored by every history move. A
 *    state that roundtrips while its integrity verdict does not is either a bug
 *    in the checker or a divergence the snapshot cannot see. `integrityCompared`
 *    counts the comparisons so this cannot pass vacuously.
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
 *   bug 5  naharon seed 16 x40       sections[20].surveyor null -> kept
 *   bug 6  naharon seed 1  x40       "undo at depth 2 failed outright"
 *          naharon seed 21 x60       the same wedge reached through undoTo; its
 *                                    6-move sequence later became the clearest
 *                                    statement of finding 11's residue (entry 11)
 *   bug 7  fractal seed 12 x40       undoTo(0) 2.4 m section_offsets divergence
 *          fractal seed 15 x40       same bug via the INTEGRITY oracle instead:
 *                                    undoTo skipped reconcile_geometry, so
 *                                    realign_merges never ran and the reloaded
 *                                    project reported "Merged stations 6:0 and
 *                                    3:2 are 5.00 m apart". The state compare
 *                                    had absorbed the geometry paths as
 *                                    reload-resolved across the epoch, so ONLY
 *                                    the integrity delta caught it - which is
 *                                    the argument for having three oracles.
 *   bug 8  fractal seed 14 x6        createShot(before=) into a 1-shot section,
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
 * `undoRedoFuzzImportActions.ts`: synthetic `ImportSectionConfig` batches across
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
 * `execute_import_sections` refuses a non-finite value in an
 * `existing_shot_updates` patch by name, before any mutation, and has a Rust
 * test for it (`test_execute_import_sections_rejects_non_finite_shot_update`,
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
 * non-nullable numbers, exactly as finding 5 made `SectionUpdates` do.
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
 * non-empty return: `createMotifSection` plants a run and `probeSuggestMotif`
 * asks for a placement using a jittered copy of it. Both are appended at the END
 * of the v3 group, so v1 and v2 arrays are unchanged; the metric-for-metric v3
 * replay record above predates them.
 *
 * There is no Rust-tier analogue of this group in
 * `src-tauri/src/core/state/fuzz_undo_tests.rs`, and adding one is a design
 * problem rather than a transcription. `Refs::learn` pairs what a recorded move
 * created against what the replayed move created POSITIONALLY, with `zip` and no
 * length check - which is sound only because every existing `Op` creates a fixed
 * number of entities (at most one section, one shot, one closure; `BatchOp` has
 * no creating variant at all). An import creates an arbitrary number in one
 * move, and `execute_import_sections` drops sections whose placement does not
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
 *    `capture_missing_gauge_anchors`, the orphan `section_offsets` retain -
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
 *    onto a restored section only from a LIVE section with the same id that
 *    already has one. That could not cover a snapshot that RE-CREATES a section
 *    the live project does not currently have - nothing to carry forward from, so
 *    the anchors the save captured were lost. (The `CreateSection` path never had
 *    the problem: its entry carries a `SectionIdentity` refreshed from the live
 *    section at undo time, finding 1. It was specifically the snapshot-pair path.)
 *    Fixed by making snapshot pairs SELF-REFRESHING WALLS: an undo re-records the
 *    entry's `after` half from the live project before restoring `before`, and a
 *    redo does the mirror, so pair undo/redo are exact inverses by construction
 *    rather than by re-healing afterwards.
 *
 *    Kept as the regression check, ddmin'd from fractal seed 1 x40 by
 *    `pnpm agent:fuzz-shrink` (43 steps -> 2, 1-minimal, no classifier involved):
 *
 *        1. executeBatch(revSec+crtSec+updShot)   <- pushes a ProjectSnapshotPair
 *                                                    whose `after` holds a NEW section
 *        2. save                                  <- captures that section's gauge pair
 *        (then the replayer's own unwind/rewind: undo to 0, redo back)
 *
 *        redo did not restore the state recorded at depth 2
 *          at project.sections[32].gauge_neighbour_uuid
 *            expected: "3178f6da-..."   actual: null
 *
 *    Six-move variant on naharon (seed 21 x60), a different command reaching the
 *    same place - `deleteShotRangeSplit` also produces a snapshot pair that
 *    re-creates a section: reverseSection, deleteShotRangeSplit, save+reload,
 *    updateShot, undo, undo, redo -> `sections[37].gauge_shot_uuid` null.
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
 *    `execute_delete_last_shot_and_section` converts orphan closures and
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
import { DEFAULT_FIXTURE, loadFixtureWithoutUndoHistory } from './common';
import { allowKnownBugs, dumpFullHistory, fuzzFixture } from '../lib/fuzzKnobs';
import {
  MoveLog,
  RERUN_WITH_IDS,
  failureText,
  openReplay,
  throwFailure,
  writeRunTrace,
} from '../lib/fuzzHarness';
import {
  assertHistoryStep,
  assertReplayableMutation,
  historyReach,
  readUndoPersistLimit,
  replayTrace,
  requireAutoSaveOff,
  undoFuzzHarness,
} from '../lib/undoFuzzHarness';
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
import { FILE_ARG_ACTIONS, TraceRecorder, type TraceStep } from '../lib/fuzzTrace';
import { foldLocalPaths, valueShape } from '../lib/shareableFailure';
import { initModel, injectOracle, type StepResponse } from './undoRedoFuzzBridge';

// The shared env knobs (`replayTracePath`, `traceOutPath`, `fuzzFixture`,
// `allowKnownBugs`, `dumpFullHistory`) live in `lib/fuzzKnobs.ts`, with the
// read-at-CALL-time rule they all follow. The knobs below are this
// scenario's own, and follow the same rule.

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

/**
 * Repair runs ONCE per run, after the main loop and before the boundary
 * save/reload, never inside the loop.
 *
 * `repair_open_project_integrity` drops dangling refs, re-mints duplicate shot
 * uuids, converts sole-bridge closures to merges and then does a zero-trust
 * `resolve_offsets()`. Unlike a save it PUSHES AN UNDO ENTRY whenever the
 * project actually changed - so firing it inside the loop would move the cursor
 * and shift the trajectory, which is exactly what every documented repro line
 * depends on not happening. After the loop it is still fully exercised: the
 * final unwind/rewind walks its entry in both directions like any other.
 *
 * It is not a no-op in practice - bug 2's leaked all-zero offset keys and
 * finding 10's orphan closures are both things repair heals.
 *
 * OFF BY DEFAULT behind `STALAGMIGHT_FUZZ_REPAIR=1`, and it stays off - but for
 * a different reason than when the gate went in. It was added because Repair
 * surfaced finding 11, which is now root-caused and reproduces WITHOUT Repair,
 * so that justification is gone. The gate remains because Repair pushes an undo
 * entry whenever it changes the project, and that entry is walked by the final
 * unwind/rewind - so enabling it by default would add one undo and one redo to
 * every run's counts, changing the metrics the faithfulness checks compare
 * byte-for-byte. Opt in when hunting; leave off when verifying.
 */
function repairEnabled(): boolean {
  return process.env.STALAGMIGHT_FUZZ_REPAIR === '1';
}

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
/** Headroom kept below the undo history limit so FIFO trimming can never fire. */
const DEPTH_HEADROOM = 5;

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
  // Folded (#576 W51): a metric key prints in the runner's PASS line
  // unfolded, and the fixture is the knob as the user wrote it.
  ctx.setMetric(`fixture:${foldLocalPaths(fixture)}`, 1);
  ctx.setMetric('strippedUndoRows', strippedRows);

  await requireAutoSaveOff(ctx);

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

  // `replay` is non-null when this run is replaying a recorded trace instead of
  // drawing. `openReplay` refuses a trace recorded by another scenario, or on
  // another fixture: the recorded steps carry RESOLVED section and shot ids, so
  // replaying them against a different project drives moves at ids that do not
  // exist there - an invented failure, or a green run, either way not the
  // recorded bug.
  const { replayPath, traceOut, replay } = openReplay(undoRedoFuzz.name, fixture, RERUN_WITH_IDS);

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
  const persistLimit = await readUndoPersistLimit(ctx);

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
      throw new Error(`generated file path ${valueShape(rel)} escapes ${root}`);
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
        `${action} takes an input file path as args[0], got ${valueShape(rel)}`,
      );
    }
    return [resolveSessionPath(rel), ...args.slice(1)];
  };

  /**
   * The run's move history and the move RUNNING RIGHT NOW (`lib/fuzzHarness.ts`).
   *
   * A `note` line reports what a move DID to the stacks, so it can only be
   * written once the move RETURNS; `startingMove` arms the label before each
   * step and `finishedMove` disarms it after, so a move that throws is named
   * in the failure tail instead of the one before it. Rendered by `fail` only,
   * so a run that does not fail behaves exactly as before and the trace is
   * untouched.
   */
  const log = new MoveLog();
  const { note, startingMove, finishedMove } = log;

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
  const dumpTrace = (failure: { message: string } | null, explicitPath: string): string | null =>
    writeRunTrace(
      ctx,
      recorder,
      { scenario: undoRedoFuzz.name, iterations, fixture, stream: streamName() },
      failure,
      explicitPath,
    );

  // Annotated on the CONST, not just the arrow: that is what makes TypeScript
  // treat a `fail(...)` call as never-returning, so a caller ending in one does
  // not need a dead `return` after it.
  const fail: (headline: string, detail?: string) => never = (headline, detail = '') => {
    const context = [
      `  reproduce: run_scenario("undo-redo-fuzz", seed=${ctx.seed}, iterations=${iterations})` +
        `${fixture === DEFAULT_FIXTURE ? '' : ` with STALAGMIGHT_FUZZ_FIXTURE=${fixture}`}` +
        `${streamClause()}` +
        `${allowKnownBugs() ? ' with STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS=1 (INERT: nothing left to allow)' : ''}` +
        `${replayPath ? ` (REPLAY of ${replayPath})` : ''}`,
      `  fixture=${fixture} stream=${streamName()} swarm=${swarm.name} ` +
        `depth=${cursor} modelTop=${top} baseUndo=${baseUndo}`,
    ];
    // The tail ends on the move that had not RETURNED, if any - without that it
    // would end on the move BEFORE the one that failed.
    const tail = log.tail(HISTORY_TAIL, dumpFullHistory());
    throwFailure(failureText(headline, context, detail, 'action(s)', tail), (failure) =>
      dumpTrace(failure, traceOut),
    );
  };

  const record = (res: StepResponse): void => {
    undoLen = res.undoLen;
    redoLen = res.redoLen;
    digest = res.digest;
    maxBytes = Math.max(maxBytes, res.bytes);
    deepest = Math.max(deepest, cursor);
  };

  /**
   * The fused step, the model / invariant / stack / integrity assertions, the
   * epoch-attributed mismatch resolution and the save/reload boundary, shared
   * with undoPersistenceFuzz (`lib/undoFuzzHarness.ts`). They read and write
   * THIS run's model through the view passed here, so `cursor`, `top` and the
   * counters stay the variables the rest of this body reads and writes.
   */
  const {
    step,
    assertModel,
    assertInvariants,
    assertStacks,
    assertIntegrityDelta,
    resolveMismatch,
    saveAndReload: crossBoundary,
  } = undoFuzzHarness(
    ctx,
    {
      get cursor() {
        return cursor;
      },
      get top() {
        return top;
      },
      set top(value) {
        top = value;
      },
      get undoLen() {
        return undoLen;
      },
      get savesDone() {
        return savesDone;
      },
      set savesDone(value) {
        savesDone = value;
      },
      get reloadsDone() {
        return reloadsDone;
      },
      set reloadsDone(value) {
        reloadsDone = value;
      },
    },
    {
      fail,
      log,
      record,
      baseUndo,
      limits: { diffLimit: DIFF_LIMIT, shownDiffs: SHOWN_DIFFS },
      noteFailedSave: true,
    },
  );

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
    const resynced =
      res.verdict !== 'equal' &&
      (await resolveMismatch(
        `${label} did not restore the state recorded at depth ${cursor}`,
        res,
      ));
    assertIntegrityDelta(res, label, resynced);
    record(res);
    ctx.metric(direction === 'undo' ? 'undos' : 'redos');
    if (jumped) {
      ctx.metric('jumps');
      ctx.metric(`jump:${action}`);
      ctx.metric('jumpSteps', steps);
    }
  };

  /**
   * Save, close, reopen at this run's one save path: the persistence boundary.
   * The three oracles it runs (the save is attributable, the stacks survive, the
   * restored stack is the same history) are documented on
   * `undoFuzzHarness`'s `saveAndReload`; the run's final unwind/rewind then walks
   * the whole RESTORED stack against the snapshots recorded before the save.
   */
  const saveAndReload = (tag: string): Promise<void> => crossBoundary(tag, savePath);

  /**
   * Run Repair as an additive move. It is a normal mutation as far as the
   * model is concerned - it either pushes one snapshot-pair entry or nothing -
   * so the same case analysis the mutate branch uses applies here.
   */
  const runRepair = async (tag: string): Promise<void> => {
    const beforeUndo = undoLen;
    const beforeRedo = redoLen;
    startingMove(`${tag} repairProjectIntegrity`);
    const res = await step('mutate', 'repairProjectIntegrity', []);
    finishedMove();
    const accepted = res.actionOk && res.commandSuccess !== false;
    const pushed = res.undoLen - beforeUndo;
    note(
      `${tag} repairProjectIntegrity -> ${accepted ? 'ok' : 'rejected'} ` +
        `(undo ${beforeUndo}->${res.undoLen}, redo ${beforeRedo}->${res.redoLen})`,
    );
    assertInvariants(res, 'repairProjectIntegrity');
    if (!accepted) {
      ctx.metric('rejected');
      ctx.metric('rejected:repairProjectIntegrity');
      record(res);
      return;
    }
    if (pushed === 1) {
      // Repair changed the project: it becomes the new top of the branch.
      snapshotsTopChanged(res);
      ctx.metric('repairs');
      record(res);
      return;
    }
    if (pushed !== 0) {
      fail(`repair pushed ${pushed} undo entries (expected 0 or 1)`);
    }
    // No-op repair: nothing changed, so nothing may have changed.
    ctx.metric('repairNoops');
    if (res.redoLen === 0) top = cursor;
    if (res.verdict !== 'equal') {
      await resolveMismatch('a no-op Repair changed the project state', res);
    }
    record(res);
  };

  /** Shared bookkeeping for a move that pushed exactly one undo entry. */
  const snapshotsTopChanged = (res: StepResponse): void => {
    if (res.redoLen !== 0) {
      fail('a successful Repair did not clear the redo stack');
    }
    cursor += 1;
    top = cursor;
    assertModel(res, 'Repair recorded its snapshot at an unexpected depth');
    if (res.verdict !== 'recorded') {
      fail(`the page did not record a snapshot for Repair (verdict=${res.verdict})`);
    }
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
      const resynced =
        res.verdict !== 'equal' &&
        (await resolveMismatch(
          `a REJECTED command (${action.action}) mutated the project: ` +
            `${res.commandError ?? res.actionError ?? 'unknown'}`,
          res,
        ));
      assertIntegrityDelta(res, `a rejected ${action.action}`, resynced);
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
    const resynced =
      res.verdict !== 'equal' &&
      (await resolveMismatch(
        `a no-op command (${action.action}) changed the project state`,
        res,
      ));
    assertIntegrityDelta(res, `a no-op ${action.action}`, resynced);
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
      // Only the mutations a generator can produce (see assertReplayableMutation).
      assertReplayableMutation(traceStep.action);
      await applyMutation(
        { action: traceStep.action, args: traceStep.args, label: traceStep.label },
        tag,
      );
      return;
    }
    if (traceStep.kind === 'repair') {
      recorder.record({ kind: 'repair' });
      await runRepair(tag);
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
    // Anything else is a UI step this store-driven scenario cannot run
    // (see assertHistoryStep); a history move that cannot move is skipped.
    assertHistoryStep(traceStep, undoRedoFuzz.name);
    const moved = historyReach(ctx, traceStep, cursor, top);
    if (moved === null) return;
    const { back, reach } = moved;
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

  if (replay) await replayTrace(ctx, replay, applyTraceStep);

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

  // Repair once, before the boundary crossing, so its entry is both persisted
  // and walked by the final roundtrip. Skipped on a replay: the trace already
  // contains whatever Repair the recorded run did, in the place it did it.
  if (repairEnabled() && !replay) {
    recorder.record({ kind: 'repair' });
    await runRepair('repair');
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
