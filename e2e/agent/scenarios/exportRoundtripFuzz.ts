/**
 * export-roundtrip-fuzz: a seeded random walk over EVERY export flow, plus the
 * round trip back in.
 *
 * `import-dialog-fuzz` drives files INTO the app. This one drives them out, and
 * then back in again: it draws a format, a route, a destination-path shape and
 * an option set, exports through the real surface, and then asserts that what
 * the app reported, what is on disk and what the user was told all agree - and
 * that a survey which leaves as a `.tml` and comes back is still the same
 * survey.
 *
 * ## Three routes, and why all three
 *
 * `ExportControls` is ONE component rendered twice: the modal Export dialog
 * (local `useState`) and the Export-mode panel (`exportStore`). The third route
 * is the raw store action, which is what the IPC boundary looks like. They are
 * not interchangeable and the differences are the point:
 *
 *   - the UI routes push the answered path through `ensureExtension`, so a
 *     destination the user typed is REWRITTEN before the command sees it;
 *   - the store route hands the path over verbatim, which is the only way the
 *     backend's own guards are ever reached;
 *   - only the UI routes can raise a toast, so only they can tell the user
 *     anything.
 *
 * Each route is asserted against ITS OWN contract rather than forced to agree
 * move-for-move with the others - the header of `checkProjectFileSurvived` in
 * `lib/exportOracles.ts` states where that matters most.
 *
 * ## The shape of a run
 *
 *   EPISODE (= one iteration)
 *     open a fresh copy of the fixture with NO inherited undo history
 *     turn auto-save off
 *     K = randInt(1, STALAGMIGHT_FUZZ_EXPORT_MOVES) export moves, each into its
 *       OWN directory so a stray file is attributable to the move that made it
 *     a rotation of the structural oracles (see below)
 *     teardown: close, assert nothing survived, delete this episode's files
 *
 * The structural oracles run on a ROTATION keyed by the episode index rather
 * than every episode, because three of them create projects and import files
 * and one rasterizes the survey three times. At the default 30 iterations every
 * one runs ten or more times; at `--iterations 2` every one runs at least once.
 *
 * ## What is asserted (all in `lib/exportOracles.ts`, all throwing)
 *
 *   X1  file/result agreement: success => a non-empty file; failure => no file
 *       AND, on a UI route, a visible report. No stray artifacts either way.
 *   X2  refusal parity: the open project's `.stalx` is byte-for-byte untouched
 *       by every attempt to export over it, and the RAW command refuses it.
 *   X3  svg/png/pdf differential: the SVG succeeding while the raster or the
 *       PDF fails on the same options means malformed markup the SVG path
 *       cannot notice, because only it has no parse gate.
 *   X4  byte identity: three `.tml`/`.tmlu` exports of an unmutated project.
 *   X5  convergence: T1 -> fresh project -> T2 -> fresh project -> T3, and
 *       T2 == T3.
 *   X6  shot multiset + uuid identity across the first re-import, and no
 *       surviving duplicate uuid after importing the same file twice. X6c:
 *       per shot by uuid, every station at its true depth except an averaged
 *       junction (the mean of one reading per section), the length bound, and
 *       the plan (`junctionDepthOracle.ts`).
 *   X7  hostile comments survive a CSV export neutralised, and a TML export
 *       intact.
 *   X8  the modal's options and the panel's do not leak into each other, and
 *       the panel's reset on a project switch.
 *   X9  Export mode forces closure-adjusted, read-only geometry and gives the
 *       previous state back.
 *
 * Per EPISODE: `check_project_integrity` after every oracle that imported, and
 * a teardown that ASSERTS rather than merely performs - `fuzz-shrink` reuses
 * ONE live session across ddmin candidates, so a candidate that leaves a
 * project open or a file on disk poisons the verdict of every candidate after
 * it, and ddmin would minimize toward the leak rather than toward the bug.
 *
 * ## Impossible moves are skipped and counted, never asserted
 *
 * Every driver in `exportFuzzActions.ts` returns a reason instead of throwing.
 * `diveProfile` is the one that skips most: it needs a computed gas route, and
 * nothing in this scenario builds one, so on the tracked fixtures it counts
 * `skip:no-dive-route` on every draw. That is DECLARED coverage rather than
 * pretended coverage - the gate is the real one (`buildRouteProfile`'s
 * predicate, which is what greys the button), so the day a fixture or a
 * sibling scenario leaves a plan behind, the moves start running.
 *
 * ## Record and replay
 *
 * A RUN IS RECORDED AS ITS WHOLE STRUCTURE, not just its clicks. Per episode
 * the trace carries a `reload` step, one `export` step per move (route, path
 * shape, option set), and one `oracle` step per structural oracle the rotation
 * selected - each carrying whatever that oracle DREW. `pnpm agent:scenario
 * export-roundtrip-fuzz --trace FILE` walks that list and dispatches it through
 * the same drivers and the same `runOracle` table the recording used, so the
 * replay runs the same oracles, at the same points, on the same inputs.
 *
 * BOTH HALVES OF THAT WERE WRONG BEFORE v4, and each on its own could turn a
 * red trace green:
 *
 *   - `reload` steps were IGNORED. The replay loaded the fixture once and then
 *     ran every episode's moves in a row, so episode 2 exported whatever
 *     episode 1 had left in the project rather than a fresh copy. A
 *     multi-episode failure could vanish, or land somewhere else and be
 *     "reproduced" as a different bug. A reload now ends the previous episode -
 *     teardown included, which is the part that ASSERTS nothing leaked - and
 *     opens the next.
 *   - THE ORACLES RE-DREW. X3 draws an image-option set and X4 draws a
 *     container extension, both off `ctx.rng`. A replay rebuilds its moves from
 *     the trace, so it consumed none of the draws the recording's move
 *     generation had, and every structural oracle then ran ONCE, at the end, on
 *     options from a differently-advanced rng. An option-dependent X3 or X4
 *     failure was therefore retested with options that do not provoke it. The
 *     inputs are now recorded and replayed, which is the same rule this harness
 *     already applies to moves (`fuzzTrace.ts`, "Why a trace and not just a
 *     seed"): a replay needs neither the generators nor the PRNG.
 *
 * A replay consumes NO randomness at all as a result, so it cannot drift from
 * the generators - and the generation side is unchanged, because recording is
 * still observe-only: the same draws happen in the same order, they are simply
 * made by the caller and handed in rather than made inside the oracle.
 *
 * A trace never carries an absolute path: every `rel` is session-relative, and
 * `readTrace` refuses one that is not. An `oracle` step's name is checked
 * against this scenario's own list, the way a `ui` step's testid is checked
 * against the allowlist.
 *
 * ## Knobs (all read at CALL time, so a replay's restored env reaches them)
 *
 *   STALAGMIGHT_FUZZ_FIXTURE              which `.stalx` to open
 *   STALAGMIGHT_FUZZ_EXPORT_FORMATS       csv list, default all nine
 *   STALAGMIGHT_FUZZ_EXPORT_MAX_DPI       default 600
 *   STALAGMIGHT_FUZZ_EXPORT_ROUNDTRIP     `0` disables the X5/X6 chains
 *   STALAGMIGHT_FUZZ_EXPORT_MOVES         export moves per episode, default 3
 *   STALAGMIGHT_FUZZ_EXPORT_SETTLE_MS     default 20000
 *   STALAGMIGHT_FUZZ_TRACE / _REPLAY      record / replay a trace
 *
 * ## Probe results (measurements, not assertions)
 *
 * These are the two the wave-4 brief asked to MEASURE rather than assert, and
 * what the measurements said.
 *
 * THE `.plnx` x IMAGE ASYMMETRY WAS REAL, AND IS NOW FIXED (commit ca0b7d85).
 * `commands/export.rs` had two guards: `is_open_project_file`, which knows only
 * the `.stalx`, and `is_project_owned_file`, which also covers the `.plnx`
 * dive-plan sibling. The text exports and `export_dive_profile` used the wide
 * one; `export_image` used the narrow one. So a PNG/SVG/PDF aimed at
 * `<project>.plnx` was WRITTEN, taking every saved dive plan for that project
 * with it - while the doc comment on `is_project_owned_file` already claimed
 * the gap was closed. Not reachable through the save dialog (the frontend
 * forces the image extension), reachable at the IPC boundary, which is the
 * boundary the guard exists for. Found by reading the guard table while
 * designing X2, pinned by
 * `test_export_image_refuses_to_overwrite_the_plnx_sibling` (confirmed
 * non-vacuous: with the guard reverted it fails on `result.is_err()`, i.e. the
 * export succeeded). The probe remains a METRIC here - `x2:plnx:*` - because a
 * guard's REACH is a judgment about which files an export may touch, and this
 * scenario reports what it measured rather than legislating it.
 *
 * ONE MORE THING THE `.plnx` PROBE MEASURED, which is by design and worth
 * writing down: a `.plnx` that does not exist yet is NOT refused, because
 * `is_open_project_file` canonicalizes `existing` and gives up when that fails.
 * So the probe has to create a sentinel first, and it deletes it again
 * immediately - the metric would otherwise read `written` on every run and mean
 * nothing.
 *
 * `ExportSettings` CONFIG DEFAULTS ARE INERT, and always were. `ExportConfig`
 * (`src-tauri/src/config/types.rs:1302-1309`) carries `default_image_format`,
 * `default_dpi`, `default_include_satellite`, `default_include_grid` and
 * `default_include_labels`, all settable in the Settings dialog and persisted.
 * NO export path reads any of them: `DEFAULT_IMAGE_OPTIONS`
 * (`src/store/exportStore.ts`) hardcodes its own literals and says it mirrors
 * the backend's `ExportImageOptions::default`, which is a different struct, and
 * `ExportDialog.tsx` never touches `config`. Two of the five have no analogue
 * in `ExportImageOptions` at all (`_include_satellite`, `_include_grid`), and
 * one DISAGREES with the shipped default: the config says
 * `default_include_labels: true` while `show_station_labels` and
 * `show_section_names` both default `false`. Metered as
 * `x8:configDefaultsHonored` (0 or 1) rather than asserted, because "should a
 * settings default seed the export form" is a product question, not a
 * correctness one. Recorded here so the answer is a decision rather than an
 * oversight.
 *
 * ## Findings
 *
 * The RECORD, in `importDialogFuzz.ts`'s conventions: every repro line stays
 * runnable, a fixed finding names its commit and becomes that fix's regression
 * check, and no classifier or tolerance is ever added to turn a red green.
 *
 * 1. FIXED (a15cd3bb). A FAILED EXPORT WAS INVISIBLE.
 *    `ExportControls.handleExport`'s catch was a bare `console.error`, so every
 *    reachable refusal - the project-owned guard, `export_png`'s DPI-zero and
 *    16384px bails, a `Tree::from_str` failure, any I/O error - left the modal
 *    sitting there with its button un-greyed and the panel doing nothing at
 *    all, which is indistinguishable from "the file was written". Nothing else
 *    covered it: the four `projectStore` export actions do not go through
 *    `executeCommand`, so `TOAST_KEYS.COMMAND_FAILED`'s catch-all never fires
 *    for them. X1's third claim is now that fix's regression check.
 *
 *        pnpm agent:scenario export-roundtrip-fuzz --seed 2 --iterations 30
 *
 * 2. FIXED (c2648a53). THE EXPORT-MODE OPTIONS OUTLIVED THEIR PROJECT.
 *    `exportStore` is app-global and had three setters and nothing else, so a
 *    600-DPI PDF framed for one survey stayed selected when the next opened,
 *    with the Export-mode preview rendering a different cave at settings nobody
 *    chose for it. `setAppMode` says explicitly that it does not reset it, and
 *    neither `loadProject`, `createProject` nor `closeProject` did either.
 *    X8b is that fix's regression check.
 *
 * 3. FIXED (ca0b7d85). AN IMAGE EXPORT COULD CLOBBER THE OPEN PROJECT'S
 *    `.plnx`. See the probe write-up above; the Rust regression test is the
 *    durable form of this one, because the harness probe is deliberately a
 *    metric.
 *
 * 4. RESOLVED. A REAL SURVEY DID NOT ROUND-TRIP THROUGH `.tmlu`: 78 SHOT UUIDS
 *    WERE RE-MINTED AND EVERY ZERO LRUD CAME BACK NULL. Half A is FIXED
 *    (6a640872); half B is INTENDED and now documented. This is that fix's
 *    regression check - a red here again is a NEW bug.
 *
 *        STALAGMIGHT_FUZZ_FIXTURE=naharon.stalx \
 *          pnpm agent:scenario export-roundtrip-fuzz --seed 1 --iterations 8
 *
 *    Found on the FIRST run against a fixture other than the default. X6 on a
 *    532-shot cave, exported to `.tmlu` and imported into a fresh project,
 *    reported two signals with nothing else between them.
 *
 *      a. FIXED (6a640872). THE EXPORTER OMITTED THE UUID ON THE REVERSED
 *         PRE-ZERO-DATUM LEG. Measured, not inferred. In the fixture's own
 *         database all 532 shots have a uuid, all 532 are DISTINCT project-wide,
 *         and the longest is 36 characters - so neither `shot_export_uuid`'s
 *         absent-uuid mint (`export/tml.rs:556-561`) nor `remint_duplicate_uuids`
 *         (`:588-596`) could fire, and neither was involved. The exported `.tmlu`
 *         held 538 `<SRVD>` blocks of which only 454 carried a `<UU>` element;
 *         the 84 without were 1 START, 5 CLOSURE, 74 REAL and 4 VIRTUAL. START
 *         and CLOSURE are synthetic and have no project shot behind them, so
 *         74 + 4 = exactly the 78. The file contained ZERO uuids absent from the
 *         database and ZERO repeats, which ruled out any export-side mint. They
 *         came from the three `uuid: None` constructors at `export/tml.rs`'s
 *         `reverse_shots` / `rev_shots` - the reversed "(pre-zero-datum)" and
 *         merge legs - whose `shot_type` is
 *         `if shot.kind.is_virtual() { VIRTUAL } else { REAL }`, which is the
 *         74/4 split. The re-mint then happened on IMPORT and correctly:
 *         `foreign_shot_uuid` got nothing, and `Shot::with_uuid` fell back.
 *
 *         Fixed by giving the reversed legs `shot_export_uuid(shot)` like their
 *         forward siblings, because a reversed leg is not a synthetic shot - it
 *         is a real shot emitted with its heading and LRUD mirrored. START,
 *         CLOSURE and the VIRTUAL padding shot stand for no project shot and
 *         stay bare. The knock-on was checked rather than assumed: a shot cannot
 *         appear twice in a document (every constructor pairs its reversed block
 *         with a forward block over a DISJOINT index range, or replaces the
 *         forward block entirely), so `remint_duplicate_uuids` cannot read a
 *         forward/reverse pair as a duplicate. Four Rust tests pin it, including
 *         the SpeleoDB sidecar's byte-determinism.
 *
 *      b. INTENDED, and now documented rather than asserted against. THE ZERO /
 *         NULL SENTINEL COLLISION, per FIELD. Export writes
 *         `shot.left.unwrap_or(0.0)` (`export/tml.rs:744-747` and its two
 *         siblings) because Ariane's format has no null; import maps any field
 *         with `abs() < f64::EPSILON` back to `None`
 *         (`parsers/tml/mod.rs:1908-1914`, applied to L, R, U and D
 *         independently). So `0.0` IS the format's absent sentinel: an
 *         unmeasured dimension round-trips and a MEASURED zero does not.
 *         Direction confirmed from the database - naharon holds 261 all-null and
 *         271 all-zero shots and NO shot with a non-zero LRUD, so the originals
 *         are measured zeros and it is the import that nulls them. 271 x 2 = 542
 *         was exactly the geometry-key count X6 reported.
 *
 *         X6 now folds that ONE equivalence, through `lrudSentinel`, citing
 *         `docs/ariane-export.md#the-lrud-zeronull-collision` - which states the
 *         collision, why neither end can be "fixed" without inventing a spelling
 *         Ariane does not define, and the measured scale of it. The distinction
 *         from a tolerance is the point and the unit tests pin it: a zero
 *         becoming 3.5, a 3.5 becoming null, or a 3.5 becoming 3.6 all still
 *         fail.
 *
 *    A AND B WERE NOT THE WHOLE FINDING, AND THE CLAIM THAT THEY WERE WAS MINE
 *    AND WRONG. It rested on 542 being exactly 2 x 271, read as "every geometry
 *    key is one LRUD pair". With the sentinel folded, 198 keys remain - so some
 *    of those 542 were shots whose LRUD flipped AND whose geometry moved, and
 *    folding one revealed the other. The lesson is the arithmetic: a multiset
 *    delta counts KEYS, and a key can differ for two reasons at once, so a count
 *    that matches one explanation does not exclude a second.
 *
 *    WHAT IS LEFT IS FINDING 5, AND naharon IS STILL RED. Fixed and documented,
 *    a and b no longer fire: the uuid half is gone from X6's output entirely and
 *    the LRUD half is folded. See finding 5 for what the 198 are.
 *
 *    What a and b did settle: the 261 all-null shots round-trip, X5 converges on
 *    RAW bytes throughout, and neither half would have gone green under a
 *    narrower oracle - all 532 shots held a unique uuid, so a
 *    "unique-uuid-only" check would have excluded nothing.
 *
 * 5. MEASURED AND SPLIT. The 198 residual geometry keys of finding 4 were three
 *    families, not one. Two are closed; one is OPEN and needs a product call.
 *
 *        STALAGMIGHT_FUZZ_FIXTURE=naharon.stalx \
 *          pnpm agent:scenario export-roundtrip-fuzz --seed 1 --iterations 8
 *
 *    Classified by dumping both digests and matching them offline, so the counts
 *    below are exact rather than sampled. 99 keys vanished and 99 appeared.
 *
 *      a. CLOSED - ORACLE FIX. 75 shots, 150 of the 198 keys, were EXACT
 *         RECIPROCAL PAIRS: same tape, heading through 180, the two depths
 *         swapped, left and right swapped with them. Every one matched under
 *         that transform with nothing left over. They are the reversed legs of
 *         finding 4a arriving reversed, which is the exporter working as
 *         designed - a `.tml` cannot say "surveyed outward from here", so a
 *         section surveyed away from its datum is written walking back toward
 *         it. X6's key was DIRECTION-BEARING and called one leg a lost shot and
 *         a gained shot. It is now direction-agnostic (`shotKey` orients each
 *         shot to the lexicographically smaller of itself and its reciprocal),
 *         which is a fact about the key rather than a fold: the transform is
 *         exact, and a heading off by one degree, an unswapped depth pair, a
 *         changed length or an LRUD that is not the mirror all still fail.
 *
 *      b. CLOSED - ORACLE FIX, and the measurement settled it rather than a
 *         judgment call. 24 shots, the remaining 48 keys. Matched all 532 shots
 *         BY UUID - only possible because 4a made uuids survive the trip - and
 *         of those 24: LENGTH changed on 0, AZIMUTH on 0, LRUD on 0, and
 *         DESCENT on exactly 1. Absolute depths moved by 0.01 to 7.16 m across
 *         18 distinct offsets, in runs sharing one value.
 *
 *         That is precisely `enforce_ariane_geometry_invariants`
 *         (`export/tml.rs:1608`) doing the two things its own test family names:
 *         RE-HANGING a branch whose junction depth Ariane cannot represent, and
 *         moving the rest of the branch with it. A round trip is entitled to
 *         change where a branch HANGS. It is not entitled to change what a shot
 *         MEASURES, and it did not.
 *
 *         THE ONE DESCENT CHANGE IS THE CLAMP, AND THE SOURCE SHOT IS
 *         IMPOSSIBLE. Exactly one shot in the whole survey has |descent| greater
 *         than its tape: a 0.150 m shot carrying a 2.350 m depth change. That is
 *         not a survey the world contains - it is a data-entry error the fixture
 *         happens to hold - and the exporter pins it to exactly -0.150, i.e. to
 *         the tape, which is `a_shot_exactly_on_the_bound_is_left_alone_and_one_
 *         past_it_is_clamped`. It is the only shot in the file the clamp
 *         touches.
 *
 *         So X6 now keys the DESCENT, clamped to the tape, and not the absolute
 *         depths - asserting exactly what the transform contracts to preserve
 *         and dropping the one thing it contracts to change. `clampedDescent`
 *         carries the argument and the numbers. Absolute-depth agreement is kept
 *         as the `x6:absoluteDepthShifts` metric (`strictShotKey`) so a change
 *         in how often the exporter re-hangs stays visible instead of silently
 *         not mattering.
 *
 *         SUPERSEDED (2026-11, #576 TML-DEPTH-D1, Q1-D1, Q2-D1): the exporter
 *         no longer re-hangs or clamps. A junction whose readings disagree is
 *         written at the mean of one reading per section, every other station
 *         at its true depth, and the shots touching the junction take the
 *         length that keeps their run - so their descent and tape change. A
 *         shot whose drop exceeds its tape keeps both depths and goes out
 *         vertical, its drop as its length. A descent key would report those as
 *         lost and gained, so X6 now keys heading and LRUD only, and X6c
 *         (`junctionDepthOracle.ts`) asserts the rule per shot by uuid.
 *         `clampedDescent`, `strictShotKey` and the absolute-depth metric are
 *         gone; `x6:averagedJunctions`, `x6:refittedShots` and
 *         `x6:brokenShots` count what the rule changed.
 *
 *    NET: X6 IS GREEN ON naharon. 198 geometry keys -> 0, and no comment losses.
 *
 *    ONE CORRECTION TO (c) THAT THE FIX ITSELF PRODUCED. Keeping comments on
 *    shots moves the blind spot rather than closing it: the AFTER side now sees
 *    66 texts that were station comments before the trip and shot comments
 *    after, so they read as GAINS. A gain is not data loss and nothing supports
 *    asserting it, so X6 asserts LOSSES only - every comment the before side can
 *    see must still be there - and counts the gains as
 *    `x6:commentTextsGained`. Making it symmetric needs `station_comments` on
 *    the wire, which `ProjectIpc` does not put there.
 *
 * 6. CLOSED - THE ORACLE'S PREMISE WAS OFF BY ONE PASS. NOT A BUG. X5 asserted
 *    `T2 == T3`; on a real survey the chain settles at T3 == T4 instead. This is
 *    now that correction's regression check.
 *
 *        STALAGMIGHT_FUZZ_FIXTURE=naharon.stalx \
 *          pnpm agent:scenario export-roundtrip-fuzz --seed 1 --iterations 8
 *
 *    NEWLY REACHABLE rather than new: X6 runs before X5 in the chain, so every
 *    earlier naharon run died at X6 and X5 had never executed on this fixture.
 *    Closing findings 4 and 5 is what let it run.
 *
 *    MEASURED, by parsing T1..T4 rather than diffing bytes, matching shots by
 *    uuid:
 *      - T3 and T4 are BYTE-IDENTICAL. The chain converges; it does not churn.
 *      - T2 and T3 hold the same 40 sections, the same NAME SET, and the same
 *        MEMBERSHIP - ZERO shots changed section between them.
 *      - Setting aside the Ariane `<ID>`/`<FR>` node numbers, which renumber
 *        with position, T2 and T3 have an IDENTICAL BLOCK MULTISET.
 *    So no boundary moved, nothing was lost or gained, and the only thing that
 *    differs is the ORDER the sections are written in.
 *
 *    THE MECHANISM. The first import does not DERIVE its section order - it
 *    inherits it from the source project, which for T1 is the fixture's own.
 *    The second import derives it from a file the exporter wrote, and from then
 *    on the composition maps that order to itself. So the trip needs one extra
 *    pass to reach its own fixed point, and reaches it. It was NOT the suspected
 *    map-iteration nondeterminism: that would make T3 != T4 as well, and T3 and
 *    T4 are the same bytes.
 *
 *    THE FIX IS THE ORACLE'S. X5 now asserts a fixed POINT rather than a fixed
 *    INDEX - it runs the chain until two consecutive exports match, up to
 *    `MAX_ROUNDTRIP_EXPORTS`, and reports `x5:settledAt:N`. That is the same
 *    claim correctly stated and it is not weaker: a chain that really never
 *    converged fails, and so does one that loses or gains anything, because a
 *    loss makes every later pass differ too. Measured: the synthetic fixture
 *    settles at 2, naharon at 3, every episode.
 *
 * ## Observations that are NOT findings
 *
 * A SUCCESSFUL export from the PANEL says nothing at all. The modal closes,
 * which is its report; the panel just stays there. `docs/notifications.md`
 * lists "export results" as a toast case, and the decision-tree question -
 * "outcome already visible where the user is looking?" - has different answers
 * for the two surfaces. This scenario does not report it, because the wave-4
 * brief scoped the notification work to FAILURES and a success toast is a
 * product decision about how chatty the panel should be, not a defect. It is
 * written down here so the asymmetry is a decision rather than an oversight.
 *
 * A SYNTHETIC FLOATING SECTION DOES NOT SURVIVE THE TML ROUND TRIP INTACT, and
 * this scenario measures it without claiming to have characterised it. X7 builds
 * a 14-shot chain with `createSection` - unconnected, unanchored - and when the
 * rotation put it in the same episode as X5/X6, the re-imported copy came back
 * with every depth shifted by a constant 7 and every comment gone (seed 1,
 * episode 3, both counts exact). The depth shift is the exporter re-rooting an
 * orphan chain (`ensure_rooted_chain`), and the comments are somewhere between
 * the exporter's U+200B co-encoding and the importer's `comment_split::split` -
 * `docs/ariane-export.md` already tabulates several comment-dropping cases
 * around the START shot, and this may be one of them or may not.
 *
 * IT IS NOT REPORTED AS A FINDING, because the input is one this scenario
 * fabricated rather than one a user can make: `createSection` builds a chain
 * that no import or draw flow produces in that state, and the same argument
 * `importDialogFuzz.ts` makes for `uiStore.confirmPlacement()` applies - a bug
 * found by driving a store into a state no UI can reach is a fabricated bug.
 * X5/X6 therefore reload the pristine fixture first and ask their question of
 * the SURVEY, and X7 keeps the hostile comments where its claim is grounded:
 * at the file level, a .tmlu write followed by a `parse_tml` read.
 *
 * If a future wave wants this characterised, the repro is one line - drop the
 * reload at the top of `runRoundtrip` and run seed 1 with 10 iterations.
 *
 * `preview_svg_export` ALWAYS returns SVG, whatever `options.format` says
 * (`ExportPreview.tsx` passes the live options through unchanged, and the
 * command ignores the format field). So the PNG and PDF previews are the SVG
 * preview, and the preview cannot warn about a raster that will not rasterize.
 * That is what X3 is for, and it is a limitation of the preview rather than a
 * bug in it.
 */
