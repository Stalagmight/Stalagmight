# Testing Recipes

Lookup reference: commands, per-framework patterns, mock and fixture
conventions, and the traps that have cost real debugging time.

For *which tier a test belongs in*, and the testability contract a component has
to satisfy, see the [Testing Guide](TESTING.md).

For SpeleoDB live coverage, see [SpeleoDB testing](speleodb-dev-testing.md).
It is opt-in, validates both email/password and token authentication, visibly skips
incomplete credentials, and deletes only projects created by the run. Deterministic
workspace validation must set `SPELEODB_API_TEST_ENABLED=false`.

## The Rust fuzz and property gates

Every gate below runs with the default Rust suite (`cargo test --workspace`), is
deterministic (an integer-only PRNG, a fresh `AppState` and temp dir per run,
results merged in seed order), and has an `#[ignore]`d wide run for sweeps and
for choosing gate seeds. Each module's header is the reference for its oracles
and knobs; this page says what each one is for and how to run it. The agent
harness never runs in CI.

All of them run on the station-graph model and the `.stalx` version 2 file.
There are **no tolerances and no known-bug classifiers** left in them: the
changeset engine restores a state to the bit, so the undo oracles compare the
canonical v2 bytes (`stalx_v2::encode`, `modified_at` masked, id counters
allowed to stand higher after an undo, plan 11.4). A gate failure prints the
seed and the test that replays it. A known bug is pinned by an `#[ignore]`d test
that fails until it is fixed (the `#[ignore]` goes with the fix); the farm-style
windows below are how the wide runs are split.

| Knob | Used by |
|---|---|
| `STALAGMIGHT_FUZZ_SEED_START`, `STALAGMIGHT_FUZZ_SEEDS`, `STALAGMIGHT_FUZZ_THREADS` | the migration, decode and saving fuzzers (rolling windows) |
| `STALAGMIGHT_RUST_FUZZ_*` | the undo fuzzer |
| `STALAGMIGHT_RUST_META_*`, `STALAGMIGHT_RUST_FORMAT_*`, `STALAGMIGHT_RUST_CRASH_*`, `STALAGMIGHT_RUST_ADV_*`, `STALAGMIGHT_RUST_CONC_*` | metamorphic, file-format, crash-timing, adversarial, concurrency |
| `STALAGMIGHT_DATUM_FUZZ_*` | the multi-datum command-chain fuzzer |
| `STALAGMIGHT_RUST_MIGRATE_*`, `STALAGMIGHT_RUST_WIRE_*` | the migration property gate, the wire-equivalence sweep |
| `STALAGMIGHT_MIGRATION_CORPUS`, `STALAGMIGHT_CORPUS_FILE`, `STALAGMIGHT_FUZZ_WALKS` | the corpus modes (real files, below) |
| `FZ706_*` | the #706 file-churn fuzz (app crate) |

### The undo fuzz gate

`src-tauri/crates/core/src/core/state/fuzz_undo_tests.rs` (with
`fuzz_extended_tests.rs`, `fuzz_reload_tests.rs`, `fuzz_corrupt_tests.rs`)
drives random `AppState` commands - every command family, the recipes (split
shot, delete and split, range deletes, Split Line Here, Combine Lines, create
and merge, imports, Add .stalx) and batches - interleaved with undo, redo,
multi-step jumps and save/close/reopen crossings. After every move it checks:
`validate()` is empty; at every recorded depth the canonical v2 bytes are the
bytes recorded there; session shot ids are exact in the session and sound
after a reopen; the counters never go down; the memoised raw layout and
Computed View are the fresh ones; every `UndoHistoryItem` on both stacks is the
one recorded when its entry was pushed, through a save and a reopen; after a
reopen every kept entry steps onto the state it leads to; a refused command, a
refused batch, an undo past the floor or a step through a tampered entry
changes nothing; a jump of N lands where N single steps do.

```bash
# The gates (in-memory, persistence, multi-crossing, legacy origin, combined).
cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_undo

# Per-seed status, used to choose GATE_SEEDS.
STALAGMIGHT_RUST_FUZZ_SEEDS=40 STALAGMIGHT_RUST_FUZZ_ITERATIONS=100 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_seed_survey -- --ignored --nocapture

# Thousands of moves past the history limit (the FIFO window slides).
STALAGMIGHT_RUST_FUZZ_HORIZON=1 STALAGMIGHT_RUST_FUZZ_SEEDS=4 \
  STALAGMIGHT_RUST_FUZZ_ITERATIONS=10000 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib undo_redo_fuzz_horizon -- --ignored --nocapture

# Soak: no iteration budget, every optional feature.
STALAGMIGHT_RUST_FUZZ_SEEDS=200 STALAGMIGHT_RUST_FUZZ_ITERATIONS=1000 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib undo_redo_fuzz_soak -- --ignored --nocapture
```

