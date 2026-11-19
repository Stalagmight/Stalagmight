# Stalagmight - Cave Survey Processing Application

The living reference for the application: its data model, its user interface, and
the workflows they serve. Read this to find out what the app is and how it
behaves.

Where the code is the source of truth - the command and undo enums, the SQLite
schema, config keys, dependency versions - this document points at the file
rather than restating it, because a copy here rots silently.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Requirements Summary](#2-requirements-summary)
3. [Glossary](#3-glossary)
4. [Data Model](#4-data-model)
5. [Architecture](#5-architecture)
6. [User Interface](#6-user-interface)
7. [Workflows](#7-workflows)
8. [Command System](#8-command-system)
9. [Configuration](#9-configuration)
10. [Settings Dialog](#10-settings-dialog)
11. [Accessibility](#11-accessibility)
12. [Internationalization](#12-internationalization)
13. [Performance](#13-performance)
14. [Error Handling](#14-error-handling)
15. [Testing](#15-testing)
16. [Logging](#16-logging)
17. [File Formats](#17-file-formats)
18. [Technical Stack](#18-technical-stack)
19. [Beyond the Current Scope](#19-beyond-the-current-scope)
- [Appendix A: Keyboard Shortcuts](#appendix-a-keyboard-shortcuts)
- [Appendix B: Validation Rules](#appendix-b-validation-rules)
- [Appendix C: Color Themes](#appendix-c-color-themes)

---

## SpeleoDB project workspace

All project editors expose a full logo + SpeleoDB toolbar button in every mode.
Survey opens on Sections and offers Sections | SpeleoDB tabs in one left dock.
Plan and Export reveal SpeleoDB only on request unless it was already selected;
returning to Survey restores its tab. Tab navigation preserves map, selection,
section expansion, scroll and unfinished drafts, without changing lock ownership.
A local project's SpeleoDB tab contains a centered explanation of version history
and team collaboration and one Convert to a SpeleoDB project action. It opens the
creation form; confirmation saves and uploads, then downloads and opens the fresh
server version. The project becomes managed. Failed upload/download recovery keeps
that managed identity and pending work without creating duplicate server projects.
Signed-out users authenticate in Settings and return to their creation draft.
Publishing blocks the app with a centered progress indicator for up to 120 seconds.
After that limit the indicator expires, a timeout explanation appears, and the
dialog can be closed while the original request finishes; duplicate submission
remains disabled.

The dock retains resize/pin/hover behavior, uses a 320px SpeleoDB minimum within the
400px maximum, and floats at constrained widths. Programmatic opening focuses the
selected tab and exits fullscreen table mode while preserving table state.

The persistent project header and everyday Save Changes, Switch to Read Only Mode/Resume editing, Reload Project
and web controls sit above an expandable Revision History section. Close Project sits below Reload Project and returns
to the project listing after unsaved-work confirmation and lock release. Account and
connection forms live exclusively in application Settings. Local settings retains
validation and elevation editing; linked project names remain server-managed. Home retains its entry points. The
project-editor SpeleoDB menus, remote settings dialog and undo-menu revision
controls are replaced by workspace navigation. Critical access, lock-loss,
upload-failure and unuploaded-work indicators remain visible outside the tab.

Remote opening automatically attempts lock acquisition and reconciliation. No
explicit editing-without-a-lock feature exists. Offline opening stays read-only;
transient disconnection after acquisition permits local editing. Confirmed lock
loss, permission loss or sign-out closes the editing gate. Backend direct mutation
and upload calls enforce the same ownership requirements.

Explicit Save, its shortcut and Save Changes share a dispatcher: local Save for
local projects, upload confirmation for linked ones, and a blocked explanation
without the lock. Upload needs a description, local save and fresh server ownership
verification; ordinary upload keeps the lock. Clean projects do not upload empty
revisions. Autosave and autosave retry remain local-only. Save and Close continues
only after confirmed upload and release, with no newer unuploaded changes. Other
exits automatically release without upload, protecting unsaved survey and form
drafts with Discard changes / Cancel. A release that fails or is uncertain never
leaves silently: Close, project switches and Quit stay put and ask "Couldn't hand
back the lock" (Retry / Close and keep the lock / Cancel). Dirtiness is independent of validity.
Switch to read-only refuses unsaved or unuploaded changes, with Save Changes guidance.
Reload requires explicit Discard changes / Cancel for pending work, preserves mode,
and never acquires a lock. Historical checkouts return to the live server version.
Start Editing is the only acquisition/reconciliation flow; fresh revision topology
is checked after acquisition. Status refresh cannot enable editing. Failed downloads,
validation, stale sessions and newer edits preserve the current project.
See [SpeleoDB](SpeleoDB.md) and [Saving](saving.md) for failure and recovery rules.

## 1. Overview

### 1.1 Purpose

Stalagmight is a desktop application for processing underwater cave survey data. It provides tools to import, visualize, edit, and export cave survey data collected by cave divers using MNemo devices or manual methods.

### 1.2 Target Users

- Experienced cave divers with survey training
- Non-technical users who need intuitive workflows
- Power users who want advanced configuration options

### 1.3 Design Principles

1. **Offline-first**: Must work without internet connection. Privacy-conscious users are suspicious of network access.
2. **Performance**: Smooth rendering of large surveys (500km+ passages, 100,000+ line segments).
3. **Ease of use**: Self-guiding workflows, no tutorial needed for basic operations.
4. **AI-friendly**: Documentation and code structure optimized for AI-assisted development and support.
5. **Config-driven**: All defaults, thresholds, and display options in editable config file.

### 1.4 Scope

**Shipped:**
- Import: DMP (MNemo devices), Walls (experimental), Compass, Ariane `.tml`/`.tmlu`, GPX - see `src-tauri/crates/parsers/src/parsers/`
- Reading an MNemo v2 directly over USB into the DMP import (desktop only, read-only on the device) - see [dmp-import.md](dmp-import.md#reading-the-device-over-usb-725)
- Manual data entry via a spreadsheet interface
- 2D plan-view map with satellite imagery
- Line management (create, edit, delete, merge)
- Jumps and reference shots for virtual survey connections
- Loop closure detection, plus the non-destructive Closures Computed View ([closures.md](closures.md))
- Multi-project workspace (Global View)
- Export: PNG, SVG, PDF (map images), CSV, JSON, Ariane `.tml` (data)
- Dive and gas planning as the Plan workspace mode, persisted to a `.plnx` sidecar
- SpeleoDB integration - remote project hand-off; see [the native-file and lock contract](SpeleoDB.md) for Save + Push, local autosave, automatic-release exits and access verification
- SQLite project storage, undo/redo with persistence
- An Android tablet build, sideload-only and not in CI, with partial touch
  support ([touch-support.md](touch-support.md))

**Not built:**
- 3D visualization
- Web version
- MNemo device communication over WiFi, and writing to the device
- LRUD passage-wall rendering on the map

In-flight work lives in the GitHub issue tracker rather than a list here
(examples in section 19).

---

## 2. Requirements Summary

### 2.1 Target Audience
- **Beta testers only** - Not general public release
- Experienced cave divers with survey training
- Technical users comfortable with early software

### 2.2 Scale Requirements
| Metric | Target |
|--------|--------|
| Maximum shots per project | 10,000 |
| Maximum shots per line | < 100 (typical) |
| Maximum lines | 200 (50-200 optimized) |
| Startup time | < 3 seconds |
| Map rendering | 30 FPS minimum |

### 2.3 Platform Support
Desktop platforms have equal priority:
- Linux (primary development)
- Windows
- macOS

There is also an Android tablet build, but it is sideload-only and no CI job
builds or exercises it. Touch input runs alongside the unchanged mouse/keyboard
controls, and is partial and untested - see
[touch-support.md](touch-support.md) before relying on a gesture.

### 2.4 Accessibility
- **WCAG 2.1 AA compliance**
- Focus management with trap in modals
- Screen reader announcements via ARIA live regions
- Keyboard navigation for all features
- High-contrast theme infrastructure (full theme v2)

### 2.5 Internationalization
- **Infrastructure only** for v1.0
- react-i18next setup with English translations
- Locale-aware number/date formatting
- No additional languages in v1.0

### 2.6 Offline Support
- **Required**: Tile caching for satellite imagery
- Auto-cache viewed tiles
- Configurable cache size (default 500 MB)
- Graceful offline degradation

### 2.7 GPS Coordinate Formats
All formats required:
- Decimal Degrees (DD)
- Degrees Minutes Seconds (DMS)
- Degrees Decimal Minutes (DDM)
- UTM
- MGRS
- Plus Codes
- What3words

MGRS and What3words are recognized and validated but not converted to
coordinates: MGRS needs a dedicated conversion library and What3words an API,
so both return an explanatory error pointing at the offline formats
(`src/utils/gps.ts`).

### 2.8 Testing Requirements
- **80% code coverage** target
- Rust unit and integration tests for the backend
- vitest for TypeScript unit and component tests
- Playwright E2E against mocked IPC: a fast `@smoke` tier on pushes that can affect it (the workflow's `paths-ignore` skips docs-only, Rust-only, and legacy-suite-only changes), the full suite nightly
- The agent harness (`e2e/agent/`) drives the real Tauri app: journeys, stress scenarios, and fuzz tiers, on demand and per release candidate

See [TESTING.md](TESTING.md) for the tiers and which one a test belongs in, and
[testing-recipes.md](testing-recipes.md) for the commands.

### 2.9 Settings
- Global settings in the Settings dialog, plus a per-project subset (project name,
  validation thresholds) stored in the `.stalx`
- Changes are staged and applied on Save, not live (section 10.4)
- Reset to defaults per category

### 2.10 Updates & Telemetry
- In-app updates from manifests on GitHub, on one of two tracks chosen in
  Settings: **Stable** (final releases only, the default) or **Beta** (release
  candidates, then the final they lead to). A build from a release candidate starts
  on Beta. The retired Weekly track's setting is read as Beta. See
  [releasing.md](releasing.md#updater-manifests)
  - **Trust:** the manifest is fetched over https and is not signed itself. The
    bundle it points at is minisign-verified against the public key built into the
    running app before anything installs it.
  - **When it checks:** about 10 seconds after launch, then every 4 hours while the
    app is open (a 5-minute timer compares wall-clock time, so a machine waking from
    sleep catches up within minutes), when About's "Check for updates" is pressed,
    and whenever the update track is switched. The 4-hour check is skipped while an
    update is downloading or downloaded, so a download in hand is not re-checked
    until the app restarts.
  - **"Update automatically"** (Settings > General > Updates, on by default): an
    update a check finds downloads straight away. From a launch, 4-hour or
    track-switch check there is no banner until it is ready (a failed download then
    shows the banner's Download); from About's check the banner shows progress.
    Turned off, no automatic checks run, and a download a check started that has not
    installed yet is cancelled (#645); one the user started with Download is kept.
    About's check and a track switch still check, and the banner then offers
    Download, which waits for a click.
  - **Installing:** on macOS and Linux a downloaded update installs when the app
    quits, after the usual quit guards, so the next launch runs it. The banner says
    "Stalagmight X will be installed when you quit" and offers "Restart now". Nothing
    restarts the app on its own. Two cases show "Restart to update" instead:
    Windows (the updater's NSIS installer always relaunches the app, so installing
    on quit would reopen the window the user just closed) and a Mac whose app folder
    the user cannot write to (the install would raise an administrator prompt after
    the window had closed).
  - **Restarting** to update goes through the same quit guards as any other quit,
    with one difference: a SpeleoDB project holding the write lock over work
    SpeleoDB has not seen is not asked the exit question. It keeps the lock, saves
    unsaved edits, and restarts. A restart while a SpeleoDB transfer is running
    waits for it, like a quit: "Keep Stalagmight open" calls the restart off, and
    "Quit now" cuts the transfer. A transfer that starts after the wait has passed
    makes the backend refuse the restart with a toast, keeping the download.
  - **"Hide"** on the banner hides it for that version for the rest of the session.
    About's "Check for updates" brings it back, and a newer version shows again.
    Where updates install on quit, a hidden, downloaded update still does.
  - **Packages that cannot update themselves:** the updater replaces an AppImage,
    an NSIS-installed Windows app, or a macOS `.app` (Apple silicon, the only macOS
    build released). A copy installed from a `.deb`, `.rpm` or `.msi` is never
    offered an update: its check fails before reading the manifest, automatic
    checks stay silent, and About says "Could not check for updates" without
    giving the reason.
  - **Development builds** do not update themselves. A debug build checks,
    downloads and installs only when `STALAGMIGHT_UPDATE_URL` names a manifest
    (#644; see [releasing.md](releasing.md#local-end-to-end-updater-test)).
- Opt-in telemetry only
- No external network calls except tile servers, SpeleoDB sync, the update check, and opt-in telemetry

### 2.11 Recovery
- Auto-save only (no crash recovery dialog)
- Debounced auto-save on a frontend timer, `general.auto_save_interval_seconds` (default 30) - see [saving.md](saving.md)
- SQLite integrity checks on project open

---

## 3. Glossary

| Term | Definition |
|------|------------|
| **Shot** | A single survey measurement between two stations. Contains azimuth, length, depth, and optionally LRUD. |
| **Station** | A point in the cave where a shot measurement is taken. Tied to cave line at physical markers. |
| **Line** | A group of consecutive shots collected during a single survey session; a continuous run of stations joined by shots. Has metadata (name, colour, direction, quality, azimuth reference); who surveyed it and when is recorded per shot. Called a Section before the rename: the `.stalx` file still stores lines in its `sections` table (see [Persisted names](saving.md#persisted-names-lines-are-sections-on-disk)). |
| **Section** | A named, coloured grouping of lines - a region of the cave such as "Upper level" (#327). Each line belongs to at most one Section; a line in none is ungrouped. Section names are unique in a project. Not to be confused with the old name of a Line: in code a Section is `Section` / `SectionId` / `Project.sections`, serialized as `cave_sections` / `cave_section_id`, stored in the `cave_sections` / `cave_section_lines` tables. |
| **Sections panel** | The left-dock tab that lists the project's lines, grouped under their Sections (ungrouped lines listed separately). It keeps its name. |
| **Virtual Shot** | A shot that is not a permanent surveyed leg. Since #27 it is one of two kinds, and the distinction drives gas and penetration. |
| **Jump** | A virtual shot a diver **swims**: a shortcut between two mainlines. Counts toward gas (at `jump_rmv_multiplier`) and penetration. Drawn faded. |
| **Reference Shot** | A virtual shot that is **not swimmable**: a tape tie or backsight used to connect lines that aren't physically connected yet. Excluded from gas, penetration and route distance unless `gas.include_reference_legs` is on. Always excluded from survey length, since it repeats passage already surveyed (#689). Drawn fainter than a jump. |
| **Zero Datum** | A station with GPS coordinates assigned. The fixed reference point for the coordinate system. |
| **LRUD** | Left, Right, Up, Down - passage dimension measurements at a station. |
| **Loop Closure** | When two survey paths meet at the same physical point, creating a loop. |
| **Closure Error** | The discrepancy between expected and actual positions when a loop closes. Expressed as percentage. |
| **DMP File** | Binary file format from MNemo device containing survey data. |
| **Cave System** | A complete cave survey project. One cave system per project file. |
| **Global View** | Application view showing all cave systems on a map. |
| **Single System View** | Application view for editing a specific cave system. |

---

## 4. Data Model

### 4.1 Entity Hierarchy

```
Project (Cave System)
├── Metadata
│   ├── name: String
│   ├── location: Option<String>
│   ├── created_at: DateTime
│   └── modified_at: DateTime
├── Lines[]   (the `sections` table on disk)
│   ├── id: u32 (internal, immutable, never shown)
│   ├── name: String (required, unique in the project; seeded `Line N`)
│   ├── surveyor: Option<String> (legacy; credit is per shot)
│   ├── date: Option<Date> (legacy; credit is per shot)
│   ├── color: Color
│   ├── direction: SurveyDirection
│   ├── quality: f64
│   ├── azimuth_reference: Option<AzimuthReference>
│   ├── section_id: Option<SectionId> (its Section; serde `cave_section_id`)
│   └── Shots[]
│       ├── id: u32 (internal session handle, never shown)
│       ├── uuid: Option<String> (persistent identity, schema v6+, unique across the project)
│       ├── azimuth_in: f64 (degrees, 0-360)
│       ├── azimuth_out: f64 (degrees, 0-360)
│       ├── length: f64 (meters)
│       ├── depth_in: f64 (meters)
│       ├── depth_out: f64 (meters)
│       ├── pitch_in: Option<f64> (degrees, -90 to 90)
│       ├── pitch_out: Option<f64> (degrees, -90 to 90)
│       ├── left: Option<f64> (meters)
│       ├── right: Option<f64> (meters)
│       ├── up: Option<f64> (meters)
│       ├── down: Option<f64> (meters)
│       ├── comment: Option<String>
│       ├── kind: ShotKind (Normal | Jump | Reference)
│       ├── marker_index: Option<u32>
│       └── temperature: Option<f64> (celsius)
├── Sections[]   (named groupings of lines, panel order; serde `cave_sections`)
│   ├── id: SectionId (u32; `next_section_id` allocates, serde `next_cave_section_id`)
│   ├── name: String (project-unique, never empty)
│   └── color: String (#rgb / #rrggbb)
├── Stations[] (computed from shots)
│   ├── station_ref: StationRef (line_id:station_index, internal handle, never shown)
│   ├── id: StationId (u32, project-unique Station ID, shown)
│   ├── name: String (unique within the line, shown)
│   ├── coordinates: Coordinates (computed or GPS)
│   ├── is_zero_datum: bool
│   ├── gps_coordinates: Option<GpsCoordinates>
│   ├── merged_with: Vec<StationRef>
│   ├── closures_with: Option<Vec<ClosureRef>> (direct + transitive closure relationships)
│   └── comment: Option<String>
│
│   ClosureRef:
│     ├── closure_id: u64 (references LoopClosure.id)
│     ├── station_ref: StationRef (the other station in the closure)
│     ├── via_station: Option<StationRef> (intermediate station for transitive closures)
│     └── is_transitive: bool (false = direct closure, true = transitive)
├── station_names: {StationKey -> String} (every station's name; `station_names` table)
├── station_ids: {StationKey -> StationId} (every station's Station ID; `station_ids` table)
├── next_station_id: StationId (the next id to allocate; `project_settings`)
└── Settings (per-project, stored in the .stalx: project_settings + display_settings tables)
    ├── validation: ValidationConfig
    ├── surface_elevation: Option<f64>
    ├── default_azimuth_reference: Option<AzimuthReference>
    ├── counter_parity_shots: {shot_uuid} (shots whose record orientation runs against their line's parity after a Combine; dive-plan pins only)
    └── display toggles (show_stations, show_shots, grid_spacing, geometry_mode,
        hiddenSections - the Sections hidden on the map, a view setting, ...)
```

#### Sections (groupings of lines, #327)

A Section groups lines for the Sections panel and the map; it holds no survey
data and changes no geometry or statistics. Rules:

- **Membership**: at most one Section per line. A Split Line Here, and the
  tail a delete-and-split creates, stay in the line's Section; Combine Lines
  keeps the kept line's; a deleted line takes its membership with it (undo
  puts it back). A line created through New Line can go straight into a Section
  (`create_line`'s `caveSectionId`).
- **Names**: unique in the project, compared exactly after trimming, never
  empty, at most 200 characters. A name the user types that is taken is refused
  ("There is already a section named ..."); automatic paths (imports, Add / Merge)
  make it unique with ` (2)`, ` (3)`, ...
- **Commands** (each one undo step, through the read-only gate):
  `create_cave_section` (name, colour, optional lines - "New Section" from a
  selection), `update_cave_section` (rename and/or recolour),
  `delete_cave_section` (its lines become ungrouped; undo restores the Section at
  its place in the list with every membership), `move_lines_to_cave_section`
  (into a Section, or out of any), `apply_cave_section_color` (every line of the
  Section takes its colour). History labels name the Section: `Create section
  "Upper level"`, `Move 3 lines to section "Upper level"`, `Delete section
  "Upper level"`.
- **Show/hide** is a view setting, not an edit: the `hiddenSections` display
  setting (a JSON array of Section ids), saved like the other display toggles,
  never undone, and not pushed to SpeleoDB. Hidden lines still count in the
  statistics.
- **Auto-grouping on import**: Ariane - the lines one Ariane section name fanned
  out into (`X`, `X (branch 2)`, `X (trip 2)`) join a Section `X` when there are
  two or more of them; Compass - the lines of a survey that produced two or more
  lines join a Section named after the survey; Walls - every line of a survey
  with a non-empty segment path joins a Section named after the segment's last
  level (the whole path when two segments of the import end in the same level).
  An import into a project that already has a Section of that name joins it.
- **Add .stalx / Merge projects** append the source's Sections with fresh ids and
  deduped names; the copied lines keep their membership.

The code names (`Line`, `LineId`, `Project.lines`, `line_id`,
`Project.line_offsets`) stop at the file boundary: the `.stalx` keeps the
pre-rename SQL names (`sections`, `section_id`, `section_offsets`, ...) and the
JSON it stores (undo history, held gauges, the dive-plan sidecar) keeps
`section_id`-style keys. See
[Persisted names](saving.md#persisted-names-lines-are-sections-on-disk).

### 4.2 Station Identification

The user identifies a station by its **name** and its **Station ID**, a line
by its **name**, and a shot by its two stations. The positional handles the
code addresses them with (`line_id`, the shot id, `StationRef =
line_id:station_index`) are internal session handles and are never shown.

| Thing | Identity the user sees | Unique within | Example |
|-------|------------------------|---------------|---------|
| Line | name | the project | `Main Line` |
| Station | name | its line | `17`, `4a`, `A3` |
| Station | Station ID (integer) | the project | `512` |
| Station, with its line | `line / name` | the project | `Main Line / 17` |
| Shot | `from → to` | its line | `4 → 5`, with its line `Main Line: 4 → 5` |

A station's stored identity is the shot UUID before it
(`StationAnchor::AfterShot`; `LineStart` for station 0). Shot UUIDs are unique
across the whole project: an import, merge or Add that brings an id the project
already holds renames the newcomer's copy to a derived id, and a file saved
before that rule is healed the same way on load, with its undo history
translated alongside (see [ariane-export.md](ariane-export.md#shot-uuids) and
[undo-redo.md](undo-redo.md#a-loaded-history-follows-the-loads-shot-uuid-heal)).
The names (`Project::station_names`) and Station IDs
(`Project::station_ids`) are keyed to that physical station, so they stay put
when shots are inserted, deleted or reversed, and follow their station through
Split Line Here and Combine Lines.

**Normalization.** One entry point, `Project::ensure_station_identities`,
guarantees after every load, import, Add/Merge, mutating command and undo/redo
step that:

- every line has a non-empty name, unique in the project: an unnamed line is
  seeded `Line 1`, `Line 2`, ... (smallest free number, in line order); a
  repeated name gets ` (2)`, ` (3)`, ... on the later line;
- every station has a Station ID: one without (or holding an id an earlier
  station holds) gets the project's `next_station_id`, allocated in line order
  then station order, so a file an older build rewrote without the ids re-mints
  the same ids on every open; ids are never reused within a timeline (an undo
  that restores a snapshot restores the counter with it);
- every station has a name, unique within its line: a missing name is seeded
  from the station's index (`i` -> `"i"`), and a name an earlier station of the
  line holds gets the first free lowercase suffix by the Mid rule below (`5`,
  `5` -> `5`, `5a`; `Sump`, `Sump` -> `Sump`, `Sump-a`).

A station or line that already has a valid name and id keeps them.

**Naming a new station.** An edit that creates a station names it:

- **End** (a shot appended at the end of a line): the previous station's name
  plus one - `5` -> `6`, `A3` -> `A4`, `007` -> `008` - incremented while taken;
  a previous name with no trailing number gives the line's next free integer
  (one above its highest all-digit name, `0` when there is none).
- **Mid** (Insert Shot after/before inside a line, Insert Station on a shot,
  split shot at distance): the previous station's name plus the first free
  lowercase suffix, `a`..`z`, then `aa`..`zz`, `aaa`, ... - after `2` -> `2a`,
  again -> `2b` (giving `2 - 2b - 2a - 3`); Insert Station on `4 → 5` -> `4a`.
  A name with a digit takes the suffix directly (`2a` -> `2aa`, `A3` ->
  `A3a`); a name with no ASCII digit takes it after a hyphen, so a word does
  not run into its letter: after `Junction` -> `Junction-a`, then
  `Junction-b`.
- **Prepend** (Add Shot Before at the start): the next free number of the old
  first station's prefix family - `0 1 .. 6` -> `7`, `A0 .. A3` -> `A4`.
- **Imports**: a line whose file names some stations (or rows the user added in
  an import wizard) gets the rest by the same rules - a run after the last
  named station continues it (End), a run between two named stations takes Mid
  suffixes of the station before it, a run before the first named station takes
  Prepend names going backwards; a line with no name at all is seeded.
- **Split Line Here** at a station: both halves carry the station; the copy that
  starts the new line keeps the name (names are unique per line, not per
  project) and gets a **new** Station ID.

**Combine Lines.** The joined line must keep its names unique, and it renames
stations only when it has to. The two lines are prefixed when

- a name of the other line (its join station aside - the joined station keeps
  the kept line's name and Station ID, the other join station's are retired)
  is also a name of the kept line: a real **collision**; or
- the kept line has two or more **letter families** (it is the product of an
  earlier lettered combine; a letter family is the names starting with one
  ASCII capital directly followed by a digit, `A0`, `A3`, `B12a`) and the other
  line has none, so the other line continues the scheme.

When they are prefixed:

- a line that already has letter families keeps its names, except that the
  other line's families whose letter the kept line uses are re-lettered to the
  next free capital (the kept line goes first);
- a line with no letter family gets the first capital neither line uses as a
  family letter on its **number-led** names only (the first character an
  ASCII digit: `0`, `12`, `1a`) - the kept line first (`3` -> `A3`), then the
  other (`0` -> `B0`). A name that does not start with a digit (a custom
  `Junction` or `Sump`) keeps its name; a line with no number-led name spends
  no letter;
- a name of the other line (its join station aside) that still equals one of
  the kept line's after that - a custom name both lines hold, or a plain name
  of a lettered line - is made unique on the other line's copy by the Mid
  rule (`Sump` -> `Sump-a`, `7` -> `7a`); the kept line's copy keeps it.

So kept `0 1 2 3` + other `0 1 2` joined at kept `3` = other `0` gives
`A0 A1 A2 A3 B1 B2`, and a later combine of that line with `0 1` gives
`A0 .. A3 B1 B2 C1`. Kept `0 1 Junction Sump 4 .. 8` + other `0 1 Sump 3`
joined at kept `8` = other `0` gives `A0 A1 Junction Sump A4 .. A8 B1 Sump-a
B3`. Lines whose names do not collide keep every name: `0 1 2 3` + `3 4 5`
joined at `3` gives `0 1 2 3 4 5`; kept `0 .. 200` + other `A1 .. A9` keeps
both; a line imported as `E0 .. E10`, split at `E5` and combined back is
`E0 .. E10` again. A final pass letter-suffixes any repeat the prefixing could
not prevent (all 26 letters in use). The Combine Lines dialog says, for the
line it shows as kept, which stations will be renamed: each line's lettering
by its first and last lettered station (the other line's join station, which
becomes the joined station, left out), with how many are lettered and how
many keep their names when not all are, then the names made unique by the Mid
rule one by one (the first three, then a count): "Station names collide, so
stations will be renamed: Main 0..8 -> A0..A8 (7 renamed, 2 keep their
names), Side 1..3 -> B1..B3 (2 renamed), Side Sump -> Sump-a". It names the
joined station by its name after the combine.

**User edits.** Renaming a station (`SetStationName`, undoable) trims the
name and refuses an empty one or one another station of the line holds ("Line
X already has a station named Y"). Renaming a station to the name it already
has (after trimming) succeeds as a no-op: no undo entry, no unsaved-changes
flag. Renaming a line (Edit Line) or creating one from the UI refuses an empty
name or one another line holds ("A line named X already exists"); so do the
names a user gives the halves of Split Line Here or a delete-and-split and the
name of a combined line (the line or lines being replaced are no collision).
Automatic paths (imports, default names) de-duplicate instead, renaming the
line the operation made, never an unrelated one. The backend and the dialogs,
the data table and the Sections panel (inline, before sending anything) also
refuse a station, line or Section name holding a control character or an
invisible formatting character (the left-to-right / right-to-left marks, the
bidi embeddings, overrides and isolates, the zero-width space and joiners, the
BOM; `name_characters_refusal` in the backend, `nameValidation.ts` in the
UI), which can make a name read as something it is not; visible separators
such as ` / ` are fine. Names are stored trimmed, and compared trimmed:
`Main ` is `Main`, and a file holding both is normalized to `Main` and
`Main (2)`. A name that only differs from the current one by surrounding
spaces is no rename: nothing is sent.

**Labels.** The "Station IDs" map and image-export label draws the Station ID;
the "Station names" label draws the name (every station has one). The two are
exclusive. Merged stations are one point, so a merge group (the stations
joined by station merges; the image export also counts the two ends of an
aligned loop closure as one point) draws exactly ONE label, for its
representative: a station that is not its line's first station if there is
one, then one that is not its line's last, ties broken by the line's order in
the project and then by station index. A branch merged onto the middle of the
main line therefore shows the main line's name there (`Junction`), never the
branch's `0`. The label keeps the first-in-line rule (it always shows when any
member starts its line) and hides with the shots only when every member's
marker would. Stations that are not merged each draw their own label.

The undo history names lines, stations and shots by name (`Update shot Main
Line: 4 → 5`, `Merge stations Main Line / 0 and Side / 1`, `Delete line "Main
Line"`); a name that can no longer be resolved falls back to the wording
without it.

A DMP import records each station's MNemo station number as its name (see
[dmp-import.md](dmp-import.md#station-names)); MNemo restarts at 0 in every
section, which per-line uniqueness allows.

### 4.3 Coordinate System

- **Internal coordinates**: X (East), Y (North), Z (Depth) in meters
- **Origin**: The frame datum's station, or (0,0,0) if no GPS assigned. The frame
  datum is the first entry of `zero_datums` that still **resolves**
  (`Project::frame_datum`; a dangling datum is dropped from the wire and anchors
  nothing), so it is a **position in that list** rather than a property of any one
  datum. Anything that promotes a different datum
  to first (removing the primary one, deleting its station or its line) - or
  ADDS the first datum, which establishes the frame - would re-define what every
  stored coordinate means. It does not: the whole survey is translated so that
  datum's station lands on local (0,0), free-floating components included, which
  preserves every station's real-world position (and, for the add, every relative
  position) and changes only the local coordinates
  (`Project::renormalize_frame`, issue #469). When the promoted datum was a GPS
  fix (#741), its piece is first moved onto its GPS against the old frame, so
  every piece stays on its own GPS through the re-frame
- **GPS coordinates**: WGS84 (latitude, longitude, depth reference)
- **Azimuth**: degrees (0-360), stored **as measured**. The stored number is
  never rotated: each line declares what its readings refer to
  (`Line::azimuth_reference` - magnetic / true / grid, the correction in
  degrees, and where that correction came from) and the correction is added
  when geometry is computed, by the one accessor
  (`Shot::average_azimuth_true`). A line with no reference resolves to the
  project default, and a project with no default means "nothing recorded",
  correction 0 (#438).
  - **Where the correction comes from, per importer.** Compass records the
    survey's `DECLINATION` alone: a UTM grid convergence in the file is
    reported as a warning and deliberately **not** applied, because the app's
    world frame is true north while Compass's display frame is the grid. Walls
    does the same: it resolves `Decl=` or a derived declination and warns about
    a `.REF` or `#Units Grid=` convergence without applying it. The one Walls
    difference is per **vector**, not per survey - a RECT vector's bearing is
    already true north in the file's Cartesian frame, so it takes no
    declination at all and its correction is 0. An Ariane `.tml` reads the
    file's `useMagneticAzimuth` flag; a georeferenced file carrying a survey
    date gets its declination from the bundled IGRF model, evaluated at the
    **earliest dated line's** date rather than today, so the same file
    always imports to the same geometry.
  - **Exporting to a format with only one north** (`AzimuthExport::decide`).
    Every bearing is rewritten to true north when the lines do not all
    agree, or when they agree on grid north - but only if every line's
    correction is on record. If any one is `Unknown`, nothing is rotated: the
    readings go out exactly as measured and the file declares magnetic. That
    declaration is reported as misdescribing whichever lines did carry a
    correction, except where the declared lines were uniformly magnetic, in
    which case it is simply true and nothing is reported. There is no refusal
    path.
- **Depth**: Positive values = below surface

#### GPS-to-Local Coordinate Transformation

When a zero datum is set with GPS coordinates, a coordinate transformation converts GPS (WGS84) to local ENU (East-North-Up) meters:

1. **Geodetic to ECEF**: Convert (lat, lon) to Earth-Centered Earth-Fixed coordinates
   - Uses WGS84 ellipsoid (semi-major axis a = 6,378,137m, eccentricity² = 0.00669437999014)
   - Altitude is always 0 (depth_reference is meters below surface, not ellipsoid height)

2. **ECEF to ENU**: Convert ECEF difference to local tangent plane coordinates
   - Origin is the zero datum's GPS position
   - East (+X) points east, North (+Y) points north
   - Up component is discarded (cave surveys use depth measurements)

3. **Result**: (east, north) offset in meters
   - These become the local X, Y coordinates for positioning lines
   - All other stations are computed relative to this origin

The transformation is implemented in `src-tauri/crates/models/src/geo.rs` with:
- `geodetic_to_ecef(lat, lon, alt)` - WGS84 to ECEF
- `gps_to_local(gps, origin)` - Main entry point returning (east, north) meters

### 4.4 Computed Values

These are derived from shot data, not stored:

- Station coordinates (from azimuth, length, depth chain)
- Line statistics (total length, depth range, shot count). Total length
  sums permanent and jump shots only: a reference shot repeats passage already
  surveyed, so counting it would double-count (#689). Shot count includes every
  shot.
- Loop closure errors
- Penetration distance (maximum line distance from the nearest entrance zero datum; a GPS fix
  mid-passage is not an entrance unless ticked as one, #741)

---

## 5. Architecture

### 5.1 High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    Tauri Application                            │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │              React + TypeScript (Strict Mode)            │   │
│  │                                                          │   │
│  │  ┌──────────────┐  ┌────────────────┐  ┌─────────────┐ │   │
│  │  │ UI Components │  │  Map Component  │  │   Dialogs   │ │   │
│  │  │ (Panels,      │  │  (MapLibre GL + │  │  (Import,   │ │   │
│  │  │  Toolbar)     │  │   WebGL layer)  │  │   Export)   │ │   │
│  │  └──────────────┘  └────────────────┘  └─────────────┘ │   │
│  │           │                │                  │         │   │
│  │           └────────────────┼──────────────────┘         │   │
│  │                            ▼                            │   │
│  │  ┌──────────────────────────────────────────────────┐  │   │
│  │  │           Zustand Store (State Cache)             │  │   │
│  │  │  • Caches backend state for UI                   │  │   │
│  │  │  • Manages transient UI state (drag, hover)      │  │   │
│  │  └──────────────────────────────────────────────────┘  │   │
│  │                            │                            │   │
│  │                            ▼                            │   │
│  │  ┌──────────────────────────────────────────────────┐  │   │
│  │  │         Command Dispatcher (TypeScript)           │  │   │
│  │  │  • Converts UI actions to typed Commands         │  │   │
│  │  │  • Batches transient operations                  │  │   │
│  │  │  • Handles optimistic updates                    │  │   │
│  │  └──────────────────────────────────────────────────┘  │   │
│  │                                                          │   │
│  └──────────────────────────────────────────────────────────┘   │
│                              │                                  │
│                              │ Tauri IPC (invoke)               │
│                              ▼                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                    Rust Backend                           │   │
│  │                                                           │   │
│  │  ┌─────────────────────────────────────────────────────┐ │   │
│  │  │              Command Executor                        │ │   │
│  │  │  1. Validate command                                │ │   │
│  │  │  2. Execute against state                           │ │   │
│  │  │  3. Generate undo data                              │ │   │
│  │  │  4. Push to undo stack                              │ │   │
│  │  │  5. Log command                                     │ │   │
│  │  │  6. Invalidate caches                               │ │   │
│  │  │  7. Return result                                   │ │   │
│  │  └─────────────────────────────────────────────────────┘ │   │
│  │                            │                              │   │
│  │  ┌─────────────────────────┼─────────────────────────┐   │   │
│  │  │                         ▼                          │   │   │
│  │  │  ┌─────────────┐  ┌─────────────┐  ┌───────────┐ │   │   │
│  │  │  │    State    │  │  Undo/Redo  │  │   Logger  │ │   │   │
│  │  │  │   Manager   │  │    Stack    │  │ (tracing) │ │   │   │
│  │  │  └─────────────┘  └─────────────┘  └───────────┘ │   │   │
│  │  │         │                                         │   │   │
│  │  │         ▼                                         │   │   │
│  │  │  ┌─────────────────────────────────────────────┐ │   │   │
│  │  │  │              SQLite Database                 │ │   │   │
│  │  │  └─────────────────────────────────────────────┘ │   │   │
│  │  └───────────────────────────────────────────────────┘   │   │
│  │                                                           │   │
│  │  ┌─────────────────────────────────────────────────────┐ │   │
│  │  │              File Parsers                            │ │   │
│  │  │  • DMP parser (rewritten from pytool_mnemo_lib)     │ │   │
│  │  │  • Project file I/O                                 │ │   │
│  │  │  • Export generators (PNG, SVG, PDF, CSV, JSON)     │ │   │
│  │  └─────────────────────────────────────────────────────┘ │   │
│  │                                                           │   │
│  └───────────────────────────────────────────────────────────┘   │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

Survey geometry is drawn by a custom WebGL layer inside MapLibre
(`src/components/Map/surveyLayer/customRender/`), the sole survey renderer since
#297 - there is no stock-layer fallback.

### 5.2 State Management

**Single Source of Truth**: Rust backend holds authoritative state.

**Zustand Store** (TypeScript):
- Caches backend state for efficient UI rendering
- Manages transient state (drag previews, hover states, pan/zoom)
- Invalidated and refreshed after each command execution

**State Flow**:
```
User Action → Command Dispatcher → Tauri IPC → Rust Executor
     ↑                                              │
     │                                              ▼
     └──────────── Zustand Update ←──────── Command Result
```

### 5.3 Transient vs Persistent Operations

| Type | Examples | Logging | Undo | Storage |
|------|----------|---------|------|---------|
| **Transient** | Pan, zoom, hover, drag preview | No | No | Zustand only |
| **Persistent** | Create/edit/delete shots, merge stations | Yes | Yes | SQLite |

---

## 6. User Interface

### 6.1 Workspace Modes

An open project is worked in one of three top-level workspace modes, chosen from
the `ModeCarousel` in the middle of the toolbar (`uiStore.appMode`): **Survey**,
**Plan**, **Export**. Survey is the editable workspace. Plan and Export force the
read-only Closures Computed View (on a project with no eligible closures the
geometry falls back to the Raw Data View; the modes are read-only regardless - gating keys
off `appMode`, not the geometry), so the Sections tab and every
geometry-mutating tool are Survey-only. Linked projects retain the SpeleoDB left
workspace on request in every mode; selection, inspection and measurement stay
available in all three.

Plan is dive and gas planning: the gas reach field over the survey, route and
turnaround points, and the plan tables, persisted to a `.plnx` sidecar beside the
project file. Export is the WYSIWYG map export: the right dock hosts the export
controls and the map area previews what will be written. Both swap the dock
contents in `SingleSystemView`; the map itself is the same map.

### 6.2 Application Views

#### 6.2.1 Global View

Shows all cave systems the user has loaded, with survey geometry rendering for GPS-positioned projects.

```
┌─────────────────────────────────────────────────────────────────┐
│  [Import] [New Project] [Settings]                    Stalagmight │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  [Fit All]                                    [Navigation]      │
│                                                  [+][-]         │
│                     Map showing all cave systems                │
│                     Survey geometry: shots and stations         │
│                     (positioned by GPS coordinates)             │
│                                               [View Toggle]     │
│                     Click system → metadata in side panel       │
│                     Double-click → open in Single System View   │
│                                                                 │
│                     Multi-select: Ctrl+click                    │
│                     Right-click → context menu                  │
│                                                                 │
│  [Search: ____________]                  N systems on map       │
├─────────────────────────────────────────────────────────────────┤
│  Systems: 5 | Total Length: 127.4 km | Status: Ready           │
└─────────────────────────────────────────────────────────────────┘
```

**Survey Rendering:**
- GPS-positioned projects render actual survey geometry (shots and stations)
- Each system has a distinct auto-assigned color (stable across sessions)
- System-level selection with hover highlighting
- Lazy loading: markers with spinners show while geometry loads; markers hidden once geometry loads successfully
- Error states: red marker remains visible with popup on click
- In survey mode, only loading-spinner and error markers are visible; loaded-geometry markers are hidden

**View Mode Toggle:**
- Toggle button in top-right corner (below navigation controls)
- Two modes: `survey` (default) shows geometry with conditional marker visibility, `markers` shows all markers
- Survey mode: markers visible during loading (with spinner) and on error (red); hidden once geometry loads
- Markers mode: all GPS-positioned system markers visible regardless of geometry state
- Mode persisted in localStorage (`globalViewMode` key)
- Switching to markers cancels pending geometry loads
- Toggling between modes changes marker visibility instantly (no fade transition)

**Map Position Persistence:**
- Map position and zoom persisted to localStorage on every move
- Restored on component mount
- "Fit All" button temporarily overrides but does not persist
- Validation prevents corrupted storage from crashing MapLibre

**Soft Bounds:**
- Systems constrained with rubber-band panning effect
- Soft bounds (20% padding): animate back when user releases pan outside
- Hard bounds (50% padding): cannot pan beyond this limit

**Search Filter:**
- Text input in the Recent list filters the list by name, location or file path
- The map is not filtered by search (it always shows every project)

**FPS Monitoring:**
- Auto-throttle when FPS drops below threshold (default 30)
- Increases GeoJSON simplification tolerance when throttled
- "(reduced detail)" indicator shown when throttled
- Hysteresis: recovers at 45 FPS to prevent oscillation

#### 6.2.2 Single System View

Primary view for survey work.

```
┌─────────────────────────────────────────────────────────────────┐
│  [←Back] [Save] [Import] [Export] [Undo] [Redo]  │ Tools │     │
│                                                   │[S][P][Z]... │
├────────────┬────────────────────────────┬───────────────────────┤
│            │                            │                       │
│  Sections  │                            │    Detail Panel       │
│   Panel    │        Map Canvas          │    (collapsible)      │
│            │                            │                       │
│ [+] Main   │   ★ ──●──●──●──●          │  Station: 0:15        │
│ [ ] Side A │        \                   │  Depth: 12.3m         │
│ [ ] Side B │         ●──●──●            │  Line: Main Line      │
│            │                            │                       │
│ (resize)   │                            │  [Edit] [Delete]      │
│ (collapse) │                            │                       │
├────────────┴────────────────────────────┴───────────────────────┤
│  Data Table (collapsible)                                       │
│  Line: [Main Line ▼]                                            │
│  ┌────┬────────┬────────┬───────┬───┬───┬───┬───┬─────────────┐│
│  │ ID │ Length │ Azimuth│ Depth │ L │ R │ U │ D │ Comment     ││
│  ├────┼────────┼────────┼───────┼───┼───┼───┼───┼─────────────┤│
│  │ 1  │ 3.2    │ 45.0   │ 5.1   │...│...│...│...│             ││
│  │ 2  │ 4.1    │ 52.3   │ 6.2   │...│...│...│...│ ⚠          ││
│  └────┴────────┴────────┴───────┴───┴───┴───┴───┴─────────────┘│
└─────────────────────────────────────────────────────────────────┘
```

### 6.3 Panel Behavior

| Panel | Default State | Behavior |
|-------|---------------|----------|
| Left workspace | Sections open, 250px | Resizable, collapsible; linked projects also offer SpeleoDB (320px minimum, 400px maximum, overlay when constrained) |
| Detail Panel | Open, 300px | Resizable, collapsible |
| Data Table | Collapsed | Resizable (height), collapsible |

#### 6.3.1 Detail Panel Hyperlinks

The Detail Panel provides interactive hyperlinks that enable quick navigation between related survey elements (shots, stations, lines).

**Display labels (names, never ids):**
The Detail Panel names elements the way the rest of the UI does (4.1/4.2); the
positional `StationRef`, shot ids and line ids address them internally (and key
the test ids) but are never shown:
- Station: its name, `17`; with line context `Main Line / 17` (merged and
  closure partners, which may be on another line). The Station detail also
  shows the read-only **Station ID**.
- Shot: its two station names, `4 → 5`; with line context `Main Line: 4 → 5`
  (aria-labels and selection announcements always carry the line).
- Line: its name (`Unnamed line` only for a line that somehow has none).
- At the default `full` syntactic-highlighting level a link renders as a pill:
  entity symbol, the label, then (for a station or shot on another line) the
  line name dimmed after a divider.

**Shot Detail - Connected Line:**
```
┌─ Shot ─────────────────────────────────────────────┐
│  Shot: 2 → 3                                       │
│  Length: 4.5m  Azimuth: 127.3 deg  Depth: 8.2m    │
│                                                    │
│  ┌─ Connected ───────────────────────────────────┐ │
│  │  Start Station: 2 (link)                      │ │
│  │  End Station: 3 (link)                        │ │
│  │  Line: Main Line (link)                       │ │
│  └───────────────────────────────────────────────┘ │
└────────────────────────────────────────────────────┘
```

**Station Detail - Connected Line:**
```
┌─ Station ──────────────────────────────────────────┐
│  Name: 5   Station ID: 812                         │
│  Depth: 12.3m  Line: Main Line                     │
│                                                    │
│  ┌─ Loop Closures (collapsible, default expanded) ┐│
│  │  Closure to Side / 3 (link), Side / 4 (link)  ││
│  │  Path: 15.2m | Error: 0.30m (1.97%)           ││
│  │  Transitive closure to Upper / 0 (link)       ││
│  │    (via Side / 3)                             ││
│  │  Path: 42.1m | Error: 0.85m (2.02%)           ││
│  └────────────────────────────────────────────────┘│
│                                                    │
│  ┌─ Connected ───────────────────────────────────┐ │
│  │  Previous Shot: 4 → 5 (link)                  │ │
│  │  Next Shot: 5 → 6 (link)                      │ │
│  │  Line: Main Line (link)                       │ │
│  └───────────────────────────────────────────────┘ │
│                                                    │
│  (Previous/Next hidden at line boundaries)         │
│  (Loop Closures hidden when station has none)      │
└────────────────────────────────────────────────────┘
```

**Loop Closures Section:**
- Placed after "Merged with", before "Connected" section
- Collapsible with default expanded state
- Hidden when station has no `closures_with` entries
- Closures grouped by `closure_id`; direct closures listed first, then transitive
- Direct closures: "Direct: B, C" with station links (DetailLink)
- Transitive closures: "Transitive: X (via A)"; a transitive-only group is dimmed with `opacity-50`
- Metrics below each group: "Path 15.2m | Error 0.30m | 1.97%"
- Metrics from `project.loop_closures` by matching `closure_id`
- Station links navigate to target station (same behavior as merged station links)

**Line Detail - Endpoints Section:**
```
┌─ Line ─────────────────────────────────────────────┐
│  Name: Main Line                                   │
│  Length: 127.4m  Shots: 42                         │
│                                                    │
│  ┌─ Endpoints ───────────────────────────────────┐ │
│  │  First Station: 0 (link)                      │ │
│  │  Last Station: 42 (link)                      │ │
│  └───────────────────────────────────────────────┘ │
│  (Single station lines show only "Station:")       │
└────────────────────────────────────────────────────┘
```

**Link Behavior:**
- Click changes selection and updates Detail Panel content
- Line links also highlight the line in the Sections Panel
- Missing targets show warning indicator and are non-clickable
- Full keyboard accessibility (Tab + Enter)
- Links use accent color with underline styling

**Shot-to-Station Mapping:**
Shot at array index `i` (0-indexed) connects:
- Start station: `station_index = i` (shown by its name)
- End station: `station_index = i + 1` (shown by its name)

Station at `station_index` has:
- Previous shot: `line.shots[station_index - 1]` (hidden when `station_index === 0`)
- Next shot: `line.shots[station_index]` (hidden when `station_index >= line.shots.length`)

#### 6.3.2 Detail Panel Center Button

Each detail view (Shot, Station, Line) includes a center button in the panel header that pans and zooms the map to the selected element.

**Appearance:**
- Inline SVG crosshairs icon in panel header
- Uses `headerActions` prop pattern (same as other panel header buttons)
- Not shown in Project Stats view

**Behavior:**
- Click pans and zooms map to fit the element
- Stations/Shots: Uses `easeTo()` with configured `point_zoom` level
- Lines: Applies 10% margins on all sides (no panel offsets - the map container already excludes the side panels) and always uses `fitBounds()`. A single-station or zero-area line instead uses `easeTo()` at the configured `point_zoom`
- Full keyboard accessibility (Tab + Enter)
- Dynamic aria-label: "Center map on [type] [id]"

**Example aria-labels:**
- Station: "Center map on station 2:5"
- Shot: "Center map on shot 2:3"
- Line: "Center map on line 2: Main Line"

#### 6.3.3 ARIA Live Announcements

Selection changes are announced to screen readers via an ARIA live region.

**Implementation:**
- Single ARIA live region at App component root level (always mounted)
- Uses `role="status" aria-live="polite" aria-atomic="true"`
- Announcements triggered by: Detail Panel hyperlinks, Data Table shot selection
- Map clicks do not trigger announcements (avoids excessive verbosity during exploration)
- Visually hidden with `sr-only` class

**Announcement Formats:**

Detail Panel hyperlinks:
```
"Selected [type] [id]"
```
Examples: "Selected station 2:5", "Selected shot 2:3", "Selected line 2: Main Line"

Data Table shot selection:
```
"Selected shot [id]."
```
Example: "Selected shot 2:3."

#### 6.3.4 Sections Panel Tree (#327)

A project without Sections lists its lines flat, as it always did. Once it has
one, the panel is a tree: each Section is a header row (colour dot, name, "N
lines · L m", eye toggle, collapse chevron) with its lines beneath, in project
order, then an **Ungrouped** header for the lines in no Section. With every line
in a Section the Ungrouped header is left out, except while a line is dragged:
it is then the drop target that takes a line out of its Section.

- **One virtualized list.** The tree is flattened into one list of rows
  (`sectionsTree.ts`), virtualized like the flat list; line rows keep their test
  ids (`line-row-{id}`), headers are `section-header-{id}` /
  `sections-ungrouped-header`.
- **ARIA tree.** `role="tree"` (`aria-multiselectable`), rows are `treeitem`s
  with `aria-level` (1 for headers and flat lines, 2 under a header),
  `aria-posinset` / `aria-setsize`, `aria-expanded` on headers and
  `aria-selected` on lines. Up / Down / Home / End walk the rows; Right expands
  a header, then steps into it; Left collapses it, or steps from a line to its
  header; Enter selects and zooms to a line, or frames a header's lines; Space
  selects a line, or opens / closes a header; F2 renames a Section; H hides or
  shows a Section on the map (said through the selection live region);
  Shift+F10 / the Menu key opens the row's context menu; Escape clears the
  selection.
- **Headers are not selected.** A click on a header only focuses it: a Section
  is not a map item, so the map, Details and the data table would have nothing
  to show, and selecting its lines would replace the user's selection and draw
  a hidden Section. Shift-click adds all its lines to the selection;
  Ctrl/Cmd-click toggles them (takes them out when every one is selected
  already, else adds the rest); double-click frames them on the map. A hidden
  Section has nothing drawn to frame: double-click only focuses its header,
  Enter says "Show the section on the map first", and the menu's Zoom to
  Section is disabled with that hint.
- **Collapse** is per project, for the session. Selecting a line of a collapsed
  group elsewhere (map, data table, Details) opens the group, so a selection is
  always visible in the panel.
- **Editing** - New Section (header button: a pending row above the list, its
  field prefilled with the next free `Section N` and selected, and the dot in
  the next free palette colour; nothing exists until Enter, or leaving the field,
  with a usable name sends ONE `create_cave_section` - one undo step, named as
  typed; Escape drops the row and creates nothing), the header menu (Rename, Color..., Apply Color
  to Lines, Hide / Show on Map, Zoom to Section, Delete Section), the line menu's **Move to
  Section** flyout (each Section, No Section, New Section...: the same pending
  row, holding the right-clicked line, or the whole line selection it is part of), dragging a line row (or the
  line selection it is part of) onto a group, and the New Line dialog's Section.
  Every one is gated on `useEditingBlocked()` and re-checked at commit time; the
  drag uses pointer events (not HTML5 drag-and-drop, which WebKitGTK runs
  unreliably), mouse and pen only, so a touch long-press keeps opening the menu.
  The inline rename refuses what the backend refuses (empty, over 200
  characters, another Section's name, a control or invisible formatting
  character): Enter keeps the field open with the reason under it; leaving the
  field keeps the old name and says why in a warning toast, as the data
  table's inline edits revert.
- **Show / hide** is allowed in the read-only views too (it is a view setting).
  A hidden Section's lines are left out of the map's survey data
  (`store/sectionVisibility.ts`), so their geometry, stations, labels, comments,
  closures, hover and picking go at once; the panel shows them dimmed. A line
  the user selects is drawn while it is selected (so a selection is never
  invisible and Zoom to Line frames something); the Section stays hidden. A
  station or shot drag still moves stations of hidden lines merged with (or
  tied by an aligned closure to) the dragged one. The image export and its
  preview leave the hidden Sections' lines out too (`ExportImageOptions.
  hidden_line_ids`, stamped by `withHiddenLines` in `utils/exportOptions.ts`):
  no shots, stations, labels, comments, closures or datum stars of theirs, and
  the picture's extent covers only what is drawn. Unlike the map, a selected
  hidden line is still left out (the export follows the Sections, not the
  selection). Penetration labels and the distance colouring still measure along
  the whole survey, hidden lines included. The setting forgets a Section that
  no longer exists, on load and after every edit that refreshes the project
  (Delete Section, an Undo of New Section; `pruneHiddenSections`), so a
  Section created later with the same id starts shown. Fit to View, the fit
  after a load or import and the map's initial centre frame what is drawn: the
  hidden lines are left out while at least one line is shown
  (`utils/surveyFrame.ts`); with every line hidden they frame the whole survey.

### 6.4 Undo/Redo History Dropdown

The Undo and Redo toolbar buttons feature dropdown menus showing recent actions with timestamps and diff previews.

#### Trigger Behavior

- **Hover**: Dropdown opens after 300ms delay when hovering over Undo/Redo buttons
- **Chevron Indicator**: A small `▾` chevron appears next to the button text when history is available
- **Click**: Single click on button performs standard undo/redo (one step)
- **Empty State**: The button stays disabled and hover opens nothing. SpeleoDB revision inspection and restore actions live in the left workspace Revision History section.

#### Dropdown Layout

```
┌─────────────────────────────────────────────┐
│  UNDO HISTORY                               │
├─────────────────────────────────────────────┤
│  + Create line 'Main Line'       2 min ago  │
│  ✏ Update shot 3                 5 min ago  │
│  ✕ Delete line 3                 Jan 12...  │
│  ⊕ Merge stations                Jan 12...  │
│  ...                                        │
└─────────────────────────────────────────────┘
         │
         ▼ (hover item)
    ┌──────────────────────┐
    │  length              │
    │  5.0 → 10.5          │
    │                      │
    │  azimuth_in          │
    │  45.0 → 52.3         │
    │                      │
    │  +1 more             │
    └──────────────────────┘
```

#### Item Display

Each item shows:
- **Icon**: Inline SVG icon component chosen by command type (`getCommandIcon`, same icon set as the toolbar): plus (create line/shot), pencil (update shot/line metadata), trash (delete line/shot), link (merge) / link-off (unmerge), target (set zero datum) / circle (remove zero datum), move (move station/line) and move-horizontal (move segment), dashed line (set shot kind), layers (batch), dot (default)
- **Description**: Truncated at ~40 characters with ellipsis
- **Timestamp**: Smart hybrid format
  - Less than 1 day: relative (e.g., "2 minutes ago")
  - 1 day or older: absolute (e.g., "Jan 12, 3:45 PM")
  - Legacy entries without timestamp: "Unknown"

#### Detail Preview Popup

Hovering an item shows a preview popup with diff information (where available):
- Displays up to 3 field changes with old → new values
- Shows count of additional changes beyond those displayed
- Flips to left side if would overflow right viewport edge
- Commands without preview data show "No details"

#### Scrolling

- Fixed height: 5 items visible (~220px)
- Uses `height` (not `maxHeight`) to maintain consistent container size regardless of item count
- Auto-hide scrollbar (appears on hover)
- Smooth scroll behavior

#### Index Semantics

- **Index 0 = most recent action** (top of stack, first to undo/redo)
- Clicking item at index N executes N+1 undo/redo operations
- Example: `undo_to(3)` undos items at indices 0, 1, 2, 3 (4 total undos)

#### Close Behavior

The dropdown stays open when performing actions (button click, item click, keyboard shortcut) to allow rapid sequential operations with visual feedback.

- Dropdown **stays open** on:
  - Clicking the undo/redo button (performs action and refreshes item list)
  - Clicking a dropdown item (performs action and refreshes item list)
  - Global undo/redo shortcuts while open (Ctrl+Z, Ctrl+Y, Ctrl+Shift+Z)
- Dropdown **closes** on:
  - Mouse leaving the button/dropdown area (100ms buffer)
  - Pressing Escape key
  - Clicking outside the dropdown
  - Stack becoming empty after an action (auto-close)

#### Action Behavior

When an action is performed while the dropdown is open:
- A loading indicator (spinner with "Processing..." text) appears at the top of the dropdown
- The item list refreshes after the action completes
- Keyboard navigation and scroll position reset to the top (first item)
- Detail preview popup is dismissed immediately on any action
- Rapid clicks are queued and executed sequentially with a refresh after each
- If the stack becomes empty after an action, remaining queued actions are discarded and the dropdown closes
- On error, the dropdown stays open, the stack is refreshed to reflect backend state, and an error dialog is shown with title, message, and stack trace (if available from a caught exception)
- Ctrl+Z/Y/Shift+Z keyboard shortcuts are intercepted by the dropdown (capture phase) to prevent the global handler from causing a double-undo/redo
- Keyboard shortcuts are NOT intercepted when focus is in a text input, textarea, select, or contentEditable element (native text editing undo/redo is preserved)

### 6.5 Tools

The tool set is the `ToolType` union in `src/store/uiStore.ts`; per-tool click and
drag handling lives in `src/hooks/useMapTools.ts`. Shortcuts below are the
defaults from `src-tauri/crates/core/src/config/defaults.rs` and are all rebindable.

| Tool | Shortcut | Click Station | Click Shot | Click Empty | Drag |
|------|----------|---------------|------------|-------------|------|
| **Select** | S | Open detail panel | Open shot details | Deselect | Pan map |
| **Pan** *(hidden by default; `map.controls.show_navigation_tools`)* | P | Pan | Pan | Pan | Pan map |
| **Zoom** *(hidden by default; `map.controls.show_navigation_tools`)* | Z | Zoom in | Zoom in | Zoom in | Zoom area |
| **Merge** | M | Select for merge | - | - | - |
| **Measure** | R | Start measurement | Measure from/to a point along the shot | Start measurement | - |
| **Move Station** | T | Pick up an unanchored station | - | Drop what is being carried | Pan map |
| **Move Shot** | G | - | Pick up one shot: both endpoints plus any co-located stations | Drop what is being carried | Pan map |
| **Move Segment** | V | Pick up the whole connected branch (lines joined by merges and aligned closures) | Same, grabbed on a shot | Drop what is being carried, else deselect | Pan map |
| **Comment** | C | Open the comment editor on the station | Open the comment editor on the shot | Close the editor | - |
| **Move Comment** | Shift+C | Pick up the comment; second click moves it (swap dialog if the target already has one) | Same, for shot comments | Cancel | - |

There is no Draw or Delete tool. Shots are added from the map context menu:
**Add Shot Before/After** at the ends of a line, or **Insert Station...** on
any shot, which splits it in two (also available from the data table's row
menu). Deletion is select plus Delete/Backspace, or the context menu. The same
menu restructures lines without deleting anything: **Split Line Here** on
a mid-line station or a point along a shot, and **Combine Line With
<name>...** (or, at a junction, **Combine Lines...**) on an end station where
another line ends (see 7.11).

Panning is not a tool either. A plain left-click-drag pans the map under every
tool in the table except **Zoom**, whose drag draws its zoom box, and in
placement mode, which confirms on press rather than on release. The single
predicate is `mapGestureMode` in `src/utils/mapGestures.ts`. The move tools are
click-to-pick-up and click-to-drop throughout: a drag pans even while geometry is
being carried, leaving the ghost on the cursor, and only a press that stays put
drops it. **Pan** and **Zoom** survive as
an accessibility fallback for anyone who cannot drag or scroll: they are hidden
from the toolbar, and their P/Z shortcuts inert, unless
`map.controls.show_navigation_tools` is on (Settings -> Map -> Controls).
Turning that setting off while one of them is active drops the map back to
Select.

The three move tools are documented in detail in [move-tools.md](move-tools.md).

### 6.6 Map Features

#### Display Toggles (Right-click or menu)
- Stations (dots)
- Line names (labels)
- Station IDs or Station names (labels; one or the other, never both). Station
  IDs label every station with its project-unique Station ID
  (`SurveyData.stationIds`), never its position. Station names label every
  station with its name (unique within its line, e.g. its MNemo station
  number); a name that is empty once control characters are stripped draws no
  label.
- Station comments, shot comments
- Shot data (azimuth, length, depth)
- Errors: one label per shot with how far its readings disagree (Δaz azimuth in
  vs out, Δinc inclination in vs out, Δi/d recorded inclination vs the one the
  length and depth change imply, ΔL recorded length vs the one the depth change
  and inclination imply - only on shots steeper than 15°), and one per loop
  closure ("Loop 2.1% 0.84m"). A part that rounds to zero or whose warning was
  ignored is left out. Where labels collide the worst error is shown. Saved per
  project; off by default
- Reference shots (faded, semi-transparent lines)
- Zero datum (enlarged golden station dot)
- Grid overlay (theme-aware; spacing chosen by the user from 1, 5, 10, 25, 50, 100 m - default 10 m)
- Scale bar (always shown; not a toggle)
- Satellite imagery
- Depth color gradient, and the distance gradient: palette and Levels range from the legend cog (docs/color-palettes.md)
- Survey Issues coloring: each shot graded by its worst disagreement, green
  (agree) -> yellow (at the warning threshold) -> red (3x the threshold, or an
  error); unresolved errors are red, other unresolved warnings at least yellow.
  The legend cog sets the palette, worst or average, the errors that count and
  per-error Min / Warn / Max levels (map only; never the validation thresholds)
- While the Warnings tab is open (setting "Highlight warnings on tab switch"),
  Survey Issues and Errors are on. Changes made then last until the tab is left
  and are never saved; leaving restores the saved display settings
- LRUD passage walls (future)

#### Map Interactions
- Pan: Click-hold-drag anywhere, middle mouse drag, or Ctrl+drag
- Zoom: Scroll wheel, pinch, double-click, +/- keys, F to fit
- Select: Left click
- Multi-select: Shift+click (add), Ctrl+click (toggle) - **Cmd+click on macOS**,
  where Ctrl+click is a secondary click and opens the context menu instead, so
  no `click` event ever reaches the toggle. Both modifiers are accepted by the
  handler (`useMapTools.ts`, the `ctrlKey || metaKey` branch); it is the
  platform that decides which one arrives
- Context menu: Right click
- Snap to station: Automatic when within threshold (configurable)

### 6.7 Visual Indicators

| Element | Appearance |
|---------|------------|
| Regular shot | Solid colored line (line color) |
| Reference shot | Solid line at reduced opacity (~0.28) |
| Station | Filled circle |
| Selected station | Highlighted ring |
| Zero datum | Golden station circle at 2x radius |
| Validation warning | Survey Issues mode: green -> yellow -> red by how far the readings disagree; ⚠ icon in the table and Warnings panel |
| Marked as resolved | No map highlight; dimmed and struck through in the Detail Panel |
| Loop closure line | Dashed line with error percentage |

### 6.8 Dialogs

#### DMP Import - Connection Wizard (DmpNewImportDialog)

After line selection, the primary import flow uses a connection wizard that auto-detects overlapping shots between lines and places them with a single batch import. The wizard handles overlap treatment (interpolation, import data, existing data, keep both), doubly-anchored guards to protect closure/datum constraints, and atomic updates to existing line shots. See [DMP Import Wizard](dmp-import.md) for full architecture documentation.

#### DMP Import - Line Selection (DmpSelectionDialog)

Each MNemo section in the file is offered as one line. After selecting lines, the workflow continues into zero-datum setup (if needed) and the connection/placement wizard.

##### Step 1: Line Selection (DmpSelectionDialog)

```
┌─ Import DMP File ────────────────────────────────────────────────┐
│                                                                  │
│  File: cave_survey_2024.dmp                                     │
│  Parsed: 3 lines, 156 shots total                               │
│                                                                  │
│  Default Values (apply to all lines):                           │
│  Surveyor: [J. Smith____________] (remembered for session)      │
│  Date:     [2024-01-12__________]                               │
│                                                                  │
│  ┌─ Lines ──────────────────────────────────────────────────────┐│
│  │ [✓] AAA - Main Line (42 shots, 187.3m)           [Preview]  ││
│  │ [✓] AAB - Side Passage (28 shots, 94.5m)         [Preview]  ││
│  │ [ ] ZZZ - Test Data (86 shots, 412.1m)           [Preview]  ││
│  └──────────────────────────────────────────────────────────────┘│
│                                                                  │
│  70 shots | 281.8m | Depth: 2.1-15.4m                            │
│                                                                  │
│  ⚠ File may have been imported before (same hash detected)     │
│                                                                  │
│  [ Cancel ]                              [ Continue (2 of 3) ]   │
└──────────────────────────────────────────────────────────────────┘
```

**Step 1 Behavior:**
- Select/deselect lines for import
- Set default surveyor and date (inherited by all lines)
- Surveyor remembered for session (not persisted)
- File hash tracking detects duplicate imports
- Preview button shows line outline in popup
- Selection stats update in real-time when lines are checked/unchecked
- Shows "0 shots | 0.0m | Depth: --" when no lines selected

**Selection Stats Formatting:**
- Format: `X shots | Y.Ym | Depth: A.A-B.Bm`
- Units read from project config (length and depth independently)
- Values converted to feet when config is set to feet (shows `ft` suffix)
- 1 decimal place precision for length and depth
- Pipe separators between stats
- "Depth: --" shown when no depth data available
- ARIA label provides screen reader description (e.g., "Selection statistics: 70 shots, 281.8m total length, depth range 2.1-15.4m")

##### Step 2: Connection + Placement Wizard (DmpNewImportDialog)

```
┌─ Configure Line: AAA - Main Line ────────────────────────────────┐
│                                                                  │
│  Line: 1 of 2                       [◀ Previous] [Next ▶]       │
│                                                                  │
│  ┌─ Parameters ─────────┐  ┌─ Preview ──────────────────────────┐│
│  │ 42 shots | 187.3m |  │  │                                    ││
│  │ Depth: 3.2-17.8m     │  │        ●──●──●                     ││
│  │ ─────────────────────│  │           \                        ││
│  │ Name:    [Main Line] │  │            ●──●                    ││
│  │ Surveyor:[J. Smith ] │  │   (pan/zoom enabled)               ││
│  │ Date:    [2024-01-12]│  └────────────────────────────────────┘│
│  │ Color:   [████ ▼]    │                                        │
│  │ Direction:[IN ▼]     │                                        │
│  └──────────────────────┘                                        │
│                                                                  │
│  ┌─ Placement ───────────────────────────────────────────────────┐│
│  │ (●) At Origin (0,0)                                          ││
│  │ (○) From Existing Station                                    ││
│  │     Connect [Line ▼] Station [# ▼] → [Start/End ▼]          ││
│  │ (○) Manual Placement                                         ││
│  │     Click on map to position                                 ││
│  └───────────────────────────────────────────────────────────────┘│
│                                                                  │
│  ┌─ Shot Table ──────────────────────────────────────────────────┐│
│  │ ID │ Len  │ Azi  │ DepIn│DepOut│ L  │ R  │ U  │ D  │ ⚠ │ ✕ ││
│  │ 1  │ 3.2  │ 45.0 │ 5.1  │ 5.8  │1.2 │0.8 │2.1 │1.5 │   │ x ││
│  │ 2  │ 4.1  │ 52.3 │ 5.8  │ 6.2  │1.0 │1.1 │1.8 │1.2 │ ⚠ │ x ││
│  │ [+ Add Row]                                                  ││
│  └───────────────────────────────────────────────────────────────┘│
│                                                                  │
│  Statistics: 42 shots │ 187.3m total │ Depth: 3.2m - 17.8m      │
│                                                                  │
│  [ Cancel ]                                     [ Place segment ]│
└──────────────────────────────────────────────────────────────────┘
```

**Step 2 Behavior:**
- Per-line editing with navigation between lines
- Parameters panel with line stats at top:
  - Shows shot count, total length, depth range for current line
  - Stats reflect current shot table state (including edits and deletions)
  - Grayed out when line is excluded
  - Subtle separator line below stats
- Parameters panel: name, surveyor (inherited), date, color, direction
- Interactive preview with pan/zoom and click-to-select-station
- Placement modes (exclusive radio group): **Suggested connection** (when one was
  skipped), **From existing station**, **Manual placement** (click on map), **Do
  not place** (skip the segment)
  - GPS placement is not a mode here: if the import needs a zero datum, it is
    set beforehand via the separate line-selection and configuration
    dialogs (§7.10)
- Shot table with:
  - Azimuth as single column (shows average, edits set both in/out)
  - Warning column (⚠) for azimuth/pitch discrepancies
  - Right-click context menu: insert before/after, delete
  - Keyboard navigation (Tab, Enter, Escape)
- Line created on "Place segment" click
- Warnings inform but don't block; errors block import

**Line Stats Formatting:**
- Format: `X shots | Y.Ym | Depth: A.A-B.Bm`
- Units read from project config (length and depth independently)
- Values converted to feet when config is set to feet (shows `ft` suffix)
- 1 decimal place precision for length and depth
- Pipe separators between stats
- "Depth: --" shown when no depth data available
- Stats exclude deleted shots (those marked with delete checkbox)
- ARIA label provides screen reader description (e.g., "Line statistics: 42 shots, 187.3m total length, depth range 3.2-17.8m")

**Design Decisions:**
| Decision | Rationale |
|----------|-----------|
| Lines created on placement | Enables granular undo, immediate persistence |
| Exclusive placement modes | Simpler UX, clearer mental model |
| Warnings don't block, errors block | Warnings inform, don't impede workflow |
| Session-only surveyor memory | Respects privacy, not persisted to config |
| Azimuth as average column | Cleaner table, edit sets both in/out |

##### Create Line Dialog (Manual Entry)

For manual entry (no DMP file), use the generic Import dialog with an empty shot table.

#### Reverse Line Dialog (ReverseLineDialog.tsx)

Confirms reversing a line end-to-end, so the far end becomes station 0.

```
┌─ Reverse Line ───────────────────────────────────────────────────┐
│                                                                  │
│  Renumber this line end-to-end, so the last station becomes      │
│  station 0.                                                      │
│                                                                  │
│   • Main Line (station 0 → 14, station 14 → 0)                   │
│                                                                  │
│  The cave does not move: every station keeps its position on     │
│  the map, and merges, loop closures, zero datums and comments    │
│  stay on the station they were set on.                           │
│                                                                  │
│  [ ] Swap left/right passage measurements                        │
│      Tick this if LRUD was recorded facing along the survey      │
│      direction, so what was on the left is now on the right.     │
│                                                                  │
│  [ Cancel ]                                     [ Reverse ]      │
└──────────────────────────────────────────────────────────────────┘
```

**Semantics.** Reverse is a one-shot action, not persisted state. Geometry is
invariant - every station keeps its world coordinate, and `reverse(reverse(s))`
is the identity in exact arithmetic. In f64 it is only *bit*-exact when the
`+180` shift is representable (`17.5` round-trips exactly; `0.1` comes back a
couple of ulps out), which is why undo snapshots the project rather than
re-running the transform. Per shot:

| field | operation |
|-------|-----------|
| shot order | reversed; ids stay attached to positions, `uuid` travels with the record |
| `azimuth_in` / `azimuth_out` | reciprocated (`+180`) **and** swapped |
| `depth_in` / `depth_out` | swapped |
| `pitch_in` / `pitch_out` | swapped **and negated** |
| `left` / `right` | swapped only when the user opts in |
| `up`, `down`, `length`, `marker_index`, `comment`, `kind` | unchanged |

`azimuth_in`/`azimuth_out` are two **forward** readings and geometry uses their
circular mean, so swapping them alone is a no-op - the reciprocal is what
actually turns the shot around. The exception is a pair exactly 180 degrees
apart (a gross reading blunder): its circular mean is undefined, and
reciprocating maps the pair to itself, so such a shot is **swapped** instead.
That keeps both recorded values and flips the arbitrary tie-break the mean is
resolved with, which is what turns the bearing around.

**Azimuths are stored in `[0, 360)`.** The circular mean wraps with `rem_euclid`,
not Rust's `%`: `%` is *truncated*, so for a reading pair more than 540 degrees
apart it left the intermediate difference outside `(-180, 180]` and returned the
**antipode** of the true bearing. Nothing about a stored value changed meaning -
an azimuth is still degrees clockwise from north and the old rendering was simply
wrong - so, unlike the v11 to v12 pitch flip, the fix must not rewrite readings to
preserve the old geometry. Instead the ambiguity is taken out of the file:
schema **v14** normalises every stored reading into `[0, 360)`, and
`DmpShot::to_shot_data` now does the same at import (Compass and TML already did),
so no build can reach the truncation case again. Normalising is a no-op for
geometry - `600` and `240` are the same reading - so the same `.stalx` renders
identically either side of the fix. Affected data is narrow and already flagged:
both readings inside `[0, 360)` put their difference in `(-360, 360)`, so a shot
that could be misread always has an out-of-range reading, which
`Validator::validate_shot` reports as `INVALID_AZIMUTH`.

Beyond the shots, `line_offsets` (the `section_offsets` table) gains the line's
old total displacement (station 0 is now the far end), and every external
station ref in the line -
merges, loop closures, zero datums, station offsets and comments - is remapped
to its mirrored station so each relationship stays on its original physical
station. That list is the complete set of `StationKey`-bearing collections on
`Project` (`for_each_station_ref`), but it is *not* everything in the app that
names a station: the gas plan's pins (origin, endpoint, way points, marked
points, stage-drop overrides) and the UI's selection are positional
`StationRef`s the backend cannot reach. The plan's pins are mirrored frontend-
side so the plan stays on the physical stations the diver picked; selection and
merge staging are cleared.

Pins inside a *saved* `.plnx` need **no** rewriting, because a plan does not name a
station by its position. It records the two shots a station sits BETWEEN, and a
reverse does not change what a station sits between - the same pair still names
the same junction whichever way the line is walked.

That matters because the old `AfterShot(uuid)` form failed silently: after a
reverse such an anchor still *resolved*, to the station next door, since the
anchoring shot travelled with its record. Nothing counted as stale, so
`stale_ref_count` stayed 0 and the panel showed no warning while the route - and
with it the gas plan - had quietly been re-priced against a different pair of
stations. Every other survey edit leaves an unresolvable ref that fails loudly.

A station at the END of a line has only one neighbouring shot, so its side
comes from the line's recorded orientation (`reverse_parity`) instead. The one
case that cannot be tracked is splitting that shot: the split keeps the original
UUID on the shortened tail, so afterwards no shot in the file has that station as
an endpoint. That is detected via the neighbour's recorded length and reported as
a stale ref for the diver to re-pin, rather than guessed at.

One other store outside `Project` is settled the same way: position-keyed
(`NULL`-uuid) `resolved_warnings` rows are pinned to their shot's UUID, because
the reverse keeps shot *ids* on positions - so the save-time carry-over would
otherwise re-read the row's `(section_id, shot_id)` columns as an identity and migrate
the ignore onto the mirrored shot.

`CommandResult.reversed_lines` names the lines a command renumbered -
on the forward reverse, on a batch of them, and on an undo or redo of either
(undoing a reverse re-reverses the numbering). A reverse keeps every station's
world position, so there is nothing in the geometry for a consumer to diff; the
field exists so the frontend mirrors its positional refs exactly instead of
inferring a reverse from moved stations, an inference that cannot tell a
reversed line from one that was reversed *and* moved in the same action. It
is omitted from the payload entirely when nothing was renumbered.

The line's `direction` (In/Out) is metadata and is deliberately **not**
flipped - it records how the survey was swum, which a renumbering does not
change. Nothing in the geometry pipeline reads it.

Frozen shots do not block a reverse: the physical line is preserved exactly, so
the frozen invariant still holds. Undo is a before/after project snapshot pair.

**Entry points.** Sections-panel, map-station and map-shot context menus
("Reverse Line", plus "Reverse N Lines" when several lines are
multi-selected), and a button in the Edit Line dialog that is disabled while
that dialog has unsaved edits.

A multi-line reverse is **one action**: it goes through `execute_batch`, so
either every selected line reverses or none does, and a single Undo restores
them all.

**On import.** `ImportLineConfig.reverse` applies the same transform before
the line is created. `placement` indices must already address the final
(reversed) order - only the wizard knows which side of a line-to-line
connection this batch renumbers (`src/utils/reversePlacement.ts`) - while
`station_comments` indices are given in the file's original order and mirrored
backend-side. Left/right is never swapped on the import path (no dialog to ask).
The toggle is offered by the DMP wizard only; the TML, Compass, Walls and GPX
importers build their configs with `reverse: false`.

Ticking the toggle changes numbering only, never where the passage lands: the
line anchor gains the same displacement the in-app reverse applies, because
station 0 is now the far end. Merge- and datum-based placements re-anchor the
line anyway; `Origin` and `Manual` position station 0 directly and would
otherwise land a full end-to-end displacement away.

Both sides of a connection renumber independently. A wizard connection whose
target is created **by the same batch** has its target index mirrored too - and
that includes a target the wizard rewrites into a `from_existing` placement
because it was already created earlier in the run. A connection to a line
that was already in the project keeps the index the user picked.

#### Merge Stations Dialog

Shown when merging two **unconnected** stations (different line groups).

```
┌─ Merge Stations ─────────────────────────────────────────────────┐
│                                                                  │
│  Station A: Line "Main Line" #14 (depth: 12.3m)                 │
│  Station B: Line "Side Passage" #0 (depth: 12.8m)               │
│                                                                  │
│  Distance between: 2.4m                                          │
│  Depth difference: 0.5m                                          │
│                                                                  │
│  [ Cancel ]                                      [ Merge ]       │
└──────────────────────────────────────────────────────────────────┘
```

If the two stations are already connected (same line group), MergeStations is rejected and the frontend should show the Closure Dialog instead.

#### Closure Dialog (ClosureDialog.tsx)

Shown when creating a loop closure between two **connected** stations.

```
┌─ Create Loop Closure ────────────────────────────────────────────┐
│                                                                  │
│  Station A: Line "Main Line" #14 (depth: 12.3m)                 │
│  Station B: Line "Side Passage" #0 (depth: 12.8m)               │
│                                                                  │
│  Path length: 156.2m                                             │
│  Misclosure: 2.4m (1.5%)                                        │
│  Horizontal: 2.1m  Vertical: 0.5m                               │
│                                                                  │
│  Closure strategy: Batch (distribution deferred)                │
│                                                                  │
│  [ Cancel ]                              [ Create Closure ]      │
└──────────────────────────────────────────────────────────────────┘
```

When the backend would refuse the pair (#737), the connectivity check carries the
reason (`ConnectivityResult.closure_refusal`) and the dialog shows it above the
station details in an error box (`closure-refusal`) with **Create Closure**
disabled. The one refusal is a closure that would join the two ends of one shot
(directly, or through merges and other closures): "This closure would join the two
ends of one shot (A:1 to A:2) and squeeze that shot to zero length. Pick two
stations that are not the ends of a single shot." A pair that other merges and
closures already tie together is still accepted. The backend refuses the same
pairs regardless of the dialog. See [closures.md](closures.md#create-loop-closure).

#### Export Dialog

```
┌─ Export ─────────────────────────────────────────────────────────┐
│                                                                  │
│  Export Type: [Map Image ▼]                                     │
│    • Map Image (PNG, SVG, PDF)                                  │
│    • Survey Data (CSV, JSON, TML)                               │
│                                                                  │
│  Format: [PNG ▼]                                                │
│                                                                  │
│  Options:                                                        │
│    [ ] Include station comments                                 │
│    [ ] Include shot comments                                    │
│    [ ] Include station IDs                                      │
│    [ ] Include station names                                    │
│    [ ] Include line names                                       │
│    [ ] Include shot data (azimuth, length, depth)               │
│                                                                  │
│  Resolution: [High (300 DPI) ▼]                                 │
│                                                                  │
│  [ Cancel ]                                      [ Export ]      │
└──────────────────────────────────────────────────────────────────┘
```

---

## 7. Workflows

### 7.1 First Launch

1. Application opens to Global View (empty)
2. Empty state title: "Open or drop any survey file"
3. Two controls: a drop zone [Open or drop a file] and [Start a new project instead]
4. No tutorial - interface should be self-guiding

### 7.2 Creating New Project

1. User clicks [New Project]
2. Dialog: Enter name, optional location description
3. Project created, opens in Single System View
4. User can now import lines or add manually

### 7.3 Importing DMP File (Wizard Flow)

**Step 1: Line Selection (DmpSelectionDialog)**

1. User clicks [Import] or drags .dmp file onto application
2. DMP parsed, Step 1 dialog opens showing each MNemo section as a line
3. User sets defaults:
   - Default surveyor (remembered for session)
   - Default date
4. User selects which lines to import (checkboxes)
5. If file hash matches previous import, warning shown
6. User clicks [Continue] to proceed

**Step 1b: Zero Datum Setup (ZeroDatumSelectionDialog / ZeroDatumConfigDialog)**

Runs only if the project has no zero datum yet:

7. ZeroDatumSelectionDialog shows the selected lines as cards; user picks
   the one that should carry the datum, or clicks Skip - lines still get
   placed in Step 2 below, just with no real-world GPS anchor until a datum
   is set later (see §7.10)
8. Picking a line opens ZeroDatumConfigDialog: choose the station, enter
   its GPS coordinates, and optionally edit the line's metadata and shots
9. Confirm creates the line with its GPS-derived position and proceeds to
   the connection wizard for the remaining lines

**Step 2: Connection + Placement Wizard (DmpNewImportDialog)**

10. Step 2 dialog opens with the next selected line
11. User configures each line:
    - Edit metadata (name, surveyor, date, color, direction)
    - Review/edit shot data in table
    - Azimuth shown as average; editing sets both in/out
    - Warning icons for azimuth/pitch discrepancies
12. User selects a placement method (exclusive): Suggested connection (when one
    is offered), From existing station, Manual placement, or Do not place
13. User clicks [Place segment]
14. Line created in backend immediately
15. User navigates to next line or clicks [Done]
16. Each placed line is auto-saved

**Undo/Redo within Import:**
- Local undo/redo tracks changes within the dialog
- After placement, the line becomes part of project undo stack
- Closing app during import prompts confirmation to discard

**Error Handling:**
- Warnings (e.g., azimuth mismatch) are shown but don't block
- Errors (e.g., invalid data) block the Place segment button

### 7.4 Manual Data Entry

1. User clicks [New Line] or [+] in Sections Panel
2. Import Dialog opens with empty table
3. User enters metadata and shot data row by row
4. Live preview updates as data entered
5. Quick entry mode: Tab advances cells, Enter advances rows
6. Same placement flow as DMP import
7. Auto-saved

### 7.5 Connecting Lines (Merge vs Loop Closure)

The Merge Tool connects two stations. The behavior depends on whether the stations are already in the same connected group:

**Merging unconnected stations** (different line groups):
1. User activates Merge Tool (M)
2. Selects two stations from different, unconnected line groups
3. MergeDialog appears showing station details and distance
4. Confirm → Stations merged, lines repositioned to align

**Creating a loop closure** (already connected stations):
1. User activates Merge Tool (M)
2. Selects two stations that are already in the same connected group
3. ClosureDialog appears with closure preview data (path length, misclosure, error %)
   - If the two stations are the two ends of one shot (directly or through merges
     and other closures), the dialog says why and Create is disabled: the closure
     would squeeze that shot to zero length (#737)
4. User selects error distribution method
5. Confirm → Loop closure created, shown as dashed line on map
6. Closure stations updated with `closures_with` references (direct + transitive)
7. Detail Panel shows "Loop Closures" section for involved stations

**Connecting Lines with Reference Shots:**
1. User imports/creates new line with the connection survey
2. In data table, user cycles the connecting shots' J/R cell to Reference
   (click cycles blank -> J -> R -> blank; shift-click reverses; the fill
   handle stamps one kind down a whole run)
3. User places line using "From Station" to connect one end
4. User uses Merge Tool to connect other end to second line
5. If lines are unconnected → MergeDialog (simple merge)
6. If lines are already connected → ClosureDialog (loop closure with error preview)
7. Reference shots drawn faded (fainter than jumps)
8. Jumps and references toggle independently in display options

   Note (#27): this workflow ends with the run merged on BOTH ends, which is
   exactly what the import-time chain rule reads as a Jump. The rule cannot
   tell a swum jump from a tape tie, since both bridge two real points, so a
   run built this way needs its kind set explicitly in the data table.

### 7.6 Editing Existing Data

**Via Data Table:**
1. User expands Data Table panel
2. Selects line from dropdown
3. Clicks cell, edits value
4. Map updates in real-time
5. Auto-saved

**Via Map:**
1. User clicks station/shot with Select tool
2. Detail Panel shows properties
3. User edits in Detail Panel
4. Map updates in real-time
5. Auto-saved

**Via Right-Click:**
1. Right-click station/shot
2. Context menu → Edit, Delete, Mark Reference, etc.
3. Dialog or inline edit as appropriate

### 7.7 Deleting Data

**Deleting a Shot:**
1. Select shot, press Delete (or right-click → Delete)
2. From the map context menu, the data table or the detail panel, the confirm is
   position-aware: an edge shot gets "Are you sure you want to delete this
   shot?" and is deleted without splitting; a mid-line shot opens the Delete
   Shot dialog offering [Split Line] or [Shift Geometry]; the last remaining
   shot removes the whole line
3. On Split Line, the line splits into two, named "<line> A" and
   "<line> B" by default, each made unique against the other lines (both
   editable; an empty name, another line's name or the two names equal is
   refused inline and disables the button)
4. A delete with shift (a single shot's Shift Geometry, or a range of shots with
   shift) that leaves a loop closure spanning a single shot - e.g. closure A:1-A:4
   after shots 1..2 are gap-closed - removes that closure with the delete. The
   Delete Shots dialog lists it before you confirm ("Close Gap & Delete removes
   these loop closures (they would span a single shot)"; Split removes none), and
   a warning toast says so afterwards ("Removed
   loop closure A:1 to A:4: closing the gap left its two stations at the two ends
   of one shot."). Both name the closure by the
   stations it was created with. One Undo restores the shots and the closure
   (#737)

**Deleting a Station:** Not supported. Selecting a station and pressing Delete
opens the Delete Station dialog, whose Delete button is disabled; it tells the
user to delete the connected shots instead.

**Deleting a Line:**
1. Right-click line in Sections Panel → Delete
2. Confirmation: "Are you sure you want to delete this line and all its
   shots?", plus a list of relationships (merges, closures, offsets, zero
   datums) that will also be removed
3. Confirm → Line removed

### 7.8 Merging Cave Systems

1. User is in Global View
2. Multi-select two systems (Ctrl/Cmd+click)
3. Right-click → "Merge N projects"
4. Dialog:
   - Shows both system names and statistics
   - Enter new merged system name
   - Optional checkbox: "Archive original project files (rename to
     .stalx.archived)" - off by default
5. Confirm → Systems merged into single file. Each source is read like an
   open would read it (through a private copy, so the source files are only
   read - renamed only if you choose to archive them - and an older file's
   pending shot classification is finished).
   A system with a GPS zero datum is placed at its real position: the first
   one's datum station becomes the origin, and every later one lands at its
   GPS position relative to it. A system without a datum keeps its own layout.
6. Auto-switch to Single System View
7. User connects the systems using Merge Tool

### 7.9 Moving Unanchored Lines

Unanchored lines can be repositioned via the **Adjust Line Position** dialog (`LineAdjustDialog`; right-click line → "Adjust Position..."). Individual unanchored stations can be moved with the Move Station tool (T).

### 7.10 Setting a Zero Datum

The zero datum establishes the coordinate system origin and enables GPS-accurate positioning:

**Setting Zero Datum via Map:**
1. Right-click on a station → "Set Zero Datum"
2. The Edit Station dialog opens with "Set as Zero Datum" ticked and the GPS
   field focused: enter the coordinates in one field (DD, DMS, DDM, UTM, Plus
   Code or map URL; MGRS and What3words are recognized and validated but not
   converted - see 2.7)
3. Click Save
4. Station marked with the enlarged golden zero-datum station dot
5. Map auto-zooms to fit survey in new position
6. All line offsets are recalculated

A depth-reference input (meters below surface, alongside lat/lon) is not yet
built anywhere in this flow; it is tracked as an enhancement in #524.

**Setting Zero Datum during Import:**
1. If the project has no zero datum yet, the DMP wizard opens
   ZeroDatumSelectionDialog after line selection: pick which selected
   line should carry the datum, or Skip - lines still get placed
   (Origin/From Existing/Manual, per line, in the connection wizard) but
   the survey has no real-world GPS anchor until a datum is set later (see
   §7.3 Step 1b)
2. Picking a line opens ZeroDatumConfigDialog: choose the station and
   enter its GPS coordinates
3. Confirm positions that line using the GPS-to-local transformation; the
   rest of the import continues through the normal connection wizard

**What Happens Internally:**
1. GPS coordinates (WGS84) are stored with the station
2. Coordinates are converted to local ENU meters via geodetic transformation
3. Line offsets are recalculated to maintain relative positions
4. The zero datum station becomes the (0,0) origin of the local coordinate system
5. Connected lines' positions are preserved through offset tracking

**Updating an Existing Zero Datum:**
1. Right-click on the zero datum station → "Set Zero Datum" (reopens the Edit
   Station dialog)
2. Modify coordinates and confirm
3. All line offsets are recalculated
4. Undo restores previous GPS coordinates

Changing only the accuracy of a datum (its GPS unchanged) moves nothing, for a
placement datum and a GPS fix alike; only a new GPS reading re-places a piece.
A station holds at most one zero datum: setting one on a station that already
holds a datum under another identity is refused, and re-setting a datum
updates it in place.

**Removing a Zero Datum:**
1. Right-click the zero datum station → "Set Zero Datum", untick "Set as Zero
   Datum", and click Save
2. Station loses the enlarged golden zero-datum styling (back to a normal
   station dot)
3. If this was the only datum, coordinate system reverts to (0,0) origin
4. Undo restores the GPS coordinates

---

### 7.11 Splitting and Combining Lines

**Split Line Here** (station or shot context menu, Survey mode): cuts a
line in two at a mid-line station, or at a point along a shot (a station
is inserted there first; within 1 cm of a shot end the cut snaps to that end).
Nothing is deleted. The first half keeps the line id and ends on the split
station; the second half starts on a co-located copy and copies the line's
direction, quality and north reference (every shot keeps its own surveyors,
explorers and date). The two copies are NOT
merged. Merges on the split station, its name and its manual offset go to both
copies; its loop closures stay on the first half; the dialog picks the half for
its comment (A, B or both) and its zero datum (A or B). Names default to
`<name> A` / `<name> B` made unique (` (2)`, ...; line names are required and
project-unique, 4.1), each half has a colour picker: A keeps the line's
colour, B defaults to the colour a new line gets (the next palette colour no
line uses, never A's). The dialog notes that the halves are not connected
(merge the two cut stations for routes and gas planning to continue across
them). A loop closure from
before the cut to past it, with nothing else tying the halves, is left as their
only tie, which the split turns into a merge (a closure must close a loop),
moving one half by the loop's misclosure; the dialog says which half moves and
how far before the user confirms (`split_line_preview`). A second closure
that tied that half to the same point would then tie the point to itself (its
two stations are merged), so the split removes it (#738), and the dialog lists
each one by the station ids it was created with: "Loop closure 1:0 to 0:3 is
removed: its two stations become one point." A closure that already tied a
point to itself goes too, listed as "...: its two stations are already one
point." One undo step brings back every closure. The delete-and-split commands,
Delete Shot, Delete Line, Unmerge and Remove Zero Datum remove such closures
the same way and say so in a toast, by the station ids they were created with.
A project file that already holds a closure between merged stations, or from a
station to itself, opens without it ([closures.md](closures.md)).

**Combine Lines** (an end station's context menu, or **Combine 2 Lines...**
in the line menu when exactly two lines are selected): the two chosen
lines meet end to end and
become one; other lines there stay attached as branches. They meet when an
end station of each is the same point: in one merge group; unmerged and
co-located within 1 mm horizontally (the state a split leaves); or tied by a
loop closure, the survey's own statement that the two are one point, apart by
the loop's misclosure. Another line's end merged there stays merged to the
joined station, now a station in the middle of the combined line; a line
merged there at a station in its middle stays merged too (a crossing); one
ending there unmerged is left as it is, and the dialog says so. The station
menu offers, by how many lines end at the point: with one other, **Combine
Line With <name>...**, opening the dialog directly; with three in all,
**Combine Lines...** and a submenu of every pair ("A with B"); with four or
more, **Combine Lines...** > "A with..." > the lines A can be combined with.
Each pair says how the two meet (merged, side by side, or across a loop
closure), or is disabled with its reason; the line named first is the one
the dialog keeps by default. The dialog shows the name, colour, the kept
line and any choices; everything informational (what moves, what becomes a
loop closure, branches, depth, direction, credit) is folded under **Details**.
Two lines that meet at more than one end (a
loop of two) join at the end the station menu was opened on; the Sections panel
picks a merge first, then co-located ends, then a closure. The kept line (the
lower id by default; the dialog can swap) keeps its id, shot numbers, direction
and position; the other line is reversed first when the two meet end to end
or start to start. Every per-shot field travels, and every station reference
follows its station; the join station keeps one comment and one name (the
dialog asks when they differ).

Whatever stands in the way changes instead of blocking, and the dialog lists
it, for the line it shows as kept, before the user confirms:

- **A side moves.** Across a loop closure (or any other gap over 1 mm) the
  other line, and everything still merged to it, moves by the gap so the two
  ends meet. The kept side moves instead when a zero datum holds the other side
  and not the kept one, or, with no datum deciding, when the other side holds
  the station the Closures Computed View holds the survey at: moving it would
  shift the whole cave in Plan and Export. The dialog says which and why. Ends
  side by side move a side by their residual under 1 mm the same way, with
  everything merged to it, so merged stations stay one point and reopening
  the project moves nothing (#740). The dialog lists a move that small only
  when it turns a tie into a loop closure (a snap from the moving side, or,
  with zero datums on both sides, the moving line's merge), so the tie's
  listed reason has its cause beside it.
- **Ties become loop closures.** A merge or snapped (aligned) closure that
  would tie the combined line to itself, or keep the moving side tied to what
  stays, becomes an ordinary loop closure between the same two stations: the
  one nearest the moving line, so the least moves. The loop's misclosure
  moves there, and the closures the combine made or stretched are measured
  again. A snapped closure whose stations had come apart (more than 1 cm)
  counts as snapped when the combine brings them back together there, so it
  becomes an ordinary loop closure too rather than snapping again inside the
  combined line; Details name it as the loop closure left from a snap. So
  does a snapped closure the combine pulls apart, where the joined station
  keeps the other join station's position as shown, and Details say so
  (#736). When both sides are anchored by zero datums, the moving side's merge
  towards its own datum becomes a closure too (a closure may tie two anchored
  groups; a merge may not). Where two loop closures end up tying the same two
  stations, one stays: the first that the Closures Computed View adjusts (the
  first of them when it adjusts none), so a legacy (proportional or equal) or
  snapped closure there gives way to an ordinary one, such as the closure a
  torn merge becomes (#749). A snapped closure that gives way no longer
  averages its two stations' depths. Details lists each closure that gives
  way and what takes its place ("This closure is replaced:"); two closures
  the Closures Computed View treats alike keep the first without a word.
- **Ties across one shot are removed.** A tie that, combined, would be a loop
  closure joining the two ends of one shot (for example the merge at the far
  end of a one-shot passage merged to both join stations) would only be skipped
  by the Closures Computed View, so the combine removes it instead (#746).
  Before the confirm the dialog says how many connections it removes, outside
  the folded Details, and Details list each under "This connection is
  removed:" (or "These N connections are removed:"), each with why: as a loop
  closure it would join the two ends of one shot. A warning toast names them
  afterwards, each station with its line's name, and the one Undo brings
  them back. A closure that already joined the two ends of one shot is left
  alone.
- **Bearings convert.** The combined line has one north reference, the kept
  one's: the other line's stored azimuths turn by the difference, so every
  station stays where it is.
- The kept line's quality factor wins; the fixed side's manual offset on the
  join station wins (the dialog says how far the other moves), and a line
  merged at the other join station, on a join that is not merged, moves with
  the joined station (its side's shift plus the difference of the two
  offsets), so it stays merged to it (#755; the dialog lists it); shots of the
  other line that share an identity (uuid) with the kept one's get new ones;
  a depth gap between unmerged join stations is reported, whatever its size
  (the joined station takes the average); a line with no shots is absorbed
  at its one station.

Refused, with the reason in the menu row: both lines placed by their own zero
datum on one of their own stations (merge them first, so the later datum becomes
a GPS fix), the two join stations each holding a zero datum (the combined
station would hold two; remove one first), lines that
meet nowhere (merge two of their ends, or create a loop closure between them,
first), more than 99,999 shots, and a kept line that has used up its shot
numbers for the session. A line is never combined with itself. Every shot
keeps its own surveyors, explorers and date; the dialog summarises the combined
credit. Combining the two halves of a split gives the line back, with
whatever was merged at the split station still attached there. One undo step.
Rules and edge cases: [plans/2026-11-12_merge-split-sections.md](plans/2026-11-12_merge-split-sections.md).

## 8. Command System

### 8.1 Command Pattern

Persistent operations are variants of the `Command` enum, almost all executed
through `AppState::execute`: acquire the command lock, run the read-only edit
gate, capture undo data, apply the command, finalize the undo entry, push it,
log. Persistence to SQLite happens on save, not per command. Two variants are
labels rather than executable commands: `RepairIntegrity` and
`UpdateProjectSettings` name snapshot undo entries recorded by their dedicated
methods (`repair_project_integrity`, `update_project_settings`), and
`AppState::execute` rejects them with an error.

The variants cover project lifecycle (create/open/save/close/merge), lines,
shots, station merges, loop closures, zero datums, station comments, the move
operations, and a `Batch` wrapper that applies a list atomically. The
authoritative list - every variant with its fields - is
`src-tauri/crates/core/src/core/command.rs`; it grows with the feature set, so it is
deliberately not mirrored here.

### 8.2 UndoEntry Structure

Each entry in the undo/redo stack contains:

```rust
pub struct UndoEntry {
    /// The command that was executed
    pub command: Command,
    /// Data needed to undo this command
    pub undo_data: UndoData,
    /// Timestamp when the command was executed (None for legacy entries)
    pub timestamp: Option<DateTime<Utc>>,
    /// Serialized byte length of `command`, measured at push (None for restored/legacy entries)
    pub command_bytes: Option<usize>,
}
```

**Timestamp Handling:**
- New entries automatically receive `Utc::now()` timestamp on creation
- Timestamps persist across app restarts via the `executed_at` column in SQLite
- Legacy entries (from before timestamps were added) have `None` and display as "Unknown" in UI
- Format: ISO 8601 / RFC 3339 (e.g., `2024-01-12T14:32:15.123Z`)

### 8.3 UndoData Variants

Undo data is captured in two phases - before execution for "previous" state, after
execution for anything that exists only once IDs are assigned - and stored as an
`UndoData` variant: an inverse command, previous field values, an entity snapshot,
a merge / closure / datum / move record, or a whole-project snapshot pair for
batches.

`src-tauri/crates/core/src/core/undo.rs` defines the variants.
[undo-redo.md](undo-redo.md) is the reference for which command uses which
variant and what undo and redo each does with it.

### 8.4 Stack Exposure Commands

The undo/redo stacks are exposed to the frontend via Tauri IPC commands:

| Command | Description | Returns |
|---------|-------------|---------|
| `get_undo_stack` | Get full undo stack with preview data | `Vec<UndoHistoryItem>` |
| `get_redo_stack` | Get full redo stack with preview data | `Vec<UndoHistoryItem>` |
| `undo_to` | Undo multiple steps to index N | `CommandResult` |
| `redo_to` | Redo multiple steps to index N | `CommandResult` |

**UndoHistoryItem DTO:**
```typescript
interface UndoHistoryItem {
  index: number;           // Position in stack (0 = most recent)
  description: string;     // Human-readable command description
  timestamp: string | null; // ISO 8601 or null for legacy
  commandType: string;     // For icon mapping: "create_line", etc.
  previewData: {           // Diff preview (or null)
    changes: Array<{ field: string; oldValue: string; newValue: string }>;
    additionalChanges: number;
  } | null;
}
```

**Multi-step undo/redo:**
- `undo_to(N)` executes N+1 undo operations (indices 0 through N)
- Operations execute sequentially (not transactional)
- Partial completion on failure is acceptable

**Failure behavior:**
- On single `undo()`/`redo()` failure, the popped entry is pushed back to the original stack so no entry is lost
- On `undo_to(N)`/`redo_to(N)` failure, execution stops at the failing step; already-completed steps remain applied, and the failing entry is pushed back to its original stack
- `undo_to`/`redo_to` return an enhanced error message on partial failure: `"Undo failed at step {current} of {total}: {reason}"` (or `"Redo failed at step..."`)
- A `tracing::error!` is logged whenever a popped entry fails to apply, including command description, undo data variant name, and failure reason (no full snapshot JSON)
- Frontend shows an error toast with the failure reason (`result.error ?? 'Unknown error'`) using de-duplication keys `'undo-failed'` / `'redo-failed'`
- Frontend always calls `refreshProjectState()` on both success and failure (via `try/finally`) to sync UI with backend state

### 8.5 Command Execution

`AppState::execute` (`src-tauri/crates/core/src/core/state/commands.rs`) is the generic entry
point; a handful of commands with heavier bookkeeping have their own `execute_*`
methods alongside it, and `undo()`/`redo()`/`undo_to()`/`redo_to()` live in
`src-tauri/crates/core/src/core/state/undo.rs`. Lock ordering
(`command_lock` -> `undo_stack` -> `project`), the two-phase undo capture and the
batch rollback rules are documented in [undo-redo.md](undo-redo.md).

### 8.6 Transient Operations

These do NOT go through Command Executor:

- Pan/zoom map
- Hover highlights
- Drag preview (before release)
- UI panel resize/collapse
- Tooltip display

Managed entirely in Zustand store on frontend.

---

## 9. Configuration

All settings live in a single TOML file. Ground truth for the keys is the `Config`
struct in `src-tauri/crates/core/src/config/types.rs`, with the shipped values in
`src-tauri/crates/core/src/config/defaults.rs`; [configuration.md](configuration.md) is the
maintained reference for what each block does.

### 9.1 File Location

| Platform | Path |
|----------|------|
| Linux | `~/.config/com.stalagmight.desktop/config.toml` |
| macOS | `~/Library/Application Support/com.stalagmight.desktop/config.toml` |
| Windows | `%APPDATA%\com.stalagmight.desktop\config.toml` |

The directory is resolved through Tauri's path API (`src-tauri/crates/models/src/app_dirs.rs`)
rather than the `directories` crate, which returns nothing on Android.

### 9.2 Loading

`Config::load` reads that file if it exists and otherwise uses the compiled-in
defaults. There is no system-wide layer to merge: a file that is present is parsed
whole, then put through legacy-key migration and normalization. Unknown and
removed keys are ignored (no `deny_unknown_fields`), and the optional blocks
(`closure_adjustment`, `panels`, `cache`, `debug`, `global_view`, `appearance`,
`import`, `placement_suggestion`, `syntactic_highlighting`, `gas`, `speleodb`)
default when absent - but the core tables (`general`, `units`, `display`,
`section_colors`, `map`, `validation`, `export`, `logging`, `shortcuts`) and
their non-defaulted fields are required. (`section_colors` is `line_colors` in
code: `config.toml` keeps the pre-rename names, translated by
`src-tauri/crates/core/src/config/toml_names.rs`, so an older build still
parses the file.) A file that fails to parse is
discarded whole at startup (`Config::load().unwrap_or_default()` in
`AppState::new`) - the app runs on the full compiled-in defaults, not a per-key
merge. The Reload Config path instead surfaces the parse error and keeps the
current in-memory config.

---

## 10. Settings Dialog

### 10.1 Dialog Structure

Modal dialog with sidebar navigation, accessible via the Settings button in the Global View header or the Cmd/Ctrl+, keyboard shortcut.

**Layout:**
- Fixed size dialog: `min(800px, 95vw)` wide × `clamp(500px, 80vh, 800px)` tall
- Dimensions remain constant across category switches (no height/width shifts)
- DialogWrapper `fixedSize` prop overrides `size` and `maxHeight` when provided
- Left sidebar (150px) with category navigation
- Right content area with scrollable settings (overflow-auto)
- Scroll position resets to top on category switch
- Short-content categories show content top-aligned with empty space below
- Inner container minimum width: `min(500px, 100%)`
- Footer with Save/Cancel buttons

### 10.2 Categories

The sidebar displays 12 settings categories, with a visual separator before the Advanced category:

| Category | Contents |
|----------|----------|
| General | Language/number/date formats, auto-save toggle/interval, recent projects limit, undo history/persist limits, shortcut feedback preference, splash screen and onboarding, telemetry opt-in (telemetry-enabled builds), "Update automatically" and the update track (Stable/Beta) |
| Appearance | Theme mode (dark/light/auto), day and night color theme selection with preview cards, 10 color themes |
| Units | Length/depth/angle units, decimal precision |
| Gas planning | Swim speed, jump RMV multiplier, whether reference legs count toward gas |
| Display | Grid/font colors, grid spacing, font size, measure-path color, line color palette |
| Map | Satellite provider/toggle/GPS origin, zoom settings, pan/zoom controls, station/shot appearance, snap settings, animation settings |
| Validation | Shot length min/max warnings, depth change warnings, azimuth deviation warnings, in/out difference thresholds |
| Export | Default image format (PNG/SVG/PDF), default DPI, default layer includes (satellite, grid, labels) |
| Shortcuts | Searchable shortcut editor with categories (Tools, Edit, View, Panels), primary/secondary binding slots, conflict detection, reserved shortcuts display |
| Cache | Tile cache statistics, clear cache button, cache enable toggle, max size, TTL days |
| SpeleoDB | Account Connect / Disconnect, API endpoint, token authentication, automatic transport policy, startup and manual refresh, "Purge all cached files" (keeps copies with un-uploaded changes unless their deletion is confirmed twice) |
| Advanced | Debug settings (hitboxes, hit detection logging), logging level/max size, placement suggestion heuristic constants, panel dimensions, reload config from disk |

### 10.3 Navigation

#### Keyboard Navigation

The sidebar implements the WAI-ARIA vertical tab pattern with manual activation:

| Key | Action |
|-----|--------|
| Arrow Up/Down | Move focus between categories (does not activate) |
| Home | Focus first category |
| End | Focus last category |
| Enter / Space | Activate the focused category |
| Escape | Close dialog (or close nested confirmation dialog if open) |

Focus is indicated with a ring outline. Active category has highlighted background.

#### Mouse Interaction

- Click any category to activate it immediately
- Focus follows click (roving tabindex pattern)

### 10.4 Behavior

- **Deferred save**: Changes are stored locally until Save button is clicked
- **Dirty state tracking**: Tracks which fields have changed from original values
- **Save button disabled** when no changes or validation errors exist
- **Reset to Defaults**: Per-category reset button in content header; shows confirmation dialog
- **Modal**: Blocks background interaction; backdrop does not close dialog
- **Unsaved changes dialog**: When closing with unsaved changes, shows confirmation with Save/Discard/Cancel options

#### Validation

- Each category validates its fields on change
- Invalid fields prevent saving (Save button disabled)
- Categories with validation errors show a red error dot in the sidebar

### 10.5 Accessibility

#### ARIA Attributes

```html
<!-- Sidebar -->
<div role="tablist" aria-orientation="vertical" aria-label="Settings categories">
  <div role="tab" aria-selected="true|false" aria-controls="panel-id" tabindex="0|-1">
    Category Name
  </div>
</div>

<!-- Content -->
<div role="tabpanel" id="panel-id" aria-labelledby="tab-id">
  <!-- Category settings -->
</div>
```

#### Focus Management

- Tab cycles within dialog (focus trap)
- Initial focus on first category tab
- Roving tabindex on category tabs (only focused tab has tabindex=0)
- Return focus to trigger element on close

#### Screen Reader Support

- Live regions announce validation errors
- Error dots have `aria-label="Has validation errors"`
- All form controls have appropriate labels

### 10.6 Confirmation Dialogs

Two nested confirmation dialogs may appear:

#### Unsaved Changes Dialog

Shown when closing the settings dialog with unsaved changes:
- Title: "Unsaved Changes"
- Message: "You have unsaved changes. What would you like to do?"
- Buttons: Cancel (stay), Discard Changes (close without saving), Save (save and close)
- Role: `alertdialog` with `aria-modal="true"`

#### Reset to Defaults Dialog

Shown when clicking "Reset to Defaults" button:
- Title: "Reset to Defaults"
- Message: "Reset all [Category] settings to their default values?"
- Note: "This will only affect the current category. Changes are not saved until you click Save."
- Buttons: Cancel, Reset (danger variant)
- Role: `alertdialog` with `aria-modal="true"`

Both dialogs:
- Close on Escape key (captured before main dialog)
- Have semi-transparent backdrop
- Are centered in viewport

---

## 11. Accessibility

### 11.1 Focus Management

#### Modal Focus Trap
- Tab cycles within modal dialogs only
- Shift+Tab at first element goes to last
- Tab at last element goes to first
- Implemented via `useFocusTrap` hook with `dialogStack` for nested dialog support

#### Initial Focus Heuristic
On dialog open, focus is placed on the first matching element in priority order:
1. `[data-autofocus]` - explicit autofocus target
2. `input` / `select` / `textarea` - first form field
3. `[data-primary-action]` - primary action button
4. First non-close `button` - any button that is not the close/X button
5. Close X button - fallback to the dialog's close button

Confirmation dialogs: focus the primary action button (marked with `[data-primary-action]`).

#### Return Focus
- On dialog close, return focus to the element that triggered the dialog
- If the trigger element no longer exists in the DOM, focus is not forcibly moved (browser default applies)

#### Dialog Portal Architecture
- All dialogs render via a `DialogWrapper` component
- `DialogWrapper` handles focus trapping, initial focus placement, return focus, and Escape key
- Nested dialogs are managed via a `dialogStack` utility (`src/utils/dialogStack.ts`) that tracks open dialog layers
- Only the topmost dialog in the stack receives keyboard events

#### CSS Custom Properties for Focus Ring
```css
:root {
  --cc-focus-ring: #4ecdc4;
  --cc-focus-ring-offset: 2px;
  --cc-focus-ring-shadow: rgba(78, 205, 196, 0.5);
}
```
All `:focus-visible` outlines use `var(--cc-focus-ring)` and `var(--cc-focus-ring-offset)` instead of hardcoded values. The focus ring box-shadow uses `var(--cc-focus-ring-shadow)`. See also Section 11.4.

### 11.2 Keyboard Navigation

#### Table Navigation
| Key | Action |
|-----|--------|
| Arrow Up/Down | Move between rows |
| Arrow Left/Right | Move between cells |
| Home/End | First/last cell in the row |
| Ctrl+Home/End | First/last cell of the grid |
| Enter | Move down one cell (Shift+Enter up); cycles the active cell within a multi-cell selection |
| Space | Activate a button cell; otherwise start editing |
| F2 | Enter edit mode |

#### Tool Shortcuts
All shortcuts are configurable and can be cleared:
- S: Select, P: Pan, Z: Zoom, M: Merge, R: Measure
- T: Move Station, G: Move Shot, V: Move Segment
- C: Comment, Shift+C: Move Comment

P and Z act only when `map.controls.show_navigation_tools` is on; the Pan and
Zoom tools are hidden by default, and while they are hidden those two keys do
nothing at all.

### 11.3 Screen Reader Support

#### ARIA Live Regions
```typescript
<LiveRegion
  politeness="polite"  // or "assertive"
  clearAfter={5000}    // ms
/>
```

#### Announcement Events (Configurable)
- Tool selection changes
- Station selection changes
- Save status (saved, saving, error)
- Validation results
- Dialog open/close
- Error messages (assertive)
- Undo/redo dropdown refresh (announces remaining action count, e.g., "3 actions remaining.", or "No more actions remaining." when the stack empties)

### 11.4 Visual Accessibility

#### Focus Indicators
- Clear outline on all focusable elements
- Custom focus ring: `--cc-focus-ring`, `--cc-focus-ring-offset`, `--cc-focus-ring-shadow`
- Never suppressed with `outline: none`

#### Contrast Ratios (WCAG AA)
- Normal text: 4.5:1 minimum
- Large text: 3:1 minimum
- UI components: 3:1 minimum

### 11.5 Map Accessibility

- Map not keyboard navigable (too complex)
- Users use data table for keyboard access to survey data

---

## 12. Internationalization

### 12.1 Infrastructure (v1.0 Scope)

Library: react-i18next

```typescript
// src/i18n/index.ts
import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import LanguageDetector from 'i18next-browser-languagedetector';
import en from './locales/en.json';

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources: { en: { translation: en } },
    fallbackLng: 'en',
    interpolation: { escapeValue: false },
  });
```

### 12.2 Namespace Structure

```json
{
  "common": {
    "actions": { "save": "Save", "cancel": "Cancel" },
    "labels": { "name": "Name" }
  },
  "dialogs": {
    "import": { "title": "Import DMP File" },
    "export": { "title": "Export Project" },
    "settings": { "title": "Settings" }
  },
  "tools": { "select": "Select (S)", "pan": "Pan (P)" },
  "validation": { "required": "This field is required" },
  "status": { "saved": "Project saved", "saving": "Saving..." }
}
```

### 12.3 Number/Date Formatting

```typescript
// Locale-aware formatting
export function formatNumber(value: number, locale: string): string {
  return new Intl.NumberFormat(locale).format(value);
}

export function formatDate(date: Date, locale: string): string {
  return new Intl.DateTimeFormat(locale).format(date);
}
```

- Storage: Always ISO 8601
- Display: Localized based on user preference
- Units: display unit comes from the Units settings category (meters or feet), not from the locale; stored values are always metric

### 12.4 Missing Key Handling

- Development: Show key name (e.g., `[common.missing]`)
- Production: Show fallback or empty string
- Log missing keys for debugging

---

## 13. Performance

### 13.1 Targets

| Metric | Target |
|--------|--------|
| Startup time | < 3 seconds |
| Map FPS | 30 minimum |
| Shots per project | 10,000 |
| Memory usage | < 500 MB |

### 13.2 Map Rendering Optimization

#### Level of Detail (LOD)
| Zoom Level | Detail |
|------------|--------|
| < 10 | No labels (line names appear at z10, station/shot/comment labels at z14) |
| 10-14 | Normal paths, line labels only |
| 14-17 | Full detail, all labels |
| > 17 | Maximum detail |

#### Zoom-Dependent Thinning

Only labels are thinned by zoom: each label carries a baked reveal zoom that folds
the collision declutter together with a nested decimation band
(`labelReveal.ts` / `labelDecimation.ts`), and the shader draws a label only at or
above that zoom - see [map-renderer.md](map-renderer.md). Where labels compete for
the same space, a real-line label wins over a label that belongs only to jump or
reference shots, and a jump label wins over a reference one. Station markers
and shot lines follow the same order: where they coincide or overlap (a line of jump or
reference shots tied in at a real station, or surveyed along the real line),
the real-line marker or line is drawn on top and takes the hover and the click,
then jump, then reference. The selection's line (the selected line, or
the line of the selected station or shot) is drawn above all of them and
takes the hover and the click where it overlaps them; labels keep the rank
order. Shot
geometry itself is not simplified: no path simplification, no station merging,
no zoom-based hiding.

### 13.3 Data Table Virtualization

Library: TanStack Virtual, always on - there is no row-count threshold. The shot
grid virtualizes its rows at `ROW_HEIGHT` = 33 px (defined in
`src/components/common/SpreadsheetGrid/types.ts`) with `overscan: 8`
(`src/components/common/SpreadsheetGrid/SpreadsheetGrid.tsx`).

### 13.4 Auto-Save Debouncing

- Debounce after the last change: `general.auto_save_interval_seconds`, default 30
- Repeated changes reset the timer; a manual Ctrl+S cancels it
- Frontend-only (`src/hooks/useAutoSave.ts`) - the backend has no per-command save. See [saving.md](saving.md)

### 13.5 Tile Caching

Satellite tiles are cached in SQLite strictly under the provider's own
`Cache-Control`. Imagery is never stored for offline use or for any other
purpose: a tile is kept only where the response grants it, for no longer than it
grants, and is revalidated with the provider's validator once that runs out. A
stored copy is served only as a last resort, when the provider cannot be reached
at all (`stale-if-error`, RFC 5861):

```
~/.cache/com.stalagmight.desktop/tile_cache.db

-- Schema
CREATE TABLE tiles (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    z INTEGER NOT NULL,
    y INTEGER NOT NULL,
    x INTEGER NOT NULL,
    data BLOB NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'image/jpeg',
    size_bytes INTEGER NOT NULL,
    fetched_at INTEGER NOT NULL,
    accessed_at INTEGER NOT NULL,
    expires_at INTEGER,
    etag TEXT,
    no_cache INTEGER NOT NULL DEFAULT 0,
    UNIQUE(provider, z, y, x)
);
```

**Cache Flow:**
1. MapLibre requests tile via `stalagmight-tiles://esri/{z}/{y}/{x}` protocol
2. Frontend calls `invoke('fetch_tile', {provider, z, y, x})`
3. Backend checks SQLite cache → returns if fresh
4. On cache miss: fetch from ESRI servers → store in cache → return
5. LRU eviction when cache exceeds max size

**Configuration** (in `[cache]` section):
- `tile_cache_enabled`: Enable/disable caching (default: true)
- `tile_cache_max_mb`: Maximum cache size (default: 500 MB)
- `tile_ttl_days`: Days before tiles considered stale (default: 30)

**IPC Commands:**
- `get_cache_stats`: Returns tile count, size, per-provider breakdown
- `clear_tile_cache`: Clears all or provider-specific tiles
- `fetch_tile`: Fetches a tile (cache-first, network fallback)
- `get_tile_providers`: Returns available tile providers with metadata (name, display_label, attribution, tile_size, min/max zoom, available flag)

---

## 14. Error Handling

### 14.1 Error Boundaries (Per-Panel)

```tsx
<ErrorBoundary panelName="Map">
  <Panel />
</ErrorBoundary>
```

Wrap each major panel:
- Map panel
- Sections panel
- Detail panel
- Data table panel

### 14.2 Recovery Options

| Option | Description |
|--------|-------------|
| Try Again | Clear the boundary's error state and re-render the panel |

### 14.3 Network Error Handling

```typescript
function useNetworkStatus() {
  const [isOnline, setIsOnline] = useState(navigator.onLine);
  // Listen for online/offline events
}
```

- Detect offline state
- Show offline indicator in status bar
- Serve an already-cached tile when the provider cannot be reached
  (`stale-if-error`; nothing is pre-fetched or stored for offline use)
- Auto-recovery when back online

### 14.4 File Corruption Recovery

#### Integrity Checks
- Quick check on open: `PRAGMA quick_check`
- Full check on-demand: `PRAGMA integrity_check`

#### Recovery Options (Always Ask User)
1. Attempt Repair
2. Restore from Backup
3. Export Salvageable Data
4. Cancel

#### Restore from Backup
The dialog offers the files beside the project named after it plus a suffix
that contains `.backup` or ends in `.tmp` (`cave.stalx.backup`,
`cave.stalx.backup.1`, `cave.stalx.tmp`), and only those that start with
SQLite's header. It never offers the project itself (nor, on Unix, a hard
link to it), its
`.pre-restore` copy, SQLite's `-journal`/`-wal`/`-shm` files, or an empty file
(#720).

A restore never writes into a file in place, and a restore that is refused or
fails changes nothing:
- Refused while the project, its `<project>.stalx.pre-restore` or the backup is
  open in the app, or while a project is being opened; when the backup is the
  project itself; when the backup does not open as a project (empty, not a
  project, damaged too); when an armed journal or a WAL with frames sits beside
  the backup, the project or the `.pre-restore` copy; when one of those files,
  or such a sidecar, is not a regular file; and when the project or the backup
  was saved by a newer version.
- Otherwise the backup is copied beside the project and opened as a project
  first. The previous `.pre-restore` copy is set aside, a copy of the project
  replaces it, and the backup copy replaces the project, each by rename. If the
  last step fails, the previous `.pre-restore` copy goes back. On Unix the
  restored file keeps the project's permission bits.
- The dialog never offers the `.pre-restore` copy. Restoring it directly (over
  IPC) swaps the two files back.

#### Backup Before Repair
Always create backup before attempting repair:
```rust
fn backup_before_repair(path: &Path) -> Result<PathBuf> {
    let backup = path.with_extension("stalx.backup");
    fs::copy(path, &backup)?;
    Ok(backup)
}
```

### 14.5 Validation Error Recovery

- Per-warning: a "Resolve" button opens the resolution flow, which offers per-code
  fixes plus "Ignore (keep value, don't warn again)" for advisory warnings (errors
  are never ignorable); a resolved entry offers "Undo" to mark it unresolved
- "Resolve all" walks the unresolved warnings one at a time and ends in a
  resolved/skipped/remaining summary; resolved entries are hidden until the
  "Show resolved" toggle reveals them
- Auto-clear when field corrected
- Cleared errors stay cleared (don't re-validate automatically)
- Closure entries (#737): a loop closure that joins the two ends of one shot
  (`CLOSURE_ACROSS_ONE_SHOT`, one per closure, on that shot) is an error and
  resolves with "Remove this loop closure" (disabled while editing is blocked) or
  "Go to closure station", never Ignore. A part of the survey the Closures Computed
  View could not adjust (`CLOSURE_ADJUSTMENT_FAILED`) is a warning: it is listed
  once the computed view, Plan or Export has solved the current version and goes
  with the next edit, so it must not gate like an error; it resolves with "Go to
  station" or Ignore

---

## 15. Testing

### 15.1 Coverage Target

**80% code coverage** for both Rust and TypeScript.

### 15.2 Test Types

| Type | Framework | Purpose |
|------|-----------|---------|
| Rust unit + integration tests | cargo test | Backend logic |
| Unit / component tests | vitest | Frontend logic and components |
| Playwright E2E | Playwright | Frontend with mocked IPC |
| Agent harness | webdriverio + tauri-driver | Full app integration (`e2e/agent/` journeys + scenarios), on demand |

### 15.3 Test Requirements per Feature

Every feature requires:
- 3-10 specific test cases
- Playwright coverage, tagged `@smoke` when it belongs in the per-push tier
- Rust tests for backend logic

### 15.4 Test Data Identifiers

All testable elements must have `data-testid`:

| Element | Convention |
|---------|------------|
| Views | `<view>-view` |
| Buttons | `<action>-button` |
| Dialogs | `<name>-dialog` |
| Form inputs | `<field>-input` |

### 15.5 ARIA Requirements for Testing

All dialogs must have:
```tsx
<div
  role="dialog"
  aria-modal="true"
  aria-labelledby="dialog-title-id"
>
```

---

## 16. Logging

### 16.1 Log Levels

| Level | Usage | Example |
|-------|-------|---------|
| **ERROR** | Operation failed, requires attention | "Failed to save project: permission denied" |
| **WARN** | Suspicious but recoverable | "Shot length 0.05m below threshold" |
| **INFO** | Normal operations, audit trail | "Command: CreateLine { name: 'Main' }" |
| **DEBUG** | Detailed execution info | "Generated undo data: StateDelta with 5 entities" |
| **TRACE** | Very verbose, performance debugging | "Rendering 45000 line segments" |

### 16.2 Log File Management

- **Location**: see the Logging section of [configuration.md](configuration.md)
  (not the bundle-identifier dir - logging initialises before Tauri's path API)
- **Naming**: `stalagmight.log.YYYY-MM-DD`, rotated daily (not per session)
- **Level and size cap**: the `[logging]` config settings

### 16.3 Log Format

Structured JSON for machine parsing:

```json
{"timestamp":"2024-01-12T14:32:15.123456Z","level":"INFO","fields":{"message":"Command executed","command":"CreateLine","line_name":"Main Line","shots":24},"target":"stalagmight::commands"}
{"timestamp":"2024-01-12T14:32:15.125789Z","level":"DEBUG","fields":{"message":"Undo data generated","type":"InverseCommand","inverse":"DeleteLine"},"target":"stalagmight::commands"}
{"timestamp":"2024-01-12T14:32:15.130421Z","level":"INFO","fields":{"message":"Project saved","path":"/home/user/caves/sistema.stalx","size_kb":245},"target":"stalagmight::persistence"}
```

The keys and their order are tracing-subscriber's JSON formatter defaults:
`timestamp`, `level`, a nested `fields` object holding `message` plus the event's
own fields, then `target`; events inside a span also carry `span` / `spans`.

### 16.4 Instrumentation Requirements

**All code must include appropriate logging:**

```rust
use tracing::{info, warn, error, debug, trace, instrument};

#[instrument(skip(state), fields(line_id))]
pub fn create_line(state: &mut State, cmd: CreateLineCommand) -> Result<Line> {
    info!(name = ?cmd.name, shots = cmd.shots.len(), "Creating line");

    // Validation
    if cmd.shots.is_empty() {
        warn!("Creating line with no shots");
    }

    for (i, shot) in cmd.shots.iter().enumerate() {
        if shot.length < config.validation.warn_shot_length_min {
            warn!(shot_index = i, length = shot.length, "Shot length below threshold");
        }
        trace!(shot_index = i, shot = ?shot, "Processing shot");
    }

    // Execution
    let line = state.add_line(cmd)?;
    debug!(line_id = line.id, "Line added to state");

    // Error handling
    if let Err(e) = state.validate_integrity() {
        error!(error = ?e, "State integrity check failed after line creation");
        return Err(e);
    }

    info!(line_id = line.id, "Line created successfully");
    Ok(line)
}
```

---

## 17. File Formats

### 17.1 Project File (`.stalx`)

A project is a single SQLite database carrying the `.stalx` extension. Saving
writes a fresh `<name>.stalx.tmp` and atomically renames it over the target, so an
interrupted save never leaves a half-written project - see [saving.md](saving.md).

The schema is versioned (`SCHEMA_VERSION` in
`src-tauri/crates/persistence/src/persistence/sqlite.rs`) and migrated forward when an older file is
opened. Alongside the survey tables (project, `sections` with one row per line,
shots, station merges, zero datums, loop closures, `section_offsets` and
`station_offsets`; the SQL keeps the pre-rename names, see
[Persisted names](saving.md#persisted-names-lines-are-sections-on-disk)) it holds per-project and
display settings, the persisted undo and redo stacks, resolved warnings, import
hashes, station comments and names, the SpeleoDB link row, and the Unwind chain
position in `speleo_undo_chain`. Five markers are worth knowing before touching migration
code:

- `UNDO_FORMAT_SAFE_VERSION` - files written before it get their persisted
  undo/redo stacks dropped on open, because pre-v7 `ShotUpdates` JSON cannot
  distinguish "field untouched" from "clear this field".
- v12 normalises stored pitch to the +down convention
  (`migrate_pitch_to_down_convention`); see [inclination.md](inclination.md).
- v15 drops the `collab_state` table, the storage for the since-removed Ariane
  collaboration baseline, and also clears the persisted undo/redo stacks -
  unconditionally, for every pre-v15 file, via `drop_legacy_undo_history`, which
  loops both `undo_stack` and `redo_stack`. `UNDO_FORMAT_SAFE_VERSION` is 7, so
  a v14 file reaches the migration with its history intact, and those payloads
  can name the removed `apply_collab_changes` command, which no longer
  deserializes; the loader skips a bad entry without truncating, so the stack
  would otherwise load with a hole in the middle. v13 drops the stacks the same
  way, for the legacy `is_reference` boolean.
- v16 adds `speleo_link.undo_mode` (whether this project's undo history rides to
  SpeleoDB) and two local-only tables: `speleo_undo_chain` (how far back through
  the server's revisions this machine has unwound) and `speleo_undo_watermark`
  (which loaded entries a chain step inherited, so the next push does not ship
  them again); see [undo-redo.md](undo-redo.md). Both `speleo_undo_chain` and
  `speleo_undo_watermark` are created by `migrate_schema` in `schema.rs`
  (CREATE TABLE IF NOT EXISTS), so a legacy file gains them on open;
  `save_speleo_undo_watermark` in `persistence/sqlite/settings.rs` also creates
  the watermark table defensively, and the read side probes `sqlite_master`
  first.
- v17 gives `LineUpdates`' (then `SectionUpdates`) `name` / `surveyor` / `date` the `double_option` +
  `skip_serializing_if` pair `ShotUpdates` has had since v7, so an undo can once
  again restore a line field that was null before the edit (issue #413
  finding 5). It is the v7 hazard one struct over - a pre-v17 writer serialized
  every untouched nullable field as an explicit `null`, which now reads as "clear
  this field" - so the migration drops the persisted stacks through
  `drop_legacy_undo_history` exactly as v13, v15 and v16 do.
  `UNDO_FORMAT_SAFE_VERSION` stays at 7: it is the floor for the pre-v7 purge
  alone, and bumping it would make the v13 and v15 arms' own tests pass with
  those arms removed.

`station_names` holds per-station names in the same positional shape as
`station_comments`. It was added at v18 WITHOUT a schema version bump, on
purpose: SpeleoDB turns an uploaded `.stalx` into a map with OpenSpeleo's
Python package `stalagmight_lib` (pytool_stalagmight_lib, through
`openspeleo_lib`; not the app's Rust lib crate of the same name). Before its
0.0.2 it refuses any file not at exactly v18 (0.0.2 also takes 19, and a server
may still run 0.0.1), and it reads only the tables it knows, so a new table is
invisible to it while a bump would lock it out (verified version and details in
`persistence/sqlite/station_texts.rs`). The cost is that a build without the table drops
the names on its next save. A v18 file from before the table takes the version
fast path, so the read side treats a missing table as empty and `save_project`
creates it inside its transaction; `station_names_tests.rs` in
`persistence/sqlite/` pins both, and that no table the outside reader reads
changes when a project gains names. Every station's name is written (names are
required, 4.2).

`station_ids` holds every station's Station ID in the same positional shape
(`section_id, station_index, station_id`), and the `project_settings` row
`next_station_id` the allocation counter - both without a schema bump, for the
same reason (see [saving.md](saving.md#station-names-and-station-ids)).

The dive plan lives beside the project in its own `.plnx` SQLite sidecar
(`src-tauri/crates/persistence/src/persistence/plnx.rs`), not in the `.stalx`.

Table definitions are authoritative in `src-tauri/crates/persistence/src/persistence/sqlite/` and are
not reproduced here.

### 17.2 DMP File Import

Rewrite `pytool_mnemo_lib` parser in Rust. Key structures:

```rust
pub struct DmpFile {
    pub version: u8,  // 2-5
    pub lines: Vec<DmpSection>,  // one per MNemo section; each imports as a line
}

pub struct DmpSection {
    pub version: u8,              // DMP version for this section
    pub name: String,             // 3-character identifier
    pub direction: DmpDirection,  // In or Out
    pub date: NaiveDateTime,
    pub shots: Vec<DmpShot>,
    pub content_hash: String,     // SHA256 over name + direction + standard shot measurements
}

pub struct DmpShot {
    pub shot_type: DmpShotType,  // CsA, CsB, Standard, EndOfSurvey
    pub heading_in: f64,
    pub heading_out: f64,
    pub length: f64,
    pub depth_in: f64,
    pub depth_out: f64,
    pub pitch_in: f64,
    pub pitch_out: f64,
    pub left: f64,
    pub right: f64,
    pub up: f64,
    pub down: f64,
    pub temperature: f64,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub marker_index: u8,
    pub comment: Option<String>,  // not parsed from DMP; defaults to None
}
```

The file carries no station numbers: MNemo numbers a section's (line's) start 0 and the
end of its N-th shot N, so the importer derives each station's number from the
shot's position (counting only the shots it imports - an assumption not yet
checked against a device) and records it as the station's name; see
[dmp-import.md](dmp-import.md#station-names).

### 17.3 Export Formats

**Map Image (PNG/SVG/PDF):**
- Render current map view with selected options
- SVG preserves vector quality
- PDF includes metadata
- Station labels are the station IDs or the station names (one choice, like
  the Display menu; unnamed stations get no name label, and a merged point is
  named by its first named station). If a caller sends both options, the IDs
  are drawn. Names are stripped of control and bidi characters and capped at
  24 characters.

#### DPI Scaling

The export system uses a scale factor to convert pixel values to SVG viewBox units:

```
viewBox_size = pixel_size / scale
```

where `scale = TARGET_SIZE / max(width, height)` with `TARGET_SIZE = 800px`.

Higher DPI settings produce proportionally larger raster images:
- 72 DPI: Base size (800px)
- 150 DPI: ~1667px
- 300 DPI: ~3333px
- 600 DPI: ~6667px

PNG exports are limited to 16384px maximum dimension to prevent out-of-memory errors.

#### Element Size Specifications

All SVG elements use pixel-stable sizing via the `px()` helper function. This ensures consistent visual appearance regardless of survey extent.

**Virtual legs are not drawn.** Neither Jumps nor References appear in an image
export, and they never have. This is not an oversight the per-kind opacities of
the on-screen map should be copied into: the exporter's whole layout model is
built on virtual shots being the *segmentation boundary* - a segment is a
maximal chain of non-virtual shots, and a tie is what links two segments through
a run of virtual ones. Drawing them would mean there is no boundary left, so
ties, anchors and the branching-threshold classification would all need
rethinking.

One asymmetry to be aware of, new with #27: a Jump *is* swum, so it counts in
the penetration distance the export labels, while its line is still not drawn.
An exported map can therefore report a distance routed through geometry it does
not show. Fixing that properly means teaching the exporter to draw jumps while
keeping references as the boundary - tracked separately, not attempted here.

| Element | Target Pixels | Notes |
|---------|---------------|-------|
| Shot stroke-width (thin) | 1px | `THIN_PX` - default segment stroke |
| Shot stroke-width (thick) | 2.5px | `THICK_PX` - segments whose branching factor reaches `branching_threshold` |
| Junction trim | 5px | `JUNCTION_TRIM_PX` (2 x thick) - gap where a tied segment meets its parent |
| Zero-datum star radius | 7px / 3px | `STAR_OUTER_PX` / `STAR_INNER_PX`; the star is the only station marker |
| Label font-size | 11px | `PEN_LABEL_PX` - shared by every label group (penetration, station, line, shot data, comments) |
| Label halo stroke-width | 3px | `PEN_LABEL_HALO_PX` - white outline so text stays legible over a line |
| Label offset | 9px | `PEN_LABEL_OFFSET_PX` - anchor point to label centre |

Non-zero-datum stations get no marker, and the exporter draws no grid and no
depth legend.

**Survey Data (CSV):**
```csv
line,from,to,from_id,to_id,length,azimuth_in,azimuth_out,depth_in,depth_out,left,right,up,down,kind,comment
Main Line,0,1,0,1,3.2,45.0,46.2,5.1,5.8,1.2,0.8,2.1,1.5,Permanent,
Main Line,1,2,1,2,4.1,52.3,53.0,5.8,6.2,1.0,1.1,1.8,1.2,Permanent,
```

A shot is identified by its line's name and its two stations' names (`from`,
`to`) and Station IDs (`from_id`, `to_id`); the internal line and shot ids are
not exported. Text cells (line, station names, comment) are formula-neutralised
and quoted (`export/csv.rs` `escape_csv`).

**Survey Data (JSON):**
```json
{
  "project": {
    "name": "Sistema Norte",
    "exported_at": "2024-01-12T14:32:15Z"
  },
  "lines": [
    {
      "name": "Main Line",
      "surveyor": "J. Smith",
      "date": "2024-01-10",
      "section": "Upstream",
      "shots": [
        {
          "from": "0",
          "to": "1",
          "from_id": 0,
          "to_id": 1,
          "length": 3.2,
          "azimuth_in": 45.0,
          "azimuth_out": 46.2,
          "depth_in": 5.1,
          "depth_out": 5.8,
          "lrud": [1.2, 0.8, 2.1, 1.5],
          "kind": "Normal"
        }
      ]
    }
  ]
}
```

---

## 18. Technical Stack

### 18.1 Overview

| Layer | Technology |
|-------|------------|
| **Runtime** | Tauri 2.x |
| **Backend** | Rust (stable) |
| **Frontend** | React 18+ with TypeScript (strict mode) |
| **State** | Zustand |
| **Map Rendering** | MapLibre GL JS 5.x, with a custom WebGL layer for survey geometry |
| **Virtualization** | TanStack Virtual (data tables) |
| **i18n** | react-i18next |
| **Database** | SQLite (via rusqlite) |
| **Logging** | tracing + tracing-subscriber |
| **Build** | pnpm + Vite + Tauri CLI |
| **Testing** | cargo test, vitest, Playwright; the agent harness for full-app runs |

### 18.2 Dependencies and Build Configuration

Exact versions drift with every upgrade, so they are not restated here:
`package.json` for the frontend, `src-tauri/Cargo.toml` for the backend. The
pnpm version is pinned by `packageManager` in `package.json` (corepack reads it
locally and in CI). `.nvmrc` pins node for CI and for developers whose version
manager honors it; locally the `engines` range is advisory - a newer node warns
but does not block.

TypeScript runs in strict mode with `noUncheckedIndexedAccess` and
`exactOptionalPropertyTypes` on top of it; `tsconfig.json` is the source of truth
and `pnpm tsc --noEmit` is a CI gate.

### 18.3 Project Structure

See [project-structure.md](project-structure.md) for the directory tree.

---

## 19. Beyond the Current Scope

There are no versioned roadmap buckets. Queued and in-flight work lives in the
GitHub issue tracker - currently slate-photo OCR import (#274), the remaining
release-pipeline work (#325), and continuing SpeleoDB work. [roadmap.md](roadmap.md) is the
historical record of the v1.0 phases, all of which shipped (one caveat: GPS
entry validates but does not convert MGRS and What3words - see 2.7).

Regularly asked about, and not currently being worked on: 3D visualization, a web
version, MNemo device communication beyond reading over USB, LRUD passage-wall rendering,
real-time collaborative editing (SpeleoDB's flow is single-writer file hand-off,
not live co-editing), and floating/dockable panels.

---

## Appendix A: Keyboard Shortcuts

| Action | Shortcut |
|--------|----------|
| Select Tool | S |
| Pan Tool | P |
| Zoom Tool | Z |
| Merge Tool | M |
| Measure Tool | R |
| Move Station Tool | T |
| Move Shot Tool | G |
| Move Segment Tool | V |
| Comment Tool | C |
| Move Comment Tool | Shift+C |
| Undo | Ctrl+Z |
| Redo | Ctrl+Y / Ctrl+Shift+Z |
| New | Ctrl+N |
| Open | Ctrl+O |
| Save | Ctrl+S |
| Save As | Ctrl+Shift+S |
| Import | Ctrl+I (configurable) |
| Export | Ctrl+E (configurable) |
| Quit | Ctrl+Q |
| Delete Selected | Delete / Backspace |
| Select All | Ctrl+A |
| Deselect | Escape |
| Zoom In | + / = |
| Zoom Out | - |
| Fit to View | F |
| Toggle Depth Gradient | Shift+D |
| Toggle Grid | Shift+G |
| Toggle Satellite | B |
| Toggle Station IDs | L |
| Toggle Shot Labels | Shift+L |
| Toggle Line Labels | Ctrl+L |
| Toggle Sections Panel | Ctrl+1 |
| Toggle Detail Panel | Ctrl+2 |
| Toggle Data Table | Ctrl+3 |
| Open Settings | Ctrl+, (Cmd+, on macOS) |
| Keyboard Shortcuts Overlay | ? |

**Note**: Import (Ctrl+I) and Export (Ctrl+E) shortcuts are user-configurable via the Settings dialog. Open (Ctrl+O) offers every supported file and routes it to the right open/import flow by extension (#407). Import (Ctrl+I) follows what "import" means at the time (#508): with a project open it offers `.dmp` only - the one format that adds to the open project - and with no project open it is the same unified picker as Open. The defaults shown above can be reassigned to any available key combination.

**Note**: P and Z act only when `map.controls.show_navigation_tools` is on (Settings -> Map -> Controls). The Pan and Zoom tools are hidden by default, and while they are hidden both keys behave like unbound keys. The map still pans on a plain drag and zooms on the wheel under every tool.

**Note**: When Pan (P) or Zoom (Z) tools are active, selection-related shortcuts (Ctrl+A, Delete/Backspace) are disabled to prevent accidental modifications while navigating the map. Hover effects and click selection are also disabled.

### Undo/Redo Dropdown Keyboard Navigation

When the Undo or Redo dropdown is open:

| Action | Shortcut |
|--------|----------|
| Open dropdown | Arrow Down (when button focused) |
| Navigate items | Arrow Up / Arrow Down |
| Jump to first item | Home |
| Jump to last item | End |
| Select item | Enter |
| Close dropdown | Escape |

The dropdown also closes when pressing Ctrl+Z or Ctrl+Y (global undo/redo shortcuts), allowing quick keyboard-driven workflows.

### Global View Keyboard Shortcuts

| Action | Shortcut |
|--------|----------|
| Open Selected | Enter |
| Remove from Recent | Delete / Backspace |
| Deselect All | Escape |
| Select All | Ctrl+A |
| Navigate to Nearest System | Arrow Keys (map view only) |

**Note**: Global View has limited keyboard shortcuts since the map-based interface is primarily mouse-driven. Arrow key navigation finds the nearest system in the specified direction using Euclidean distance. Escape also closes the context menu if open.

---

## Appendix B: Validation Rules

`{shot}` names the shot by its line and stations, `Main Line: 4 → 5`
(section 4.2), never by a positional id. The structured `lineId` /
`shotIndex` of a warning say which shot it is; the Warnings tab's Full level
turns the leading `Shot {shot}` into a shot pill.

| Rule | Severity | Message |
|------|----------|---------|
| Shot length < 0.3m (default; configurable) | Warning | "Shot {shot} length {length}m below minimum threshold {threshold}m" |
| Shot length > 50m | Warning | "Shot {shot} length {length}m above maximum threshold {threshold}m" |
| Depth change > 10m per shot | Warning | "Shot {shot} depth change {change}m exceeds threshold {threshold}m" |
| Azimuth in/out difference > 20 degrees (default `azimuth_diff_threshold`; configurable) | Warning | "Shot {shot} azimuth in/out difference {dev}° exceeds threshold {threshold}°" |
| Inclination in/out difference > 10 degrees (default `pitch_diff_threshold`; configurable; only when both are recorded and in range) | Warning | "Shot {shot} inclination in/out difference {dev}° exceeds threshold {threshold}°" |
| Zero length shot | Error | "Shot {shot} has zero length" |
| Negative length | Error | "Shot {shot} has negative length: {length}m" |
| Negative LRUD | Error | "Shot {shot} has negative {direction} distance: {value}m" |
| Invalid azimuth | Error | "Shot {shot} has invalid azimuth_{in/out}: {value}° (must be 0-360)" |
| Shot length < depth change | Error | "Shot {shot} length {length}m is shorter than its depth change {dz}m (impossible: slope length must be at least the vertical change)" |
| Recorded inclination disagrees with length/depth (> inclination_depth_diff_threshold, default 15°) | Warning | "Shot {shot} recorded inclination {pitch}° disagrees with the {computed}° implied by its length and depth change (exceeds {threshold}° threshold)" |
| Non-finite field value | Error | "Shot {shot} has non-finite value(s) in: {fields}" |
| Inclination outside -90 to 90 degrees | Error | "Shot {shot} has an out-of-range pitch_{in/out}: {value}° (must be between -90° and 90°)" |
| GPS fix far from its surveyed position (gap > 15 m + 2% of the survey distance from the placement datum of its piece) | Warning | "The GPS position of this zero datum is {gap} m from where the survey puts it (bearing {bearing} degrees), more than the {threshold} m expected. ..." Ignore covers that GPS reading only: a new GPS on the datum brings the warning back, and undoing that change brings the Ignore back. |
| Duplicate station merge | Error | "These stations are already merged" |

---

## Appendix C: Color Themes

Stalagmight supports 10 color themes, selectable independently for day (light) and night (dark) modes:

| Theme | Type | Description |
|-------|------|-------------|
| `dark` | Dark | Default dark blue/purple with teal accent |
| `light` | Light | Default warm beige/paper with teal accent |
| `cave` | Dark | Very dark, desaturated with gold accent |
| `dracula` | Dark | Dracula-inspired with purple accent |
| `one_dark` | Dark | Atom One Dark-inspired with blue accent |
| `solarized_light` | Light | Solarized light with blue accent |
| `catppuccin_latte` | Light | Catppuccin Latte (light) with blue accent |
| `nord` | Dark | Nord-inspired with cyan accent |
| `catppuccin_mocha` | Dark | Catppuccin Mocha (dark) with blue accent |
| `kaleido_eink` | Light | High-contrast, pre-composited for e-ink displays; CSS transitions disabled to prevent ghosting |

Theme tokens are defined in `src/theme/tokens.ts` as CSS custom properties (`--cc-*`). Each theme provides tokens for UI chrome, map visualization, map controls, canvas preview, and opacity variants.

When `appearance.theme` is set to `"auto"`, the OS `prefers-color-scheme` media query selects between `day_color_theme` and `night_color_theme`.

```
Line Colors (auto-assigned):
#4ecdc4 - Teal
#ff6b6b - Coral
#95e1d3 - Mint
#f38181 - Salmon
#aa96da - Lavender
#fcbad3 - Pink
#a8d8ea - Sky Blue
#ffcb77 - Peach

Map Colors:
Grid: #333366
Zero Datum Station: #ffd700 (Gold; enlarged dot on the map, star in map exports)
Jump Shot: 50% opacity of the line color (#27)
Reference Shot: 28% opacity of the line color (#27)
Selection Highlight: #4ecdc4
Loop Closure Line: dashed, colored by error severity (#22c55e <1%, #eab308 1-3%, #f97316 3-5%, #ef4444 >5%)
```

## Immediate SpeleoDB connection settings

Connect saves changed preferences and authenticates immediately. Success closes
Settings and refreshes the project listing. Disconnect takes effect immediately,
clears the remote listing and stays on the SpeleoDB Settings tab. Neither action
requires a subsequent Save. While connected, the server endpoint and both reset
controls are disabled; backend requests to change instances are also refused.
Disconnect first, including when the current server is temporarily unavailable.

The SpeleoDB tab has a Close button instead of Save/Cancel. Closing validates and
saves preference changes, including changes in other categories, without connecting.
Connection and persistence errors keep Settings open. The ordinary Settings
unsaved-changes prompt offers only Cancel (red) and Discard Changes (blue).

HTTPS is required except for HTTP to the exact host `localhost`, the exact IPv4
address `127.0.0.1`, or valid IPv4 addresses in `192.168.0.0/16`. Valid ports are
allowed. For HTTP, other private ranges, IPv6, localhost aliases/subdomains, public hosts,
missing schemes and malformed addresses are rejected, without any DNS lookup. An
endpoint the policy refuses produces an error dialog when connecting, or when saving
a new server. `insecure_http` lifts only the HTTP rule (never certificate
verification); it stays in the file so existing configurations load, Settings has
no control for it, and its development-build-only handling is being finalised by a
follow-up commit. Certificate verification and redirect restrictions remain.