import * as crypto from 'crypto';
import * as fs from 'fs';
import * as path from 'path';

import type { Scenario, ScenarioContext } from './framework';
import { disableAutoSave, loadFixtureWithoutUndoHistory } from './common';
import { HOSTILE_COMMENTS } from '../lib/surveyFileSpecs';
import { shotSetDelta, describeShotSetDelta } from '../lib/projectSnapshot';
import { TraceRecorder, type TraceStep } from '../lib/fuzzTrace';
import { fuzzFixture } from '../lib/fuzzKnobs';
import { detProbe } from '../lib/fuzzDeterminismProbe';
import { foldThenCut } from '../lib/fixtureCorpus';
import { valueShape } from '../lib/shareableFailure';
import { shareableTestId } from '../../../scripts/lib/shareable-selector.mjs';
import { MoveLog, openReplay, RERUN_WITH, writeRunTrace } from '../lib/fuzzHarness';
import {
  asExportedToTml,
  checkByteIdentity,
  checkConvergence,
  checkExportModeInvariants,
  checkExportStoreReset,
  checkFileResult,
  checkFormatDifferential,
  checkNoDuplicateUuids,
  checkOptionIndependence,
  checkProjectFileSurvived,
  checkShotMultiset,
  commentMultisetDelta,
  csvTextViolations,
  exportBaseName,
  ExportOracleError,
  domRouteRewritesPath,
  finalizeExportPath,
  multisetDelta,
  newToastKeys,
  naturalExtension,
  PROJECT_OWNED_SHAPES,
  type ExportAttempt,
  type ExportRoute,
  type OptionSnapshot,
  type PathShape,
  type SectionAzimuthReference,
  type ShotIdentity,
} from '../lib/exportOracles';
import {
  checkJunctionDepths,
  junctionDepthReport,
  type StationedShot,
  type StationMergeWire,
} from '../lib/junctionDepthOracle';
import {
  callProjectAction,
  clickAndAnswerSave,
  clickControl,
  closeModal,
  dataFormatValue,
  diveProfileRequest,
  drawExport,
  drawImageOptions,
  enabledFormats,
  hasDiveRoute,
  isDataFormat,
  isExportFormat,
  isImageFormat,
  modalIsOpen,
  openModal,
  readModalOptions,
  readModeSnapshot,
  readPanelOptions,
  readPreview,
  readToastSnapshot,
  roundtripEnabled,
  selectValue,
  setAppMode,
  setCheckbox,
  setThreshold,
  settleOnce,
  type ExportDraw,
  type ImageOptionDraw,
} from './exportFuzzActions';

