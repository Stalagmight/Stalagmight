# Rust Linting & Formatting Policy

This repo runs a deliberately strict Rust lint + format policy whose primary job
is to **catch AI-generated-code smells** before they land. Both gates run in the
CI `check` job and must be green to merge.

| Gate | Command | Config |
|------|---------|--------|
| Format | `cargo fmt --manifest-path src-tauri/Cargo.toml --check`, then `for m in src-tauri/crates/*/Cargo.toml; do cargo fmt --manifest-path "$m" --check; done` | `src-tauri/rustfmt.toml` (stable defaults, `edition` pinned; applies to every member) |
| Lint | `cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --features telemetry --all-targets` | `src-tauri/Cargo.toml` → `[workspace.lints.clippy]` / `[workspace.lints.rust]`, inherited by every member via `[lints] workspace = true` |

Run `cargo fmt` and `cargo clippy` locally before pushing (they're in the
[Pre-Push Checklist](../CLAUDE.md)).

## Formatting: rustfmt scope and adoption

`cargo fmt` is the formatter for the **`src-tauri` workspace** (the
`stalagmight` app package plus the eight member crates under
`src-tauri/crates/`), enforced in CI: the `check` job runs
`cargo fmt --manifest-path src-tauri/Cargo.toml --check` and then the same
check in a loop over `src-tauri/crates/*/Cargo.toml`, failing on any drift.
Run `cargo fmt` (root + the same loop, without `--check`) to fix formatting
before pushing; the `--check` forms in the Pre-Push Checklist then pass.
Config is `src-tauri/rustfmt.toml` - stable rustfmt defaults, only `edition`
pinned; do **not** add nightly-only keys (they'd diverge local vs CI).

**Why a loop and not `--all`:** `cargo fmt --all` formats every *path
dependency* as well as the workspace members, which would reformat the
deliberately-unformatted vendored `src-tauri/vendor/compass_data` crate. The
per-manifest loop touches exactly the workspace's own packages.

**Scope:** the gate formats the `stalagmight` package and the workspace
members. The separate first-party `scripts/lib/slice-rs` helper crate and the
vendored, locally-patched `src-tauri/vendor/compass_data` path-dependency are
**not** gated (vendored code is left as-is on purpose; `compass_data` is also
explicitly kept out of the workspace via `[workspace] exclude`, so
`--workspace` commands never touch it) - don't assume `cargo fmt` touched
them.

The crate was adopted into rustfmt in one mechanical commit (recorded in
`.git-blame-ignore-revs`, so `git blame` skips it - wire it locally once with
`git config blame.ignoreRevsFile .git-blame-ignore-revs`). If a `cargo fmt`
diff ever looks large, that's normal re-wrapping; the gate keeps it from
drifting again.

## Why this exists: the rot lesson

The strict policy was added in **Feb 2026 (`17dc4feb`)**, explicitly to
guardrail AI-written Rust - its comments call out the patterns directly: *"AI loves
.unwrap()"*, *"Clone-spam - AI appeases the borrow checker with .clone()"*, *"String
nonsense AI generates"*, *"Prevent AI from silencing lints"*.

But `[lints.clippy]` **only takes effect under `cargo clippy`**, and CI never ran
clippy. So the policy silently **rotted**: by June 2026, ~4 months of new code had
reintroduced **172 deny-level violations**. The fix (PR #147/#149) was to restore
cleanliness *and* **gate clippy in CI** so it can't drift again.

> **Lesson: a lint policy that isn't a CI gate is decoration.** If you tighten the
> policy, make sure `check` still enforces it.

## The deny-level policy (the "must fix" set)

Production code may not use (see `[workspace.lints.clippy]` for the full list + comments):

- **Panic safety:** `unwrap_used`, `expect_used`, `panic`, `unwrap_in_result`
- **Debug artifacts:** `dbg_macro`, `todo`, `unimplemented`, `print_stdout`, `print_stderr`
- **Clone/string spam:** `inefficient_to_string`, `clone_on_ref_ptr`, `string_to_string`, `rc_clone_in_vec_init`
- **Hygiene:** `wildcard_imports`, `allow_attributes` (you can't silence lints with `#[allow]`), `wildcard_dependencies`, `redundant_type_annotations`, `mem_forget`, `lossy_float_literal`, `needless_pass_by_ref_mut`, `semicolon_if_nothing_returned`, `explicit_into_iter_loop`
- **Async footguns:** `await_holding_lock`, `await_holding_refcell_ref`
- **`[workspace.lints.rust]`:** `unsafe_code`, `unused_must_use`, `unused_qualifications`, `unreachable_pub`

`pedantic` is on at **warn** (advisory, with specific `allow`s for known-noisy
members). Only deny-level lints fail the build.

## Test code: the per-module `#[expect]` idiom

Test code legitimately uses `unwrap`/`expect`/`panic`/`print`. The **deliberate
convention** (not `clippy.toml`, not `#[allow]` - which is denied) is a
per-test-module `#[expect(...)]` listing exactly the lints that module uses:

```rust
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::float_cmp)]
mod tests;                                   // on the `mod tests;` declaration
```
or, for an inline module, directly above `mod tests {`. `#[expect]` (not
`#[allow]`) is required because it *also* warns if the lint stops firing, so the
exemptions don't go stale. When you add a test that uses a new denied lint
(e.g. a `println!` diagnostic), add that lint to the module's `#[expect]`.

> Do **not** introduce `clippy.toml` `allow-*-in-tests` - it conflicts with the
> ~90 existing test-module `#[expect]` attributes (it suppresses the lint,
> leaving them unfulfilled). Follow the existing idiom.

## Extending the policy: measure, don't vibe

When adding lints, **measure the hit count on the real tree first** - a
conceptually-good lint can be unusable in practice:

```bash
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --features telemetry --all-targets \
  --message-format=json -- -W clippy::<candidate> | \
  python3 -c "import json,sys; print(sum('clippy::<candidate>' in l for l in sys.stdin))"
```

Prefer **surgical** lints (catch a specific AI pattern, low false-positive) over
**blunt** ones, and **respect the existing `allow`s** - `too_many_lines` and
`too_many_arguments` are allowed *on purpose* for the geometry/numerical code.

### Considered and deliberately deferred

Recorded so they aren't naively re-proposed. Counts measured June 2026 on the
clean tree.

| Lint | Hits (prod/test) | Why not (yet) |
|------|------------------|---------------|
| `indexing_slicing` | 831 / 829 | Right instinct (`v[i]` panics, which `panic=deny` doesn't catch) but pervasive in legitimate code - denying = noise. Revisit as a focused audit, not a blanket gate. |
| `default_numeric_fallback` | 1206 / - | Flags every untyped numeric literal. Impractical. |
| `str_to_string` | 1002 / 1866 | `"x".to_string()` vs `.to_owned()` - stylistic, codebase-wide; `inefficient_to_string` already covers the perf case. |
| `string_slice` | 53 / - | Byte-slicing `&s[a..b]` can panic on non-char-boundary, but most uses here are ASCII-safe. Candidate for a future audit. |
| `let_underscore_must_use` | 42 / 10 | Many `let _ =` discards are intentional. |
| `map_err_ignore` | 32 / - | `.map_err(\|_\| …)` swallows errors (real AI tell), but discarding the source error is often deliberate. Warn-only candidate. |
| `get_unwrap` | 7 / 18 | `.get(i).unwrap()` is a panic door worth closing - deferred only for the test-module `#[expect]` churn (18 sites). Good next add. |
| `panic_in_result_fn` | 1 / - | The one hit is a *legitimate* invariant assert in the LSQ solver (`project.rs`). The lint conflates invariant-asserts with slop, so it'd just need a per-site `#[expect]`. |
| `unreachable`, `exit` | 3 / 1 | Fire only on `#[tauri::command]` / `.run()` **macro expansions**, not our code - adding them would force permanent framework `#[expect]`s for ~zero AI-slop value. |
| `too_many_arguments` (deny), `cognitive_complexity`, `too-many-lines-threshold` | - | Contradict deliberate `allow`s for the complex geometry/numerical code. |
| `transmute_ptr_to_ref`, `cast_ptr_alignment` | - | Redundant: `unsafe_code = "deny"` already blocks anything that needs `unsafe`. |
| `cargo_common_metadata`, `multiple_crate_versions` | 15 / 44 | Matter for *published* crates; this is a private app binary with a large transitive tree. |