The gate tests are `undo_redo_fuzz_gate` (in-memory), `_gate_persistence` (one
crossing per seed), `_gate_multi_crossing`, `_gate_legacy_origin` (each run
starts from a seeded 0.6.0 survey written by the frozen 0.6.0 save and opened
through the migration) and `_gate_combined`. Knobs (prefix
`STALAGMIGHT_RUST_FUZZ_`; every name is kept, the farm drives them):

| env | what it does |
|---|---|
| `SEEDS=n`, `SEED_START=s` | the sweep's seeds (`s..s+n`) |
| `ITERATIONS=n` | moves per run |
| `THREADS=n` | sweep workers; the output does not depend on it (merged in seed order) |
| `TRACE=1`, `VERBOSE=1` | stream every move (one worker unless `THREADS` says otherwise); print whole failure payloads |
| `JUMPS=1` | multi-step `undo_to` / `redo_to`, each checked against N single steps |
| `CLOSURES=1`, `CLOSURE_SHAPES=1` | a genuine loop in the starting project; two closures and an orphan shape at the first crossing |
| `DIVERGENCE=1` | session ids out of positional order before a crossing |
| `HORIZON=1`, `PERSISTENCE=1` | thousands of moves past the history limit; cross as well |
| `CROSSINGS=n`, `SAVES=1`, `TRIM_CROSSINGS=1` | crossings per persistence run; saves in place; cross even when the save keeps only the newest `undo_persist_limit` entries |
| `CONFIG=1` | draw `undo_history_limit` from a ladder (a second generator, so no stream moves); printed in every summary and failure |
| `COMBINED=1` | horizon, closures, closure shapes, jumps, three crossings and a drawn limit at once |
| `LEGACY_ORIGIN=1`, `LARGE=1`, `EXTENDED=1` | start from a migrated legacy project; from long lists; more of the rare actions |
| `FORMAT_CORRUPT=1` | damage one persisted history entry before a reopen (`fuzz_corrupt_tests.rs`) |

**Minimizing a failure.** `fuzz_shrink` (`fuzz_shrink_tests.rs`) records a run's
move stream and runs signature-matched ddmin over it: remove a chunk, replay the
rest through the same engine and oracles, keep the candidate only if it still
fails THE SAME WAY. A move names everything by what survives a reload (lines by
id, shots by uuid, stations by Station ID plus the slot's line, closures by
id), so a move whose target was shrunk away is skipped, not fatal.

```bash
STALAGMIGHT_RUST_FUZZ_SHRINK=1 STALAGMIGHT_RUST_FUZZ_ITERATIONS=200 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_shrink -- --ignored --nocapture

STALAGMIGHT_RUST_FUZZ_REPLAY=/tmp/stalagmight-shrink-seed1-3moves.json \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_replay -- --ignored --nocapture
```

`SHRINK=<seed>` records and shrinks that seed under the same knobs the sweeps
read (so a seed is shrunk under exactly the environment that found it),
`SHRINK=<path.json>` shrinks a trace file further, `SHRINK_TARGET` names the
failure to keep, `SHRINK_OUT` picks where the minimized trace goes,
`SHRINK_PROBES` caps the probe budget.

**Regression guards.** `open_undo_bugs_tests.rs` holds a reduction of every
#413 finding, #469 and #557 whose user-visible property still applies,
rewritten on the changeset engine, and none is `#[ignore]`d. The cases that
pinned deleted machinery (gauge pairs, the old datum undo arm, line offsets as
stored values, per-line stored `created_at`) are retired with their reasons in
the module. `undo_pipeline_tests.rs` and `undo_after_reopen_tests.rs` cover the
pipeline and the dropped-history banner; `undo_size_gate_tests.rs` the
pipeline's share of each entry.

### The metamorphic gate

`src-tauri/crates/core/src/core/state/metamorphic_undo_tests.rs` asks what the
roundtrip fuzzer structurally cannot: do two DIFFERENT ways of asking for the
same edit agree? It builds one survey, saves it as a v2 file and opens it into
two `AppState`s (asserting they start equal, which also checks that the loader
is a function of the file), drives each down one route, and compares the
canonical survey, the integrity verdict, the Computed View and the id
allocators - on the results, and again after both are fully unwound, where
each unwound twin must be its own starting state. A failed batch must be a
strict no-op while the same commands run singly apply up to the failure. The
one documented divergence is the atomic range delete across a junction (#413
finding 22, resolved as intended).

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib metamorphic

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

