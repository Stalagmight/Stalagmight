# Documentation index

Every page under `docs/`, grouped. Where a page and the code disagree, the code
wins - the pages that age best are the ones that point at a file instead of
copying it, and most of these do.

## Start here

1. [`../README.md`](../README.md) - what the app is, and how to build and run it.
2. [`../CONTRIBUTING.md`](../CONTRIBUTING.md) - contribution scope, dev setup,
   and the checks a change has to pass. The canonical copy of that gate list is
   the Pre-Push Checklist in [`../AGENTS.md`](../AGENTS.md) (a symlink to
   `CLAUDE.md`); if the two drift, that one is authoritative.
3. [`project-structure.md`](project-structure.md) - where code lives.
4. [`architecture.md`](architecture.md) - how a user action becomes a persisted
   change.
5. [`SPEC.md`](SPEC.md) - the behavioral reference. Read the section you need;
   it is not a document you read front to back.

Then [`TESTING.md`](TESTING.md) before your first PR - which tier a test belongs
in determines how you write it, and picking wrong is the most common rework.

## User guide

| Doc | Contents |
|-----|----------|
| [user-guide/README.md](user-guide/README.md) | For people using the app: workflows and screens, with screenshots. Has its own index. |

## Orientation

| Doc | Contents |
|-----|----------|
| [project-structure.md](project-structure.md) | Directory tree and tech stack. |
| [architecture.md](architecture.md) | The 9-crate workspace layout, state flow (Zustand -> IPC -> Rust command executor), the command pattern, key files, backend idioms. |
| [SPEC.md](SPEC.md) | Data model, UI, workflows, file formats, validation rules. Defers to code for the enums, schema, and config keys. |
| [roadmap.md](roadmap.md) | Historical record of the v1.0 scope; live work is in the issue tracker. |

## Core reference

| Doc | Contents |
|-----|----------|
| [app-modes.md](app-modes.md) | Survey / Data / Plan / Export modes, mode entry and exit, and the read-only contract every mutation path has to check. |
| [saving.md](saving.md) | Atomic save (`.stalx.tmp` then rename), auto-save, undo persistence, and the deferred-import save contract. |
| [undo-redo.md](undo-redo.md) | Command-pattern undo: two-phase capture, `UndoData` variants, batch and non-undoable commands, persistence, lock ordering. |
| [closures.md](closures.md) | Loop closure as a non-destructive display mode: Raw Data View vs Closures Computed View, the weighted least-squares solve, co-location. |
| [inclination.md](inclination.md) | Stored pitch is +down; where each importer normalises, and what the v12 migration has to discard. |
| [configuration.md](configuration.md) | `config.toml` reference - file locations, every section, logging. Ground truth is the `Config` struct. |
| [ipc-typegen.md](ipc-typegen.md) | How the TypeScript IPC types are generated from Rust with `ts-rs`, and what must never be hand-edited. |

## Subsystems

| Doc | Contents |
|-----|----------|
| [map-renderer.md](map-renderer.md) | The custom WebGL survey renderer (a MapLibre `CustomLayerInterface`): tessellation, shaders, labels, incremental updates. |
| [color-palettes.md](color-palettes.md) | Depth / distance color palettes and the Levels range: where each setting is stored, the gamma mapping, the byte-exact default, palette sources and licences, the live-preview path, export. |
| [gas-planning.md](gas-planning.md) | Plan mode's gas engine: gas-weighted reachability over the survey graph, turnaround and stage-drop points. |
| [move-tools.md](move-tools.md) | Move Station / Shot / Segment: scope of each, the shared drag-preview engine, Ctrl+drag panning, immovable stations. |
| [SpeleoDB.md](SpeleoDB.md) | Complete SpeleoDB exchange contract: API calls and callers, artifacts, authentication, permissions, mutexes, revision fences, recovery, and workflow diagrams. |
| [telemetry.md](telemetry.md) | Opt-in anonymous counters: the event registry, source tracking, and how to add events for a new feature. |
| [style-guide.md](style-guide.md) | UI conventions: design tokens, typography, buttons, panels, tables, forms, accessibility. |
| [notifications.md](notifications.md) | Which surface (toast, banner, dialog, log) an event gets: the decision tree, severity, dedup keys, copy rules. |
| [touch-support.md](touch-support.md) | What touch and pointer support exists, the long-press paths, and the verified gaps. |

## Import and formats

| Doc | Contents |
|-----|----------|
| [dmp-import.md](dmp-import.md) | The MNemo `.dmp` import wizard: phases, virtual connections, suggestion scoring, overlap handling, batch import. |
| [ariane-import.md](ariane-import.md) | Reading `.tml` / `.tmlu`: the shared schema, topology reconstruction, unit conversion, azimuth semantics, limitations. |
| [ariane-export.md](ariane-export.md) | Writing `.tml` / `.tmlu`: data mapping, topology, color conversion, station-comment co-encoding, limitations. |

## Testing and tooling

| Doc | Contents |
|-----|----------|
| [TESTING.md](TESTING.md) | The tier model and how to pick one, plus the contract a UI component has to satisfy to be testable. Read before your first PR. |
| [testing-recipes.md](testing-recipes.md) | Lookup reference: exact commands per tier, per-framework patterns, mock and fixture conventions, troubleshooting. |
| [speleodb-dev-testing.md](speleodb-dev-testing.md) | Testing the SpeleoDB client against a real server: env setup, env-gated live tests, the verified v2 API contract. |
| [agent-harness.md](agent-harness.md) | The MCP + webdriverio harness that drives the real app: tools, oracles, stress scenarios, replay and repro reports. Runs on the repo-pinned Node. |
| [uat/ledger.md](uat/ledger.md) | The acceptance tier: one line per real-app UAT run, the per-RC checklist in `uat/checklists/`, and the machine-readable records in `uat/runs/`. |
| [debug-gates.md](debug-gates.md) | Where every dev-only log, assert and profile harness lives, how to switch it on, and the rules for adding more. |
| [linting.md](linting.md) | The `cargo fmt` + clippy CI gates, the anti-AI-slop lint policy, the per-test-module `#[expect]` idiom, deferred lints. |
| [performance.md](performance.md) | Where the time goes and how to measure it: the closure solver, frozen-shot detection, geometry reconcile, and the WebGL renderer's per-frame vs per-data-change cost. |

## Platform and ops

| Doc | Contents |
|-----|----------|
| [toolchain.md](toolchain.md) | The pnpm/node version pins: the corepack integrity hash, `allowBuilds` build-script approvals, and the lockfile version policy. |
| [macos-build.md](macos-build.md) | The macOS setup and build scripts, and their known issues. |
| [releasing.md](releasing.md) | Changelog to tag to release: `scripts/release.sh`, release candidates and the Stable/Beta update tracks, the CI workflows, and where version numbers live. |
| [debugging-macos-pan-zoom.md](debugging-macos-pan-zoom.md) | Postmortem of the macOS UI lockup, and how to re-enable the pan/zoom debug toolkit if it returns. |

`plans/` holds dated design docs written before or during a feature's
implementation; they are snapshots of intent, not maintained afterwards, so
trust the pages above over a plan. `branding/` is the brand pack - logos,
social image, and design tokens.
