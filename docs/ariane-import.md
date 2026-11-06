# Ariane TML/TMLU Import

Stalagmight can import cave survey data from Ariane's TML and TMLU file formats.

## File Formats

| Extension | Structure | Description |
|-----------|-----------|-------------|
| `.tml` | ZIP archive containing `Data.xml` | Standard Ariane export |
| `.tmlu` | Plain XML | Uncompressed variant |

Both formats use the same XML schema. TML files use long element names (`Azimut`, `Length`, `DepthIn`, `FromID`, `ClosureToID`) while TMLU files use short abbreviations (`AZ`, `LG`, `DPI`, `FRID`, `CID`).

## Supported Data

"Supported" here means "parsed". What the import then DOES with each field is the workflow below; the fields that are parsed and go nowhere are listed under [Limitations](#limitations).

### Per-file metadata
- Cave name
- Measurement unit (metric / imperial)
- Magnetic azimuth flag (parsed, unused)
- Entrance elevation (parsed, used - see [Limitations](#limitations))
- GPS reference (latitude, longitude, altitude) - extracted from the first shot with non-zero coordinates

### Per-section
- Section name (with `SectionDescription` suffix stripped)
- Surveyor / explorer, taken from the section's first shot. Ariane's `Explorer`
  is ONE element holding two roles, in two encodings: a plain compound string
  (`"nh / ds"`) or an entity-escaped element pair
  (`<Explorer>nh</Explorer><Surveyor>ds</Surveyor>`). Both halves are kept and
  rejoined with `" / "` - the form real files already use, and the form export
  writes back - so the round trip is idempotent. Splitting the compound into
  people is [#329](https://github.com/Stalagmight/Stalagmight/issues/329) and is
  not done here. A half spelled `unknown`, `None` or empty is dropped on its
  own, so an unfilled explorer no longer takes the surveyor with it (#427).
- Survey date, taken from the section's first shot
- Section color (converted from Ariane's integer RGB format), taken from the section's first shot

Those three are the section's **trip**, and Ariane records them on EVERY shot
row - see [The trip cut](#the-trip-cut) for what happens when they change
mid-chain.

### Per-shot
- Azimuth (normalized to 0-360; Ariane stores a single forward azimuth, not a backsight pair)
- Length
- Depth in / depth out
- Inclination
- LRUD (left, right, up, down)
- Comment
- Shot type (Real, Virtual, Start, Closure) - Ariane's "Virtual" type maps to a virtual `ShotKind`
- Excluded flag
- `UUID` / `UU` - Ariane 26+'s stable per-row id, kept verbatim as the shot's
  identity so a file that leaves Ariane and comes back is not re-identified
  (#423). Absent or empty means "no identity" and gets a fresh v4. The value is
  never validated or coerced; see
  [Ariane export](ariane-export.md) for the uniqueness scope and the SpeleoDB
  sidecar consequence.

### Loop closures
CLOSURE-type shots in Ariane encode loop closure metadata. During import these are resolved into Stalagmight loop closures linking two stations across sections.

## Import Workflow

The Ariane import is a single dialog (not the DMP wizard's multi-step flow), but since #425 it reuses the wizard's pieces: the same resizable map column (`WizardMapPanel`, whose width is shared with the DMP wizard and persisted - see [#404 / `panels.wizard_map_panel_width`](configuration.md)), per-section toggles, and a placement step. It always creates a NEW project - importing a `.tml` into an open project is not supported (the unified router offers to close the current project first).

1. **Pick the file** - the unified-picker entry points (the hamburger's Open / Import row, Ctrl+O, drag-drop, and Ctrl+I with no project open) route through `routeOpenedFile`, which parses the file (`parse_tml`) and marks the flow as a new-project import. The in-project Import affordances (toolbar button, hamburger Import row, Ctrl+I) offer `.dmp` only, since a `.tml` cannot be imported into an open project (#508) - drag-drop still accepts one and offers to close the project first.
2. **Name the project** - `NewProjectForImportDialog` creates the project BEFORE the import is confirmed. For a project created without an explicit save path the initial save is deferred, so nothing is on disk yet.
3. **Confirm the import** - `TmlImportDialog` shows the file's cave name, a map preview of the file laid out as the import will build it, the sections table with a checkbox per section, and the counts that will actually import under the current selection and toggles (excluded shots, comment placement, virtual-shot classification). It then runs `execute_tml_import`.
4. **Receipt** - a successful import opens the import receipt (sections, shots, loops, unit conversion, any converter warnings).
5. **Orphan placement (optional)** - if the newly imported sections suggest connections to existing ones, the orphan-placement dialog opens INSTEAD of the receipt handoff, and the receipt follows it.

Cancelling the dialog - or a terminal failure - leaves the project created in step 2 behind, so the dialog asks whether to keep that empty project or discard it (discard closes a deferred project, or deletes the `.stalx` if one was written).

### Section selection (#425)

Each section has a checkbox; the selection travels as `selected_sections: Option<Vec<u32>>` (indices into `TmlFile::sections`), `None` meaning "all", mirroring the Walls importer's `selected_surveys`. Filtering is by the ORIGINAL index - `ImportSectionConfig::section_index`, the `FromImporting` targets and the closure endpoints all address that numbering, and `execute_import_sections` resolves them through a map, so a sparse selection needs no renumbering.

The topology rules, in `build_selection_index` / `determine_placement`
(`crates/parsers/src/parsers/tml/import_options.rs`):

- The Ariane-id index is built over the SELECTED sections only. A parent in a deselected section therefore resolves to nothing, so no surviving section can emit a `FromImporting` onto a section that is never created - which is a hard, whole-import rollback in `execute_import_sections`, not a skip.
- The ROOT is the lowest SELECTED index (not "the first section that survived shot filtering" - promoting a successor would silently drop the disconnected-section warning that case has always produced).
- A section that had a parent and lost it to the selection falls back to the origin with its own warning ("attached to a section you did not select"), kept distinct from the file-orphan warning ("was disconnected") so the receipt names the right cause.
- A loop closure whose far endpoint is in a deselected section is skipped and counted separately from one whose endpoint was EXCLUDED - two different toggles, two different messages.
- A deselected section produces no "no shots after filtering" warning: the user said no, which is not an anomaly to report back at them.

### Placement (#425)

Placement of every section but the root is still decided by the file's topology: each attaches to the station its `FromID` chain resolves to in an already-imported section.

The ROOT is a user choice, taken in the dialog and sent as `root_placement: Option<TmlRootPlacement>` (`None` keeps the pre-#425 behaviour for callers with no user in front of them, e.g. the SpeleoDB adoption path):

- **Use the position in the file** - offered when the root section's own first shot carries non-zero latitude/longitude (what the importer would have picked anyway, now visible), or when the file has a `geo_coding` on some OTHER section, which the importer otherwise ignores entirely.
- **Place at the origin** - what a file with no georeference used to get in silence.
- **Enter coordinates** - typed latitude/longitude, validated through `GpsCoordinates::new` in the converter, so a NaN or an out-of-range value fails the import rather than landing in a project.

**The file's GPS lands on the station it was measured at** (#576 D39(j), NF-AN4-4). Ariane writes the anchor on the START, and the parser copies it onto the shots leaving it, so it rides on a section's raw first shot - including when that shot is EXCLUDED. With "include excluded shots" off, that shot is not built and the section's station 0 is the start of its first imported shot, so `gps_placement` carries the datum forward along the dropped leading shots (their horizontal legs, walked with the section's own north reference; CLOSURE rows skipped, as the import never draws them) instead of pinning the file's coordinates on the wrong station. The same rule applies to the root and to a detached section placed by its own GPS, and to the dialog's "use the position in the file": the converter recognises those coordinates (the root's raw first-shot values, returned verbatim) and treats them as the file's anchor, while typed coordinates always mean station 0. The import preview reads the raw first shot the same way, so a section placed by such a GPS is not badged "placed at the origin".

The typed datum deliberately carries `depth_reference: 0` ("not recorded"), the same value the file-derived anchor uses: Ariane states a surface position, not a datum depth. The zero-datum config dialog's own depth/elevation field (#524) is the eventual home for that number if this step ever needs to collect it - it is not a second convention, just a field this step does not ask for yet. Both magnitudes are bounded by `MAX_DEPTH_REFERENCE_M` (11 000 m, mirrored in `src/utils/gpsBounds.ts`).

Deliberately NOT the DMP wizard's manual-placement engine: this dialog only ever opens on a brand-new, empty project (Ariane is a new-project-only format in `openOrImport.ts`), so there is no existing station to attach to and nothing on the map to drag against. A fuller step would need the dialog to be reachable with a project open, which is a separate change to the import router.

### Excluded shots

Ariane marks some shots as "excluded" (e.g. survey errors). By default these are filtered out during import. The import dialog provides a toggle to include them if desired, and the preview counts move with it.

When excluded shots are filtered:
- Sections that lose all shots are dropped entirely
- Branch names like "Main Line (branch 1)" are simplified back to "Main Line" when only one branch survives
- Sections are topologically sorted so parent sections are always created before dependents

**A dropped lead moves the first imported shot's start depth** (#576 NF-TML-2). When a section's leading shots are dropped, its first imported shot starts where the last of them ends, so a "same as previous" `DepthIn` on that shot resolves to the last dropped shot's depth instead of `start_depth` (the depth the file records for the station the section leaves from). CLOSURE rows are no leg and are skipped; an explicit `DepthIn` is used as written. For a section placed by its own GPS, that is the station `gps_placement` carries the datum to. For a non-root section attached to its parent at its raw start (`FromImporting`: station 0 merged onto the parent's station), only the depth follows the dropped lead: station 0's plan position stays merged on the parent's station, and the merged point's depth, the average of the shot depths around it, now takes the dropped lead's end depth where it used to take `start_depth`.

**So does a dropped leg mid-section** (#576 W27). When an excluded shot inside a section is dropped, the next kept shot's "same as previous" `DepthIn` resolves to the dropped shot's depth - the depth the file records at the station that shot leaves from - not to the depth of the kept shot before it. It is one rule: a "same as previous" `DepthIn` is the depth of the raw leg before the shot, kept or dropped, so a shot starts at the same depth whether its excluded predecessor leads the section, sits inside it, or is included. Where the dropped leg sat between two kept shots, the kept shot after it now starts at a different depth from the one the shot before ends at; the import keeps both readings as the file records them.

## Shot Types

| Type | Handling |
|------|----------|
| **Real** | Imported as regular survey shots |
| **Virtual** | Imported as a virtual shot, then classified Jump or Reference by the #27 chain rule at import finalize (a run bounded by real survey on both ends is a Jump). Faded display, hidden by the "Jumps" / "References" toggles. The TML type string remains "VIRTUAL" for Ariane compatibility. |
| **Start** | Marks the first station; not imported as a shot |
| **Closure** | Not imported as a shot - used to create loop closures between sections |

## Topology Reconstruction

Ariane stores survey data as a flat list of shots with `FromID` linkage. The parser reconstructs section topology by:

1. Building a graph of shot connectivity from `FromID` references
2. Detecting branch points where multiple shots share the same `FromID`
3. Splitting branches into separate sections with parent-child placement relationships
4. Resolving cross-section connections via a shot ID → (section index, station index) mapping

### The trip cut

A `Section` holds ONE surveyor, one date and one colour; a `Shot` holds none of
the three. Ariane writes all three on every shot row, so a chain surveyed by two
teams over three days used to import entirely under its FIRST shot's values,
with the rest discarded silently (#427).

Each linearized run is therefore **cut where the trip changes between two
adjacent stations**, and each part keeps its own surveyor, date and colour. The
parts after the first are named `"<name> (trip N)"` - backend-owned English, the
same as the `(branch N)` suffix beside it (`linearize.rs`, `format!("{section_name}
(branch {branch_number})")`); the only translated string in this area is the
wizard's `(branch)` BADGE, which is driven by `is_branch_split` and not by a
name. Each part hangs off the station it was cut at (`FromImporting`), so the
geometry is byte-for-byte what the uncut run produced. This is the same
(surveyor, date, colour) triple `join_contiguous_chains` refuses to merge
across, applied to the cut side.

A run takes those three values from its first shot that could be a STATION of it
(non-CLOSURE, non-excluded), not simply its first shot: a run headed by an
excluded shot was otherwise named after a shot the default import does not keep,
and every shot it did keep then read as a collapsed trip that never happened.

Three rules bound it, the first two mirroring the join:

- **A cut needs a station on both sides.** A trip that changes across a CLOSURE
  or excluded shot is not cut, because that shot is not in the import's station
  index and the part after it would resolve to no parent - and whether it does
  would depend on the include-excluded toggle, which sectioning must be
  invariant to. What cannot be cut is counted and named on the receipt instead
  ("N shots in M sections recorded a different surveyor/date/colour...").
- **The cut runs AFTER the join.** A cut end is not a chain end - it has an
  in-section successor by construction - and offering it to the join let an
  unrelated branch at that junction splice onto the mainline's head. The cost,
  pinned by `a_chain_continuing_a_cut_tail_stays_its_own_section`, is that a
  chain continuing a cut tail under its own name is not absorbed into it.
- **A cut budget, per run AND per file.** The trip is file-controlled and
  written on every row, so a file carrying a per-ROW date - a data-entry
  timestamp, or a crafted file - would otherwise mint one section per shot from
  a single declared name. `MAX_TRIP_CUTS_PER_RUN` (256) sits far above real
  multi-trip surveys (about five years of WEEKLY survey days under one declared
  name; the largest real file measured cuts none at all) and far below
  degeneracy. It is not enough on its own, because a file chooses how many RUNS
  it has too, so `MAX_TRIP_CUTS_PER_FILE` (4096 = 16 runs at the per-run cap)
  bounds the total, and the budget is further clamped to the id space the file
  has not already used (below). Past either bound the remaining trips stay on
  the last part and are named on the receipt, so the bounds cost reporting, not
  data.

#### Why sections are budgeted at all

A shot id is `section_id * FEATURE_ID_STRIDE + sequence_order` with a stride of
100_000, in `u32`. The last shot of section `id` encodes as `id * 100_000 +
99_999`, so the encode holds up to **42_949 sections** (ids `0..=42_948`); past
that a site either panics (debug) or saturates, and saturating ALIASES distinct
shots onto one id, which silently moves the station cross-references anchored on
them. The cut's budget is therefore also clamped to `42_949 - (runs the file
already has)`, so importing a file can never use the CUT to cross that line.

The budget is `models::MAX_SECTIONS_IN_ID_SPACE` (42_949) with
`models::MAX_SECTION_ID` (42_948) beside it - shared constants, so the TML cut
budget (`parsers/tml/linearize.rs` `cut_budget`) and the mint-time refusals
below are the same number by construction rather than by two copies agreeing.

Reaching the boundary without the cut - by declaring tens of thousands of
sections in the file - is refused where section ids are MINTED (#555), which is
the only place that can see both the file and what the project has already
spent:

- **`core/state/command_import.rs` `execute_import_sections_inner`** - the batch
  door every importer funnels through (Ariane TML/TMLU, Compass, Walls, GPX, the
  mNemo DMP wizard). Before any mutation it compares the configs it was handed
  against `Project::next_section_id` and refuses the WHOLE import if the sum
  crosses the limit, naming the file's section count, the ids already spent and
  the limit. Nothing is created, so there is no partial import to roll back.
- **`core/state/command_section.rs` `create_section`** - the per-section door
  (in-app section creation, and each section of an import). Refuses when
  `next_section_id` is already past `MAX_SECTION_ID`, alongside the existing
  max-shots-per-section check.
- **`core/state/project_merge.rs` `merge_projects`** - refuses before appending
  anything when the sources hold more sections between them than one project can
  address; no output file is written.
- **`core/state/command_delete_split.rs`** `execute_delete_shot_and_split` and
  `execute_delete_shot_range_split` - a delete that splits a section mints
  section B through `Project::add_section` directly, bypassing `create_section`,
  so both carry the same refusal with their other pre-checks. It has to sit
  there and not at the mint: by then section A has been truncated (and, on the
  range path, connectivity rewritten), so the shots section B is supposed to
  carry are already gone.

What each encode site does past the boundary, for the cases that come off disk
rather than through those doors:

| Site | Behaviour past 42_949 sections |
|------|-------------------------------|
| `models/section.rs` `Section::new` (`next_shot_id`) | unchecked multiply - panics in debug, wraps in release. Unreachable from disk since #557: the `.stalx` open refuses first, and the persisted-undo `restore_section` replay is bounded identically |
| `core/state/project_io.rs` (uuid index, twice) | unchecked multiply - same |
| `persistence/sqlite/read.rs` (shot id, `next_shot_id`) | saturating (#427) - no panic, ids alias. The #557 open-refusal vets `sections` rows only, so an orphan `shots` row (a section id with no `sections` row) still reaches this encode and saturates |
| `persistence/sqlite/read.rs` `unrecognised_shot_kinds` (`shot_id`) | saturating (#557) - it runs its own query, so a shots row can name a section id the open-refusal never vetted |
| `models/shot.rs` `shot_display_position` | saturating (#413) - display only |
| `src/utils/ids.ts` `encodeFeatureId` | JS doubles, exact past 2^32 - no wrap. The SoA typed arrays feeding the renderer were widened from `Int32Array` to `Uint32Array` (#555), so the JS side is exact across the full `u32` range, not just the positive half of `i32` |

The load half of the split is closed (#557): opening a `.stalx` that names a
section id past `MAX_SECTION_ID` is refused outright (an O(1) `MAX(id)`
precheck plus a per-row backstop), with a message naming the offending id and
the bound - a pre-#556 file, another tool's output, or a hostile author cannot
reach the unchecked encodes through the open path. The persisted-undo replay
is bounded at both of its section-construction points: `restore_section`
(legacy `SectionSnapshot` and `CreatedSectionSnapshot` entries) refuses an
over-bound id, and `restore_snapshot_into` - the chokepoint the
`ProjectSnapshotPair` arm current builds write goes through, where serde
rebuilds sections without `Section::new` - refuses the whole snapshot. The
counter rebuild (`next_section_id`) saturates as defense in depth.
`resolved_warnings` rows whose section id cell does not coerce to a live id
are dropped-and-counted on read (post-#517, logged) - correct, because
suppression is keyed by `(section_id, uuid)`, so such a row can never match a
live shot.

One accepted consequence: a file that genuinely carries over-bound section
ids is refused, not salvaged - `repair.rs` recovers a project through
`load_project`, so the salvage path refuses it too. Such a file can only come
from a foreign writer or corruption (no shipped build could mint past the
bound without the encode already misbehaving), so refusal-with-receipt beats
loading data whose shot ids alias.

The suffix can collide with a name the file itself declares (a file with both
`A` and `A (trip 2)`). That is accepted, not disambiguated: nothing in the
import resolves a section by name - grouping keys on the file's own `<Section>`
text before any suffix exists, placement and closures key on Ariane ids, and
`simplify_lone_branch_names` only rewrites `(branch N)` names - so the result is
two sections sharing a label, exactly as a file declaring one name twice has
always produced (`a_declared_name_that_collides_with_a_cut_suffix_is_accepted`).

Measured on a large real-world survey (14 declared names): 17 configs, median 14
shots - identical to the pre-cut numbers, because no declared section in it
changes trip mid-chain.

## Unit Conversion

When the file specifies imperial units, lengths, LRUD values, depths, section start depths and the file's entrance elevation are converted from feet to meters during parsing. All data enters Stalagmight in metric.

An absent or recognised `<unit>` (empty, `m`, `metric`, `ft`, `imperial`) is handled silently. Anything else falls back to metric AND raises an import warning naming the unrecognised string, so a silent mis-scaling is visible on the receipt.

### Readings beyond the 10,000 km limit (#576 FZ-PARSE-5-D1)

A length, depth, `DepthIn`, passage dimension or entrance elevation that is finite but beyond 10,000 km (1e7 m, after the unit conversion) is read as missing, the way a non-finite one already is: 0.0 for a length, depth or LRUD, the "same as previous" marker for a `DepthIn`, and the no-data 0.0 for the entrance elevation. The check runs on the raw shots before the sections are built (`parsers/tml/reading_bounds.rs`), so a branch's start depth never inherits the value. Each one is named by shot ID in `TmlFile::warnings`, which the import dialog shows before Import is pressed and the receipt repeats: "Shot 12: depth 1e308 m is beyond the 10,000 km limit no real survey reaches; read as missing." The limit and the other importers' versions of it are in [SPEC.md section 17.4](SPEC.md#174-import-reading-rules). A length read as missing on a real shot is a zero-length shot, which the core refuses; that is the same outcome a non-finite length already had.

### A "same as previous" marker scaled from feet (#576 HFI-1)

The `DepthIn` value `-1.0` is the format's "this shot starts where the previous one ended" marker, not a depth, and the feet-to-metre conversion above leaves it alone. A file converted to metres by a tool that did NOT leave it alone carries -1 ft written in metres instead: -0.3048, which Ariane (two decimals, trailing zero trimmed) writes as `-0.3`. Read literally, every such shot started 0.3 m above the datum, and the validator flagged the impossible geometry without naming the cause.

In a metric file, the spellings of -0.3048 at 1-4 decimals, rounded or truncated (`-0.3`, `-0.304`, `-0.305`, `-0.3048`, each matched to a micrometre), are read as the marker when MORE THAN HALF of the file's shots carry one - the marker is on nearly every shot of an Ariane file, a real start depth shared by half a survey is not, and a flat stretch that did sit there reads back the same depth either way. Below that a `-0.3` is a measurement (a station 0.3 m above the datum), and in a feet file `-0.3` is always -0.3 ft. The reading is `restore_scaled_depth_in_marker` (`parsers/tml/mod.rs`) with the spellings in `schema::DEPTH_IN_SAME_AS_PREVIOUS_SCALED`. When it applies, the import receipt carries one warning line naming the spelling and the shot count (#576 HFI-1-D2): "Read the depth marker written as -0.3 (a scaled -1) as same as previous on N shots." (`TmlFile::scaled_depth_in_marker`).

### Entrance elevation's zero-is-absent convention

Ariane encodes "no entrance elevation recorded" as `firstStartAbsoluteElevation = 0.0` rather than an absent field, so import treats an exact `0.0` as "not recorded" and anything else, however small, as a real value. This is format-forced, not a Stalagmight choice: a genuine entrance at exactly 0.0 m above mean sea level is not representable and will read back as unset. The convention lives in `crate::models::ariane_elevation_or_absent`, called by import and SpeleoDB pull-adoption; export mirrors it independently (it writes `0.0` back out for an unset `Project::surface_elevation` via its own `unwrap_or(0.0)`, rather than calling the helper), so the round trip is consistent.

## Azimuth Semantics

Ariane has one azimuth field per shot (`Azimut` / `AZ`). Stalagmight duplicates that value into internal `azimuth_in` and `azimuth_out` on import for model compatibility, but `azimuth_out` must not be interpreted as an Ariane backsight measurement.

## Implementation

| Component | File | Role |
|-----------|------|------|
| Parser | `src-tauri/crates/parsers/src/parsers/tml/` | XML parsing, section linearization, unit conversion |
| Import command | `src-tauri/src/commands/import.rs` | IPC handler, deduplication, undo support |
| Import dialog | `src/components/Dialogs/TmlImportDialog.tsx` | Preview, import options, progress, failure/cancel handling |

## Azimuth convention (#438)

Azimuths are copied **verbatim** - no per-shot arithmetic at import - and the
file's `useMagneticAzimuth` becomes the section's declared north reference:

| File says | Section records | Correction |
|-----------|-----------------|------------|
| `useMagneticAzimuth=false` | `north: true`, `source: file` | 0 |
| `true` + `<GeoCoding>`, date in the IGRF range | `north: magnetic`, `source: igrf` with the epoch | the modelled declination at the file's lat/lon on its earliest survey date |
| `true`, no georeference, or no survey date | `north: magnetic`, `source: unknown` | 0. No receipt line: nothing was degraded and there is nothing to act on. An UNDATED file asks the model nothing rather than substituting today, so the same file imports the same way on any two days |
| `true`, georeferenced AND dated, but the model cannot serve that date | `north: magnetic`, `source: unknown` | 0, with a receipt line naming the date it could not serve |

The correction is applied when geometry is computed, never written into a shot,
so a re-export hands Ariane back the numbers the file carried.

**Compass and Walls: the grid convergence is reported, not applied.** Both
formats subtract a meridian convergence because their own output is
grid-relative. Stalagmight's world frame is TRUE north - `gps_to_local` returns
true-north ENU (`models/geo.rs:79-81`) and every georeferencing path places
datums against it - so a survey walked in grid bearings would sit at an angle to
its own GPS. The section correction is therefore the declination alone, and a
file that declares a convergence gets a receipt line saying it was dropped.
Bearings then differ from that app's grid-relative display by the convergence
angle, typically well under a degree.

## Limitations

**Entrance elevation round-trips, subject to the zero-is-absent convention above.** `firstStartAbsoluteElevation` is imported into `Project::surface_elevation` (+up from mean sea level, project-level rather than per-entrance) and read back by TML export; it does not feed the geometry - the zero datum's `depth_reference` (+down from the surface) is what positions stations ([#431](https://github.com/Stalagmight/Stalagmight/issues/431)).

Parsed but dropped:

- **Ariane shot UUIDs are PRESERVED**, and have been since [#423](https://github.com/Stalagmight/Stalagmight/issues/423)
  - this entry used to say the opposite and was stale. `foreign_shot_uuid`
  (`parsers/tml/mod.rs:541`) validates the SHAPE of a `UUID`/`UU` element (non-empty, at most 128
  characters, no XML-unexportable character) and never coerces the value; `Shot::with_uuid`
  (`models/shot.rs:628`) lets the file's value win over the caller's fresh mint. So re-importing
  a file this app exported round-trips by ID, and importing the SAME file twice hands two shots
  the same foreign uuid - which is why the export path re-mints document-level duplicates
  (`export/tml.rs::remint_duplicate_uuids`). A uuid whose shape is rejected costs THAT SHOT its
  foreign identity and nothing else.
- **A `0.0` LRUD field is read as UNMEASURED, per field.** `lrud_opt`
  (`parsers/tml/mod.rs:1902-1908`) maps any of `Left`/`Right`/`Up`/`Down` whose
  `abs()` is under `f64::EPSILON` to `None`, independently of the other three - so
  `left=0, right=3` imports as `null/3`. This is the read half of an INTENDED
  sentinel collision, not a loss of information the file had: the export writes
  `unwrap_or(0.0)` because Ariane's format has no null, so an unmeasured dimension
  and one measured as zero arrive identical. The consequence is that a MEASURED
  zero does not survive a round trip through this app's own `.tml`/`.tmlu`; an
  unmeasured one does. The full argument, and the measured scale of it, is in
  [ariane-export.md](ariane-export.md#the-lrud-zeronull-collision).

- **The Inclination zero/null collision.** `Inclination`/`INC` has the same
  format-forced ambiguity as LRUD (above): Ariane has no null, so
  `pitch_to_inclination` (`parsers/tml/schema.rs`) writes both `None` (never
  sighted) and `Some(0.0)` (genuinely level) as the same wire `0.0`. Unlike
  LRUD, though, a plain "0.0 means absent" read would fabricate data:
  `Validator::validate_shot`'s inclination/depth cross-check treats an absent
  pitch as nothing to check, but a `Some(0.0)` as a claim to verify against the
  shot's depth change, so reading every wire `0.0` back as `Some(0.0)` turned a
  clean round trip of a depth-only survey into a wall of fabricated
  `INCLINATION_DEPTH_MISMATCH` warnings ([#482](https://github.com/Stalagmight/Stalagmight/issues/482)).

  The importer (`parsers::tml::tml_pitch_in`) gates on the REAL condition
  rather than a proxy: it computes what the shot's own length and depth
  change imply the inclination should have been
  (`stalagmight_models::shot::implied_pitch_deg` - the SAME computation
  `core::validation` uses for its cross-check, extracted to one shared
  helper so the two cannot drift) and compares that against
  `DEFAULT_INCLINATION_DEPTH_DIFF_THRESHOLD`, the `ValidationConfig` default
  (not the user's live, editable config - the import mapping must be
  deterministic). A wire `0.0` whose implied pitch would have exceeded the
  threshold imports as `None`; one whose implied pitch is under the
  threshold - a near-level shot, where a recorded `Some(0.0)` would not have
  been flagged anyway - keeps `Some(0.0)`.

  Two losses are the deliberate, accepted cost of this rule, not bugs:
  - A GENUINELY recorded `Some(0.0)` pitch on a STEEP shot is erased to
    `None` by one round trip. The wire cannot tell it apart from an absent
    reading, and the mismatch warning it would have produced was never
    earnable back from a lossy `0.0` in the first place (pinned by
    `a_genuinely_recorded_zero_pitch_on_a_steep_shot_is_erased_by_one_round_trip`
    in `core/state/import_pipeline_tests.rs`).
  - Corrupt geometry - a wire `0.0` paired with a genuinely wrong depth or
    length that implies a steep pitch - imports silently, with no warning.
    No import-side rule can recover that warning without re-fabricating
    #482's false positives for every ordinary depth-only survey.

- **Leading and trailing whitespace is trimmed off every text field.** The reader does
  `e.unescape().unwrap_or_default().trim()` on every shot element's text
  (`parsers/tml/mod.rs:414`), which is what
  makes a pretty-printed `<Comment>\n  text\n</Comment>` from another tool read as `text` rather
  than as `text` with its indentation. The cost is that a comment which BEGINS or ENDS with
  whitespace does not round-trip: `"\tleading tab"` comes back as `"leading tab"`. A literal CR is
  lost twice over - XML 1.0 line-end handling normalises it to LF before the reader sees it, and
  then the trim removes it - so writing `&#13;` would not help either. Interior whitespace, and the
  U+200B shot-comment marker (not whitespace in Unicode terms), are unaffected. CSV export is
  lossless here by contrast; this is a property of the XML text format, not of the comment.
- **Per-shot surveyor / date / color survive as far as the section model allows.**
  A run is cut per trip ([The trip cut](#the-trip-cut)), so several teams over
  several days arrive as several sections rather than one. What a cut cannot
  express - a trip that changes across a CLOSURE or excluded shot, with no
  station seam to cut at - still collapses to the run head, and is counted on
  the receipt rather than dropped in silence ([#427](https://github.com/Stalagmight/Stalagmight/issues/427)).
- **Ariane 26.3's separate `XMLExplorer` / `XMLSurveyor` elements are not read.**
  The survey behind #427 found them synthesized EMPTY on a resave of our own
  export, with the legacy combined `Explorer` still carrying the data, so
  reading them would change nothing on any file seen so far; preferring them
  when non-empty belongs with [#329](https://github.com/Stalagmight/Stalagmight/issues/329),
  which is where the element names can be confirmed against a file that fills
  them.
- **Dates parse strictly as `%Y-%m-%d`.** Any other spelling reads as no date at
  all, with nothing said (#427 records this; it is not yet fixed).
- **`2000-01-01` reads back as no date.** A dateless section is exported as
  `2000-01-01` followed by the XML comment `<!--stalagmight:no-survey-date-->`
  (see [Ariane export](ariane-export.md)). The importer reads every
  `2000-01-01` as no date, with or without the comment, so the placeholder
  neither shows a fabricated date nor sets the declination epoch (#576 B2).
  The comment alone is not enough: Ariane drops it when it re-saves a file,
  exports from before it existed never had it, and the SpeleoDB sidecar never
  writes it. The cost: a survey genuinely dated 1 January 2000 also imports
  undated.

Structural:

- **The mainline fragments at branch points.** Which branch continues the section is decided by file order, not by which branch is the mainline, so a long trunk can arrive split into "(branch N)" sections ([#412](https://github.com/Stalagmight/Stalagmight/issues/412)).
- Ariane's shape/cross-section geometry (`Shape`/`SH` elements) is not imported.
- The `Inclination` field is parsed and normalised to Stalagmight's positive-down pitch on import (`parsers::tml::schema::inclination_to_pitch`), but station coordinates stay depth-based, so pitch never feeds the geometry - see [docs/inclination.md](inclination.md), which is canonical for the sign convention.
- Only the first GPS coordinate found in the file is used as the project origin.