### The file-format gate

`src-tauri/crates/core/src/core/state/format_property_tests.rs` checks the file
at the AppState level; the codec's own properties are the persistence crate's
`stalx_v2/format_v2_*_tests.rs` (round trip, canonical form, gating, hostile
input, limits, modes, diff locality, salvage, crash) and are not duplicated.

- save, open fresh, save: three generations of identical BYTES, no mask; a
  re-save of an unchanged project rewrites the same bytes;
- the open is a function of the file (several opens give the same survey,
  verdict, Computed View, allocators and history);
- the guard: a same-size flip, a truncation, another valid project or a delete
  refuses the plain save and the local-only save with `EXTERNALLY_MODIFIED`;
  Overwrite works; a touch or a same-bytes rewrite is no change;
- unknown content: tier 2 survives a local-only save, an edit and a save
  verbatim; tier 3 is dropped; tier 1 opens read-only (edit, undo and save
  refused, the local-only save works and does not rewrite the body);
- legacy degradations (no gauge columns, an ancient `user_version`,
  disagreeing version markers, no undo tables, no station names or ids) open
  through the frozen reader, and the first save upgrades them to a v2 file that
  reopens as the same survey.

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib format_

STALAGMIGHT_RUST_FORMAT_SEEDS=60 STALAGMIGHT_RUST_FORMAT_CLOSURES=1 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib format_property_sweep -- --ignored --nocapture
```

### The crash-timing gate

`src-tauri/crates/core/src/core/state/crash_property_tests.rs` tests the save's
central promise ([Saving](saving.md#atomic-save-temp-file--rename)) by building
the exact files a kill at each step would leave, rather than racing a signal:
the destination alone, the temp torn at six cut points, the whole temp before
the rename, the renamed file. Oracles: no panic; the destination opens as
EXACTLY the old or the new survey; the next save succeeds and leaves no `.tmp`;
a torn temp never decodes and is never offered by the Recovery dialog, a
complete one is offered and restores as the interrupted save.

The gate and the sweep also run every state of the legacy upgrade
(`legacy_upgrade_tests.rs`, [Saving](saving.md#upgrading-a-legacy-file)), on
four originals: clean, with debris sidecars, with the hot journal of an old
build killed mid-write, and in WAL mode with a commit never checkpointed. Two
engines: the files a kill between two steps leaves, built by hand (the settled
copy staged torn and whole, renamed over the original beside its old sidecars,
the sidecars gone, the backup torn and kept, the temp torn and whole, renamed),
and the real save killed after each step (`legacy_upgrade::KILL_AT`: the
injected kill unwinds past every cleanup, as a kill would). Oracles: never a
JSON file beside an armed journal or a WAL; the project's path opens as EXACTLY
the survey the open showed or the upgraded one; the kept original opens as that
survey, Recovery offers it (never a hidden staging file) and every offered
backup restores; the next save leaves no sidecar and nothing hidden behind.

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib crash_property

STALAGMIGHT_RUST_CRASH_SEEDS=60 STALAGMIGHT_RUST_CRASH_MOVES=26 \
  STALAGMIGHT_RUST_CRASH_EXTRA=12 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib crash_property_sweep -- --ignored --nocapture
```

```bash
STALAGMIGHT_RUST_CRASH_SEEDS=40 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib legacy_upgrade_sweep -- --ignored --nocapture
```

`an_upgrade_interrupted_after_its_sidecar_move_leaves_the_original_whole` pins
the wave B bug on the real pipeline: an upgrade that moved a hot journal (or a
WAL) away from the original before settling it left the original alone as
another survey.

### The adversarial gate

`src-tauri/crates/core/src/core/state/adversarial_tests.rs` fuzzes the command
layer's VALIDATION with hostile ids, slot indices, floats, strings and shots
over 26 move kinds (the recipes and the topology commands included, and a
`Batch` whose second sub-command is hostile). Three invariants: a rejected
command is a PERFECT no-op on every oracle, the unsaved-changes flag included;
an accepted command leaves a valid survey; no command ever panics (every move
runs inside `catch_unwind`, the panic site recorded by a hook).

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib adversarial