const SCENARIO = 'export-roundtrip-fuzz';
/** Session-relative homes for what a run writes. Both are wiped per episode. */
const EXPORTS_REL = 'fuzz-exports';
const PROJECTS_REL = 'fuzz-export-projects';
/** Failure messages print this many trailing moves. */
const HISTORY_TAIL = 20;
/**
 * DPI the differential pins itself to.
 *
 * Low enough that the derived pixel dimensions cannot reach
 * `MAX_PNG_DIMENSION` on any fixture, so a PNG failure at this DPI cannot be
 * the size bail - which is a legitimate refusal of the raster path alone and
 * says nothing about the markup X3 is asking about.
 */
const DIFFERENTIAL_DPI = 72;
/**
 * The name every holding project in the X5 chain is created with.
 *
 * It reaches the export as `<caveName>`, so it must not vary between the two
 * projects T2 and T3 come out of - see `freshProjectWithImport`.
 */
const ROUNDTRIP_PROJECT_NAME = 'export-fuzz-roundtrip';
/**
 * How far the X5 chain runs before it calls a non-convergence.
 *
 * Measured: the synthetic fixture settles at the 2nd export and a real survey at
 * the 3rd (finding 6). Four gives that a pass of headroom without turning a
 * genuine never-settles into an expensive one - each pass is a project create
 * plus a full import.
 */
const MAX_ROUNDTRIP_EXPORTS = 4;
/**
 * `DEFAULT_IMAGE_OPTIONS` + the two scalar defaults, as `exportStore.ts`
 * declares them.
 *
 * Copied rather than imported, the same way `importDialogFuzz.ts` copies its
 * toast keys: the agent tier cannot follow an import into `src/`. The drift
 * risk is real and is covered from the other side - `exportStore.test.ts`
 * asserts the full `DEFAULT_IMAGE_OPTIONS` object literal, so a changed default
 * fails there first, loudly, rather than turning X8b vacuous here.
 */
const EXPECTED_DEFAULT_OPTIONS: OptionSnapshot = {
  exportType: 'image',
  dataFormat: 'csv',
  imageFormat: 'png',
  dpi: 300,
  branchingThreshold: 2,
};

// ---------------------------------------------------------------------------
// Knobs. Read at CALL time, never frozen at import. The fixture and the trace
// replay/record paths are the shared ones in `lib/fuzzKnobs.ts`.
// ---------------------------------------------------------------------------

