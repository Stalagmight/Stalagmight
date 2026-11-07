# IPC Type Generation

This project generates TypeScript types for IPC payloads directly from the Rust
backend using `ts-rs`. This keeps the frontend and backend in sync and reduces
manual drift.

## Command

Run the generator from the repo root:

```bash
pnpm gen:ipc-types
```

This runs `src-tauri/examples/gen_ipc_types.rs` (via `--example`) and produces:

> The example is gated behind the `ipc-typegen` cargo feature
> (`required-features` in `src-tauri/Cargo.toml`) so plain `cargo build` /
> `cargo build --release` skip it - it's dev-only tooling and was paying its
> own full LTO pass in release builds. `pnpm gen:ipc-types` enables the
> feature automatically; it is the only supported way to run the generator.

- `src/types/ipc.generated.ts` (re-exports for all generated IPC types)
- `src/types/ipc/*.ts` (one file per exported Rust type)
- `src/types/ipc/IpcCommands.ts` + `src/types/ipc/ipcCommands.json` (the command
  manifest, see below)
- `src/types/ipc/IpcEvents.ts` (the event manifest, see below)

The exported types live across the workspace member crates
(`src-tauri/crates/*`) as well as the app crate; the generator collects them
all, and `flatten_import_paths` rewrites the ts-rs-emitted relative import
specifiers so the flat `src/types/ipc/` output stays internally consistent no
matter which crate a type came from.

### The freshness gate