STALAGMIGHT_RUST_ADV_SEEDS=200 STALAGMIGHT_RUST_ADV_MOVES=400 \
  STALAGMIGHT_RUST_ADV_CLOSURES=1 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib adversarial_sweep -- --ignored --nocapture
```

### The concurrency gate

`src-tauri/crates/core/src/core/state/concurrency_tests.rs` drives ONE
`AppState` from several threads, the paths that skip `command_lock` included
(display toggles, which are local-only durable saves; full loads; the reads),
plus saves, opens, jumps and the derived views. Oracles: liveness (a whole-run
deadline plus a no-progress window), no panic, no poisoned mutex (`project`,
`file`, `undo_stack`, `derived`, `command_lock`), a coherent derived cache (the
memoised views equal a fresh computation), a guard that describes the file on
disk, and a whole history that steps end to end afterwards.

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib concurrency

STALAGMIGHT_RUST_CONC_SEEDS=40 STALAGMIGHT_RUST_CONC_THREADS=8 \
  STALAGMIGHT_RUST_CONC_OPS=2000 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib concurrency_soak -- --ignored --nocapture
```

Two regression tests pin races the soak found:
`a_display_toggle_racing_undo_never_writes_a_history_that_does_not_fit_the_body`
(`STALAGMIGHT_RUST_CONC_ROUNDS`, default 40) and
`a_full_load_racing_edits_never_leaves_a_stale_derived_cache`
(`STALAGMIGHT_RUST_CONC_LOADS`, default 300; 20000 reproduces the race on a
build without the fix). The deadline is generous on purpose: Windows and macOS
hand mutexes over differently from Linux. Poll `JoinHandle::is_finished` rather
than a progress counter, which reads a worker that panicked as one that is
stuck.

### The multi-datum fuzzer

`src-tauri/crates/core/src/core/state/multi_datum_fuzz_tests.rs` (with
`multi_datum_steps_fuzz.rs`) runs chains of datum commands, merges of placed
pieces, detaches, splits, moves and reopens, and checks plan 4.3 after every
step: every placement datum exactly at `gps_to_local(GPS, frame) + correction`,
no correction on a fix and none below the epsilon, a frame change that
translates every station by one vector, undo, redo and reopen exact to the bit.
The 13 pinned replay strings run in the gate.

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib multi_datum

STALAGMIGHT_DATUM_FUZZ_SEEDS=2000 STALAGMIGHT_DATUM_FUZZ_STEPS=30 \
  cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
  datum_command_chains_keep_every_invariant -- --nocapture

STALAGMIGHT_DATUM_FUZZ_REPLAY="seed;op;op;..." \
  cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
  replay_a_datum_chain -- --ignored --nocapture
```

### Migration and the v2 decoder

The legacy SQLite format is read only through the frozen 0.6.0 reader
(`crates/legacy`), and these suites hold the conversion to the 0.6.0 app's own
answers:

| Suite (persistence crate unless noted) | What |
|---|---|
| `migrate/migrate_property_tests.rs` | 48 generated legacy projects (`test_support::random_case`, written by the legacy crate's `writer`): total station map, every slot raw and Computed View within tolerance or in a declared class, measurements bit-identical, `save(migrate(x))` canonical and reopen-identical. `STALAGMIGHT_RUST_MIGRATE_SEEDS` / `_FIRST_SEED` |
| `migrate/migrate_v2_roundtrip_tests.rs`, `migrate_fixture_tests.rs` | the same seeds and the two tracked fixtures through encode, decode and the fixpoint |
| `migrate/wire_equivalence_tests.rs` | the IPC payloads of the migrated project against 0.6.0's own, field by field, every difference in a named class; positions differ only in a component the migration moved, and only to the migrated model's own position (per slot); `wire_equivalence_sweep` with `STALAGMIGHT_RUST_WIRE_SEEDS` prints the classes |
| `migrate/migrate_fuzz_tests.rs` | hostile legacy projects (merge chains and cycles, merges onto closure ends, several datums per piece and per merged station, hand moves, drifted offsets, odd names, unknown tables and columns, persisted undo rows, extreme values, both 0.6.0 file shapes): the migration succeeds or refuses with a typed reason, then the property gate's checks, the legacy bytes refused by the v2 decoder, the v2 file a byte fixpoint, the wire classes, the Computed View's maximum principle (no station moves past the misclosures it distributes), and a `legacy_origin` undo walk. `migration_fuzz_window` (CI), `migration_fuzz_wide` (2,000 seeds), `migration_fuzz_explain` (one seed); every run reads `STALAGMIGHT_FUZZ_NO_EXTREMES=1`, `STALAGMIGHT_FUZZ_HUGE=0` and `STALAGMIGHT_FUZZ_DEGENERATE_LSQ=0` to switch the extreme values, km-scale shots (C24 F3) and short grid-axis shots (C24 F2) out of the draw |
| `stalx_v2/decode_fuzz_tests.rs` | 23 mutation operators on encoder output (truncation at every row boundary, duplicate and reordered members, NaN and out-of-range numbers, deep nesting, huge strings, versions, `requires`, bad uuids, dangling references, forest violations, BOM, history floods): probe, sniff, decode and salvage never panic and finish in bounded time, every refusal is typed, anything accepted validates and re-encodes canonically. `decode_fuzz_window` (CI), `decode_fuzz_wide` |
| core `state/transition_fuzz_tests.rs` | AppState-level saving fuzz from every origin (v2, v2 linked, v2 with unknown members, the tracked legacy fixtures, a new project): commands (its own generator, plus one move in six from the undo fuzzer's), undo and redo mixed with save and reopen (a byte fixpoint, the stacks redo and undo after it), display toggles and sync-state changes (only `local` changes on disk), external changes (refused, then overwritten), Save a Copy; `read_only_files_are_never_written`. `saving_fuzz_window` (CI), `saving_fuzz_wide` |

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-persistence --lib migrate
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-persistence --lib decode_fuzz

STALAGMIGHT_FUZZ_SEED_START=0 STALAGMIGHT_FUZZ_SEEDS=2000 STALAGMIGHT_FUZZ_THREADS=3 \
  cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-persistence --lib \
  migration_fuzz_wide -- --ignored --nocapture
```

