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

The exported types live across the workspace member crates
(`src-tauri/crates/*`) as well as the app crate; the generator collects them
all, and `flatten_import_paths` rewrites the ts-rs-emitted relative import
specifiers so the flat `src/types/ipc/` output stays internally consistent no
matter which crate a type came from.

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
crate hosts no bindings-defaulted exports** - keep it that way, so every
writer of the committed tree follows the one explicit `export_to` convention.
The two types in `commands/memory.rs` use
`#[ts(export_to = "../src/types/ipc/")]` without `export`, so they generate no
automatic export tests. Only `pnpm gen:ipc-types` writes their frontend files,
anchored to the compile-time app-crate manifest directory. Automatic ts-rs
exports instead resolve paths against the runtime `CARGO_MANIFEST_DIR`; if a
test runner sets that to the app root, `../src/types/ipc/` escapes Stalagmight
and writes into the parent `apps/src/` directory. Keep `export` off these types
to avoid that second, environment-dependent writer.

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

The two live examples, one of each:

- `CommandResult::section_ids` (`crates/core/src/core/command.rs`) dropped the
  skip. One `"section_ids":[]` per command result is unmeasurable, and absence
  and empty stated the same fact.
- `Station::closures_with` (`crates/models/src/station.rs`) kept it and became
  `Option<Vec<ClosureRef>>` + `#[ts(optional)]`. That skip is paid per station on
  every `get_project_state` refresh - and the msgpack `load_project_full_binary`
  route does NOT cover it: the binary encoder is the LOAD path only (taken
  unconditionally, no station-count threshold), while `refreshProjectState`
  re-fetches `get_project_state` as JSON after mutations, and
  `get_adjusted_stations` returns a bare `Vec<Station>` as JSON too. On the
  89k-station projects `core::load_binary` cites, that is the payload the skip
  protects. Reads go through `Station::closures()` / `closures_mut()`.

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
- `core::command::tests::omitted_fields_are_exactly_the_ones_declared_optional`

Add the same pair when you add a serde-omitted field. In the Playwright IPC mock
(`e2e-playwright/mocks/tauri-ipc.ts`) the same convention applies by hand: an
omitted field is written **absent**, not present-and-empty (`closures_with` on
the `get_project_state` stations), and an always-present one is written even when
empty (`section_ids` on every `-> CommandResult` handler). Nothing enforces
that - `pnpm test:mock-contract` checks only that the mock covers the command
NAMES in `generate_handler!`, not the shape of what it returns - so a mock
literal that drifts from the wire is caught by review, not by a gate.

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
   field-level overrides like `#[ts(type = "string")]`.
5. For any field carrying `skip_serializing_if`, apply the rule above - a
   collection field must either lose the skip or become an `Option`, never a
   skipped `Vec`.
6. Add the type to the export list in `src-tauri/examples/gen_ipc_types.rs`.
7. Re-run `pnpm gen:ipc-types` and commit the updated generated files.

## Notes

- Generated files are committed to the repo for CI and editor support.
- Do not edit `src/types/ipc.generated.ts` or files under `src/types/ipc/`
  by hand; regenerate instead.