function movesPerEpisode(): number {
  const raw = Number(process.env.STALAGMIGHT_FUZZ_EXPORT_MOVES);
  return Number.isInteger(raw) && raw > 0 ? raw : 3;
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

function sha256(bytes: Buffer): string {
  return crypto.createHash('sha256').update(bytes).digest('hex');
}

/** `fs.statSync().size`, or null when the file is not there. */
function sizeOf(target: string): number | null {
  try {
    const stat = fs.statSync(target);
    return stat.isFile() ? stat.size : null;
  } catch {
    return null;
  }
}

function hashOf(target: string): string | null {
  try {
    return sha256(fs.readFileSync(target));
  } catch {
    return null;
  }
}

function rmrf(target: string): void {
  fs.rmSync(target, { recursive: true, force: true });
}

/**
 * A canonical-snapshot-shaped document holding every uuid in ONE section.
 *
 * `shotSetDelta` reads `project.sections[].shots[].uuid` and answers per
 * section. A round trip is allowed to re-partition sections, so the flattening
 * is what makes its answer the project-wide identity question instead of a
 * restatement of the fragmentation.
 */
function flatUuidSnapshot(uuids: readonly string[]): string {
  return JSON.stringify({ project: { sections: [{ id: 0, shots: uuids.map((uuid) => ({ uuid })) }] } });
}

/** Everything in `dir` that is not one of `expected`. */
function straysIn(dir: string, expected: readonly string[]): string[] {
  let entries: string[] = [];
  try {
    entries = fs.readdirSync(dir);
  } catch {
    return [];
  }
  return entries.filter((name) => !expected.includes(name));
}

/** The project state, reduced in page to what the oracles need. */
interface ProjectDigest {
  open: boolean;
  sections: number;
  /** Each shot with its section id and index, which X6c reads. */
  shots: (ShotIdentity & StationedShot)[];
  /**
   * The station merges as `[sectionA, stationA, sectionB, stationB]`. X6c reads
   * the RE-IMPORTED project's: they are the file's `FromID` links, so they say
   * which stations the export wrote as one node.
   */
  merges: StationMergeWire[];
  /**
   * Each shot's section reference, PARALLEL to `shots`, resolved the way
   * `Project::azimuth_reference_for` does. X6 needs it to know what the TML
   * export does to a bearing - see `asExportedToTml`.
   */
  shotReferences: SectionAzimuthReference[];
  uuids: string[];
  /**
   * The same uuids, grouped BY SECTION.
   *
   * Shot-uuid uniqueness is a per-section invariant in this app
   * (`models/project.rs::ensure_unique_shot_uuids`, and the `(section_id, uuid)`
   * lookup cache), so X6b needs the grouping and X4's precondition needs the
   * flat list - they are different questions and both are carried.
   */
  uuidsBySection: string[][];
  /**
   * Every comment TEXT in the project, U+200B-split, pooled over shot comments
   * AND station comments.
   *
   * Pooled because the round trip moves text between the two by design - see
   * `shotKey`'s header - so the only honest question is whether the text is
   * still in the project at all.
   */
  commentParts: string[];
  /** Shots whose uuid is missing or empty. */
  missingUuids: number;
  /** Uuids that appear more than once ACROSS the whole project. */
  duplicateUuids: number;
  filePath: string | null;
}

/**
 * Reduce `get_project_state` IN PAGE.
 *
 * A fractal fixture's project state is megabytes of JSON and the oracles need a
 * few numbers per shot, so the reduction happens where the data already is. The
 * same argument `fuzzOracleCore.ts` makes for its injected snapshot.
 */
async function readProjectDigest(ctx: ScenarioContext): Promise<ProjectDigest> {
  return ctx.eval<ProjectDigest>(`
    const core = window.__TAURI__ && window.__TAURI__.core;
    const state = await core.invoke('get_project_state');
    const project = state && state.project ? state.project : state;
    if (!project || !Array.isArray(project.sections)) {
      return { open: false, sections: 0, shots: [], merges: [], shotReferences: [], uuids: [], uuidsBySection: [], commentParts: [], missingUuids: 0, duplicateUuids: 0, filePath: null };
    }
    // A section's azimuth reference, resolved as Project::azimuth_reference_for
    // does: the section's own, else the project default, else nothing recorded
    // (magnetic, 0, source unknown). Wire shape per src-tauri/bindings: north is
    // a lowercase string, a non-finite correction serializes as null (read as 0,
    // which is what applied_correction makes of it), and source is the string
    // "unknown" / "file_declared" / "manual" or an object such as
    // {"igrf_auto":{"epoch_year":2018}}. Only "unknown" (or no source) means no
    // correction is on record.
    const resolveReference = (raw) => {
      if (!raw || typeof raw !== 'object') return { north: 'magnetic', correction: 0, onRecord: false };
      const north = raw.north === 'true' || raw.north === 'grid' ? raw.north : 'magnetic';
      const correction = typeof raw.correction === 'number' ? raw.correction : 0;
      const source = raw.source;
      const onRecord =
        (typeof source === 'string' && source !== 'unknown') || (typeof source === 'object' && source !== null);
      return { north: north, correction: correction, onRecord: onRecord };
    };
    const shots = [];
    const shotReferences = [];
    const uuids = [];
    const uuidsBySection = [];
    const commentParts = [];
    const addComment = (text) => {
      if (typeof text !== 'string' || text === '') return;
      const parts = text.split('\u200b');
      for (let i = 0; i < parts.length; i++) if (parts[i] !== '') commentParts.push(parts[i]);
    };
    // The bearing the app draws and the Ariane export writes (#576 NF-AN5-1):
    // the circular mean of the two FORWARD readings, the same arithmetic as
    // the models crate's average_azimuth_of. A round trip collapses the pair
    // onto it, so keying either reading alone would report a moved shot.
    const wrap = (x) => ((x % 360) + 360) % 360;
    const drawnBearing = (a, b) => wrap(a + (wrap(b - a + 540) - 180) / 2);
    let missing = 0;
    const seen = Object.create(null);
    let duplicates = 0;
    for (const section of project.sections) {
      if (!section || !Array.isArray(section.shots)) continue;
      const sectionUuids = [];
      uuidsBySection.push(sectionUuids);
      const reference = resolveReference(
        section.azimuth_reference != null ? section.azimuth_reference : project.default_azimuth_reference,
      );
      for (let index = 0; index < section.shots.length; index++) {
        const shot = section.shots[index];
        shotReferences.push(reference);
        const uuid = typeof shot.uuid === 'string' ? shot.uuid : '';
        if (uuid === '') missing += 1;
        else {
          if (seen[uuid]) duplicates += 1;
          seen[uuid] = true;
          uuids.push(uuid);
          sectionUuids.push(uuid);
        }
        addComment(shot.comment);
        shots.push({
          // Carried but NOT keyed: shotKey reads only the geometry fields, so
          // this is inert for the oracle and is what lets a failure be matched
          // shot-by-shot by identity rather than by length. 5a is what made it
          // useful - before it, a round trip re-minted the uuids that matter.
          // (No backticks: this is inside a template literal.)
          uuid: uuid,
          section: Number(section.id),
          index: index,
          length: Number(shot.length) || 0,
          azimuthIn: drawnBearing(Number(shot.azimuth_in) || 0, Number(shot.azimuth_out) || 0),
          depthIn: Number(shot.depth_in) || 0,
          depthOut: Number(shot.depth_out) || 0,
          lrud: [
            shot.left == null ? null : Number(shot.left),
            shot.right == null ? null : Number(shot.right),
            shot.up == null ? null : Number(shot.up),
            shot.down == null ? null : Number(shot.down),
          ],
        });
      }
    }
    // Station merges, as StationMerge's wire shape (station_a / station_b, each
    // a section_id and a station_index). X6c reads the re-imported project's.
    const merges = [];
    for (const merge of (project.station_merges || [])) {
      const a = merge && merge.station_a;
      const b = merge && merge.station_b;
      if (!a || !b) continue;
      merges.push([Number(a.section_id), Number(a.station_index), Number(b.section_id), Number(b.station_index)]);
    }
    // Station comments: serde renders a HashMap with a struct key as an array of
    // [key, value] pairs, and the binary loader calls the same field
    // station_comments_map. Both shapes are read rather than assumed, because
    // reading the wrong one would silently report every station comment as lost
    // - which is the exact failure mode this pooling exists to avoid.
    //
    // (No backticks anywhere in this string: it is a template literal, and one
    // would close it. That mistake compiled as a TS syntax error 40 lines away.)
    const stationComments = project.station_comments || project.station_comments_map;
    if (Array.isArray(stationComments)) {
      for (const entry of stationComments) {
        if (Array.isArray(entry)) addComment(entry[1]);
      }
    } else if (stationComments && typeof stationComments === 'object') {
      for (const key of Object.keys(stationComments)) addComment(stationComments[key]);
    }

    return {
      open: true,
      sections: project.sections.length,
      shots: shots,
      merges: merges,
      shotReferences: shotReferences,
      uuids: uuids,
      uuidsBySection: uuidsBySection,
      commentParts: commentParts,
      missingUuids: missing,
      duplicateUuids: duplicates,
      filePath: typeof project.file_path === 'string' ? project.file_path : null,
    };
  `);
}

interface Integrity {
  messages: string[];
  save_blocked: boolean;
}

// ---------------------------------------------------------------------------

export const exportRoundtripFuzz: Scenario = {
  name: SCENARIO,
  description:
    'Exports CSV/JSON/TML/TMLU/PNG/SVG/PDF/preview through the modal, the Export-mode panel and the ' +
    'raw store action at hostile destination paths, asserting file/result agreement, refusal parity, ' +
    'a svg/png/pdf differential, byte identity, round-trip convergence and shot survival.',
  defaultIterations: 30,

  async run(ctx: ScenarioContext, { iterations }): Promise<void> {
    const fixture = fuzzFixture();
    const sessionDir = ctx.artifactsDir;
    if (!sessionDir) {
      throw new Error(`${SCENARIO} needs a session artifacts dir to write its exports into`);
    }
    const exportsDir = path.join(sessionDir, EXPORTS_REL);
    const projectsDir = path.join(sessionDir, PROJECTS_REL);

    ctx.log(`formats: ${enabledFormats().join(',')}`);
    ctx.setMetric('roundtripEnabled', roundtripEnabled() ? 1 : 0);

    // ---- replay wiring ---------------------------------------------------
    // `openReplay` refuses a trace recorded by another scenario or on another
    // fixture: the recorded moves resolve their paths against THAT project, so
    // replaying them on a different fixture drives exports of a different
    // survey - an invented failure, or a green run, either way not the recorded
    // bug.
    const { replayPath, traceOut, replay } = openReplay(SCENARIO, fixture, RERUN_WITH);

    const recorder = new TraceRecorder();
    const moveLog = new MoveLog();
    const { note } = moveLog;

    /**
     * The trace a failure should point at.
     *
     * A REPLAY records nothing - every step it runs came out of a file - so
     * writing one here would drop a zero-step artifact next to a real failure
     * and offer it as the thing to replay and shrink. The trace that reproduces
     * a replay's failure is the trace it was given, so that is what is named.
     */
    const dumpTrace = (failure: { message: string } | null): string | null => {
      if (replay !== null) return replayPath;
      return writeRunTrace(
        ctx,
        recorder,
        { scenario: SCENARIO, iterations, fixture, stream: 'export-v1' },
        failure,
        traceOut,
      );
    };

    /**
     * Annotated on the CONST so TypeScript treats a `fail(...)` call as
     * never-returning.
     */
    const fail: (message: string) => never = (message: string): never => {
      const tail = moveLog.history.slice(-HISTORY_TAIL);
      const tracePath = dumpTrace({ message });
      throw new Error(
        `${message}\n\n  last ${tail.length} move(s):\n` +
          tail.map((line, index) => `    ${index + 1}. ${line}`).join('\n') +
          (tracePath === null ? '' : `\n\n  trace: ${tracePath}\n  replay: pnpm agent:scenario ${SCENARIO} --trace ${tracePath}`),
      );
    };

    const reproduce = `reproduce: pnpm agent:scenario ${SCENARIO} --seed ${ctx.seed} --iterations ${iterations}`;

    /** Run an oracle, turning its throw into this scenario's failure shape. */
    const guarded = (fn: () => void): void => {
      try {
        fn();
      } catch (err) {
        if (err instanceof ExportOracleError) fail(err.message);
        throw err;
      }
    };

    const integrity = async (): Promise<Integrity> =>
      ctx.ipc<Integrity>('check_project_integrity').catch(() => ({ messages: [], save_blocked: false }));

    const assertIntegrity = async (after: string): Promise<void> => {
      const result = await integrity();
      if (result.save_blocked) {
        fail(`project integrity: SAVING IS BLOCKED after ${after}\n    ${result.messages.join('\n    ')}\n  ${reproduce}`);
      }
      const fatal = result.messages.filter((m) => /non-finite|NaN|infinite/i.test(m));
      if (fatal.length > 0) {
        fail(`project integrity reported non-finite geometry after ${after}\n    ${fatal.join('\n    ')}\n  ${reproduce}`);
      }
      if (result.messages.length > 0) ctx.metric('integrityMessages', result.messages.length);
    };

    // -----------------------------------------------------------------------
    // One export move
    // -----------------------------------------------------------------------

    /**
     * Resolve a draw's destination.
     *
     * Returns the path ANSWERED into the dialog (or handed to the command) and
     * the path the app should actually write to, which differ on the UI routes
     * because `ensureExtension` rewrites the answer.
     */
    const resolveDestination = (
      draw: ExportDraw,
      moveDir: string,
      projectFile: string | null,
    ): { requested: string; effective: string; ownDir: boolean } | null => {
      if (PROJECT_OWNED_SHAPES.has(draw.shape)) {
        if (projectFile === null) return null; // no saved project to aim at
        const requested =
          draw.shape === 'openProject'
            ? projectFile
            : `${projectFile.slice(0, projectFile.length - path.extname(projectFile).length)}.plnx`;
        const effective =
          draw.route === 'store' ? requested : finalizeExportPath(requested, draw.format);
        return { requested, effective, ownDir: false };
      }
      const base = exportBaseName(draw.shape, naturalExtension(draw.format), draw.seq);
      const requested = path.join(moveDir, base);
      const effective = draw.route === 'store' ? requested : finalizeExportPath(requested, draw.format);
      return { requested, effective, ownDir: true };
    };

    /** Put the shared form into the state a draw asks for. Skips are counted. */
    const applyFormControls = async (draw: ExportDraw): Promise<void> => {
      if (isDataFormat(draw.format)) {
        const typed = await clickControl(ctx, 'export-type-data');
        if (!typed.performed) ctx.metric(`skip:type-data:${typed.reason}`);
        const chosen = await selectValue(ctx, 'data-format-select', dataFormatValue(draw.format));
        if (!chosen.performed) ctx.metric(`skip:data-format:${chosen.reason}`);
        return;
      }
      const typed = await clickControl(ctx, 'export-type-image');
      if (!typed.performed) ctx.metric(`skip:type-image:${typed.reason}`);
      const options = draw.imageOptions;
      const chosen = await selectValue(ctx, 'image-format-select', options.format);
      if (!chosen.performed) ctx.metric(`skip:image-format:${chosen.reason}`);
      if (options.format === 'png') {
        // The Select offers 72/150/300/600 only; the backend edges (0 and 1)
        // are reachable through the store route alone, and the draw says so by
        // being skipped-and-counted here rather than silently coerced.
        const dpi = await selectValue(ctx, 'resolution-select', String(options.dpi));
        if (!dpi.performed) ctx.metric(`skip:dpi:${dpi.reason}`);
      }
      const threshold = await setThreshold(ctx, options.branching_threshold);
      if (!threshold.performed) ctx.metric(`skip:threshold:${threshold.reason}`);
      // The label toggles live behind a collapsed disclosure.
      const toggle = await clickControl(ctx, 'export-labels-options-toggle');
      if (!toggle.performed) ctx.metric(`skip:labels-toggle:${toggle.reason}`);
      const checkboxes: [string, boolean][] = [
        ['export-show-section-names', options.show_section_names],
        ['export-show-station-labels', options.show_station_labels],
        ['export-show-station-comments', options.show_station_comments],
        ['export-show-shot-comments', options.show_shot_comments],
        ['export-show-shot-data', options.show_shot_data],
        ['export-show-labels', options.show_penetration_labels],
      ];
      for (const [testid, want] of checkboxes) {
        const outcome = await setCheckbox(ctx, testid, want);
        if (!outcome.performed && outcome.reason !== 'no-change') {
          ctx.metric(`skip:${testid}:${outcome.reason}`);
        }
      }
      const color = await selectValue(ctx, 'export-color-mode', options.color_mode);
      if (!color.performed) ctx.metric(`skip:color-mode:${color.reason}`);
    };

    /** The store-route call for one draw. */
    const runStoreExport = async (
      draw: ExportDraw,
      target: string,
    ): Promise<{ ok: boolean; error: string | null; skipped: string | null }> => {
      if (draw.format === 'csv' || draw.format === 'json') {
        const action = draw.format === 'csv' ? 'exportCsv' : 'exportJson';
        const res = await callProjectAction(ctx, action, [target]);
        return { ok: res.ok, error: res.err, skipped: null };
      }
      if (draw.format === 'tml' || draw.format === 'tmlu') {
        const res = await callProjectAction(ctx, 'exportTml', [target]);
        return { ok: res.ok, error: res.err, skipped: null };
      }
      if (isImageFormat(draw.format)) {
        // Through `projectStore.exportImage`, NOT raw `export_image`: that
        // action is the one funnel every image export passes through, and it is
        // where `withThemeDepthRamp` stamps the live theme's ramp (#360). A raw
        // invoke here would be this scenario hand-rolling the ramp instead of
        // exercising the pipeline the app uses.
        const res = await callProjectAction(ctx, 'exportImage', [target, draw.imageOptions]);
        return { ok: res.ok, error: res.err, skipped: null };
      }
      if (draw.format === 'diveProfile') {
        if (!(await hasDiveRoute(ctx))) return { ok: false, error: null, skipped: 'no-dive-route' };
        const request = await diveProfileRequest(ctx);
        if (request === null) return { ok: false, error: null, skipped: 'no-dive-route' };
        try {
          await ctx.ipc('export_dive_profile', { path: target, format: 'subsurface-xml', request });
          return { ok: true, error: null, skipped: null };
        } catch (err) {
          return { ok: false, error: err instanceof Error ? err.message : String(err), skipped: null };
        }
      }
      return { ok: false, error: null, skipped: 'preview-has-no-store-route' };
    };

    /**
     * Drive one export move end to end and judge it.
     *
     * `record` is false during a replay's own dispatch, which already has the
     * step it is executing.
     */
    const performExport = async (
      draw: ExportDraw,
      episodeDir: string,
      projectFile: string | null,
      record: boolean,
    ): Promise<void> => {
      const moveDir = path.join(episodeDir, `m${draw.seq}`);
      fs.mkdirSync(moveDir, { recursive: true });
      const destination = resolveDestination(draw, moveDir, projectFile);
      if (destination === null) {
        ctx.metric('skip:no-saved-project-to-aim-at');
        return;
      }
      const { requested, effective, ownDir } = destination;
      const label = `${draw.format} via ${draw.route} at ${draw.shape}`;
      note(label);

      if (record) {
        recorder.record({
          kind: 'export',
          format: draw.format,
          options: {
            route: draw.route,
            shape: draw.shape,
            seq: draw.seq,
            imageOptions: draw.imageOptions,
          },
          rel: path.relative(sessionDir, requested).split(path.sep).join('/'),
          label,
        });
      }

      // The `.plnx` probe needs a sentinel: `is_open_project_file`
      // canonicalizes the file it is protecting and gives up when that fails,
      // so a `.plnx` that is not there is correctly not refused and the metric
      // would read `written` on every run and mean nothing.
      let plnxSentinel: string | null = null;
      if (draw.shape === 'plnxSibling') {
        plnxSentinel = requested;
        try {
          fs.writeFileSync(plnxSentinel, 'export-fuzz dive-plan sentinel');
        } catch {
          plnxSentinel = null;
        }
      }

      let ok = false;
      let error: string | null = null;
      let toastKeys: string[] = [];
      let skipped: string | null = null;

      // The project's own bytes, read IMMEDIATELY before the move rather than
      // once per episode. The `.stalx` is a live SQLite database and every
      // mutating command writes to it, so an episode-scoped baseline reports
      // the hostile-comment oracle's `createSection` as "the export modified
      // the database". Measured on seed 2. Taken as late as possible so the
      // only thing between the two reads is the export.
      const projectBytesBefore =
        draw.shape === 'openProject' && projectFile !== null ? hashOf(projectFile) : null;

      // The toast stack BEFORE the move, for the same reason and taken at the
      // same moment: what the app says about this export is the DIFFERENCE the
      // move makes, not whatever is on screen when it ends. An `export-failed`
      // toast is an error toast, so it stays up for ten and a half seconds
      // (`ToastStack.tsx`: 9000 ms dwell + 1500 ms fade), which is several
      // moves at CSV speed. Reading the live key set made a later SUCCESSFUL
      // export inherit an earlier one's failure, and X1 then reported "an
      // export reported FAILURE and left a file behind" against an app that had
      // done nothing wrong. `newToastKeys` states the other half - why a
      // key-set diff is not enough either.
      const toastsBefore = await readToastSnapshot(ctx);
      if (toastsBefore.keys.includes('export-failed')) ctx.metric('staleExportFailedToast');

      if (draw.route === 'store') {
        const outcome = await runStoreExport(draw, requested);
        ok = outcome.ok;
        error = outcome.error;
        skipped = outcome.skipped;
        toastKeys = newToastKeys(toastsBefore, await readToastSnapshot(ctx));
      } else if (draw.format === 'preview') {
        // No file, no save dialog: mounting the preview IS the move. Reading it
        // back through the DOM is also the only way to exercise
        // `withThemeDepthRamp` without hand-rolling the ramp.
        //
        // The form has to be driven FIRST: `ExportPreview` only mounts on the
        // image type (the dialog opens on data/CSV), so reading before setting
        // it would measure "absent" every time and call it coverage.
        await applyFormControls(draw);
        await settleOnce(ctx);
        const preview = await readPreview(ctx);
        ctx.metric(`preview:${preview.mounted ? (preview.error ? 'error' : preview.svgBytes > 0 ? 'rendered' : 'blank') : 'absent'}`);
        if (preview.mounted && preview.error !== null) {
          ctx.metric('previewError');
          note(`preview error: ${foldThenCut(preview.error, 120)}`);
        }
        rmrf(moveDir);
        return;
      } else {
        await applyFormControls(draw);
        let answered = await clickAndAnswerSave(ctx, 'export-button', requested);
        if (answered.sawUnanchoredNotice) {
          ctx.metric('tmlUnanchoredNotice');
          // `handleExport` returned before opening the picker; Continue calls
          // the same function with `skipTmlAnchorCheck`, so the answer is
          // re-armed on that button rather than a second code path.
          answered = await clickAndAnswerSave(ctx, 'tml-unanchored-warning-continue', requested);
        }
        if (!answered.performed) {
          ctx.metric(`skip:export-button:${answered.reason}`);
          rmrf(moveDir);
          return;
        }
        if (!answered.consumed) {
          ctx.metric('skip:save-picker-unconsumed');
          rmrf(moveDir);
          return;
        }
        // The Replace File alert (answered Replace by clickAndAnswerSave). Only
        // a path the app rewrote can raise it; one on any other move is the
        // app asking about a name the OS dialog already confirmed.
        if (answered.sawOverwritePrompt) {
          ctx.metric(`overwritePrompt:${draw.shape}`);
          if (!domRouteRewritesPath(requested, draw.format)) ctx.metric('overwritePrompt:unrewrittenPath');
        }
        toastKeys = newToastKeys(toastsBefore, await readToastSnapshot(ctx));
        // A UI route has no promise to inspect. The app's own report is the
        // toast THIS move raised, and X1 then asks whether the disk agrees with
        // it - which is the non-tautological half. Finding 1's regression check
        // is unaffected by the diff: an export that fails and says nothing
        // still produces no new toast, still reads as `ok`, and X1 still
        // demands the file it did not write.
        ok = !toastKeys.includes('export-failed');
        error = ok ? null : 'reported by the export-failed toast raised by THIS move';
      }

      if (skipped !== null) {
        ctx.metric(`skip:${skipped}`);
        rmrf(moveDir);
        return;
      }

      ctx.metric(`export:${draw.format}:${draw.route}:${ok ? 'ok' : 'failed'}`);
      ctx.metric(`shape:${draw.shape}:${ok ? 'ok' : 'failed'}`);

      if (PROJECT_OWNED_SHAPES.has(draw.shape)) {
        if (draw.shape === 'openProject') {
          const after = projectFile === null ? null : hashOf(projectFile);
          guarded(() =>
            checkProjectFileSurvived(
              {
                route: draw.route,
                format: draw.format,
                before: projectBytesBefore ?? '(unread)',
                after,
                ok,
                error,
                // Only the raw command sees the path as given; the UI routes
                // have already had `ensureExtension` rewrite it.
                expectRefusal: draw.route === 'store',
              },
              reproduce,
            ),
          );
          ctx.metric(`x2:stalx:${draw.route}:${ok ? 'accepted' : 'refused'}`);
          // The redirected artifact is real and belongs to nobody: clean it up.
          if (draw.route !== 'store' && effective !== projectFile) rmrf(effective);
        } else {
          // The `.plnx` probe: measured, never asserted. See the header.
          const survived = plnxSentinel !== null && sizeOf(plnxSentinel) !== null;
          const clobbered =
            plnxSentinel !== null &&
            survived &&
            (fs.readFileSync(plnxSentinel, 'utf8') !== 'export-fuzz dive-plan sentinel');
          ctx.metric(`x2:plnx:${draw.format}:${draw.route}:${clobbered ? 'CLOBBERED' : ok ? 'written-elsewhere' : 'refused'}`);
          if (plnxSentinel !== null) rmrf(plnxSentinel);
          if (draw.route !== 'store') rmrf(effective);
        }
        rmrf(moveDir);
        return;
      }

      const attempt: ExportAttempt = {
        format: draw.format,
        route: draw.route,
        shape: draw.shape,
        requested,
        effective,
        ok,
        error,
        size: sizeOf(effective),
        toastKeys,
        strays: ownDir ? straysIn(moveDir, [path.basename(effective)]) : [],
      };
      // Determinism mode: what this move reported and wrote (no-op when off).
      detProbe()?.exportMove(draw.seq, attempt);
      guarded(() => checkFileResult(attempt, reproduce));
      rmrf(moveDir);
    };

    // -----------------------------------------------------------------------
    // Structural oracles
    // -----------------------------------------------------------------------

    /**
     * X3: the same options through SVG, PNG and PDF.
     *
     * `base` is DRAWN by the caller and passed in, not drawn here, because it
     * is what the trace records: a replay runs this oracle on the option set
     * the recording used rather than on whatever a differently-consumed rng
     * hands it. See the "Record and replay" section of the header.
     */
    const runDifferential = async (episodeDir: string, base: ImageOptionDraw): Promise<void> => {
      const dir = path.join(episodeDir, 'differential');
      fs.mkdirSync(dir, { recursive: true });
      const arms: { format: 'svg' | 'png' | 'pdf'; ok: boolean; error: string | null }[] = [];
      for (const format of ['svg', 'png', 'pdf'] as const) {
        const options: ImageOptionDraw = { ...base, format, dpi: DIFFERENTIAL_DPI };
        const target = path.join(dir, `differential.${format}`);
        const res = await callProjectAction(ctx, 'exportImage', [target, options]);
        arms.push({ format, ok: res.ok, error: res.err });
        ctx.metric(`x3:${format}:${res.ok ? 'ok' : 'failed'}`);
      }
      const svgArm = arms[0] as { format: 'svg'; ok: boolean; error: string | null };
      guarded(() => checkFormatDifferential(svgArm, arms.slice(1), reproduce));
      rmrf(dir);
    };

    /**
     * X4: three exports of an unmutated project.
     *
     * `extension` is DRAWN by the caller for the same reason X3's options are:
     * both container shapes are worth a run (`.tml` is a zip and `.tmlu` is
     * plain XML, and only one of them can carry a container timestamp), so
     * which one a failing run used is part of what the trace has to carry.
     */
    const runByteIdentity = async (
      episodeDir: string,
      digest: ProjectDigest,
      extension: 'tml' | 'tmlu',
    ): Promise<void> => {
      const dir = path.join(episodeDir, 'identity');
      fs.mkdirSync(dir, { recursive: true });
      const hashes: string[] = [];
      const sizes: (number | null)[] = [];
      for (const label of ['a', 'b', 'c']) {
        const target = path.join(dir, `identity-${label}.${extension}`);
        const res = await callProjectAction(ctx, 'exportTml', [target]);
        if (!res.ok) {
          // A refusal here is content-driven (`ensure_rooted_chain` bails on
          // orphans plus closures) and is the same for all three, so it is not
          // an identity question at all.
          ctx.metric('x4:refused');
          rmrf(dir);
          return;
        }
        hashes.push(hashOf(target) ?? '(unreadable)');
        sizes.push(sizeOf(target));
      }
      const verdict = guardedValue(() =>
        checkByteIdentity(
          {
            hashes,
            sizes,
            precondition: { missing: digest.missingUuids, duplicated: digest.duplicateUuids },
            extension,
          },
          reproduce,
        ),
      );
      ctx.metric(`x4:${extension}:${verdict.skipped ? (verdict.stable ? 'skipped-stable' : 'skipped-unstable') : 'stable'}`);
      if (verdict.skipped) {
        ctx.metric('x4:uuidPreconditionBroken');
        ctx.log(
          `x4: ${digest.missingUuids} shot(s) with no uuid and ${digest.duplicateUuids} document-level ` +
            `duplicate(s), so the exporter re-mints per export by design - byte identity not asserted`,
        );
      }
      rmrf(dir);
    };

    const guardedValue = <T,>(fn: () => T): T => {
      try {
        return fn();
      } catch (err) {
        if (err instanceof ExportOracleError) fail(err.message);
        throw err;
      }
    };

    /**
     * Create a fresh project under the session dir and import `file` into it.
     *
     * `execute_tml_import` requires a project to be OPEN and does not create
     * one (`core/state/app_state.rs:648-650` answers "No project open"), which
     * is why the frontend routes every non-DMP import through
     * `new-project-for-import`. This is that route's store-level equivalent.
     */
    const freshProjectWithImport = async (
      slot: string,
      file: string,
    ): Promise<{ ok: boolean; error: string | null; projectPath: string }> => {
      fs.mkdirSync(projectsDir, { recursive: true });
      // ONE project NAME across every slot, different files.
      //
      // The name lands in the export as `<caveName>` (`export/tml.rs`), so two
      // differently-named holding projects make T2 and T3 differ in a byte the
      // round trip did not introduce - this harness did. Measured: on seed 1
      // the ONLY difference between T2 and T3 was `roundtrip-b` vs
      // `roundtrip-c` at offset 91.
      const projectPath = path.join(projectsDir, `${slot}.stalx`);
      rmrf(projectPath);
      const created = await callProjectAction(ctx, 'createProject', [
        ROUNDTRIP_PROJECT_NAME,
        undefined,
        // A real `savePath` rather than a deferred initial save: the deferred
        // one reserves a path and leaves the database unopened, and an import
        // into it cannot run. It also means `ensure_shot_uuids` has run by the
        // time anything is exported back out.
        { savePath: projectPath },
      ]);
      if (!created.ok) {
        return { ok: false, error: created.err ?? 'createProject failed', projectPath };
      }
      // `commentsOnStations: false`, deliberately, and it is the ONE place this
      // scenario departs from the app's default (`executeTmlImport` sends
      // `?? true`).
      //
      // With it on, anything the source file carries as a STATION comment
      // lands in `project.station_comments` - and `ProjectIpc`
      // (`models/project.rs:612-629`) serializes 17 fields, none of them
      // `station_comments`, so `get_project_state` cannot show them and X6's
      // text check would read every one as lost. Measured on naharon: its 74
      // station comments were never visible over IPC. (Shot comments used to
      // move to stations too - #564 fixed that by marking every exported shot
      // comment - but the station-comment blindness alone still forces `false`
      // here.) Keeping every comment on a shot is a real user-facing import
      // option, not a fabricated state, and it is what makes the text half of
      // X6 an assertion rather than a formality.
      const imported = await callProjectAction(ctx, 'executeTmlImport', [file, null, false, false, null]);
      if (!imported.ok) {
        return { ok: false, error: imported.err ?? 'import failed', projectPath };
      }
      await settleOnce(ctx);
      return { ok: true, error: null, projectPath };
    };

    /** X5 + X6: T1 -> fresh -> T2 -> fresh -> T3, and the shot multiset. */
    const runRoundtrip = async (episodeDir: string): Promise<void> => {
      // START FROM THE PRISTINE FIXTURE, not from whatever this episode has
      // done to the project so far.
      //
      // The rotation can put the hostile-comment oracle (X7) in the same
      // episode, and its section is a SYNTHETIC FLOATING chain built by
      // `createSection` - not connected to anything, not anchored to a datum.
      // Measured on seed 1 episode 3: those 14 shots come back with their
      // depths shifted by a constant 7 and their comments gone, which is the
      // exporter re-rooting an orphan chain (`ensure_rooted_chain`) and the
      // importer's comment split doing something this scenario has not
      // characterised - see the OBSERVATIONS in the header. Neither is a
      // question about the SURVEY's round trip, which is what X5 and X6 ask, so
      // the answer is to ask it on the survey rather than to relax the oracle.
      //
      // X7 still covers the hostile comments where they belong: at the file
      // level, where it asserts they survive a .tmlu write and a parse_tml read.
      await loadFixtureWithoutUndoHistory(ctx, fuzzFixture());
      await disableAutoSave(ctx);
      const original = await readProjectDigest(ctx);
      const dir = path.join(episodeDir, 'roundtrip');
      fs.mkdirSync(dir, { recursive: true });
      // `.tmlu` throughout: it is plain XML, so a canonical comparison is
      // meaningful text rather than a deflate stream, and the byte comparison
      // is not measuring the zip container.
      const t1 = path.join(dir, 'trip-1.tmlu');
      const first = await callProjectAction(ctx, 'exportTml', [t1]);
      if (!first.ok) {
        ctx.metric('x5:t1-refused');
        rmrf(dir);
        return;
      }

      const importedOnce = await freshProjectWithImport('roundtrip-a', t1);
      if (!importedOnce.ok) {
        ctx.metric('x5:import-refused');
        ctx.log(`x5: first import failed: ${importedOnce.error ?? 'no message'}`);
        rmrf(dir);
        return;
      }
      await assertIntegrity('the first round-trip import');

      const afterFirst = await readProjectDigest(ctx);
      // X6a: the shots survived the trip out and back.
      //
      // Identity goes through `projectSnapshot.ts`'s `shotSetDelta`, the
      // harness's one answer to "which shots did this gain or lose", over a
      // FLATTENED snapshot: that function is per-SECTION, and section identity
      // is exactly what a round trip is allowed to change (`parse_tml` rebuilds
      // sections as connected runs of the shot graph, so one exported section
      // can come back as several - #412, by design). Collapsing both sides onto
      // one synthetic section asks the question that survives the trip.
      const identity = shotSetDelta(flatUuidSnapshot(original.uuids), flatUuidSnapshot(afterFirst.uuids));
      // The BEFORE side as the TML export writes it. On a project whose
      // sections carry different declinations the exporter rotates every
      // bearing into true north (docs/ariane-export.md, the third case) and the
      // re-import reads them as measured true-north ones - a documented loss
      // that decision HFI-8-D1 keeps, so X6 compares against the rotated
      // bearings instead of reporting every shot as lost and gained (#576
      // HFI-8). `asExportedToTml` states what it folds and what still fails.
      const exported = asExportedToTml(original.shots, original.shotReferences);
      if (exported.plan === 'mixed-to-true') ctx.metric('x6:rotatedToTrue', exported.rotated);
      guarded(() =>
        checkShotMultiset(
          {
            deltas: multisetDelta(exported.shots, afterFirst.shots),
            azimuthPlan: exported.plan,
            // LOSSES ONLY, and the asymmetry is forced by what this tier can
            // see rather than chosen to quiet a red.
            //
            // `ProjectIpc` (`models/project.rs:585-602`) serializes 16 fields
            // and `station_comments` is not one of them, so the BEFORE side is
            // blind to every station comment - naharon's 74 of them, against 2
            // shot comments. The exporter co-encodes station comments into the
            // shot comment, so after a trip that keeps comments on shots they
            // land somewhere the digest CAN see, and 66 texts read as gained.
            // A gain is not data loss and there is no evidence behind asserting
            // it; a LOSS is exactly the claim the oracle is for, and every
            // comment the before side can see is still asserted to survive.
            //
            // Making this symmetric needs station comments on the wire. Until
            // then the gain count is a metric.
            commentDeltas: commentMultisetDelta(original.commentParts, afterFirst.commentParts).filter(
              (delta) => delta.before > delta.after,
            ),
            uuidsBefore: original.uuids,
            uuidsAfter: afterFirst.uuids,
            countBefore: original.shots.length,
            countAfter: afterFirst.shots.length,
            identityDetail: identity.length > 0 ? describeShotSetDelta(identity) : null,
          },
          reproduce,
        ),
      );
      ctx.metric('x6:multisetIntact');
      const commentGains = commentMultisetDelta(original.commentParts, afterFirst.commentParts).filter(
        (delta) => delta.after > delta.before,
      );
      if (commentGains.length > 0) ctx.metric('x6:commentTextsGained', commentGains.length);
      // X6c: the depths, tapes and plan the multiset key does not see, per shot
      // by uuid, under the junction rule (#576 TML-DEPTH-D1, Q1-D1, Q2-D1,
      // Q3-D1): every station at its true depth except an averaged junction, at
      // the mean of one reading per section; the length bound; the plan. Which
      // stations are one node comes from the RE-IMPORTED merges - see
      // `junctionDepthOracle.ts`. The counts stay visible as metrics, so a change
      // in how often the export averages or refits shows up.
      const depths = junctionDepthReport(exported.shots, afterFirst);
      guarded(() => checkJunctionDepths(depths, reproduce));
      ctx.metric('x6:depthRule');
      if (depths.averagedJunctions > 0) ctx.metric('x6:averagedJunctions', depths.averagedJunctions);
      if (depths.refittedShots > 0) ctx.metric('x6:refittedShots', depths.refittedShots);
      if (depths.brokenShots > 0) ctx.metric('x6:brokenShots', depths.brokenShots);
      if (depths.unmatched > 0) ctx.metric('x6:depthRuleUnmatched', depths.unmatched);

      // X6b: importing the SAME file again must not leave two shots on one
      // uuid. The second import carries the first's foreign uuids verbatim, so
      // `remint_duplicate_uuids`' project-side twin has to fire.
      const second = await callProjectAction(ctx, 'executeTmlImport', [t1, null, false, false, null]);
      if (second.ok) {
        await settleOnce(ctx);
        await assertIntegrity('a second import of the same file');
        const doubled = await readProjectDigest(ctx);
        guarded(() => checkNoDuplicateUuids(doubled.uuidsBySection, reproduce));
        ctx.metric('x6:doubleImportUnique');
      } else {
        ctx.metric('skip:second-import-refused');
      }

      // X5 needs a chain from projects holding ONE copy each, so it is re-derived
      // from clean imports rather than from the doubled one above.
      //
      // The chain runs until two consecutive exports match, up to
      // MAX_ROUNDTRIP_EXPORTS. Asserting a fixed POINT rather than a fixed INDEX
      // is what finding 6 corrected: on a real survey the first import inherits
      // its section ORDER from the source project rather than deriving it, so
      // the composition takes one more pass to reach its own fixed point - and
      // nothing but that order moves on the way.
      const chain: string[] = [];
      let source = t1;
      for (let pass = 2; pass <= MAX_ROUNDTRIP_EXPORTS; pass += 1) {
        const holding = await freshProjectWithImport(`roundtrip-${pass}`, source);
        if (!holding.ok) {
          ctx.metric('x5:chain-import-refused');
          rmrf(dir);
          return;
        }
        await assertIntegrity(`round-trip import pass ${pass}`);
        const target = path.join(dir, `trip-${pass}.tmlu`);
        const exported = await callProjectAction(ctx, 'exportTml', [target]);
        if (!exported.ok) {
          ctx.metric(`x5:t${pass}-refused`);
          rmrf(dir);
          return;
        }
        chain.push(fs.readFileSync(target, 'utf8'));
        source = target;
        // Stop as soon as it has settled; the oracle re-checks and the extra
        // passes cost a project create and an import each.
        const last = chain[chain.length - 1];
        const previous = chain[chain.length - 2];
        if (previous !== undefined && previous === last) break;
      }

      const verdict = guardedValue(() => checkConvergence({ exports: chain }, reproduce));
      ctx.metric(`x5:converged:${verdict.via}`);
      ctx.metric(`x5:settledAt:${verdict.settledAt}`);
      if (verdict.via === 'canonical') {
        ctx.log(
          'x5: the chain settles only after canonicalizing uuids - the TML importer preserves the ' +
            "file's uuids, so identity moved somewhere it should not have",
        );
        // Counted, not asserted: the fallback is what the oracle is for, and
        // the metric is what makes it visible in a matrix cell.
        ctx.metric('x5:identityMoved');
      }
      rmrf(dir);
    };


    /** X7: hostile comments through the CSV and the TML writers. */
    const runNeutralisation = async (episodeDir: string): Promise<void> => {
      const dir = path.join(episodeDir, 'hostile');
      fs.mkdirSync(dir, { recursive: true });
      // Direct section creation rather than a synthetic import: the comments
      // are the input under test, and routing them through an importer first
      // would mean testing the importer's escaping instead of the exporter's.
      const shots = HOSTILE_COMMENTS.map((comment, index) => ({
        azimuth_in: (index * 17) % 360,
        azimuth_out: (index * 17) % 360,
        length: 3 + (index % 5),
        depth_in: index % 7,
        depth_out: (index % 7) + 1,
        pitch_in: null,
        pitch_out: null,
        left: 1,
        right: 1,
        up: 1,
        down: 1,
        comment,
        kind: 'Normal',
        marker_index: null,
        temperature: null,
      }));
      const created = await callProjectAction(ctx, 'createSection', [
        {
          name: 'hostile "comments", =1+1',
          surveyor: 'export-fuzz',
          date: '2026-01-01',
          color: '#3388FF',
          direction: 'out',
          quality: 3,
          shots,
        },
      ]);
      if (!created.ok) {
        ctx.metric('skip:hostile-section-refused');
        ctx.log(`x7: createSection failed: ${created.err ?? 'no message'}`);
        rmrf(dir);
        return;
      }
      // No `mutate` step: since v4 this whole oracle travels as ONE `oracle`
      // step, recorded by its caller before it runs. A `mutate` step here as
      // well would replay the oracle twice - once for the step and once for the
      // dispatch - and a `createSection` recorded from INSIDE an oracle could
      // only ever be replayed by re-running the oracle from the top anyway.
      note('createSection(hostile comments)');
      await assertIntegrity('seeding hostile comments');

      const csvPath = path.join(dir, 'hostile.csv');
      const csvExport = await callProjectAction(ctx, 'exportCsv', [csvPath]);
      if (!csvExport.ok) {
        ctx.metric('x7:csv-refused');
      } else {
        const text = fs.readFileSync(csvPath, 'utf8');
        const result = csvTextViolations(text, HOSTILE_COMMENTS);
        if (result.violations.length > 0 || result.missing.length > 0) {
          fail(
            `X7 the CSV export did not honour escape_csv for every cell\n` +
              result.violations
                .slice(0, 8)
                .map((v) => `    row ${v.row} [${v.column}]: ${v.detail}`)
                .join('\n') +
              (result.missing.length > 0
                ? `\n    ${result.missing.length} seeded comment(s) did not survive at all: ` +
                  `${JSON.stringify(result.missing.map((c) => foldThenCut(c, 40)))}`
                : '') +
              `\n    escape_csv (src-tauri/src/export/csv.rs:66-79) neutralises a leading\n` +
              `    = + - @ TAB or CR with an apostrophe and then RFC-4180-quotes anything\n` +
              `    containing , " LF or CR. A cell that reaches a spreadsheet un-neutralised\n` +
              `    is a formula that runs on open.\n  ${reproduce}`,
          );
        }
        ctx.metric('x7:csvClean');
        ctx.setMetric('x7:csvRows', result.rows);
      }

      const tmlPath = path.join(dir, 'hostile.tmlu');
      const tmlExport = await callProjectAction(ctx, 'exportTml', [tmlPath]);
      if (!tmlExport.ok) {
        ctx.metric('x7:tml-refused');
        rmrf(dir);
        return;
      }
      // Re-parse through the app's own reader rather than by reading the XML:
      // the question is whether the text survives a round trip, and a substring
      // search over the raw file would pass on markup the parser rejects.
      let parsed: unknown;
      try {
        parsed = await ctx.ipc('parse_tml', { path: tmlPath });
      } catch (err) {
        fail(
          `X7 the app could not re-parse a .tmlu it had just written\n` +
            `    ${err instanceof Error ? err.message : String(err)}\n` +
            `    the comments in it are hostile, but the FILE is the exporter's own output;\n` +
            `    a document only the writer can produce and not the reader is a broken\n` +
            `    round trip whatever provoked it.\n  ${reproduce}`,
        );
      }
      const haystack = JSON.stringify(parsed);
      const inHaystack = (part: string): boolean =>
        haystack.includes(JSON.stringify(part).slice(1, -1));
      // Compared as ZWSP-split PARTS: the exporter co-encodes station comments
      // into the shot comment with U+200B separators, so the joined string
      // legitimately changes shape while every part survives. U+200B is not
      // whitespace in Unicode terms, so the trim below cannot eat a separator.
      const parts = (comment: string): string[] => comment.split('\u200b');
      let trimmed = 0;
      const lost = HOSTILE_COMMENTS.filter((comment) =>
        parts(comment).some((part) => {
          if (part === '') return false;
          if (inHaystack(part)) return false;
          // LEADING/TRAILING WHITESPACE IS FORMAT LOSSINESS, NOT A DEFECT, and
          // this is a corrected PREMISE rather than a tolerance. The reader
          // does `e.unescape()?.trim()` on every shot element's text
          // (`parsers/tml/mod.rs:398`), which is what makes a pretty-printed
          // `<Comment>\n  text\n</Comment>` written by another tool read as
          // `text`. A comment that begins with whitespace therefore cannot
          // round-trip, and writing `&#13;` would not help: XML 1.0 line-end
          // handling normalises a literal CR to LF before the reader sees it,
          // and the trim then removes that too. Measured on this scenario's
          // first run (seed 1) and written into docs/ariane-import.md's
          // lossiness table, which is where a claim like this belongs.
          const bare = part.trim();
          if (bare !== '' && inHaystack(bare)) {
            trimmed += 1;
            return false;
          }
          return true;
        }),
      );
      if (trimmed > 0) ctx.metric('x7:tmlWhitespaceTrimmed', trimmed);
      if (lost.length > 0) {
        fail(
          `X7 ${lost.length} hostile comment(s) did not survive a .tmlu write + parse_tml read\n` +
            `    ${JSON.stringify(lost.map((c) => foldThenCut(c, 40)))}\n` +
            `    Leading/trailing whitespace is already excused (the reader trims every text\n` +
            `    field by design - see docs/ariane-import.md), so this is CONTENT loss:\n` +
            `    escaping that dropped or mangled a character the document may legally carry.\n` +
            `  ${reproduce}`,
        );
      }
      ctx.metric('x7:tmlCommentsIntact');
      rmrf(dir);
    };

    /**
     * Does `config.export` reach the export form at all?
     *
     * Returns 1 when a CHANGED config default shows up in a freshly reset
     * `exportStore`, 0 when it does not, and -1 when the config was unreadable
     * (which is a skip, not an answer).
     */
    const probeConfigDefaults = async (): Promise<number> => {
      return ctx.eval<number>(`
        const project = window.__E2E_TEST__ && window.__E2E_TEST__.projectStore;
        const exportStore = window.__E2E_TEST__ && window.__E2E_TEST__.exportStore;
        if (!project || !exportStore) return -1;
        const state = project.getState();
        const config = state.config;
        if (!config || !config.export) return -1;
        const original = config.export;
        try {
          state.setConfig({
            ...config,
            export: { ...original, default_image_format: 'pdf', default_dpi: 111 },
          });
          exportStore.getState().reset();
          const after = exportStore.getState().imageOptions || {};
          return after.dpi === 111 && after.format === 'pdf' ? 1 : 0;
        } finally {
          const now = project.getState().config;
          if (now) project.getState().setConfig({ ...now, export: original });
          exportStore.getState().reset();
        }
      `);
    };

    /** X8: the two option sets, and the store's reset on a project switch. */
    const runOptionIndependence = async (): Promise<void> => {
      // The DECLARED defaults, not a read of the live store. Reading would make
      // X8b vacuous the moment it mattered: if a previous move had dirtied the
      // store and nothing reset it, the "defaults" would BE the dirty values
      // and the comparison would pass on exactly the bug it is for.
      const defaults = EXPECTED_DEFAULT_OPTIONS;
      // Dirty the panel's set first, through the store the panel is bound to.
      const dirtyImage = {
        format: 'pdf' as const,
        dpi: 600,
        branching_threshold: 9,
        show_penetration_labels: false,
        color_mode: 'depth' as const,
        show_station_labels: true,
        show_section_names: true,
        show_station_comments: true,
        show_shot_comments: true,
        show_shot_data: true,
      };
      await ctx.callAction('export', 'setExportType', ['data']);
      await ctx.callAction('export', 'setDataFormat', ['tml']);
      await ctx.callAction('export', 'setImageOptions', [dirtyImage]);
      const panelBefore = await readPanelOptions(ctx);

      // Now open the modal and change ITS options.
      if (!(await openModal(ctx))) {
        ctx.metric('skip:modal-would-not-open');
      } else {
        await clickControl(ctx, 'export-type-image');
        await selectValue(ctx, 'image-format-select', 'svg');
        await setThreshold(ctx, 7);
        const modalChangedTo = (await readModalOptions(ctx)) ?? panelBefore;
        const panelAfter = await readPanelOptions(ctx);
        guarded(() => checkOptionIndependence({ panelBefore, panelAfter, modalChangedTo }, reproduce));
        ctx.metric('x8:independent');
        await closeModal(ctx);
      }

      // X8b: a project switch drops the panel's set.
      const dirty = await readPanelOptions(ctx);
      await loadFixtureWithoutUndoHistory(ctx, fuzzFixture());
      await disableAutoSave(ctx);
      const afterSwitch = await readPanelOptions(ctx);
      guarded(() => checkExportStoreReset({ dirty, afterSwitch, defaults }, reproduce));
      ctx.metric('x8:resetOnSwitch');

      // Measured, never asserted: see the header.
      //
      // The probe CHANGES `config.export` and then resets the store, rather than
      // comparing the shipped config default against the shipped store default.
      // Those two agree by coincidence today (both say png / 300), so a
      // comparison would report "honored" for a pipeline that reads neither -
      // which is exactly the wrong answer and the reason this is a probe rather
      // than a read. Frontend-only (`setConfig` does not persist), and the
      // original config is put back.
      ctx.setMetric('x8:configDefaultsHonored', await probeConfigDefaults());
    };

    /** X9: export mode forces the adjusted, read-only map and gives it back. */
    const runModeInvariants = async (): Promise<void> => {
      const before = await readModeSnapshot(ctx);
      if (!(await setAppMode(ctx, 'export'))) {
        ctx.metric('skip:setAppMode-refused');
        return;
      }
      const during = await readModeSnapshot(ctx);
      await setAppMode(ctx, 'survey');
      const after = await readModeSnapshot(ctx);
      guarded(() => checkExportModeInvariants({ before, during, after }, reproduce));
      ctx.metric('x9:modeRoundTrip');
    };

    /**
     * X9b: survey -> export -> a DIFFERENT project -> survey leaks no
     * `prePlan*` state.
     *
     * Those three module variables are unreadable from anywhere
     * (`uiStore.ts:826, 858, 877` - plain `let`s, not store fields), so the
     * only way to see them is behaviourally: enter export mode on project A,
     * load project B, and check that the geometry mode and dock state B
     * arrives with are B's own rather than A's snapshot being replayed over
     * them.
     */
    const runPrePlanLeak = async (): Promise<void> => {
      await setAppMode(ctx, 'survey');
      await ctx.callAction('ui', 'setGeometryMode', ['measured']);
      const baseline = await readModeSnapshot(ctx);
      if (!(await setAppMode(ctx, 'export'))) {
        ctx.metric('skip:setAppMode-refused');
        return;
      }
      // A direct project switch mid-Export: `applyDisplaySettings` clears the
      // snapshot and forces survey, so the later exit must be a no-op rather
      // than a restore of the PREVIOUS project's geometry.
      await loadFixtureWithoutUndoHistory(ctx, fuzzFixture());
      await disableAutoSave(ctx);
      const afterLoad = await readModeSnapshot(ctx);
      if (afterLoad.appMode !== 'survey') {
        fail(
          `X9b loading a project mid-Export left appMode at "${afterLoad.appMode}"\n` +
            `    applyDisplaySettings sets APP_MODE_DEFAULT precisely so a load is\n` +
            `    authoritative over whatever workspace mode the previous project was in.\n` +
            `  ${reproduce}`,
        );
      }
      // Now leave "export mode" again - there is nothing to leave, and that is
      // the point: a stale prePlanGeometryMode would be applied here.
      await setAppMode(ctx, 'export');
      await setAppMode(ctx, 'survey');
      const settled = await readModeSnapshot(ctx);
      if (settled.geometryMode !== afterLoad.geometryMode) {
        fail(
          `X9b a project switch mid-Export leaked the PREVIOUS project's geometry mode\n` +
            `    baseline (project A): ${JSON.stringify(baseline)}\n` +
            `    after loading B:      ${JSON.stringify(afterLoad)}\n` +
            `    after export -> survey on B: ${JSON.stringify(settled)}\n` +
            `    prePlanGeometryMode is a module variable that outlives the project that\n` +
            `    set it; applyDisplaySettings nulls it for exactly this reason.\n  ${reproduce}`,
        );
      }
      ctx.metric('x9b:noPrePlanLeak');
    };

    // -----------------------------------------------------------------------
    // Teardown
    // -----------------------------------------------------------------------

    const teardown = async (episodeDir: string): Promise<void> => {
      if (await modalIsOpen(ctx)) {
        await closeModal(ctx);
        ctx.metric('teardown:modalWasOpen');
      }
      await setAppMode(ctx, 'survey');
      await ctx.callAction('project', 'closeProject', []).catch(() => undefined);
      await settleOnce(ctx);

      const digest = await readProjectDigest(ctx);
      if (digest.open) {
        fail(
          `E1 teardown left a project open\n    ${digest.sections} section(s)\n  ${reproduce}`,
        );
      }
      const dialog = (await ctx.readStore('ui', 'activeDialog')) as { value?: unknown };
      if (dialog.value !== null && dialog.value !== undefined) {
        // A store value (`activeDialog: string | null`): it prints as a test id does,
        // the words of the app's source and other text as its length (#576 W51).
        fail(`E1 teardown left activeDialog = ${shareableTestId(dialog.value)}\n  ${reproduce}`);
      }
      // The panel's option set is per-project state; a close resets it, and E1
      // is where that contract is asserted rather than merely relied on.
      const options = await readPanelOptions(ctx);
      if (options.exportType !== 'image' || options.dataFormat !== 'csv') {
        fail(
          `E1 closing the project left the Export-mode options dirty: ` +
            `${JSON.stringify(options)}\n  ${reproduce}`,
        );
      }

      rmrf(episodeDir);
      rmrf(projectsDir);
      const leftover = straysIn(exportsDir, []);
      if (leftover.length > 0) {
        fail(
          `E1 teardown left ${leftover.length} file(s) under ${EXPORTS_REL}/: ` +
            `${JSON.stringify(leftover.slice(0, 8))}\n  ${reproduce}`,
        );
      }
    };

    // -----------------------------------------------------------------------
    // The oracle dispatch - ONE table, used by generation and by replay
    // -----------------------------------------------------------------------

    /**
     * Every structural oracle, by the name a trace step carries.
     *
     * The names are the allowlist as well as the vocabulary: a replayed step is
     * a file's opinion about which function to call, so an unknown name is a
     * hard refusal rather than a skip. Same rule the testid allowlist in
     * `exportFuzzActions.ts` states for clicks.
     */
    type OracleName =
      | 'x3-differential'
      | 'x4-byte-identity'
      | 'x5x6-roundtrip'
      | 'x7-hostile-comments'
      | 'x8-option-independence'
      | 'x9-mode-invariants'
      | 'x9b-pre-plan-leak';

    const ORACLE_NAMES: readonly OracleName[] = [
      'x3-differential',
      'x4-byte-identity',
      'x5x6-roundtrip',
      'x7-hostile-comments',
      'x8-option-independence',
      'x9-mode-invariants',
      'x9b-pre-plan-leak',
    ];

    /** The resolved draw an oracle needs. `{}` for the ones that draw nothing. */
    interface OracleInputs {
      imageOptions?: ImageOptionDraw;
      extension?: 'tml' | 'tmlu';
    }

    const readImageOptions = (inputs: OracleInputs, name: string): ImageOptionDraw => {
      const options = inputs.imageOptions;
      if (options === undefined || options === null || typeof options !== 'object') {
        throw new Error(`trace step for oracle ${name} carries no imageOptions`);
      }
      return options;
    };

    const readExtension = (inputs: OracleInputs, name: string): 'tml' | 'tmlu' => {
      const extension = inputs.extension;
      if (extension !== 'tml' && extension !== 'tmlu') {
        throw new Error(
          `trace step for oracle ${name} names extension ${valueShape(extension)}, ` +
            `which this build does not export`,
        );
      }
      return extension;
    };

    /**
     * Run one oracle by name, on inputs somebody else resolved.
     *
     * BOTH sides go through here: the generation loop draws the inputs, records
     * them as a step and calls this; a replay reads the step and calls this. One
     * dispatch is what makes "the replay ran what the recording ran" a property
     * of the code rather than of two lists staying in step.
     */
    const runOracle = async (
      name: OracleName,
      inputs: OracleInputs,
      episodeDir: string,
      digest: ProjectDigest,
    ): Promise<void> => {
      switch (name) {
        case 'x3-differential':
          await runDifferential(episodeDir, readImageOptions(inputs, name));
          return;
        case 'x4-byte-identity':
          await runByteIdentity(episodeDir, digest, readExtension(inputs, name));
          return;
        case 'x5x6-roundtrip':
          await runRoundtrip(episodeDir);
          return;
        case 'x7-hostile-comments':
          await runNeutralisation(episodeDir);
          return;
        case 'x8-option-independence':
          await runOptionIndependence();
          return;
        case 'x9-mode-invariants':
          await runModeInvariants();
          return;
        case 'x9b-pre-plan-leak':
          await runPrePlanLeak();
          return;
      }
    };

    /** Record the oracle and its resolved inputs, then run it. Generation only. */
    const stageOracle = async (
      name: OracleName,
      inputs: OracleInputs,
      episodeDir: string,
      digest: ProjectDigest,
    ): Promise<void> => {
      recorder.record({
        kind: 'oracle',
        oracle: name,
        inputs,
        label: name === 'x4-byte-identity' ? `${name} .${inputs.extension}` : name,
      });
      await runOracle(name, inputs, episodeDir, digest);
    };

    const oracleFromStep = (step: TraceStep & { kind: 'oracle' }): OracleName => {
      const found = ORACLE_NAMES.find((name) => name === step.oracle);
      if (found === undefined) {
        throw new Error(
          `refusing to replay oracle ${valueShape(step.oracle)}: ${SCENARIO} runs ${ORACLE_NAMES.join(', ')}`,
        );
      }
      return found;
    };

    // -----------------------------------------------------------------------
    // Replay
    // -----------------------------------------------------------------------

    const drawFromStep = (step: TraceStep): ExportDraw | null => {
      if (step.kind !== 'export') return null;
      const options = step.options as {
        route?: unknown;
        shape?: unknown;
        seq?: unknown;
        imageOptions?: unknown;
      };
      const route = options.route;
      const shape = options.shape;
      if (route !== 'modal' && route !== 'panel' && route !== 'store') {
        throw new Error(`trace step names route ${valueShape(route)}, which this build does not run`);
      }
      if (typeof shape !== 'string' || !PATH_SHAPE_NAMES.has(shape)) {
        throw new Error(`trace step names path shape ${valueShape(shape)}, which this build does not run`);
      }
      // Refused like a route or a shape: the format names metric keys and moves,
      // and a trace's text is a forged copy's to choose (#576 W51).
      if (!isExportFormat(step.format)) {
        throw new Error(`trace step names format ${valueShape(step.format)}, which this build does not export`);
      }
      const imageOptions = options.imageOptions as ImageOptionDraw | undefined;
      if (imageOptions === undefined || typeof imageOptions !== 'object') {
        throw new Error('trace export step carries no image options');
      }
      return {
        format: step.format,
        route: route as ExportRoute,
        shape: shape as PathShape,
        imageOptions,
        seq: typeof options.seq === 'number' ? options.seq : 0,
      };
    };

    const PATH_SHAPE_NAMES = new Set<string>([
      'normal',
      'upperTmlu',
      'doubleExtension',
      'noExtension',
      'trailingDot',
      'unicodeRtl',
      'longName',
      'percentQuoteSpace',
      'openProject',
      'plnxSibling',
    ]);

    // -----------------------------------------------------------------------
    // Run
    // -----------------------------------------------------------------------

    fs.mkdirSync(exportsDir, { recursive: true });

    if (replay !== null) {
      ctx.log(`replaying ${replay.steps.length} step(s) from ${replayPath}`);

      // EPISODE BOUNDARIES ARE HONOURED, because they are what a step list
      // means. A recording opens a fresh copy of the fixture per episode and
      // tears the old one down; a replay that loaded once and then ran every
      // episode's steps in a row drove episode 2 against whatever episode 1 had
      // left behind - a different project from the one the recording exported,
      // so a multi-episode failure could vanish or land somewhere else. A
      // `reload` step ENDS the previous episode (teardown, which asserts) and
      // starts the next.
      let episode = -1;
      let episodeDir = '';
      let opened: ProjectDigest | null = null;
      let moves = 0;

      const startEpisode = async (name: string): Promise<ProjectDigest> => {
        if (episode >= 0) await teardown(episodeDir);
        episode += 1;
        episodeDir = path.join(exportsDir, `replay-ep${episode}`);
        fs.mkdirSync(episodeDir, { recursive: true });
        await loadFixtureWithoutUndoHistory(ctx, name);
        if (!(await disableAutoSave(ctx))) ctx.metric('autoSaveStillOn');
        opened = await readProjectDigest(ctx);
        ctx.metric('replay:episodes');
        return opened;
      };

      /** The open project, opening the header's fixture if nothing is open yet. */
      const ensureEpisode = async (): Promise<ProjectDigest> =>
        opened === null ? startEpisode(fixture) : opened;

      for (const step of replay.steps) {
        if (step.kind === 'reload') {
          if (step.fixture !== replay.fixture) {
            // The header check refuses a trace recorded on another fixture,
            // because the recorded moves resolve against THAT project. A reload
            // step naming a different one would walk around that check from
            // inside the file.
            throw new Error(
              `trace ${replayPath} reloads ${valueShape(step.fixture)} but was recorded on ${replay.fixture}; ` +
                `its steps resolve against the recorded project`,
            );
          }
          await startEpisode(step.fixture);
          continue;
        }
        // A trace whose first step is not a reload (a shrink candidate can be
        // exactly that) still needs a project under it.
        const current = await ensureEpisode();
        if (step.kind === 'export') {
          const draw = drawFromStep(step);
          if (draw === null) continue;
          if (draw.route !== 'store' && !(await modalIsOpen(ctx))) {
            if (draw.route === 'modal') await openModal(ctx);
            else await setAppMode(ctx, 'export');
          }
          await performExport(draw, episodeDir, current.filePath, false);
          moves += 1;
          if (draw.route === 'modal' && (await modalIsOpen(ctx))) await closeModal(ctx);
          if (draw.route === 'panel') await setAppMode(ctx, 'survey');
          continue;
        }
        if (step.kind === 'oracle') {
          // The RECORDED oracle, on the RECORDED inputs, at the recorded point
          // in the episode. It used to be "every oracle once, at the end, on
          // options freshly drawn from an rng the replay had consumed
          // differently" - so an X3 or X4 failure that depended on the drawn
          // option set was retested with a set that does not provoke it and
          // came back green on the very trace that dropped it.
          await runOracle(oracleFromStep(step), (step.inputs ?? {}) as OracleInputs, episodeDir, current);
          ctx.metric(`replay:oracle:${step.oracle}`);
          // An oracle can import, create sections and switch projects, so the
          // episode's project handle is re-read rather than assumed.
          opened = await readProjectDigest(ctx);
          continue;
        }
        ctx.metric(`replay:unsupported:${step.kind}`);
      }

      if (episode >= 0) await teardown(episodeDir);
      ctx.setMetric('replay:moves', moves);
      ctx.log(`replay complete: ${episode + 1} episode(s), ${moves} export move(s)`);
      return;
    }

    let seq = 0;
    for (let episode = 0; episode < iterations; episode += 1) {
      const episodeDir = path.join(exportsDir, `ep${episode}`);
      fs.mkdirSync(episodeDir, { recursive: true });
      recorder.record({ kind: 'reload', fixture });
      await loadFixtureWithoutUndoHistory(ctx, fixture);
      if (!(await disableAutoSave(ctx))) ctx.metric('autoSaveStillOn');
      const digest = await readProjectDigest(ctx);
      await detProbe()?.state(ctx, `episode ${episode} opened`);
      if (episode === 0) {
        ctx.setMetric('fixtureSections', digest.sections);
        ctx.setMetric('fixtureShots', digest.shots.length);
        ctx.setMetric('fixtureMissingUuids', digest.missingUuids);
        ctx.setMetric('fixtureDuplicateUuids', digest.duplicateUuids);
      }

      // ---- export moves --------------------------------------------------
      const moves = ctx.randInt(1, movesPerEpisode());
      for (let move = 0; move < moves; move += 1) {
        seq += 1;
        const draw = drawExport(ctx, seq);
        if (draw.route === 'modal') {
          if (!(await openModal(ctx))) {
            ctx.metric('skip:modal-would-not-open');
            continue;
          }
        } else if (draw.route === 'panel') {
          if (!(await setAppMode(ctx, 'export'))) {
            ctx.metric('skip:setAppMode-refused');
            continue;
          }
        }
        await performExport(draw, episodeDir, digest.filePath, true);
        if (draw.route === 'modal' && (await modalIsOpen(ctx))) await closeModal(ctx);
        if (draw.route === 'panel') await setAppMode(ctx, 'survey');
        await detProbe()?.state(ctx, `after export move ${seq}`);
      }

      // ---- the structural rotation ---------------------------------------
      //
      // Keyed by the episode index rather than drawn, so a given
      // seed/iterations pair always covers the same oracles - a repro line that
      // sometimes ran X5 and sometimes did not would not be a repro line.
      //
      // Every one goes through `stageOracle`, which RECORDS it (with whatever
      // it drew) before running it. The rotation is therefore in the trace as
      // an ordered list rather than as a rule a replay would have to re-derive.
      if (episode % 2 === 0) {
        // The draws happen HERE rather than inside the oracles, so the values
        // the trace carries are the values the oracles got. Recording is still
        // observe-only: these are the same two draws, in the same order, off
        // the same rng, so a seed's trajectory is exactly what it was.
        await stageOracle('x3-differential', { imageOptions: drawImageOptions(ctx, 'svg') }, episodeDir, digest);
        await stageOracle('x4-byte-identity', { extension: ctx.pick(['tml', 'tmlu'] as const) }, episodeDir, digest);
      }
      if (episode % 3 === 0) {
        await stageOracle('x7-hostile-comments', {}, episodeDir, digest);
      }
      if (episode % 2 === 1 && roundtripEnabled()) {
        await stageOracle('x5x6-roundtrip', {}, episodeDir, digest);
      }
      await stageOracle('x8-option-independence', {}, episodeDir, digest);
      await stageOracle('x9-mode-invariants', {}, episodeDir, digest);
      if (episode % 4 === 0) await stageOracle('x9b-pre-plan-leak', {}, episodeDir, digest);
      await detProbe()?.state(ctx, `episode ${episode} oracles done`);

      await teardown(episodeDir);
      ctx.metric('episodes');
    }

    if (traceOut) {
      const written = dumpTrace(null);
      if (written !== null) ctx.log(`trace written to ${written}`);
    }
    ctx.setMetric('exportMoves', seq);
  },
};