**Corpus modes (real files).** `migrate_corpus` (persistence),
`migration_fuzz_corpus` (persistence) and `transition_fuzz_corpus` (core) run
on every `*.stalx` under `STALAGMIGHT_MIGRATION_CORPUS=<dir>`, a folder of real
old-format files that never leaves the machine it is on. They open each file
as the app does, check the migration against the frozen reader, run
`STALAGMIGHT_FUZZ_WALKS` seeded edit sequences with undo-all / redo-all
exactness and save/reopen fixpoints, and print ONLY file-name hashes, counts,
classes and seed repros - never a file name, a station name, a coordinate or a
failure's text, which can quote the file. `STALAGMIGHT_CORPUS_FILE=<hash>`
reruns one file. All three are `#[ignore]`d and need the env var.

### The #706 file-churn fuzz (app crate)

`src-tauri/src/commands/mmap_fault_fuzz_tests.rs` churns the open project's
file under the app: edit, save, Save As, reopen, close, exports, restore,
repair, salvage, merge, create, delete, list backups, and external unlink,
rename, replace and truncate. The SQLite mmap hazard it was written for (#706)
does not exist on version 2 (no live handle, no mapping), so its hazard mode
(`FZ706_HAZARD=1`) is now expected to pass; its oracles are the #720 restore
contract, an in-memory survey that never changes across a step that must not
touch it (`new-drift`), and a save that says ok holding the survey
(`new-overwrite`). Knobs `FZ706_FIRST`, `FZ706_SEEDS`, `FZ706_STEPS`,
`FZ706_FOCUS`, `FZ706_MUTE` keep their names.

### The characterisation goldens (K)

`src-tauri/crates/core/src/core/state/characterization_dump_tests.rs` runs 18
synthetic scenarios (junction merges, loop and snap closures, two entrances,
free-piece moves, reverses, Split Line Here, combines, deletes at junctions and
of ranges, unmerges, frame changes, Add .stalx, a 1,000-cell paste, renames,
credit edits) through `AppState` commands and dumps after every step the
command result, the history items, the per-slot stations, the Computed View
and report, closures and datum roles, the wire project, the warnings, every
preview, and the final exports. `every_characterisation_scenario_does_what_it_expects`
runs them as a gate (a step that does not succeed or refuse as 0.6.0 did, a
panic, or a full undo/redo round trip that does not come back identical), and
on nothing else: the dump's contents are not compared in CI, and its outputs
are not committed. The 0.6.0 dump was recorded once before `models::project`
was deleted and compared once (plan stream C18); it cannot be recorded
again. Parity with 0.6.0 is gated in CI by the code that still runs 0.6.0:
the migration's verification and the wire-equivalence suite against the
frozen crate (`persistence::migrate`), the `models::survey` cross-checks built
with the frozen model, and the Ariane export's pinned legacy document. The
dump itself is `#[ignore]`d:

```bash
STALAGMIGHT_CHAR_OUT=<dir> \
  cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
  characterization_dump -- --ignored --nocapture

node scripts/dev/compare-characterization.mjs <old-dir> <new-dir> --summary --canon
```

`v0` is the dump of the 0.6.0 model, `v1` that of the station-graph core.
`--canon` compares what the switch re-derives in canonical form (merge pairs as
sets, Ariane ids renumbered); every remaining difference must be a declared
change (plan 12.2 and its Deviations sections). `STALAGMIGHT_CHAR_ONLY=name,...`
limits the run to some scenarios.

### Corpus guidance, and why it is not wired into anything

`fuzz_corpus_sweep` (`#[ignore]`d) measures whether remembering the shapes a
sweep reaches lets later runs reach more of them; `fuzz_rarity_sweep` and
`fuzz_rarity_carryover` (`src-tauri/crates/core/src/core/state/fuzz_rarity_tests.rs`,
both `#[ignore]`d) measured admission keyed on a RARE structural bit. On the old
engine both answers were no: guidance reached nothing unguided runs did not,
and counted in wall time it was no faster, because a continuation inherits a
bigger project and every oracle canonicalizes the whole project. The mechanisms
are kept and deliberately not wired into a gate; the module docs carry the
numbers.

```bash
STALAGMIGHT_RUST_FUZZ_THREADS=8 STALAGMIGHT_RUST_FUZZ_RARITY_BUDGET=80 \
  STALAGMIGHT_RUST_FUZZ_RARITY_REPS=6 STALAGMIGHT_RUST_FUZZ_ITERATIONS=60 \
  cargo test --manifest-path src-tauri/Cargo.toml \
  -p stalagmight-core --lib fuzz_rarity_sweep -- --ignored --nocapture
```

**Config isolation.** `AppState::new()` LOADS the global config and every save
reaches `note_recent_project` -> `Config::save()`, which WRITES it.
`Config::config_path()` resolves PER THREAD under test (see
[Config isolation](#config-isolation) below), so every fuzz and property module
is off the real `~/.config/stalagmight/config.toml` whether it says so or not;
they still open with `let _config = crate::test_helpers::isolated_config();`
because that gives the whole test ONE dir whose path it can name. To check that
nothing resolves the config through the ENVIRONMENT, point the config dir at a
temp dir and assert nothing was written:

```bash
XDG_CONFIG_HOME=$(mktemp -d) \
  cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
  -- core::state::metamorphic_undo_tests
# then: find "$XDG_CONFIG_HOME" -type f   -> must be empty
```

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

# The vendored crates (compass_data, and igrf with the IGRF-14 model, #811) are
# [workspace]-excluded, so --workspace never tests them. CI gates each as its own
# check-job step (#494); no telemetry feature or env var. Each has its own lock.
cargo test --locked --manifest-path src-tauri/vendor/compass_data/Cargo.toml
cargo test --locked --manifest-path src-tauri/vendor/igrf/Cargo.toml
```

**Trap: a plain `cargo test` can pass while CI fails, twice over.** Without
`--workspace` only the app crate's tests run - the member crates under
`src-tauri/crates/` are skipped entirely. And the `registry_covers_all_*`
telemetry-registry coverage tests are gated behind the `telemetry` feature, so
a featureless run filters them out. The telemetry member's `build.rs` also
*requires* `TELEMETRY_ENDPOINT` to be set when building with that feature.

#### Running one crate

The backend is a cargo workspace: the `stalagmight` app package at `src-tauri/`
plus nine members under `src-tauri/crates/` (`stalagmight-models`,
`-telemetry`, `-parsers`, `-legacy`, `-persistence`, `-export`, `-speleodb`,
`-core` and `stalx-reader`).
Select one with `-p` for fast iteration:

```bash
# The fuzz / metamorphic / crash / adversarial / concurrency engines all live
# in stalagmight-core (crates/core/src/core/state/):
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib fuzz_undo

# The survey model, its changeset engine and the wire projection:
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-models --lib survey::

# The v2 codec and the legacy migration (the legacy crate's test writer is a
# dev-dependency feature, so nothing has to be passed):
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-persistence --lib stalx_v2
cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-persistence --lib migrate

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

`helpers/app.ts` also has seeding helpers (`createLineViaStore`,
`seedProjectWithLines`), menu navigation (`openAppMenu`,
`openSettingsFromMenu`, `openDebugFromMenu`), the DMP wizard drivers
(`openDmpSelectionDialog`, `proceedToDmpImportWizard`, `placeAllLines`), and
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
await page.waitForSelector('[data-testid="import-line-dialog"]');
```

`Ctrl+I` is the `import_dmp` action. It does **not** open a dialog directly - it
opens the native file picker, which the mock answers with
`/mock/test-survey.dmp`, and the app then routes by extension into
`DmpSelectionDialog`. That is why `openDmpSelectionDialog(page)` presses
`Control+i` and then waits for `dmp-selection-dialog`. Three consequences:

- It never opens `ImportDialog` (manual line creation) - use the store.
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

`ImportDialog` (`'import'`, manual line creation) is a separate flow, opened
by the SectionsPanel "+" button or via the store. `e2e-playwright/dmp-import.spec.ts`
and `import-line-dialog.spec.ts` are the worked examples.

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

The mock mutates `currentProject`'s arrays **in place** (`lines`,
`zero_datums`, `loop_closures`, `station_merges` via `.push` / `.splice`).
Returning the same reference freezes every reference-keyed React memo
(`useMemo(..., [project?.lines])` and friends), and the symptom is
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
force a scenario (`__MOCK_SET_GEOMETRY_ERROR__`, `__MOCK_SET_CLOSURE_ALIGNED__`,
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

### The mock's junction model

The mock mirrors the station graph the backend now has, and the
mock-contract test (`src/test/__tests__/mockContract.test.ts`) checks the shape
as well as the names:

- **Junctions** come from the union of the stored merge pairs: `merged_with` is
  transitive, a junction's slots share one Station ID, name and comment (the
  first slot in line order is the representative), and `station_merges` goes
  out as star pairs from the first slot, with `merged_at = created_at`. There is
  no fake auto-merge.
- **Unmerge means detach**: `unmerge_stations` detaches the slot's own line,
  `unmerge_station_pair` the partner's, and any two slots of one station are a
  pair. A rename is checked on every line through the junction. Split Line Here
  records the halves' shared station as a merge.
- **The shape check**: for every identity-bearing command, each `StationRef`,
  `closure_id` or `line_id` parameter the Rust command takes must be read by its
  mock case under the camelCase key, and every mock station literal carries the
  required `Station` fields with `id` and `merged_with` taken from the junction.
- **File status hooks**: `__MOCK_SET_EXTERNALLY_MODIFIED__(true)` makes the next
  saves refuse with `EXTERNALLY_MODIFIED`, `__MOCK_SET_READ_ONLY_REASON__(text)`
  opens the file read-only, `__MOCK_SET_CONVERSION_NOTICE__(view)` queues a
  conversion notice (a `ConversionNoticeView`), `__MOCK_SET_LEGACY_ORIGIN__(true)`
  makes the next successful save report `legacy_converted` (the converting
  save of a 0.6.0 file), `__MOCK_GET_SAVE_CALLS__()` returns the save calls. The
  mock answers `save_project_overwriting`, `take_conversion_notice`,
  `has_unsaved_changes`, `project_read_only_reason`, `resolve_warning` and
  `unresolve_warning`.
- The binary load envelope is version 9 (no `line_offsets`).

### Common mock scenarios

```typescript
// A geometry command that fails, to exercise the error path
case 'get_survey_geometry_for_file': {
  const filePath = args?.path as string;
  if (filePath.includes('error')) throw new Error('Failed to load geometry');
  return {
    shots: [{ from: [-81.78, 24.55], to: [-81.77, 24.56], kind: 'Normal', line_id: 1 }],
    origin: [-81.78, 24.55],
  };
}

// The undo/redo stacks
case 'get_undo_stack': {
  return [{
    index: 0,
    description: "Create line 'Main Line'",
    timestamp: new Date().toISOString(),
    commandType: 'create_line',
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
| Station / line views | `station-detail`, `line-detail` |
| Station link (position 0-indexed) | `station-{lineId}-{position}-link` |
| Shot link (position 1-indexed) | `shot-{lineId}-{position}-link` |
| Line link | `line-{lineId}-link` |
| Center buttons | `center-station-button`, `center-shot-button`, `center-line-button` |

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
  and its read-back, and failed an assertion about credential persistence with an
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

### Project files in tests

A project is a `.stalx` version 2 file, so a test builds its survey with the
model's construction API (`models::survey` `build.rs`: `add_line`,
`set_placement`, `push_datum`, `push_closure`, or the commands themselves),
writes it with `stalx_v2::encode` into a `TempDir`, and opens it through
`AppState` like the app does. The core's test kits do this
(`state/topology_kit.rs`, `stalx_fixtures.rs`, `robustness_kit.rs`). A file is
compared by its canonical bytes; nothing needs masking but `modified_at`.

A test that needs an OLD file writes it at run time with the frozen 0.6.0 save
(`stalagmight-legacy`'s `writer` feature, `write_v18`, both 0.6.0 file shapes),
or copies a tracked old-format fixture from `e2e/fixtures/` into a temp dir, and
opens it through the migration. Never commit a generated file.

### In-memory SQLite

```rust
let conn = rusqlite::Connection::open_in_memory()?;
```

Used by the tile cache and the `.plnx` dive-plan store. The project file is not
SQLite any more; only the frozen legacy reader (`crates/legacy`) and the cheap
legacy link read (`persistence/legacy_link.rs`) open old project files.

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
    assert!(!dmp.lines.is_empty());
}
```

Because Rust tests read `e2e/fixtures/`, the CI `check` job must keep running on
changes there - never path-ignore `e2e/**`.

### Inducing save failures must be root-safe

The self-hosted CI runner executes tests as **uid 0 with `CAP_DAC_OVERRIDE`**,
so a `chmod 0444` directory does *not* produce `EACCES` and a permissions-based
inducer silently passes. Use uid-independent inducers instead:

- **Generic "cannot save":** point `file_path` at a **missing parent directory**:
  the save refuses with `FS_PATH_NOT_FOUND` before it writes anything.
- **A failing write:** squat a **directory at the `<file>.tmp` path** (parent
  present): `write_atomic` cannot remove or create its temp, the save fails at
  its first step, and the file is untouched.
- **A changed file:** rewrite the project file behind the app (other bytes, or
  truncate it, or delete it): the guard refuses with `EXTERNALLY_MODIFIED`.

A permission-denied test that needs a real `EACCES` (a `0o000` folder) skips
itself when it can still create a file there, which is what a root runner
does. Do not squat the final `.stalx` path of a project never written: its
first implicit save moves to `next_available_project_path` and succeeds under a
new name.

The three tests that own this path, all in
`src-tauri/src/commands/command_tests.rs`:
`test_import_lines_persists_deferred_new_project` (success),
`test_import_lines_deferred_save_failure_warns_no_phantom` (a failed save
warns and leaves no phantom recent-projects entry), and
`test_import_lines_deferred_save_permission_denied_humanizes_no_marker_leak`.

### Registering a new mutation entry point

`MUTATION_ENTRYPOINTS` (in the core `state` test suite; `git grep
MUTATION_ENTRYPOINTS`) is a hand-maintained tripwire. Any recipe that bypasses
the generic `execute()` must call `guard_mutation(..)` and be added there, or
the read-only edit-gate test will not cover it. The recorded hash-tracking
edits (`track_imported_file`, `track_line_hashes`) are such entry points.
`speleodb_edit_gate_tests.rs` and `speleodb_session.rs` carry the sibling
tripwires (every gate site, the slot commits, the BASE advances).

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

On the station graph the harness's oracles are strict across a save and a
reload: the attribution families for what a save healed or a reload re-derived
(`SAVE_HEALING_MUTATIONS`, `RELOAD_RESOLVED_GEOMETRY` in
`e2e/agent/lib/projectSnapshot.ts`) are empty, because a version 2 save never
heals and a load is pure. `fuzzOracleCore` adds the junction invariants (a line
passes a Station ID once and names each station once; slots that share an id
share its name and comment and list exactly each other in `merged_with`; each
`station_merges` pair is two slots of one station). `fixtures.ts` handles both
file formats without `node:sqlite`: a legacy fixture is left untouched (the app
drops a migrated file's history on open), and for a version 2 file its
`history` member is removed with every other byte kept.

## Fixtures

Shared between the Rust tests and the E2E tiers, in `e2e/fixtures/` - see
[`e2e/fixtures/README.md`](../e2e/fixtures/README.md) for the current inventory
and the DMP format notes. Playwright also keeps a small copy of the DMP files
under `e2e-playwright/fixtures/`.

The real Tauri backend parses these for real under the agent harness and in the
Rust tests. Playwright's mock does **not** read them - it returns simulated
line data.

The tracked `.stalx` files there are old-format (SQLite) projects: the
migration suites, the legacy-origin fuzz modes and the old-fixture journeys read
them. Always copy one into a temp dir before opening it through `AppState`: the
first save upgrades it to version 2 (and keeps the original in the app-data
backups folder). A version 2 file is never a fixture: tests write it at run
time.

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
