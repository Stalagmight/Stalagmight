# Testing Recipes

Lookup reference: commands, per-framework patterns, mock and fixture
conventions, and the traps that have cost real debugging time.

For *which tier a test belongs in*, and the testability contract a component has
to satisfy, see the [Testing Guide](TESTING.md).

## The undo fuzz gate (Rust tier)

`src-tauri/crates/core/src/core/state/fuzz_undo_tests.rs` runs the undo/redo fuzzing model
directly against `AppState`, as ordinary tests. It gates CI, alongside the
metamorphic, file-format, crash-timing, adversarial and concurrency gates below;
the agent harness never runs there. `open_undo_bugs_tests.rs` is no longer
`#[ignore]`d - every bug it pinned is fixed, so its tests now run as regression
guards on every CI run.

```bash
# The gate. Runs with the default suite; ~10 s for all four tests.
cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_undo

# See the classified known-bug counts (they are printed, not asserted).
cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_undo -- --nocapture --test-threads=1
```

Four gate tests: `undo_redo_fuzz_gate` (in-memory), `_gate_persistence` (one
save/close/reopen per seed), `_gate_multi_crossing` (three), and
`_gate_combined` (below).

**Five oracles per run.** Four compare what a history move DID - the project
snapshot per depth, every cross-section merge, the app's own integrity verdict,
and the adjusted geometry the map draws. The fifth compares what the entry SAYS:
the `UndoHistoryItem` rows `get_undo_stack` returns, which are the history
dropdown. It runs in two halves - the entry-intrinsic triple
(`timestamp|commandType|description`) is pinned per depth for the life of a run,
and the whole item including `previewData` is compared across every save/reload
crossing. `previewData` is left out of the first half on purpose: it resolves
section NAMES against the live project (see
[Undo/Redo](undo-redo.md#section-name-resolution)), so it is stable exactly
where the project is.

**Reproducing a specific known bug.** The module carries a BUG IDENTIFICATION
MATRIX in its header: for every finding on
[issue 413](https://github.com/Stalagmight/Stalagmight/issues/413), the mode,
seed and move count under which this tier used to reach and classify it, and the
fix that closed it - every finding is FIXED now, `CLASSIFIERS` is empty, and
each row's sweep reaches nothing at all.
Four environment flags switch the modes on, and the gate uses none of them
because each one CONSTRUCTS a known defect:

| flag | what it adds | the findings it exists for |
|---|---|---|
| `STALAGMIGHT_RUST_FUZZ_JUMPS=1` | multi-step `undo_to`/`redo_to` jumps | 7 |
| `STALAGMIGHT_RUST_FUZZ_CLOSURES=1` | a real loop closure in the starting project | 6, 14 |
| `STALAGMIGHT_RUST_FUZZ_DIVERGENCE=1` | shot ids that diverge from positional order, with every id-bearing entry contiguous on top, before the save | 8, 12, 13 |
| `STALAGMIGHT_RUST_FUZZ_CLOSURE_SHAPES=1` | two closures with the first one removed, and an orphaned closure built just before the save | 4, 10 |
| `STALAGMIGHT_RUST_FUZZ_CROSSINGS=n` | n save/close/reopen boundaries per persistence run instead of one | 8, 13 - without any constructed mode |

So, for example, findings 8 and 12 in twelve moves:

```bash
STALAGMIGHT_RUST_FUZZ_DIVERGENCE=1 STALAGMIGHT_RUST_FUZZ_SEEDS=1 \
  STALAGMIGHT_RUST_FUZZ_ITERATIONS=12 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_seed_survey -- --ignored --nocapture
```

Every seed prints one summary line per mode; with `CLASSIFIERS` empty a clean
run reaches nothing, so any `classified hits: ...` entry is a NEW open bug or
float dust past tolerance rather than a known finding.

**Long horizon.** Thousands of moves per seed, past the app's own
`undo_history_limit`, so three things nothing shorter reaches get exercised: the
FIFO trim and the sliding window it creates, float dust that ACCUMULATES past
tolerance over dozens of visits to the same depth (classified as its own
finding rather than reported as a fresh regression, then re-baselined), and id
allocators / collection growth over a long run.

```bash
STALAGMIGHT_RUST_FUZZ_HORIZON=1 STALAGMIGHT_RUST_FUZZ_SEEDS=4 \
  STALAGMIGHT_RUST_FUZZ_ITERATIONS=10000 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib undo_redo_fuzz_horizon -- --ignored --nocapture
```

The mode changes the move mix while it is CLIMBING (85/8/7 mutate/undo/redo, and
no jumps) for a measured reason: at the ordinary mix the depth random-walks
around 25 and never reaches the limit at all, because every jump lands the
cursor near the floor and the next mutation truncates everything above it.

**Drawn config.** `STALAGMIGHT_RUST_FUZZ_CONFIG=1` fuzzes the app as well as the
moves: each seed draws its own `undo_history_limit` (1, 2, 3, 5, 8, 13, 20, 40,
100 = the app's default, and `iterations + 1` = unreachable) from a SECOND
generator seeded `seed ^ CONFIG_STREAM`, so no existing stream moves and every
repro above still reproduces. At a limit of 1 every push trims, which is the
sliding window under maximum pressure at 60 moves instead of 10,000.

```bash
STALAGMIGHT_RUST_FUZZ_CONFIG=1 STALAGMIGHT_RUST_FUZZ_SEEDS=32 \
  STALAGMIGHT_RUST_FUZZ_ITERATIONS=60 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_seed_survey -- --ignored --nocapture
```

The drawn value is printed at the end of every summary line and of every failure
payload (`..., undo_history_limit=3`) - a seed alone is not a repro once the same
moves can run against differently configured apps. `undo_persist_limit` is
deliberately NOT drawn: it is read from the config struct at save time and
`save_project` writes that struct back out through `Config::save()`, so a drawn
value would outlive the moves that drew it.

**Config isolation is on by default, and these modules ask for it anyway.**
`AppState::new()` LOADS the global config and every save reaches
`note_recent_project` -> `Config::save()`, which WRITES it. `Config::config_path()`
resolves PER THREAD under test (see [Config isolation](#config-isolation) below),
so every test in the nine fuzz/property modules is off the real
`~/.config/stalagmight/config.toml` and off every sibling's file whether it says
so or not; they still open with `let _config = crate::test_helpers::isolated_config();`
because that gives the whole test ONE dir whose path it can name. To check that
nothing in a module resolves the config through the ENVIRONMENT (which is what a
missed `real_config_resolution` would look like), point the config dir at a temp
dir and assert nothing was written:

```bash
XDG_CONFIG_HOME=$(mktemp -d) \
  cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
  -- core::state::metamorphic_undo_tests
# then: find "$XDG_CONFIG_HOME" -type f   -> must be empty
```

**Combined.** `STALAGMIGHT_RUST_FUZZ_COMBINED=1` turns on horizon, closures,
closure shapes, jumps, three crossings and the drawn config at once - the
interactions rather than the modes. It is what `undo_redo_fuzz_gate_combined`
runs (seeds 15 and 21 x 300 moves, ~1.8 s, in the same band as the other gates),
and re-mining that gate's seeds means running the survey over the regime:

```bash
STALAGMIGHT_RUST_FUZZ_COMBINED=1 STALAGMIGHT_RUST_FUZZ_SEEDS=40 \
  STALAGMIGHT_RUST_FUZZ_ITERATIONS=300 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_seed_survey -- --ignored --nocapture
```

Two facts that combining exposed, both fixed only for the combined regime so no
existing seed moves (the module header has the detail): a horizon run could
never draw a jump, because its climb test compares a difference the FIFO trim
pins below the cap; and a horizon run could never cross the save boundary,
because the crossing guard measured DEPTHS against a limit that counts ENTRIES.

**Soak.** Same engine, no iteration budget, and with the features the gate
leaves off (multi-step `undo_to`/`redo_to` jumps, loop closures, and both
constructed shapes) because each reaches open findings:

```bash
STALAGMIGHT_RUST_FUZZ_SEEDS=200 STALAGMIGHT_RUST_FUZZ_ITERATIONS=1000 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib undo_redo_fuzz_soak -- --ignored --nocapture
```

**All three sweeps run their seeds in parallel.** The soak, the survey and the
horizon run spread their seeds over a worker pool; the three CI gates do not
(they are seconds, and libtest is already running them alongside every other
test in the binary). Measured at 32 seeds x 300 moves x 2 modes: 13.9 s at one
worker, 3.7 s at four, 2.7 s at eight.

| knob | effect |
|---|---|
| `STALAGMIGHT_RUST_FUZZ_THREADS=n` | n workers |
| unset | `available_parallelism` less one core, never more workers than there are runs |
| `STALAGMIGHT_RUST_FUZZ_THREADS=1` | the sequential run, on the same code path |

A run is a pure function of `(seed, plan)` - integer-only PRNG in the `Fuzzer`,
an `AppState` per run, a `TempDir` per persistence run - and the results are
merged in SEED order, so **the printed output is byte-identical at any thread
count**, including the per-seed summary lines, the clean list, the failure
buckets and the classified totals. Two things follow:

- The `oracle-us` triple in each summary line is WALL time and inflates under
  contention. Measure the oracle cost at `THREADS=1`.
- A panicking seed is caught, attributed to its seed and mode, and re-raised
  after every other seed has finished, so one bad seed no longer costs a soak
  the rest of its evidence.

**The red tests.** None are left: every bug they pinned is fixed, so none is
`#[ignore]`d any more. The 49 reductions in `open_undo_bugs_tests.rs` and the
two seed-pinned tests in the fuzz module
(`finding_14_no_insert_leaves_a_merge_out_of_alignment`,
`bug_7_a_jump_is_n_single_steps`) all run as regression guards on every CI run.
A new red test still goes in `#[ignore]`d, and the `#[ignore]` is deleted when
its bug is fixed.

```bash
# libtest takes several filters and ORs them; a shell-regex filter matches
# nothing, since cargo's own filter is a plain substring.
cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib -- open_undo_bugs finding_14 bug_7
```

## The metamorphic gate (Rust tier)

`src-tauri/crates/core/src/core/state/metamorphic_undo_tests.rs` asks a question the
roundtrip fuzzer structurally cannot: do two DIFFERENT ways of asking for the
same edit agree? The roundtrip oracle compares a state against itself at an
earlier moment, so a defect that is faithfully undone is invisible to it - and
the delete/split family is full of places where that can happen, because each
entry point is its own hand-written implementation of an operation another entry
point also implements.

It builds one project, saves it, loads it into TWO `AppState`s (which is what
makes a strict comparison possible at all - `Shot::uuid` is a v4 uuid, so two
independently-built projects share no identity), drives each down one route, and
compares them on five oracles: the roundtrip fuzzer's four STATE oracles,
re-used as free functions so the two engines cannot disagree about what a
difference is, plus the id ALLOCATORS. (The fuzzer's fifth, entry content, is
not among them: it compares one `AppState`'s stacks against what that same app
said earlier, so there is no second route to compare it across.)

```bash
# The gate. Runs with the default suite; ~11 s.
cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib metamorphic

# The sweep, and the tool for re-choosing the metamorphic GATE_SEEDS.
STALAGMIGHT_RUST_META_SEEDS=40 STALAGMIGHT_RUST_META_REPEATS=4 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib metamorphic_fuzz -- --ignored --nocapture
```

| env | effect |
|---|---|
| `STALAGMIGHT_RUST_META_SEEDS` | seeds 1..=n (default 40) |
| `STALAGMIGHT_RUST_META_REPEATS` | pair instances per seed, each on fresh twins (default 4) |
| `STALAGMIGHT_RUST_META_PRELUDE` | random commands before the twins split (default 12) |
| `STALAGMIGHT_RUST_META_CLOSURES` | put a real loop closure in the base project |
| `STALAGMIGHT_RUST_META_VERBOSE` | print every divergence, not just the seed summary |

Seven pairs, and every one of them is promised somewhere - two in the code's own
doc comments, in the word "generalizes". `a-failed-batch-is-a-no-op` is the odd
one out and asserts a DIFFERENCE: a failed batch must be a strict no-op while
the same commands run singly apply up to the failure.

Every pair is checked twice - on the states the two routes produce, and on the
states they UNWIND to. The second is where undo's per-entry-point special cases
live. It compares the two unwound twins to each other rather than to the
baseline, because findings 1, 2, 3 and 5 are all "an unwind does not return to
where it started"; the baseline distance is measured and printed instead of
asserted, so a regression in it shows up in the summary diff without being a
false failure today.

It found finding 22 on its first run. The classified-hit ids it prints follow
the same delete-when-fixed contract the roundtrip classifiers do, and `EXPLAINS`
is empty now: its last entry,
`observation-split-routes-disagree-on-station-comment-keys`, was triaged out as
this engine's own sort-before-mask rather than a defect in either route (see
`resort_set_like`), so nothing is explained away any more.

## The file-format gate (Rust tier)

`src-tauri/crates/core/src/core/state/format_property_tests.rs` is the only thing that
looks at the `.stalx` itself. Byte-comparison is the wrong instrument for a
SQLite database (page allocation, freelists, the change counter), so the
comparison is on the database's LOGICAL content: every user table, every row,
sorted, with the save's own wall clock masked by value shape.

```bash
cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib format_

STALAGMIGHT_RUST_FORMAT_SEEDS=60 STALAGMIGHT_RUST_FORMAT_CLOSURES=1 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib format_property_sweep -- --ignored --nocapture
```

Three properties: save -> load -> save is a fixpoint (finding 23, fixed - the
classifier is deleted and the check records zero classified hits); the loader is
a function of the file (four loads per seed, green); and the
documented legacy fallbacks work, tested by writing a current file and
degrading it with SQL - dropping the gauge columns `read.rs`'s
`has_gauge_columns` probe expects to be missing, winding `PRAGMA user_version`
back, desynchronising the two version markers, and dropping the undo tables.
No fixture is involved: every degraded file is built from a fresh one.

The fixpoint property still does not assert that the drift settles - and there
is no drift left to settle: the re-roll that made "settles after one cycle" hold
four times and fail four over eight gate runs was root-caused to a `HashMap` in
`UndoData::previous_offsets`, which now serializes in key order, so no
generation-1-to-2 pair differs at all. What it asserts is that no save/load
cycle changes ANYTHING in the file's logical content: finding 23 is fixed,
`is_finding_23` is deleted, and the "except bounded float dust" clause is gone.

## The crash-timing gate (Rust tier)

`src-tauri/crates/core/src/core/state/crash_property_tests.rs` tests the save pipeline's
central promise - "a temp file with atomic rename to prevent data loss on crash
or power failure" ([Saving](saving.md)) - by simulating a kill at each of its
boundaries. Nothing else does: the file-format engine above is the only other
thing that looks at a `.stalx`, and it only ever looks at files a save
FINISHED writing.

```bash
cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib crash_property

STALAGMIGHT_RUST_CRASH_SEEDS=60 STALAGMIGHT_RUST_CRASH_MOVES=26 \
  STALAGMIGHT_RUST_CRASH_EXTRA=12 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib crash_property_sweep -- --ignored --nocapture
```

The crash is SIMULATED by constructing the exact files a kill would leave -
a truncated `<stem>.stalx.tmp` at six byte prefixes, a complete one waiting for
a rename that never came, one caught mid-transaction with its hot rollback
journal, the post-rename states, and a destination caught mid-transaction -
rather than by racing a signal. `rename(2)` is atomic and the temp is a private
file, so the reachable set is small, enumerable and copyable, which is what
makes this deterministic and CI-fast. Both reference files are written by the
real pipeline.

The oracle: no panic, the destination is EXACTLY the old file or EXACTLY the
new one (never a blend, and the engine names which it must be, because every
pre-rename state leaves the destination byte-intact), it opens, and it can be
saved again leaving no staging file behind. That last step is what walks the
next save into the orphaned-journal hazard `project_io.rs:846` creates: the
leftover-temp cleanup removes the temp but not its `-journal`, so the next save
builds a fresh temp database at that name straight on top of a hot journal.
That one is measured benign - the save succeeds and SQLite discards the stale
journal, so it does not accumulate - and the engine fails if anything disarms
the hazard before the recovery save reaches it.

Green over 60 seeds / 720 states. The one thing it measures rather than
asserts is what a TORN temp does when opened directly (the recovery UI lists
`.tmp` files as restore candidates and `restore_from_backup` validates
nothing): 306 refused, 11-13 loaded as the interrupted save, 41-43 loaded with
the survey intact and the undo history gone. That last split is the one number
that moves between runs, and it moves for finding 21's reason - `HashMap` order
in the writer decides which row a fixed byte offset cuts through. The 306 does
not move, and neither does any total.

## The adversarial gate (Rust tier)

`src-tauri/crates/core/src/core/state/adversarial_tests.rs` fuzzes the command layer's
VALIDATION, which every other generator here avoids by construction.

```bash
cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib adversarial

STALAGMIGHT_RUST_ADV_SEEDS=200 STALAGMIGHT_RUST_ADV_MOVES=400 \
  STALAGMIGHT_RUST_ADV_CLOSURES=1 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib adversarial_sweep -- --ignored --nocapture
```

Two invariants: a rejected command is a PERFECT no-op (all five oracles, not
just the stacks), and no command ever panics. The second is why every move runs
inside `catch_unwind` - and why the classifier matches a panic's LOCATION
rather than its message, since "attempt to add with overflow" says nothing
about where and a text matcher would swallow every future overflow anywhere in
the app.

Draws are weighted towards the hand-written `execute_*` entry points, which do
their own argument checking after taking the project lock. Measured at 60 seeds
x 300 moves: 10 696 calls, 9 331 rejected, zero no-op violations.

## The concurrency gate (Rust tier)

`src-tauri/crates/core/src/core/state/concurrency_tests.rs` drives ONE `AppState` from
several threads. `AppState`'s `command_lock` exists to serialize
execute/undo/redo, and [Undo/Redo](undo-redo.md) documents a lock ordering
discipline underneath it; nothing exercised either, because every other engine
here is single-threaded.

```bash
cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib concurrency

STALAGMIGHT_RUST_CONC_SEEDS=40 STALAGMIGHT_RUST_CONC_THREADS=8 \
  STALAGMIGHT_RUST_CONC_OPS=2000 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib concurrency_soak -- --ignored --nocapture
```

Inputs are seeded per thread so a failing run replays in what each thread
ASKED for; the interleaving is not reproducible and no assertion depends on it.
The four oracles are liveness (a whole-run deadline plus a no-progress window),
no panic, no poisoned mutex, and a post-run consistency walk.

The deadline is generous on purpose - Windows and macOS hand mutexes over
differently from Linux and a tight bound turns a slow CI box into a bug report.
A real deadlock never finishes; the gate finishes in about two seconds.

One trap, recorded because it cost a flaky run: waiting on a progress COUNTER
reads a worker that panicked as one that is stuck, so a known assert gets
reported as a deadlock. Poll `JoinHandle::is_finished` instead, and cross-check
that every unwound worker was seen by the panic hook.

## Corpus guidance, and why it is not wired into anything

`fuzz_corpus_sweep` (`#[ignore]`d) measures whether remembering the shapes a
sweep reaches lets later runs reach more of them. `topology_signature` is a
twelve-feature integer fingerprint of a project's shape, and a corpus entry is
just `(seed, move-count)` - which is all it needs to be, since every run in
that module is a pure function of its seed.

**The answer is no.** At 80 runs x 60 moves each way: unguided reaches 78
distinct signatures, guided 73, with identical classifier id sets. 78 of 80
unguided runs are already distinct, so the corpus admits nearly everything and
guidance just spends half the budget replaying. The mechanism is kept, and
deliberately not wired into any gate; the test's own doc comment carries the
numbers and the two changes that would make it pay.

**The second answer is also no.** One of those two changes - admission keyed on
a RARE structural bit rather than on whole-signature novelty - was then measured
in full: `fuzz_rarity_sweep` and `fuzz_rarity_carryover`
(`src-tauri/crates/core/src/core/state/fuzz_rarity_tests.rs`, both `#[ignore]`d), six
repetitions of 80 runs x 60 moves per arm across four admission predicates and
two regimes.

```bash
STALAGMIGHT_RUST_FUZZ_THREADS=8 STALAGMIGHT_RUST_FUZZ_RARITY_BUDGET=80 \
  STALAGMIGHT_RUST_FUZZ_RARITY_REPS=6 STALAGMIGHT_RUST_FUZZ_ITERATIONS=60 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_rarity_sweep -- --ignored --nocapture
```

Three things came out of it, and the third is the one to remember:

- Keyed on a finished run's END state, admission is barely faithful: a corpus
  entry replays `(seed, n)` as n mutations, the admitted run had also undone and
  redone, and 8 of 9 entries (plain regime) deliver a state carrying none of the
  bits they were admitted for. `fuzz_rarity_carryover` measures exactly that.
- Keyed on the prologue's own trajectory, which is exact, guidance reaches the
  regime's rare classified findings 41% sooner **counted in runs**, 17% sooner
  **counted in moves**, and no sooner at all **counted in wall time**.
- A run is not a unit of compute. A continuation inherits a bigger project, so it
  draws more moves, and each move costs more because every oracle canonicalizes
  the whole project (2.87 ms per move against 2.34). Score a fuzzing heuristic
  per run and you pay it for being expensive; the first experiment handed
  guidance an 80% compute subsidy without noticing.

Both write-ups stay next to each other; neither mechanism is wired into a gate.

**Choosing gate seeds.** `fuzz_seed_survey` catches each run's panic instead of
propagating it, so one bad seed does not hide the twenty after it, and prints
which seeds are clean plus what the rest failed on. Re-run it as findings are
fixed and widen `GATE_SEEDS`:

```bash
STALAGMIGHT_RUST_FUZZ_SEEDS=40 STALAGMIGHT_RUST_FUZZ_ITERATIONS=100 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_seed_survey -- --ignored --nocapture
```

`COMBINED_SEEDS` is chosen the same way, from the same tool under `COMBINED=1`,
with one extra criterion: between them the pair has to exercise the regime's
three structural properties (it trimmed, it crossed more than once, it jumped).
The old `COMBINED_CLASSIFIED` id-set pin is inverted - the gate now asserts the
regime reaches nothing.

**Debugging a failure.** `STALAGMIGHT_RUST_FUZZ_TRACE=1` streams every move as it
happens, including the line the classifier writes when a hit is explained - which
matters, because a classified hit RESYNCS the model and can arm a cascade, so a
later failure that looks spontaneous usually is not (that is exactly how finding
15 came to be filed as its own defect). Tracing pins the sweeps to ONE worker
unless `STALAGMIGHT_RUST_FUZZ_THREADS` says otherwise: a dozen seeds
interleaving their moves into one stdout is not a move log.
`STALAGMIGHT_RUST_FUZZ_VERBOSE=1` prints the whole panic payload per seed in the
survey, and every differing path rather than the first eight.

The app also has assertions of its own - `debug_assert_merge_alignment` is
`#[cfg(test)]`, so it exists ONLY in this tier. The fuzzer catches that one
specifically, checks the message is that assert at one of the call sites finding
14 names, records it as a classified hit and stops the run (the unwind poisoned
the project mutex, so nothing after it would be measuring the app). Any other
panic is re-raised verbatim.

Runs are deterministic: the same seed replays identically, forever, on every
platform (integer-only PRNG, and no float is used to make a decision).

**Minimizing a failure.** A failing seed is a repro in the sense that it fails
and in no other sense - nobody reads 200 moves and says which three are the bug.
`fuzz_shrink` (`fuzz_shrink_tests.rs`) records the run's move stream, then runs
SIGNATURE-MATCHED ddmin over it: remove a chunk, replay the rest through the same
engine and the same four oracles, keep the candidate only if it still fails THE
SAME WAY, halve the chunk when nothing more can go.

```bash
# Record seed 1 under divergence mode, shrink for one named finding.
STALAGMIGHT_RUST_FUZZ_SHRINK=1 STALAGMIGHT_RUST_FUZZ_DIVERGENCE=1 \
  STALAGMIGHT_RUST_FUZZ_ITERATIONS=200 \
  STALAGMIGHT_RUST_FUZZ_SHRINK_TARGET=finding-8-stale-id-deletes-the-wrong-shot \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_shrink -- --ignored --nocapture

# Replay the minimized trace it wrote, and ASSERT the same failure comes back.
STALAGMIGHT_RUST_FUZZ_REPLAY=/tmp/stalagmight-shrink-seed1-3moves.json \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_replay -- --ignored --nocapture
```

The plan comes from the SAME knobs the survey reads (`ITERATIONS`,
`PERSISTENCE`, `JUMPS`, `CLOSURES`, `DIVERGENCE`, `CLOSURE_SHAPES`, `CROSSINGS`,
`HORIZON`, `CONFIG`, `COMBINED`), so a seed that failed under a sweep is shrunk
under exactly the environment that found it. `SHRINK_TARGET` is optional - the default is whatever
failure the recorded run produced. `SHRINK=<path.json>` shrinks a trace file
further instead of a seed, `SHRINK_OUT` picks where the minimized trace is
written (default: a temp path, printed at the end), and `SHRINK_PROBES` caps the
probe budget (default 2000).

| what is being shrunk | what "the same failure" means |
|---|---|
| a KNOWN finding (the run completes, the hit is counted) | the finding id is in the candidate's `classified hits` |
| anything else (the run panics) | `failure_signature` matches: the headline plus the first differing snapshot path, every run of digits collapsed to `#` |

Signature matching is the whole discipline: a candidate that fails DIFFERENTLY -
another finding, another panic, or not at all - is rejected, so the minimum is a
repro of the failure that was asked about rather than of whatever the search
drifted onto. Measured, on this branch:

| repro | before | after | probes |
|---|---|---|---|
| `finding-8-stale-id-deletes-the-wrong-shot` (divergence, seed 1 x 200) | 199 moves | 0 moves - the constructed prologue and the closing walk ARE the repro | 15 |
| the open seed-10 observation (persistence, `CROSSINGS=3`, seed 10 x 60) | 59 moves | 3 moves | 105 |
| an unclassified `section_offsets` panic (persistence, `CROSSINGS=3`, seed 1 x 120) | 122 moves | 5 moves | 98 |

Two things the mechanism guarantees, and one it does not:

- A move whose target was shrunk away is SKIPPED with a note rather than fatal
  (the trace names shots by `Shot::uuid`, which survives both the loader's
  renumbering and the allocator; sections and closures by id, mapped through what
  each move created). Replay has to be a total function or a candidate that kills
  the harness looks exactly like one that reproduces the bug.
- Probes in a ddmin round run in PARALLEL on the same worker pool the sweeps use
  and the first reproducing complement IN INDEX ORDER is accepted, so the result
  does not depend on `STALAGMIGHT_RUST_FUZZ_THREADS`.
- It does not shrink the PLAN or the values inside a move. A mode is on or off,
  and the shot data a move carries is replayed verbatim.

Seven tests gate the machinery on every CI run: recording does not change what a
run does, a FULL trace replays to exactly the run it was recorded from, the
ddmin schedule reaches the known minimum of a synthetic predicate, the shrinker
minimizes an injected failure, and three signature-matching tests pin what "the
same failure" means.

## Commands

The authoritative pre-push list lives in the Pre-Push Checklist in
[`CLAUDE.md`](../CLAUDE.md). These are the per-tier commands.

### Vitest (unit + component)

```bash
pnpm exec vitest run                        # one-shot, all files
pnpm exec vitest run src/store              # one-shot, a subtree
pnpm exec vitest run --coverage             # reports under docs/coverage/
CI=1 pnpm test:all                          # vitest, then the full Playwright suite
```

**Trap: `pnpm test` runs vitest in WATCH mode locally.** The `test` script is
bare `vitest`, which only runs once when it detects `CI`. In a local or agent
shell with no `CI` env it watches forever and a piped or backgrounded invocation
never returns. Use `pnpm exec vitest run` (or `CI=1 pnpm test`) for a
deterministic run. CI is unaffected.

### Playwright

```bash
pnpm test:ui                # full suite
pnpm test:ui:smoke          # the @smoke subset (what CI's smoke job runs)
pnpm test:ui:headed         # visible browser
pnpm test:ui:debug          # Playwright inspector, step through
pnpm exec playwright test e2e-playwright/theme.spec.ts        # one spec
pnpm exec playwright test --grep "closes on Escape"           # by title
pnpm lint:e2e-parallel      # parallel-safety scanner (a CI smoke-job gate)
```

**Trap: `pnpm <script> -- <flags>` does not work.** pnpm forwards the `--`
literally, so `pnpm test:ui -- --reporter=json` runs
`playwright test -- --reporter=json`, and Playwright reads the `--` as a
positional test filter: "No tests found", exit in about a second. Use
`pnpm exec playwright test <flags>`.

Playwright starts its own server on `:1420` via the `webServer` config. Which
server depends on where you are: **locally the Vite dev server**, so the
interactive loop keeps HMR, and **in CI a built bundle** behind `vite preview`,
because serving the app unbundled costs one request per module on every `goto`
and that dominates per-test setup under parallel workers. `PW_PROD_SERVER=1`
forces the built bundle locally, `PW_PROD_SERVER=0` forces the dev server in CI.
The build sets `VITE_E2E=1`, which is what keeps the `window.__E2E_TEST__` store
hooks the specs drive - a plain production build strips them.

Locally it reuses an already-running server. If you run more than one clone of
this repo, that reuse will silently point the suite at the *other* clone's
server, so you test the wrong code with no error. Do not kill theirs: run the
suite on a free port with `PW_PORT=1421 pnpm test:ui` - `PW_PORT` repoints the
webServer, the `baseURL` and the reuse probe together.

### Rust

```bash
# What CI runs. --workspace and --features telemetry are REQUIRED (see below).
TELEMETRY_ENDPOINT=http://localhost/dummy \
  cargo test --manifest-path src-tauri/Cargo.toml --workspace --features telemetry

cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-parsers        # one crate
cargo test --manifest-path src-tauri/Cargo.toml -- --nocapture   # app crate, show println!

# Vendored compass_data is [workspace]-excluded, so --workspace never tests it.
# CI gates it as its own check-job step (#494); no telemetry feature or env var.
cargo test --manifest-path src-tauri/vendor/compass_data/Cargo.toml
```

**Trap: a plain `cargo test` can pass while CI fails, twice over.** Without
`--workspace` only the app crate's tests run - the member crates under
`src-tauri/crates/` are skipped entirely. And the `registry_covers_all_*`
telemetry-registry coverage tests are gated behind the `telemetry` feature, so
a featureless run filters them out. The telemetry member's `build.rs` also
*requires* `TELEMETRY_ENDPOINT` to be set when building with that feature.

#### Running one crate

The backend is a cargo workspace: the `stalagmight` app package at `src-tauri/`
plus eight members under `src-tauri/crates/` (`stalagmight-models`,
`-telemetry`, `-parsers`, `-persistence`, `-export`, `-speleodb`, `-core`, and `stalx-reader`).
Select one with `-p` for fast iteration:

```bash
# The fuzz / metamorphic / crash / adversarial / concurrency engines all live
# in stalagmight-core (crates/core/src/core/state/):
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib fuzz_undo

# A member without the telemetry feature needs neither the feature nor
# TELEMETRY_ENDPOINT:
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-export dpi
```

`--features telemetry` only combines with the packages that define the feature -
`stalagmight` (the app), `stalagmight-telemetry` and `stalagmight-speleodb`;
cargo rejects it for any other `-p` selection (the error lists the packages
that have it). The formerly-`#[cfg(test)]` shared helpers (`TestDb`,
`isolated_config`, the AppState test setters) are stalagmight-core's
`test-support` feature; it is enabled automatically for the crates whose
dev-dependencies request it, so test commands never need to pass it by hand.

**Trap: on Windows, run the Rust tests via `./scripts/test.ps1`.** That script
sets `STALAGMIGHT_TEST_MANIFEST=1` (and the telemetry feature + endpoint).
Without it every `cargo test` harness binary aborts at startup with
`STATUS_ENTRYPOINT_NOT_FOUND` (`0xC0000139`) before a single test runs: the
loader binds to comctl32 v5, which lacks `TaskDialogIndirect` (statically
imported via rfd / tauri-plugin-dialog). The env var makes `build.rs` delay-load
comctl32 for test binaries. The shipping app is unaffected.

### Agent harness (real Tauri app, on-demand)

```bash
pnpm agent:smoke                                    # plumbing self-test
pnpm agent:scenarios                                # scenario smoke (crash gate)
pnpm agent:journeys                                 # golden flows (behavioral gate)
pnpm agent:scenario fractal-stress --iterations 4   # large-survey stress tier (#549)
```

Runs on the repo-pinned Node (`nvm use`). See [Agent Harness](agent-harness.md).
The retired WDIO suite's residual coverage lives in the journeys and the
`fractal-stress` scenario (#549).

### Typecheck

```bash
pnpm tsc --noEmit          # src/ only - tsconfig.json includes nothing else
pnpm typecheck:agent       # e2e/agent + e2e/lib (tsconfig.agent.json)
pnpm typecheck:playwright  # e2e-playwright/ + playwright.config.ts
```

The first two are `check` gates, the third is a `smoke` gate - it has to be,
because `check` path-ignores `e2e-playwright/**`.

Some non-test files are ungoverned - `scripts/**/*.ts`, `vite.config.ts`, and
`src-tauri/bindings/` are in no tsconfig's `include`. (The old ungated
`tsconfig.e2e.json` went with the WDIO suite, #549.)

Note the strictness difference: `tsconfig.playwright.json` is `strict`, but
unlike the root config it does not set `noUncheckedIndexedAccess`,
`exactOptionalPropertyTypes`, `noUnusedLocals`/`noUnusedParameters`,
`noImplicitReturns`, or `noFallthroughCasesInSwitch`. "The specs are
type-checked" is true but weaker than what `src/` gets.

The agent harness runs under `tsx`, which transpiles without type-checking, and
it is exercised on demand only, so `typecheck:agent` (a CI `check` step) is the
only thing standing between it and silent rot.

### Coverage

```bash
pnpm exec vitest run --coverage   # unit + component -> docs/coverage/
pnpm coverage:ui             # instrument the app, run Playwright
pnpm coverage:merge          # merge raw E2E output into docs/coverage/
```

## Vitest patterns

The suite is defined by `vite.config.ts` `test.include`, which is wider than
`src/`: it also picks up `scripts/**/*.{test,spec}.ts` (golden tests for the
repo's own Node tooling), `e2e/agent/lib/**/*.test.ts` and
`e2e/agent/os/**/*.test.ts` (agent-harness helper and OS-input logic), and
`e2e/lib/**/*.test.ts` (shared harness helpers). A new vitest home outside those
five globs is a deliberate one-line addition there.

Tiering is by **file extension**, via `environmentMatchGlobs`:

| Extension | Environment | Use for |
|-----------|-------------|---------|
| `*.{test,spec}.ts` | `node` | Pure logic - stores, utils, selectors, formatters. No DOM. |
| `*.{test,spec}.tsx` | `jsdom` | Rendering a React component with Testing Library. |

Naming a DOM test `.ts` puts it in the `node` environment where `document` does
not exist. Use `.tsx` even when the file contains no JSX.

`src/test/setup.ts` registers jest-dom matchers, provisions the `#dialog-root`
element that `DialogWrapper` portals into (without it, every dialog component
test renders nothing), and auto-unmounts React trees between tests. The DOM
pieces load lazily behind a `document` guard, so `node` tests pay no DOM cost.

### A component test

Render the real component and assert on accessible roles and handler calls:

```tsx
import { it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Button } from './Button';

it('fires onClick when activated', async () => {
  const onClick = vi.fn();
  render(<Button onClick={onClick}>Import</Button>);
  await userEvent.click(screen.getByRole('button', { name: 'Import' }));
  expect(onClick).toHaveBeenCalledTimes(1);
});
```

`src/components/common/Button.test.tsx` is the harness smoke test. Components
that read Zustand stores or `react-i18next` need those mocked or provided; keep
early component tests on leaf components to avoid heavy setup.

This tier mocks IPC - persistence is owned by the Rust tier, see
[Rust patterns](#rust-patterns).

## Playwright patterns

Specs live in `e2e-playwright/`, flat, one file per feature area, alongside:

```
e2e-playwright/
├── helpers/app.ts          # shared setup + waiters (import from here)
├── mocks/tauri-ipc.ts      # the mocked Tauri IPC backend
├── fixtures/coverage.ts    # the test fixture every spec imports `test` from
├── fixtures/*.dmp          # binary survey files. Nothing reads them: the mock
│                           #   fabricates data and never parses a file
└── <feature>.spec.ts
```

`playwright.config.ts`: Chromium only, `fullyParallel`, base URL
`http://localhost:1420`, 30 s test timeout, 5 s expect timeout, retries 2 in CI
and 1 locally (`PW_RETRIES` overrides), workers `100%` in CI (`PW_WORKERS`
overrides; a bare integer and a percentage string both work, because the config
coerces the integer form itself - raw Playwright would reject it, #132). Traces
on first retry, screenshots and video on failure, all under `test-results/`.

### Basic structure

```typescript
import { test, expect } from './fixtures/coverage';
import { setupMockedPage, createProject, waitForMapReady } from './helpers/app';

test.describe('Feature Name', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('does the thing @smoke', async ({ page }) => {
    await createProject(page, 'Test Project');
    await waitForMapReady(page);
    await expect(page.getByTestId('map-canvas')).toBeVisible();
  });
});
```

Import `test` from `./fixtures/coverage`, not from `@playwright/test` - that
fixture is what wires up E2E coverage collection.

### Waiters

Use these instead of `waitForTimeout`. All from `helpers/app.ts`:

| Helper | Waits for |
|--------|-----------|
| `setupMockedPage(page)` | Mocks installed and the app booted |
| `waitForAppReady(page)` | App shell mounted |
| `waitForMapReady(page)` | The map is initialized and interactive |
| `waitForMapIdle(page)` | Pan/zoom animation settled |
| `waitForActiveTool(page, tool)` | The toolbar has switched tool |
| `waitForDialogClosed(page, testId)` / `waitForNoDialogs(page)` | Dialog teardown |
| `waitForDrawnStations` / `waitForDrawnShots` | The custom WebGL layer has drawn N features |

`helpers/app.ts` also has seeding helpers (`createSectionViaStore`,
`seedProjectWithSections`), menu navigation (`openAppMenu`,
`openSettingsFromMenu`, `openDebugFromMenu`), the DMP wizard drivers
(`openDmpSelectionDialog`, `proceedToDmpImportWizard`, `placeAllSections`), and
canvas readback (`getDrawnGeometry`, `getStationScreenPositions`,
`readCanvasPixel`, `countCanvasInk`). Read the file before writing a new
one-off helper.

A waiter that is genuinely local to one spec stays in that spec -
`waitForZoomChange` in `zoom-tool.spec.ts` is the pattern.

### Parallel-safety invariant

This tier runs `fullyParallel` at multiple workers. That is safe because of the
architecture, not per-test discipline:

- IPC is mocked **per page**; all backend state lives on `window.*` inside each
  isolated browser context.
- The single shared `webServer` (Vite, `:1420`) is **stateless** - it serves the
  bundle only.
- The one real disk write (`fixtures/coverage.ts`) is uniquely named per
  `test + workerIndex` and gated behind `E2E_COVERAGE=1`.

So a spec must never introduce state that survives across pages or workers.
`pnpm lint:e2e-parallel` (`scripts/check-e2e-parallel-safe.mjs`) fails on:

| Rule | Pattern |
|------|---------|
| `own-server` | `createServer(` |
| `port-listen` | `.listen(` |
| `disk-write` | `writeFileSync`, `writeFile`, `appendFileSync`, `appendFile`, `createWriteStream`, `copyFileSync`, `cpSync`, `renameSync` |
| `foreign-host-port` | `localhost:<port>` / `127.0.0.1:<port>` for any port but `:1420` |
| `serial` | `describe.serial` / `test.serial` / `mode: 'serial'` |

It runs in the **smoke** CI job - the only job that runs on
`e2e-playwright/**` changes, since `check` path-ignores them. Run it locally
before pushing. `fixtures/coverage.ts` is the one allow-listed write; a genuinely
necessary exception goes in the `ALLOW` list in that script *with a written
reason*. The scanner is a line-based tripwire, not a parser: it deliberately
ignores `page.screenshot({path})` (framework I/O with unique names) and the root
`playwright.config.ts`.

On a 2-core runner at multiple workers, bad hygiene here surfaces as flake, not
as a clean failure.

### Opening and closing dialogs

Prefer a helper, then the store, then a keypress.

```typescript
// Via the store - the most reliable
await page.evaluate(() => {
  (window as any).__E2E_TEST__.uiStore.getState().openDialog('import');
});
await page.waitForSelector('[data-testid="import-section-dialog"]');
```

`Ctrl+I` is the `import_dmp` action. It does **not** open a dialog directly - it
opens the native file picker, which the mock answers with
`/mock/test-survey.dmp`, and the app then routes by extension into
`DmpSelectionDialog`. That is why `openDmpSelectionDialog(page)` presses
`Control+i` and then waits for `dmp-selection-dialog`. Three consequences:

- It never opens `ImportDialog` (manual section creation) - use the store.
- With a project open the picker is `.dmp`-only (#508); with none open it is the
  full unified picker. The mock returns its fixed path either way, so a spec
  cannot tell the two apart - assert filter choice in vitest, not here.
- It still opens the picker when editing is blocked (read-only map / Plan or
  Export mode / locked remote project). The rejection happens after the pick, in
  `routeOpenedFile`: an `import-blocked` prompt for the causes a click can fix,
  an error dialog for a missing SpeleoDB write lock.

Closing:

```typescript
// Cancel button
await page.getByTestId('new-project-dialog')
  .getByTestId('new-project-cancel-button').click();

// Escape - DialogWrapper closes the TOPMOST dialog
await page.keyboard.press('Escape');
```

| Method | Works | Notes |
|--------|-------|-------|
| Cancel / Close button | Yes | Preferred in tests |
| Backdrop click | Yes | Calls `onClose` |
| Escape | Yes | `DialogWrapper` closes the topmost dialog; `src/utils/dialogStack.ts` sets `inert` on everything underneath |

Two Escape edge cases that read as flake: Escape is a **no-op while a dropdown is
open** (`[data-dropdown-open]`) because the dropdown consumes it first, and on a
**dirty** dialog (`isDirty`) it opens the discard confirmation rather than
closing - a second Escape dismisses the confirmation and leaves the dialog open.

### The DMP import flow

Three dialogs, in order, all driven from `helpers/app.ts`:

| Dialog | `activeDialog` | Reached by |
|--------|----------------|------------|
| `DmpSelectionDialog` | `'dmp-selection'` | `openDmpSelectionDialog(page)` (Ctrl+I -> mocked picker) |
| `ZeroDatumSelectionDialog` / `ZeroDatumConfigDialog` | `'zero-datum-selection'` / `'zero-datum-config'` | Continue from selection, when the project has no zero datum |
| `DmpNewImportDialog` | `'dmp-import-new'` | Continue from selection or from zero-datum config |

`ImportDialog` (`'import'`, manual section creation) is a separate flow, opened
by the SectionsPanel "+" button or via the store. `e2e-playwright/dmp-import.spec.ts`
and `import-section-dialog.spec.ts` are the worked examples.

Step 1 selection stats live under `[data-testid="selection-stats"]` with
`stats-shot-count`, `stats-total-length`, `stats-depth-range` inside; the
container's `aria-label` spells out the whole thing
(`"Selection statistics: 70 shots, 281.8m total length, depth range 2.1-15.4m"`,
`"depth range --"` when there is no depth data), so assert on the label when you
want one stable string instead of three.

### Canvas interaction

```typescript
import { waitForMapReady, waitForMapIdle } from './helpers/app';

await waitForMapReady(page);
const mapCanvas = page.getByTestId('map-canvas');
await mapCanvas.click();

const box = await mapCanvas.boundingBox();
if (box) {
  await page.mouse.move(box.x + 100, box.y + 100);
  await page.mouse.down();
  await page.mouse.move(box.x + 200, box.y + 200);
  await page.mouse.up();
  await waitForMapIdle(page);
}
```

`MapCanvas` exposes two hooks for assertions: `window.__stalagmight_map__` (the
MapLibre `Map`) and `window.__stalagmight_to_meters__`,
`(lng, lat) => { x, y }` in meters, using the same transform and the same live
GPS origin the component commits with (`src/dev/mapE2eHooks.ts`). Predict an
expected offset with that hook rather than a linear degrees-to-meters constant -
the two diverge by several percent.

### Debugging a failing spec

1. `pnpm test:ui:debug` - Playwright inspector, step through the run.
2. `pnpm test:ui:headed` - watch it happen.
3. Check `test-results/` for the screenshot, video, and trace (traces are
   captured on first retry).
4. `PW_RETRIES=0` to fail fast instead of watching a flake pass on retry.
5. Add `console.log()` inside the relevant mock handler in
   `mocks/tauri-ipc.ts`.

`e2e-playwright/debug.spec.ts` is a minimal end-to-end sanity spec (create a
project, assert the map canvas renders) - useful to confirm the harness itself
works before blaming your test.

## Mocked IPC conventions

`e2e-playwright/mocks/tauri-ipc.ts` intercepts `__TAURI_INTERNALS__.invoke()`
and hand-simulates the Rust backend for every Playwright spec.

### A new IPC command needs a mock case

Unknown commands hit `default: console.warn(...); return null`, so
`result.success` is falsy and the UI silently takes its failure path - the spec
fails for the wrong reason, or worse, passes vacuously.

```typescript
case 'your_new_command': {
  const param = args?.param_name as string;
  return { success: true, data: mockData };   // shape must match the Rust return type
}
```

`src/test/__tests__/mockContract.test.ts` (vitest) enforces this in both
directions: it parses `generate_handler!` in `src-tauri/src/lib.rs` and the
mock's `switch` cases and fails on drift, with reasoned allowlists and
stale-allowlist self-checks. **Adding or removing a `#[tauri::command]` fails
vitest until the mock and the allowlist agree.**

It is gated from both sides: `check` runs it as part of the vitest suite when
Rust or `src/` changes, and `smoke` runs it on its own (`pnpm test:mock-contract`)
when `e2e-playwright/**` changes, which `check` path-ignores.

### Do not copy backend shapes into a test

`Project`, `ProjectStats`, and `Config` are ts-rs-generated in
`src/types/ipc/`. Build mock payloads against those types
(`src/types/ipc/Project.ts`, `ProjectStats.ts`, `Config.ts`; `src/types/config.ts`
re-exports them alongside the hand-written display defaults) instead of
hand-listing fields, and let `tsc` catch a missing one. `Config` in particular
has around twenty top-level blocks and grows.

Two specific gaps to watch:

- **A new Settings *category* needs its config block in the mock.** A panel that
  reads `config.<block>.*` directly crashes when the block is absent (the real
  config always has it via `#[serde(default)]`), so the panel never renders and
  the category-iterating tests in `settings-dialog.spec.ts` fail. There are
  **two** config objects in the mock to update.
- **`focus-management.spec.ts` hardcodes the Settings category order** for its
  roving-tabindex assertions. A new category shifts ArrowDown navigation.

Both are full-suite-only failures: `@smoke` and the CI smoke job do not catch
them. Run the full local Playwright suite for any Settings-category change.

### `get_project_state` must return a FRESH identity per call

The mock deep-clones (`structuredClone(currentProject)`) on every
`get_project_state`, mirroring the real Tauri IPC, which returns
`project.clone()` serialized over the boundary - a brand-new object and brand-new
nested arrays every refetch.

The mock mutates `currentProject`'s arrays **in place** (`sections`,
`zero_datums`, `loop_closures`, `station_merges` via `.push` / `.splice`).
Returning the same reference freezes every reference-keyed React memo
(`useMemo(..., [project?.sections])` and friends), and the symptom is
distinctive: **a panel or overlay renders empty while the footer or stats show
the populated count.** Real users never see it - real IPC returns fresh
references. Do not "optimize" the clone away.

### A new gate must not read state the mock never sets

Sibling failure class: a new frontend gate reads store state the e2e environment
never initializes, silently disabling UI in every spec. The fix is always to
make the **mock** mirror the real invariant, never to weaken the gate. The
worked example is `__MOCK_SET_PROJECT_PATH__`, which sets `databaseOpen: true`
for a non-null path and `false` for null, because the real load path does.

### Recording calls for assertions

The mock exposes `window.__MOCK_*` hooks - recorded call arrays
(`__MOCK_MOVE_CALLS__`, `__MOCK_EXECUTE_BATCH_CALLS__`,
`__MOCK_CREATE_SHOT_CALLS__`, `__MOCK_SPLIT_SHOT_CALLS__`, ...), setters that
force a scenario (`__MOCK_SET_GEOMETRY_ERROR__`, `__MOCK_SET_INTEGRITY_ISSUES__`,
`__MOCK_CONNECTIVITY_RESULT__`, `__MOCK_IPC_OVERRIDES__`, ...), and matching
resets. Grep the mock for `__MOCK_` for the current list; follow the existing
naming when you add one.

The `move_*` handlers are **no-ops that do not mutate the project** (the app
refetches generated coordinates), so assert on the *recorded* call, not on a
coordinate read-back.

The mock also hardcodes station `z = 0` in `get_project_state` - it does not
derive station depth from the seeded shot `depth_in`/`depth_out`, even though
the stored *shots* carry them. Any e2e reading a station `z` or a depth readout
sees `0` regardless of the seed. Assert that a depth row *renders*; pin the depth
math in a Rust unit test.

### Common mock scenarios

```typescript
// A geometry command that fails, to exercise the error path
case 'get_survey_geometry_for_file': {
  const filePath = args?.path as string;
  if (filePath.includes('error')) throw new Error('Failed to load geometry');
  return {
    shots: [{ from: [-81.78, 24.55], to: [-81.77, 24.56], kind: 'Normal', section_id: 1 }],
    origin: [-81.78, 24.55],
  };
}

// The undo/redo stacks
case 'get_undo_stack': {
  return [{
    index: 0,
    description: 'Create section "Main Line"',
    timestamp: new Date().toISOString(),
    commandType: 'create_section',
    previewData: {
      changes: [{ field: 'name', oldValue: '', newValue: 'Main Line' }],
      additionalChanges: 0,
    },
  }];
}
```

Mocked specs run with a non-null `gpsOrigin`, which changes how station
coordinates are computed - see the canvas-interaction section above before
asserting on any distance or offset.

## Per-component Playwright recipes

### Undo/redo history dropdown

| Element | `data-testid` |
|---------|---------------|
| Trigger buttons | `toolbar-undo-button`, `toolbar-redo-button` |
| Dropdown (`role="menu"`) | `toolbar-undo-button-dropdown`, `toolbar-redo-button-dropdown` |
| Item (index matches `item.index` in the stack) | `toolbar-undo-button-item-{index}` |
| Detail preview | `detail-preview-popup` |
| Chevrons (visible when the stack is non-empty) | `undo-chevron`, `redo-chevron` |
| Loading spinner (shown while an action is in flight) | `toolbar-undo-button-loading`, `toolbar-redo-button-loading` |
| `aria-live` announcer (remaining action count) | `toolbar-undo-button-announcement`, `toolbar-redo-button-announcement` |
| Error dialog | `error-dialog`, `error-dialog-stack-trace`, `error-dialog-copy-btn` |

Opens on hover after a **300 ms** delay; the mouse-leave close has a **100 ms**
buffer.

```typescript
await page.hover('[data-testid="toolbar-undo-button"]');
await expect(page.getByTestId('toolbar-undo-button-dropdown')).toBeVisible({ timeout: 1000 });
```

**Keyboard open sets the active item in a `setTimeout(0)`.** Always wait for
`aria-activedescendant` before asserting on navigation state:

```typescript
await page.focus('[data-testid="toolbar-undo-button"]');
await page.keyboard.press('ArrowDown');
await page.waitForFunction(() => {
  const menu = document.querySelector('[role="menu"]');
  return menu && menu.getAttribute('aria-activedescendant');
});
await page.keyboard.press('ArrowDown');   // second item
await page.keyboard.press('Enter');       // select and close
```

Close paths: item click, Enter on the active item, mouse leave, Escape, click
outside, empty stack. **Stay-open** paths: clicking the trigger button and
`Ctrl+Z`/`Ctrl+Y`, which refresh the items in place.

```typescript
// Escape
await page.keyboard.press('Escape');
await expect(page.locator('[role="menu"]')).not.toBeVisible();

// Click outside (mousedown)
await page.click('body', { position: { x: 10, y: 10 } });

// Item click closes
await page.click('[data-testid="toolbar-undo-button-item-0"]');
await expect(page.getByTestId('toolbar-undo-button-dropdown')).not.toBeVisible();
```

Every stay-open, empty-stack, announcement, and error test must **open the
dropdown first** (hover or keyboard), then act:

```typescript
// Button click keeps it open and shows the spinner
await page.click('[data-testid="toolbar-undo-button"]');
await expect(page.getByTestId('toolbar-undo-button-loading')).toBeVisible();
await expect(page.getByTestId('toolbar-undo-button-dropdown')).toBeVisible();

// Ctrl+Z keeps it open and refreshes (focus must NOT be in a text input)
await page.keyboard.press('Control+z');
await expect(page.getByTestId('toolbar-undo-button-dropdown')).toBeVisible();

// Announcement after an action
await expect(page.getByTestId('toolbar-undo-button-announcement'))
  .toContainText('actions remaining');

// Empty stack auto-closes: point the mock at an empty get_undo_stack, then act
await page.click('[data-testid="toolbar-undo-button"]');
await expect(page.getByTestId('toolbar-undo-button-dropdown')).not.toBeVisible();

// Failure opens the error dialog and the dropdown STAYS open
// (mock undo -> { success: false, error: 'Stack corrupted' })
await page.click('[data-testid="toolbar-undo-button"]');
await expect(page.getByTestId('error-dialog')).toBeVisible();
await expect(page.getByTestId('toolbar-undo-button-dropdown')).toBeVisible();
```

The detail preview appears on item hover with **no** delay:

```typescript
await page.locator('[data-testid="toolbar-undo-button-item-0"]').hover();
await expect(page.getByTestId('detail-preview-popup')).toBeVisible();
```

### `Select` (portaled dropdown)

`Select` puts its public test ID directly on the combobox trigger. The wrapper
uses `<testId>-root`, the portaled listbox uses `<testId>-listbox`, and each
option uses `<testId>-option-<encoded-value>`. The listbox renders via
`createPortal` into `document.body` with `position: fixed`, so it is not inside
the wrapper - it is a sibling of `<div id="root">`.

1. **Do not locate it by an `.absolute` class.** The portaled listbox has none.
2. **Do not scope options under the Select container.**
   `selectContainer.locator('[role="option"]')` finds nothing.
3. **Use `document.elementFromPoint()` to prove z-order.** `toBeVisible()` alone
   cannot tell you an element is behind a sticky header.

```typescript
const trigger = page.getByTestId('select-number-format');
await trigger.click();

// aria-controls may need a render cycle. Use an attribute selector - React
// useId() produces IDs containing colons, which break #id selectors.
await expect(trigger).toHaveAttribute('aria-controls', /.+/);
const listboxId = await trigger.getAttribute('aria-controls');
const listbox = page.locator(`[id="${listboxId}"]`);

await listbox.locator('[role="option"]:has-text("Decimal")').click();
```

Prefer the `aria-controls` lookup over `page.getByRole('listbox')` whenever
another listbox may be mounted (the sections panel has one).

```typescript
// Z-order proof
const box = await firstOption.boundingBox();
const isTopmost = await page.evaluate(({ x, y }) => {
  return document.elementFromPoint(x, y)?.closest('[role="listbox"]') !== null;
}, { x: box.x + box.width / 2, y: box.y + box.height / 2 });
expect(isTopmost).toBe(true);
```

Scroll behavior: **external** scroll (window, a table container) closes the
dropdown; **internal** scroll - through the options, or `scrollIntoView` from
keyboard navigation - does not. The mechanism is a `scrollSafe` list passed to
`useDismissableLayer` covering the listbox and its nearest scrollable ancestor.
If a portaled popover self-closes the instant it opens, the trigger's scroll
container is missing from that list; fix it there, not with a settle timer.

`aria-activedescendant` is set **synchronously** from React state here - no
`setTimeout` workaround, unlike the undo dropdown.

```typescript
await trigger.focus();
await page.keyboard.press('Enter');      // open
await page.keyboard.press('ArrowDown');  // navigate
await page.keyboard.press('Enter');      // select
await page.keyboard.press('Escape');     // close
```

`Select` also accepts an `onOptionHover(value | null)` callback that fires when
an option is highlighted by mouse or keyboard, and fires with `null` on every
close path (selection, outside click, Escape, Tab, trigger toggle, unmount,
external scroll, window resize). Its consumers are `DmpNewImportDialog` and
`ZeroDatumConfigDialog`, which use it to drive a live placement preview. To test
it, assert on the preview the consumer renders.

`e2e-playwright/select-dropdown.spec.ts` is the worked example for all of the
above.

### Detail panel

| Element | `data-testid` |
|---------|---------------|
| Container | `detail-panel` |
| Station / section views | `station-detail`, `section-detail` |
| Station link (position 0-indexed) | `station-{sectionId}-{position}-link` |
| Shot link (position 1-indexed) | `shot-{sectionId}-{position}-link` |
| Section link | `section-{sectionId}-link` |
| Center buttons | `center-station-button`, `center-shot-button`, `center-section-button` |

Links and center buttons carry descriptive `aria-label`s
(`"Select station 2:5"`, `"Center map on station 2:5"`). Links to missing
targets get `aria-disabled="true"` plus a warning indicator and do not change
selection when clicked.

```typescript
await page.click('[data-testid="station-2-5-link"]');
await expect(page.getByTestId('station-detail')).toBeVisible();

const missing = page.getByTestId('station-2-99-link');
await expect(missing).toHaveAttribute('aria-disabled', 'true');
```

**Centering tests must first pan the map off-target**, otherwise they pass or
fail on the initial view:

```typescript
await page.evaluate(() => {
  const map = (window as any).__stalagmight_map__;
  const c = map.getCenter();
  map.setCenter([c.lng + 0.001, c.lat + 0.001]);
});
await waitForMapIdle(page);

await page.click('[data-testid="center-station-button"]');
await waitForMapIdle(page);
const center = await page.evaluate(() => (window as any).__stalagmight_map__?.getCenter());
```

Same shape under the agent harness, with `ctx.eval(...)` / `element_state`
against the same test ids.

`page.hover()` is reliable for Select options. Where it is not, dispatch a
bubbling `mouseover` instead - see `triggerMouseHover` in
`e2e-playwright/detail-link-hover.spec.ts:272`.

### ARIA live region

Selection changes triggered by detail-panel hyperlinks and data-table shot
selection are announced through a single live region at the App root, always
mounted, visually hidden with `sr-only`, `id="selection-announcer"`,
`role="status"`, `aria-live="polite"`, `aria-atomic="true"`. Map clicks do not
announce.

```typescript
// Use the ID. A generic role selector also matches the toast stack's live regions.
const liveRegion = page.locator('#selection-announcer');
await expect(liveRegion).toBeAttached();
await page.click('[data-testid="station-2-5-link"]');
await expect(liveRegion).toHaveText('Selected station 2:5');
```

### Focus management

Dialogs use `useFocusTrap` (`src/hooks/useFocusTrap.ts`) via `DialogWrapper`.

```typescript
// Tab wraps inside the dialog
const dialog = page.getByTestId('new-project-dialog');
const first = dialog.locator('input, button, select, textarea, [tabindex]').first();
const last = dialog.locator('input, button, select, textarea, [tabindex]').last();

await last.focus();
await page.keyboard.press('Tab');
await expect(first).toBeFocused();

await first.focus();
await page.keyboard.press('Shift+Tab');
await expect(last).toBeFocused();
```

Initial focus priority: `[data-autofocus]` > the first enabled `input`,
`textarea` or `select` (hidden, checkbox and radio inputs excluded) >
`[data-primary-action]` > first non-close button > the close X. So a dialog with a text field focuses the
field, and a confirmation dialog focuses its primary action:

```typescript
await page.click('[data-testid="new-project-button"]');
await expect(page.getByTestId('new-project-name-input')).toBeFocused();
```

Focus returns to the triggering element on close:

```typescript
const trigger = page.getByTestId('new-project-button');
await trigger.click();
await page.keyboard.press('Escape');
await expect(page.getByTestId('new-project-dialog')).not.toBeVisible();
await expect(trigger).toBeFocused();
```

With a nested dialog, only the topmost traps focus and only the topmost closes
on Escape - `dialogStack` sets `inert` on the layers beneath.

```typescript
await page.click('[data-testid="reset-category-defaults"]');
await expect(page.getByTestId('reset-confirm-dialog')).toBeVisible();
await page.keyboard.press('Escape');
await expect(page.getByTestId('reset-confirm-dialog')).not.toBeVisible();
await expect(page.getByTestId('settings-dialog')).toBeVisible();
```

### Global View

| Element | `data-testid` |
|---------|---------------|
| Root | `global-view` |
| Map container | `globalview-map` |
| Survey/markers toggle | `view-mode-toggle` |
| Per-system marker | `system-marker-{encodeURIComponent(filePath)}` |
| Search input / clear | `globalview-search-input`, `globalview-search-clear` |
| Right-click menu on a project | `project-context-menu` |

State persists to `localStorage`: `globalViewMode` (`'survey'` / `'markers'`)
and `globalViewMapState` (an object with `center` and `zoom`, written on
`moveend`).

```typescript
await page.click('[data-testid="view-mode-toggle"]');
expect(await page.evaluate(() => localStorage.getItem('globalViewMode'))).toBe('markers');

// Multi-select
await page.click('[data-testid="system-marker-system1.stalx"]');
await page.click('[data-testid="system-marker-system2.stalx"]', { modifiers: ['Control'] });
const selected = await page.evaluate(() =>
  (window as any).__E2E_TEST__.globalViewStore.getState().selectedSystemIds);
expect(selected).toHaveLength(2);

// Search filter - non-matching systems are dimmed via a MapLibre layer filter,
// so assert on the layer paint properties, not on DOM visibility.
await page.fill('[data-testid="globalview-search-input"]', 'sistema');
await page.click('[data-testid="globalview-search-clear"]');
await expect(page.getByTestId('globalview-search-input')).toHaveValue('');
```

### Themes

`e2e-playwright/theme.spec.ts` covers three of the ten themes: default dark
(`@smoke`), switching to light through Settings (`@smoke`), and CSS variable
resolution for dark, light and cave. It also covers immediate apply, revert on
cancel, persistence across a dialog reopen, and the station circle colors.

The other seven themes, day/night palette independence, and auto mode following
the OS `prefers-color-scheme` have **no e2e coverage** - no spec references them.

One app fact worth knowing when you add coverage: `kaleido_eink` force-disables
CSS transitions and animations (`src/index.css`), so a spec that waits on a
transition behaves differently under it.

### Keyboard shortcuts overlay

`?` opens `KeyboardShortcutsDialog`. Two rules matter when you touch it: the
category order is defined in `src/utils/shortcutCategories.ts`, so a spec
asserting order reads it from there rather than hardcoding a list.

Shortcut-recorder specs (`e2e-playwright/settings-shortcuts.spec.ts`) target
rows by `shortcut-primary-<action>`. Removing an action means repointing those
at a surviving one, or they time out.

## Rust patterns

Tests live alongside the source - in the app crate (`src-tauri/src/`) and in
each workspace member (`src-tauri/crates/*/src/`). Per-test-module
`#[expect]` (never `#[allow]`) is the clippy idiom here - see
[Linting & Format Policy](linting.md).

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_valid_input() {
        let result = function_under_test(create_test_input());
        assert_eq!(result, expected_output);
    }

    #[test]
    fn rejects_an_invalid_input() {
        assert!(function_that_can_fail(invalid_input).is_err());
    }
}
```

Async tests use `#[tokio::test]`.

### Config isolation

`AppState::new()` LOADS the global config and much of the state module writes it
back (`note_recent_project` -> `Config::save()`), so any test that builds an
`AppState` reads and writes one file. Under test - `cfg(any(test, feature =
"test-support"))` - `Config::config_path()` resolves through
`crate::test_helpers`, and the default for a thread that never asked for
anything is **a temp dir of that thread's own**, created on first use and removed
when the thread exits. libtest runs each `#[test]` on a thread of its own, so
every test is isolated from the developer's real
`~/.config/stalagmight/config.toml` and from every sibling test **with no
opt-in**. (One remote exception: when libtest cannot spawn a thread it runs that
test on the MAIN thread, which shares its dir with every other test that lands
there - a test that needs a dir nothing has ever written to still takes
`isolated_config`.)

That is the fix for [#522](https://github.com/Stalagmight/Stalagmight/issues/522).
Isolation used to be a process-global override cell taken under a process-global
mutex, which cost two distinct hazards:

- With no override live, an unguarded test resolved to the developer's real
  `config.toml` and rewrote it (its `recent_projects` filling with dead
  `/tmp/.tmpXXXX/*.stalx` entries).
- While ANY test held the override, an unguarded test's save resolved **into
  that test's isolated dir**. That is #522: an unguarded test atomically renamed
  its own `config.toml` over an isolated test's file, between that test's save
  and its read-back, and failed an assertion about keychain retry logic with an
  environment problem.

| helper | what it does |
|---|---|
| (nothing) | this thread's own temp dir - the default |
| `isolated_config()` | a fresh temp dir for this thread until the guard drops, whose `path()` the test can name (to seed a config, or to squat it and force a save failure). Nests: dropping it restores the guard it replaced |
| `with_isolated_config(f)` | the same, around a closure |
| `current_config_dir()` | where this thread resolves right now. `Some(dir)` always, unless the thread is under `real_config_resolution()`, which is the only meaning of `None`: a temp dir that cannot be created panics rather than degrading to the real path |
| `ConfigHandoff::capture()` / `.adopt()` | share the parent's config FILE (and keep its dir alive) in a thread it spawns |
| `real_config_resolution()` | resolve for real (`app_dirs`, then `directories` over the environment) |

**The one trap is a spawned thread.** It is a new thread, so by default it gets a
dir of its own rather than the spawning test's. That is usually what you want; a
test whose workers must write ONE config file (a concurrent-save race, or a
worker whose write the parent reads back) has to say so:

```rust
let handoff = crate::test_helpers::ConfigHandoff::capture(); // on the test's thread
let mine = handoff.clone();
std::thread::spawn(move || {
    let _config = mine.adopt();   // inside the worker; BIND it, the guard is the adoption
    // every Config::load()/save() here is the parent's file
});
```

Three tests need it today: the worker pool in
`src-tauri/crates/core/src/core/state/concurrency_tests.rs` (its saves persist
recents), the config lost-update race in `src-tauri/src/commands/command_tests.rs`,
and `logout_waits_for_an_in_flight_lock_operation` in
`src-tauri/crates/core/src/core/state/speleodb_session.rs` (the logout under test
runs on the worker). The first two would pass VACUOUSLY without it - each worker
would race a private file - so both assert from inside the worker that they are on
the parent's path.

`real_config_resolution()` is for the handful of tests that drive
`config_path()` itself, by pointing `XDG_CONFIG_HOME` (or `HOME`, which is what
the `directories` crate reads on macOS) at an unwritable directory to force a
save failure. Those guards serialize against each other, because the environment
they mutate is process-wide, and nothing else resolves the CONFIG PATH through
the environment. They do **not** serialize against code that reads the
environment for other purposes - `TempDir::new()` reads `TMPDIR`, and
`directories::UserDirs` reads `HOME` for the default project location - which
raced those three tests before this change and still does. The cure there is a
config root injected rather than an env var mutated, which is a bigger change
than #522.

### In-memory SQLite

```rust
let conn = rusqlite::Connection::open_in_memory()?;
```

Used by the tile cache, the `.plnx` dive-plan store, and the project database
(`src-tauri/crates/persistence/src/persistence/sqlite/open.rs`).

### Fixture files

Resolve fixtures from the manifest directory - they are shared with the E2E
tiers and live outside the crate. Mind which crate the test is in: from a
workspace member (`src-tauri/crates/<member>/`) the repo root is three levels
up; from the app crate (`src-tauri/`) it is one.

```rust
#[test]
fn parses_v5_dmp() {
    // In a member crate (here: stalagmight-parsers). From the app crate the
    // prefix is "../e2e/fixtures/" instead.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/test_v5.dmp");
    assert!(path.exists(), "Fixture missing: {}", path.display());

    let dmp = parse_dmp(&path).expect("v5 DMP should parse");
    assert_eq!(dmp.version, 5);
    assert!(!dmp.sections.is_empty());
}
```

Because Rust tests read `e2e/fixtures/`, the CI `check` job must keep running on
changes there - never path-ignore `e2e/**`.

### Inducing save failures must be root-safe

The self-hosted CI runner executes tests as **uid 0 with `CAP_DAC_OVERRIDE`**,
so a `chmod 0444` directory does *not* produce `EACCES` and a permissions-based
inducer silently passes. Use uid-independent inducers instead:

- **Generic "cannot save":** point `file_path` at a **missing parent directory**
  so `Database::open(temp)` fails.
- **The permission-denied humanization branch:** squat a **directory at the
  `.stalx.tmp` path** (parent present) so SQLite cannot open it and returns
  "unable to open database file", which `is_permission_denied` in
  `core/loading.rs` classifies as permission denied.

Do not squat the final `.stalx` path: with `path=None` and no open database,
`save_project` swaps in `next_available_project_path` and the save succeeds
under a new name.

The three tests that own this path, all in
`src-tauri/src/commands/command_tests.rs`:
`test_import_sections_persists_deferred_new_project` (success),
`test_import_sections_deferred_save_failure_warns_no_phantom` (a failed save
warns and leaves no phantom recent-projects entry), and
`test_import_sections_deferred_save_permission_denied_humanizes_no_marker_leak`.

### Registering a new mutation entry point

`MUTATION_ENTRYPOINTS` in `src-tauri/crates/core/src/core/state/tests.rs` is a
hand-maintained tripwire. Any `execute_*` method that bypasses the generic
`execute()` must be added there, or the read-only edit-gate test will not cover
it.

### Telemetry registry

A new command, dialog, or tool needs its event names added to the Rust registry
*and* to the canonical lists in `src/utils/telemetry.ts`. The registry asserts
an exact `EVENT_NAMES.len()` with a self-documenting ledger comment - keep the
ledger updated when you change the count. The `registry_covers_all_*` coverage
tests run **only** under `--features telemetry`. `registry_covers_all_tools`
parses `src/store/uiStore.ts` to diff `ToolType` against the registry, so a
retired tool needs a `RETIRED_TOOLS` entry.

[telemetry.md](telemetry.md) owns this contract in full, including every named
coverage test. Treat it as authoritative if the two ever disagree.

## Export testing

Image export is `src-tauri/crates/export/src/export/image.rs`. It emits SVG, then rasterizes
to PNG or PDF.

Sizing is **pixel-stable**: world coordinates are meters, and the SVG is scaled
so the longer dimension lands on `TARGET_SIZE = 800` px:

```rust
let scale = TARGET_SIZE / width.max(height);       // image.rs
let px = |pixels: f64| -> f64 { pixels / scale };  // px -> viewBox units
```

Every visual element is authored in pixels and converted through `px()`, so an
element keeps its on-screen size regardless of survey extent. To verify a size
from a generated SVG, multiply the attribute value by `scale` and compare
against the pixel constant. `PADDING_RATIO = 0.05` adds a 5 % margin on each
side, computed from the **larger** extent so the absolute margin is uniform.

The pixel constants are declared together near `TARGET_SIZE` (stroke widths,
junction trim, zero-datum star radii, penetration-label size / halo / offset).
Read them from the source rather than duplicating the numbers here - a stale
copy of that table is worse than no table.

PNG rasterization scales by DPI: `scale = dpi / 72`.

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-export export   # the export module
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-export dpi      # the DPI tests
```

`export_png_dimensions_scale_with_dpi` pins linear DPI scaling (72 vs 288, with a
small rounding tolerance) by reading the width and height straight out of the
PNG IHDR chunk; `png_oversized_dpi_errors` and `png_dpi_zero_errors` pin the
rejection paths.

## Agent harness (real app)

The one real-app tier since the WDIO suite retired (#549): journeys, stress
scenarios, and fuzz tiers under `e2e/agent/`, on-demand and per release
candidate. Commands are in the "Agent harness" section above; the full tool and
oracle reference is [docs/agent-harness.md](agent-harness.md). Prefer adding new
UI coverage to `e2e-playwright/`; add a journey when the property needs the real
backend end-to-end.

```
e2e/
├── agent/       # LLM-agent harness (MCP + the webdriverio library) - see docs/agent-harness.md
├── fixtures/    # .dmp/.tml/.tmlu/.stalx test data (Rust tests read these too)
└── lib/         # shared plumbing: env, display detection, fractal generation
```

The harness looks for the debug binary under `$CARGO_TARGET_DIR` (default
`~/.cargo_build`) and refuses stale builds via `.e2e-build-marker`.

## Fixtures

Shared between the Rust tests and the E2E tiers, in `e2e/fixtures/` - see
[`e2e/fixtures/README.md`](../e2e/fixtures/README.md) for the current inventory
and the DMP format notes. Playwright also keeps a small copy of the DMP files
under `e2e-playwright/fixtures/`.

The real Tauri backend parses these for real under the agent harness and in the
Rust tests. Playwright's mock does **not** read them - it returns simulated
section data.

When adding one: prefer real device data (anonymized if needed), or synthetic
data for a specific edge case, and record what it is for in the fixtures README.

## Troubleshooting

### Playwright

| Symptom | Check |
|---------|-------|
| A command returns `null`, UI takes the failure path | The command has no case in `mocks/tauri-ipc.ts`. `pnpm exec vitest run src/test/__tests__/mockContract.test.ts` will say so. |
| A panel renders empty while stats show the right count | A reference-keyed memo froze - the mock returned the same project object twice. See the fresh-identity rule above. |
| A new gate disables UI in every spec | The mock never initializes the state the gate reads. Fix the mock, not the gate. |
| Settings tests fail after adding a category | The mock's **two** config objects need the new block; `focus-management.spec.ts` hardcodes the category order. |
| A dropdown closes the instant it opens | Its trigger's scroll container is missing from the `scrollSafe` list. |
| Flake that rotates identity between runs | Load timing. Reproduce with `--repeat-each=5 --workers=1` before treating it as a real bug. |
| "No tests found", exits in ~1 s | `pnpm <script> -- <flags>`. Use `pnpm exec playwright test <flags>`. |

### Agent harness

| Symptom | Fix |
|---------|-----|
| "No such file or directory" for the stalagmight binary | Set `CARGO_TARGET_DIR` to where it was built |
| Port 4444 in use | A stale driver: `pkill tauri-driver` |
| Driver not found | `cargo install tauri-driver` (Linux/Windows), or install `tauri-wd` (macOS) |
| Stale-binary "Application did not load" | Rebuild with `pnpm tauri build --debug --no-bundle`; the harness's `.e2e-build-marker` freshness check names the mismatch |
| A dialog is not found | It is missing `role="dialog"` - see the testability contract in the [Testing Guide](TESTING.md) |

### Rust

| Symptom | Check |
|---------|-------|
| Passes locally, fails in CI | Run with `--workspace`, `--features telemetry` and `TELEMETRY_ENDPOINT` set - without `--workspace` the member crates are skipped, and a featureless run filters the registry-coverage tests out |
| Every test binary aborts at startup on Windows (`0xC0000139`) | Use `./scripts/test.ps1` |
| A permission-based save-failure test passes vacuously in CI | CI runs as root. Use a root-safe inducer (above) |
| Flaky or order-dependent | Shared mutable state between tests |
