# Stalagmight

**A desktop application for processing, visualizing, and planning underwater cave survey data.**

[![License: AGPL-3.0-or-later](https://img.shields.io/badge/License-AGPL--3.0--or--later-blue.svg)](LICENSE)

**Website: [stalagmight.com](https://stalagmight.com)** ·
**[Releases](https://github.com/Stalagmight/Stalagmight/releases)**

![A surveyed underwater cave in Stalagmight: depth-coloured survey lines on the map, the section list, the data table, and project statistics](docs/screenshot.png)

Stalagmight is an intuitive, modern, open underwater cave survey application.
It takes other surveys, hand surveys, or `.dmp` files from an mNemo device
and guides the user into creating a clean, editable, georeferenced
map you can reason about and plan future exploration with. It
imports the formats divers already use (Ariane, Compass), adjusts
survey loops, renders the cave over satellite imagery, and supports
the editing and planning workflows that come after the dive.

Stalagmight is an open source project because we believe in transparency,
accessibility, and freedom for users to manipulate their tools. Stalagmight
does not store your data on our servers - everything stays local to your
machine - and the only tracking is opt-in telemetry, meant to help developers
fix issues.

Stalagmight use is meant for certified cave survey divers who
understand that cave surveying is risky, all software has bugs, and
this is meant as an optional informational addition to, but not a
replacement for, cave survey training.

> **Status:** pre-1.0, under active development. The data model, import/export
> pipelines, map editing, loop-closure adjustment, and the dive/gas planning
> mode are all implemented and in daily use. What is still in flight lives in
> the [issue tracker](https://github.com/Stalagmight/Stalagmight/issues);
> [the roadmap](docs/roadmap.md) records how v1.0 was scoped.

---

## What it does

- **Import** - mNemo `.dmp` (raw survey, versions 2-5), Ariane `.tml`/`.tmlu`,
  Compass `.mak`/`.dat` projects, Walls (experimental), and GPX (waypoints,
  tracks, routes).
- **Export** - map images (`.svg`, `.png`, `.pdf`) plus Ariane `.tml`/`.tmlu`,
  `.csv`, `.json`
- **Map** - survey lines and stations rendered with [MapLibre](https://maplibre.org/)
  over satellite imagery, with snapping, measurement, a distance-gradient overlay,
  and hover/selection highlighting.
- **Edit** - move stations / segments / shots, merge stations, adjust sections,
  edit comments and LRUD, bulk-delete shots - all with hybrid undo/redo.
- **Loop closure** - least-squares loop adjustment to distribute survey error.
- **Plan** - a dive/gas planning mode: gas-weighted reachability over the survey
  graph, with turnaround and stage-drop points along the chosen route.
- **Safe by default** - atomic saves, auto-save, dirty-state guards, and a
  recent-projects list.

## Tech stack

[Tauri 2](https://tauri.app/) desktop shell with a **Rust** backend
(survey model, parsers, geometry, persistence in SQLite) and a **React 18 +
TypeScript** frontend (MapLibre map, Zustand state, i18n). IPC types are
generated from the Rust types via `ts-rs`.

## Getting started

**Prerequisites:** [Rust](https://rustup.rs/) (stable), Node 24.x, and the
[Tauri system dependencies](https://tauri.app/start/prerequisites/) for your
platform.

The toolchain is pinned so every machine builds the same versions - pnpm
`11.18.0` (via `packageManager` in `package.json`) and Node `24.x` (via
`.nvmrc`). On a fresh clone:

```bash
corepack enable      # makes `pnpm` honor the pinned version
pnpm install
./scripts/dev.sh     # launches the dev app (handles process cleanup)
```

Production build:

```bash
pnpm tauri build
```

> Telemetry is **off** in normal builds. The `telemetry` Cargo feature (used in
> CI and official releases) additionally requires a `TELEMETRY_ENDPOINT`; you do
> **not** need it to build, run, or contribute. See
> [docs/telemetry.md](docs/telemetry.md).

## Documentation

The `docs/` tree is extensive. Good entry points:

| Document | Contents |
|----------|----------|
| [SPEC.md](docs/SPEC.md) | Full specification - data model, UI, workflows, file formats, validation |
| [architecture.md](docs/architecture.md) | State flow, command pattern, key files, Rust patterns |
| [project-structure.md](docs/project-structure.md) | Directory tree and tech stack |
| [TESTING.md](docs/TESTING.md) | Test tiers, and which one your test belongs in |
| [testing-recipes.md](docs/testing-recipes.md) | Commands and patterns (Rust, Playwright, vitest, agent harness) |
| [roadmap.md](docs/roadmap.md) | Historical record of the v1.0 phases and how they were scoped |

Everything else - subsystems, import formats, testing, platform notes - is
indexed with a suggested reading order in [docs/README.md](docs/README.md).

## Contributing

Contributions are welcome - with one important caveat about domain scope (cave
surveying is a specialized field) and some guidance on AI-assisted work. Please
read **[CONTRIBUTING.md](CONTRIBUTING.md)** before opening an issue or PR.

To report a security issue, see **[SECURITY.md](SECURITY.md)**.

## License

Stalagmight is licensed under the **GNU Affero General Public License v3.0 or
later** (AGPL-3.0-or-later). See [LICENSE](LICENSE).

It bundles a locally-patched copy of
[`compass_data`](https://github.com/zheylmun/compass_data) by Zach Heylmun
(MIT OR Apache-2.0) under `src-tauri/vendor/compass_data/`; changes are recorded
in that directory's `MODIFICATIONS.md`. The map's colour palettes include
tables derived from matplotlib's colormaps; their licences are in
`third_party/palettes/`. Third-party dependency licenses are
permissive (MIT / Apache-2.0 / BSD / ISC / MPL-2.0) and compatible with the
AGPL; every shipped package's license text is reproduced in
[THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md), which is bundled with the
app and regenerated by `pnpm gen:third-party`.