This is the tree the frontend, the Playwright IPC mock and `tsc` compile
against, and most of it has no other writer: `Project`, `CommandError`,
`Config` and the rest carry no `#[ts(export)]`, so no test regenerates them.
A Rust change that skipped `pnpm gen:ipc-types` used to leave `tsc` and the mock
agreeing with the stale declaration, and the mismatch first showed up at runtime
in a packaged build (#576 C1).

The `check` job now runs **Regenerate the IPC types** (`pnpm gen:ipc-types`)
after `cargo test`, then **Generated IPC types match the checkout**, which fails
on any modified or untracked file under `src/types/ipc/` or
`src/types/ipc.generated.ts`. Locally the same pair is on the pre-push
checklist:

```bash
pnpm gen:ipc-types
git status --porcelain src/types/ipc src/types/ipc.generated.ts   # must print nothing
```

The generator is deterministic, so a clean checkout regenerates byte-identical
files. Like ts-rs, it only writes: a file left behind after its type is removed
from the export list is not flagged - delete it by hand.

## The command manifest

`generate_handler!` in `src-tauri/src/lib.rs` is the only registry of IPC
commands, and each `#[tauri::command]` signature is the only statement of a
command's argument names. The generator reads both and writes the manifest
(#576 C3):

- `src/types/ipc/IpcCommands.ts` - `interface IpcCommands { <name>: { args;
  result; error } }` for every registered command, plus `IPC_COMMAND_NAMES`.
  `args` holds the keys Tauri deserializes (its default camelCase of the Rust
  parameter names; an `Option` parameter may be omitted); `result` is what the
  command resolves with (`ArrayBuffer` for a raw `tauri::ipc::Response`, `null`
  for `()`); `error` is its `Err` type - `CommandError` for every fallible
  command (#576 C4, see docs/architecture.md "Command results and errors") except
  the settings writes in `commands/config.rs`, which are still `string`
  (`save_display_setting` aside: its `PROJECT_CHANGED` refusal is coded, #576
  RV6b-2); `never`
  when it returns no `Result`. Tauri still rejects with a plain string when it
  cannot deserialize the arguments.
- `src/types/ipc/ipcCommands.json` - the same data, machine-readable, with the
  Rust spelling of every type, for the Rust tests.

Its consumers, which used to keep their own copies or re-parse `lib.rs`:

- `invokeIpc` (`src/utils/ipc.ts`) is typed by it: an unregistered name, a
  wrong or missing argument, or a wrong assumption about the result does not
  compile, and callers no longer assert a result type. `plugin:*` commands stay
  untyped; a dispatcher that forwards a command name it was handed (the
  mutation queue) calls it as `ForwardedInvoke` and types its own callers with
  `QueuedCallArgs<K>`, which, like `invokeIpc`, lets the arguments be left out
  only for a command that takes no required one (#576 RV5-PI-6).
- The Playwright IPC mock types every reply with `MockReply<'name'>`
  (`e2e-playwright/mocks/ipc/types.ts`), so `pnpm typecheck:playwright` fails
  when a mocked reply drifts from what the backend returns.
- `pnpm test:mock-contract` takes the registered names from
  `IPC_COMMAND_NAMES`.
- The telemetry vocabulary no longer lists `command.*` events (the generated
  names are their vocabulary; docs/telemetry.md), and the app crate's
  open_project tripwire reads `ipcCommands.json`.
- `execute_batch`'s payload is the generated `Command` (the Rust enum, with
  `?:` on its `Option` fields since the frontend only sends it);
  `BatchCommand` (`src/types/commands.ts`) narrows it to the variants the
  frontend batches.

**Mechanism.** The generator (`src-tauri/examples/gen_ipc_types/commands.rs`)
parses `lib.rs` and the `commands/` modules with `syn`, an optional dependency
compiled only under the `ipc-typegen` feature. A proc-macro attribute would
have needed a new crate and an annotation on every command, and a build script
would run on every build; reading the source in the generator needs neither,
and the CI freshness gate above makes the committed manifest as trustworthy as
a build step. Types resolve by their last path segment against everything the
generator exports, plus three small tables in that file (Tauri-injected
parameters, string/number primitives incl. the `SectionId`/`ShotId` aliases,
and the two deliberately opaque types). Anything else stops the generator with
the command and parameter named, so a new command whose types are not exported
cannot produce a partial manifest. Not modelled (the generator refuses them):
an entry under `#[cfg]` in `generate_handler!`, a `#[tauri::command(...)]` with
arguments (they change the wire keys), a command outside `commands::<module>`.

## The event manifest

Every event the backend emits to the webview is declared once in
`core::events` (#576 C13): a marker type implementing `IpcEvent` with the
wire `NAME` and the `Payload` type. Core emitters serialize through
`events::payload::<E>`, the app crate's through `commands::event_sink::emit_event::<E>`,
so a misspelled name or a wrong payload does not compile. The generator writes
`src/types/ipc/IpcEvents.ts` (`interface IpcEvents { <name>: <payload> }` and
`IPC_EVENTS`), and listeners go through `listenIpc(IPC_EVENTS.<NAME>, ...)`
(`src/utils/ipcEvents.ts`), which types the payload. A new event is a new
`IpcEvent` impl plus one line in `gen_ipc_types/events.rs`; until that line
exists the frontend has no `IPC_EVENTS` key to listen with.

## The second tree: `src-tauri/bindings/`

There is a *second* committed tree of generated TypeScript, and `pnpm gen:ipc-types`
does not write it. The generator hardcodes its output directory to
`../src/types/ipc` (`gen_ipc_types.rs`), so it never touches `src-tauri/bindings/`.

That tree is a side effect of **`cargo test --workspace`**. Every
`#[ts(export)]` derive makes ts-rs emit an `export_bindings_*` unit test whose
body writes the type's `.ts` file. ts-rs's default output directory is
`<crate>/bindings/` - which, after the workspace split, would scatter the tree
across `src-tauri/crates/*/bindings/`. So the rule is: every exported type in
a member crate carries `#[ts(export, export_to = "../../bindings/")]`, which
resolves from `crates/<member>/` back to the single `src-tauri/bindings/`
tree, and running the workspace tests rewrites every file in it. The **app
crate hosts no `#[ts(export)]` at all** - keep it that way: an `export_to` is
resolved against the declaring crate's own directory, so an app-crate export
either lands in the bindings tree by a different spelling or, as the two
memory-probe types once did with `"../../src/types/ipc/"`, outside the
checkout altogether (every `cargo test` wrote `MemoryStats.ts` and
`ProcessMemory.ts` into the checkout's parent, #576 RV5-RL-1). Those types now
live in the models crate (`process_memory.rs`); an app-crate type the frontend
needs derives `TS` without `export` and reaches `src/types/ipc/` through the
generator only. `ts_export_paths_tests` (app crate) scans every `#[ts(export
...)]` in the workspace and fails on one that would write anywhere but
`src-tauri/bindings/`, or that the app crate hosts.

**Nothing consumes it.** No module in `src/`, `e2e-playwright/`, `e2e/` or
`scripts/` imports from it, and it is in no tsconfig's `include` (see
[testing-recipes.md](testing-recipes.md#typecheck)). It exists because ts-rs
generates it by default, not because anything reads it.

Because nothing reads it, nothing used to notice it going stale - it was easy to
hand-edit or hand-delete a file there while renaming a type and have CI stay
green. The `check` job now runs a **ts-rs bindings tree matches the checkout**
step immediately after `cargo test --workspace`, which fails if that tree has any
modified or untracked file. Treat the tree exactly like `src/types/ipc/`: never hand-edit it;
run the Rust tests and commit whatever they produce.

One gap the step cannot close: ts-rs only *writes* files, it never deletes them,
so a `.ts` left behind after its Rust type is deleted leaves the worktree clean
and passes. That orphan is inert (nothing imports it), but it does mean the tree
can accumulate dead files - delete them by hand when you remove a type.

## A field the wire can omit must be DECLARED omittable

ts-rs 7 marks a field optional (`foo?: T`) in exactly one situation: the Rust
type is `Option<T>`. `#[ts(optional)]` is *rejected* on anything else -
`` `optional` can only be used on an Option<T> type `` (verified verbatim at
`ts-rs-macros-7.1.1/src/types/named.rs:138`). It also *infers* the annotation,
but only from the literal predicate string: `attr/field.rs:70` is
`out.0.optional = parse_assign_str(input)? == *"Option::is_none"`, so a NAMED
predicate (`skip_serializing_if = "my_fn"`) silently regenerates the field as
required. Any field using one must carry an explicit `#[ts(optional)]`.

So `#[serde(skip_serializing_if = "Vec::is_empty")]` on a `Vec` field generates a
**required** `Array<T>` while the wire omits the key. Nothing catches that: `tsc`
believes the declaration, so every `payload.field.length` type-checks and throws
at runtime, and the e2e IPC mock speaks the TS type, so the mocked tier cannot
produce the omitted shape either. That combination crashed the zero-section
import receipt (#411, diagnosed as #441).

**The rule.** A serde-omitted field has to pick one, and the choice is about who
pays for the bytes:

| Cost of always sending it | Do this |
|---|---|
| Once per payload (a command result, a header) | **Drop `skip_serializing_if`.** The array is a few bytes and the already-generated required type becomes true - no consumer changes, no `?? []` that encodes nothing. |
| Once per ELEMENT of a large collection | **Keep the skip, retype the field `Option<Vec<T>>`.** ts-rs then emits `foo?:` and `tsc` flags every unguarded read. |

The live examples:

- `section_ids` (`BatchOutcome`, `ImportOutcome`, `crates/core/src/core/outcome.rs`;
  formerly on `CommandResult`) dropped the skip. One `"section_ids":[]` per
  command result is unmeasurable, and absence and empty stated the same fact. The
  same holds for every per-command outcome (#576 A12): nothing on them is omitted -
  an unset `remote_link` is `null`, an empty `reversed_sections` is `[]`.
- `CommandError`'s payloads (`crates/core/src/core/command_error.rs`) keep the skip
  and are each an `Option`, so ts-rs declares them `?:`: each is set only for the
  codes named on it, which is what makes its presence a discriminant, and only
  `code` and `message` are always sent. They live in a boxed struct flattened into
  the error (`#[serde(flatten)]`), so the wire and the declaration stay one flat
  object.
- `Station::closures_with` (`crates/models/src/station.rs`) kept it and became
  `Option<Vec<ClosureRef>>` + `#[ts(optional)]`. That skip is paid per station on
  every JSON `get_project_state` and on `get_adjusted_stations`, which returns a
  bare `Vec<Station>` as JSON. The load and, since #576 A9-D2, the post-edit
  `refreshProjectState` go through the binary envelope instead
  (`load_project_full_binary` / `get_project_state_binary`), which carries
  `closures_with` as a sparse list; the JSON `get_project_state` remains for the
  agent harness and tests. On the 89k-station projects `core::load_binary` cites,
  the adjusted view's JSON is the payload the skip protects. Reads go through
  `Station::closures()` / `closures_mut()`.

  Its predicate is `closures_absent_or_empty`, not `Option::is_none`: because
  `closures_mut()` hands out a `&mut Vec`, an empty `Some` is reachable without a
  push, and `Option::is_none` alone would then put `"closures_with":[]` on every
  station for the life of the value. Emptiness-aware is the rule whenever the
  collection is reachable mutably.

**What guards it.** Not a lint - there is no repo-wide check that every exported
type obeys the rule. It is pinned per field by two tests, each asserting BOTH
halves (the key is on the wire iff the declaration lacks `?`):

- `models::station::tests::closures_with_is_omitted_when_empty_and_declared_optional`
- `models::station::tests::an_emptied_closure_list_leaves_the_wire_like_an_absent_one`
- `core::command_error::tests::omitted_fields_are_exactly_the_ones_declared_optional`
- `core::outcome::tests::no_outcome_field_is_omitted_or_declared_optional`

Add the same pair when you add a serde-omitted field. In the Playwright IPC mock
(`e2e-playwright/mocks/tauri-ipc.ts`) the same convention applies by hand: an
omitted field is written **absent**, not present-and-empty (`closures_with` on
the `get_project_state` stations), and an always-present one is written even when
empty (`section_ids` on every `BatchOutcome` / `ImportOutcome` reply,
`remote_link` on every mutation outcome); a failure is a rejection with a
`CommandError` object, its payloads absent unless set. Half of that is
gated since #576 C3: every reply is checked against the generated result type
(`MockReply`, see "The command manifest"), so a required field cannot be left
out. The other half is not - a type cannot tell an absent optional field from a
present-and-empty one - so writing an omitted field absent is still caught by
review, not by a gate.

## A 64-bit integer is declared `number`

ts-rs 7 declares every `u64`/`i64` as `bigint`, but no IPC path in this app
delivers one: `invoke()` results and events are JSON, where serde writes the
integer as a JSON number, and the msgpack load envelope is decoded with
`@msgpack/msgpack`'s default `useBigInt64: false`, which yields numbers too. A
`bigint` declaration lied about the wire, and code that trusted it failed only
against the real backend (`0 === 0n` is false - Clear cache never disabled)
while the Playwright mock, which speaks the declared type, returned real bigints
and stayed green (#576 C6).

So every such field carries `#[ts(type = "number")]` (an `Option<u64>` would
take `"number | null"`). That is sound because every one of them - ids,
sequence counters, byte and tile counts, latencies, epoch milliseconds - stays
far below 2^53, where a JS number is exact; a value that could exceed it would
already lose precision on the JSON wire whatever the declaration said.
`pnpm gen:ipc-types` panics if any generated file still says `bigint`, naming
the file, and `src/test/__tests__/ipcWireNumbers.test.ts` pins the committed
tree.

## Automation Policy

These IPC types are **always generated**, never written by hand:

- Do not create or edit files under `src/types/ipc/`.
- Do not edit `src/types/ipc.generated.ts`.
- Do not create or edit files under `src-tauri/bindings/` either - it is generated
  by `cargo test --workspace`, not by `pnpm gen:ipc-types` (see above), and CI now
  fails on any drift there.
- If a type changes, update the Rust source + export list, run `pnpm gen:ipc-types`,
  and commit the regenerated output.

## Adding a New IPC Type

1. Add `#[derive(TS)]` to the Rust type definition.
2. If the type lives in a workspace member crate (`src-tauri/crates/*` - the
   normal case), export it with
   `#[ts(export, export_to = "../../bindings/")]` so its bindings file lands
   in the shared `src-tauri/bindings/` tree. Do not add bindings-defaulted
   (bare `#[ts(export)]`) types to the app crate.
3. If the type uses `#[serde(rename_all = "...")]`, add the matching
   `#[ts(rename_all = "...")]` so the TS output matches JSON.
4. For non-JSON-friendly fields (e.g., `PathBuf`, `DateTime`), add
   field-level overrides like `#[ts(type = "string")]`. A `u64`/`i64` field
   gets `#[ts(type = "number")]` (see below); the generator refuses to write a
   `bigint`.
5. For any field carrying `skip_serializing_if`, apply the rule above - a
   collection field must either lose the skip or become an `Option`, never a
   skipped `Vec`.
6. Add the type to the export list in `src-tauri/examples/gen_ipc_types.rs`.
7. Re-run `pnpm gen:ipc-types` and commit the updated generated files.

A new **command** needs no manifest edit: register it in `generate_handler!`,
make sure its argument and result types are exported (the generator names any
that are not), and regenerate. Then add its mock case (or an
`UNMOCKED_COMMAND_REASONS` entry) - `pnpm test:mock-contract` asks for it.

## Notes

- Generated files are committed to the repo for CI and editor support.
- Do not edit `src/types/ipc.generated.ts` or files under `src/types/ipc/`
  by hand; regenerate instead.
