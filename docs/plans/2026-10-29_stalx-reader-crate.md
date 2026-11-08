# Stalagmight on SpeleoDB: `.stalx` reader crate (#593)

Status: DRAFT for review by the SpeleoDB maintainer and the Stalagmight SpeleoDB integration owner.

## Executive summary

- **Storage already works.** SpeleoDB stores the `.stalx` as an opaque file in project history.
  Push and pull round-trip it; SpeleoDB never interprets it for storage.
- **Maps don't.** SpeleoDB generates GeoJSON only for Ariane and Compass projects. A Stalagmight
  project gets no map.
- **Proposal:** Stalagmight publishes a read-only Rust crate that reads `.stalx`. SpeleoDB builds
  it, maps our survey model into its own `Survey`, and runs its existing GeoJSON generation.
- **Ownership:** SpeleoDB owns the survey shape, the mapping and all GeoJSON properties.
  Stalagmight owns the crate API (semver) and keeps it reading every `.stalx` version it ships.
- **Dropped:** a Stalagmight interchange JSON (#592) and client-uploaded GeoJSON (#220/#221).

## Current state (verified 2026-10-29)

| Fact | Where |
|---|---|
| Uploads of unrecognised files are stored "without any special treatment" | SpeleoDB `surveys/models/format.py:32` |
| GeoJSON generation is gated on project type ARIANE or COMPASS | SpeleoDB `gis/geojson_generation.py:66-69, 107, 367` |
| STALAGMIGHT is a project-type label only; no processor | SpeleoDB `docs/project-formats.md` |
| Map generation runs through `openspeleo_lib` `Survey` -> `survey_to_geojson` | SpeleoDB `surveys/models/project.py:566-572` |
| No endpoint accepts a client-supplied GeoJSON | SpeleoDB `gis/tasks.py:162` is the only writer |
| Upload limit: 24,657,920 bytes combined | SpeleoDB OpenAPI, `projects/{id}/upload/{fileformat}/` |
| Stalagmight retypes adopted projects `ariane` -> `other` | Stalagmight `speleodb.rs:459`, `speleodb_push.rs:106` |

**Consequence of the last row (bug, today):** once a project is adopted, SpeleoDB stops generating
its map, so the `ariane.tml` co-push no longer has any effect and the map freezes at its
pre-adoption state.

## Design

```
Stalagmight app --push .stalx--> SpeleoDB (stored as-is, versioned)
                                    |
                                    v
                  stalx reader crate (ours, built by SpeleoDB)
                                    |
                                    v
                  StalagmightInterface (SpeleoDB's) -> Survey -> survey_to_geojson
```

### Crate (Stalagmight)

- Never writes the source. Copies the input to a private temp file and runs the app's own
  load path on the copy (integrity check, forward migrations, foreign keys), so every shipped
  schema version reads correctly (currently v18; v12 changed the pitch sign convention).
- Exposes its own small types: project metadata, sections, shots, station merges, zero datums,
  loop closures, station comments. Optional: measured (unadjusted) station positions.
- Never loads undo/redo history (old-file migrations may still drop it from the copy).
- Hostile input: refuses files that define views, triggers, virtual tables, generated
  columns, expression/partial indexes, expression defaults or CHECKs other than the app's
  `CHECK (id = 1)` (by parsed type where SQLite exposes it); row counts are capped before
  the load; every connection runs in defensive mode with `trusted_schema` off, a value-size
  cap and a deadline; no mmap, `cell_size_check` on; size-capped, streamed input (64 MiB
  default); refuses newer schemas.
- **Bounds:** one deadline covers the read. SQLite statements are interrupted mid-run; the
  post-load Rust steps are checked between, so a started step finishes. Those steps are
  near-linear (#596, #597) except the offset cascade on load (#602). No heap cap beyond input
  size and row caps, and the bundled SQLite is old (#598). SpeleoDB should run each read in a
  worker it can kill.
- Semver. A `.stalx` schema bump is a crate release; SpeleoDB upgrades the crate to read new files.
- License: AGPL-3.0-or-later, same as both projects.

### `StalagmightInterface` (SpeleoDB)

- Lives in SpeleoDB. Maps the crate's model into `Survey`; SpeleoDB adds whatever properties it
  needs.
- Stalagmight ships a **reference stub** (not production) for local testing and as an
  implementation hint.
- SpeleoDB-side changes: add STALAGMIGHT to the generation gate, a `build_geojson` case, and
  `.stalx` source materialisation.

### Parity test

For one synthetic project: stub(crate(`.stalx`)) must equal `ArianeInterface.from_file()` on our own
exported `.tml` of the same project. SpeleoDB's existing Ariane path validates our reader.

## Round trip

- **Survey data:** lossless. The `.stalx` is stored and returned byte for byte.
- **By design, not pushed:** redo history (always), undo history (unless the project's undo mode
  pushes it), the "unsaved changes" flag.
- **Map:** derived and one-way. Nothing converts a `Survey` back to `.stalx`.

## Binary storage cost (measured on real surveys, 2026-10-29)

| Measurement | Binary `.stalx` | SQL text dump |
|---|---|---|
| Largest survey (4,816 shots), compacted / gzip -9 | 964 KB / 328 KB | 1,277 KB / 307 KB |
| Two successive revisions, git pack after `gc --aggressive` | 83 KB | 79 KB |

Binary costs about 5-7% in storage. Git delta-compresses binary blobs too. What binary loses is
human-readable diffs on SpeleoDB; merges are moot under the single-writer lock. Largest real file is
about 4% of the upload limit (more if undo history is pushed).

## Changes on the Stalagmight side

1. Stop retyping adopted projects to `other`; use `stalagmight`. Fixes the frozen map once SpeleoDB
   supports the type. Done (#599).
2. Stop co-pushing `ariane.tml` once SpeleoDB generates from `.stalx`.
3. Add a pre-push size check against the upload limit.
4. Add a push -> pull equality test (env-gated live test).

## Publishing (crates.io)

Decided: publish to crates.io. Three crates go up: `stalagmight-models`,
`stalagmight-persistence` and `stalx-reader`. `persistence` uses `parsers` and `compass_data` only
in tests, so they stay unpublished.

- **Supported API:** `stalx-reader` only. `models` and `persistence` are published as internal
  crates with no semver promise (their crate docs say so). The reader pins them exactly (`=x.y.z`),
  so SpeleoDB only ever depends on the reader's API.
- **Versions:** all three start at `0.1.0`. A `.stalx` schema bump is a new reader minor version
  while it is `0.x`, with the supported schema version in the release notes.
- **Owners:** a `crates-owners` team in the `Stalagmight` GitHub org, plus two user owners (a team
  owner can publish and yank but cannot change owners).
- **Release:** bump the three versions together, run the full check, then publish in dependency
  order with one command:
  `cargo publish --dry-run -p stalagmight-models -p stalagmight-persistence -p stalx-reader`,
  then the same without `--dry-run`. CI runs the packaging half on every change
  (`--no-verify`, in the `check` job) so the crates stay publishable between releases.
- **Packaged contents:** Rust sources and manifests only (checked with `cargo package --list`); no
  fixtures.

## Open questions

1. SpeleoDB: does the crate's model (above) cover what `StalagmightInterface` needs? Anything missing?
2. SpeleoDB: should the crate expose computed station coordinates, or does SpeleoDB compute them?
3. Both: where does the reference stub live (the crate's `examples/`, or a separate repo)?
4. ~~Both: crate distribution~~: crates.io, see "Publishing" above.
5. Integration owner: is item 1 under "Changes on the Stalagmight side" safe for already-adopted
   projects?
