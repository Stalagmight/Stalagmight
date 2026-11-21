# Ariane TML/TMLU Export

Stalagmight can export cave survey data back to Ariane's TML and TMLU file formats, enabling round-trip interoperability with Ariane software.

## File Formats

| Extension | Structure | Element names | Description |
|-----------|-----------|---------------|-------------|
| `.tml` | ZIP archive containing `Data.xml` | Long (`SurveyData`, `Azimut`, `Length`, …) | Standard Ariane format |
| `.tmlu` | Plain XML | Short (`SRVD`, `AZ`, `LG`, …) | Uncompressed variant |

The user chooses between the two formats in the save dialog's file-type filter.

## Export Workflow

1. Open the **Export** dialog and select **Survey Data** → **TML (Ariane)**.
2. Click **Export** - the save dialog offers two filters: "TML (ZIP)" and "TMLU (XML)".
3. The file extension chosen by the user determines the output format.

## Data Mapping

### Project-level fields

| Internal field | TML element | Notes |
|----------------|-------------|-------|
| `project.name` | `caveName` | |
| Always `"m"` | `unit` | Export is always metric (internal model is metric) |
| Derived from the lines' north references | `useMagneticAzimuth` | See "Azimuth convention" below (#438) |
| `project.surface_elevation` | `firstStartAbsoluteElevation` | 0.0 if unknown |

### Azimuth convention (#438)

The flag is file-level and the references are per line, so the exporter picks
one of three shapes:

Only a line that DECLARES something votes. A line whose correction source
is `unknown` - every `.dmp`, and any `.tml` with no georeference - is raw
magnetic with nothing on record; it constrains the document to be magnetic and
otherwise abstains. Counting it as "correction 0" would make the ordinary case
(a georeferenced `.tml` plus `.dmp` files from the same cave) mixed, rewriting
every bearing and declaring true north about readings known to be magnetic.

| Project | `useMagneticAzimuth` | Azimuths written |
|---------|----------------------|------------------|
| Nothing declared, or every declared line magnetic with one correction | `true` | exactly as stored - Ariane applies its own declination |
| Every line true north | `false` | exactly as stored |
| Declared corrections disagree, or any line is grid-referenced, AND every line has a correction on record | `false` | each line's correction applied, so the document is uniformly true north |
| Any of those, but at least one line has NO correction on record | `true` | as stored, with an export note: a line whose offset from true north is unknown cannot be rotated into true north, so the rewrite is refused rather than shipping an unrotated bearing under a true-north flag |

The third case is the only one that changes a number on the way out, and it
changes only the FILE: the project keeps its measured readings. The third and
fourth both raise a toast on the export (the command returns a stable note code
per `AzimuthExport::note_code`, the copy is the frontend's) and log the long
form.

**Round-trip cost of that third case.** Re-importing such a file reads the
rotated bearings as measured true-north ones, so the per-line corrections are
gone and the export/re-import multiset of azimuths legitimately differs - the
same class of documented lossiness as the LRUD zero/null collision below. The
first two cases round-trip byte-identically.

**What the first case cannot carry, and what that costs.** A magnetic document
hands Ariane the measured readings and nothing else: the format has no field for
our correction, so Ariane derives its own declination from the file's `START`
GPS and applies that instead. Where our correction came from the same
geomagnetic model (`source: igrf`) the two agree to within model revision and
the survey lands where we draw it. Where it came from a hand-entered value - a
Compass `DECLINATION:` the surveyor typed, or a manually set project reference -
they need not agree, and the survey renders in Ariane rotated by
(Ariane's IGRF - our correction): typically a degree or two, more where the
entered value was stale or deliberately local. Nothing is lost from the project
and nothing is wrong in the file; the two apps simply correct the same readings
by different numbers. Accepted as the price of handing over measurements rather
than a frame - the alternative, rotating every bearing so the file is
self-contained, is the third case above and is lossy in the other direction. See
the import side's [Azimuth convention](ariane-import.md#azimuth-convention-438)
for where each correction comes from.

### Line-level fields

Ariane has no separate line record: each Stalagmight line becomes an Ariane
section, named in every row's `Section` / `SC` element, so the line's metadata
is written per shot:

| Internal field | TML element | Notes |
|----------------|-------------|-------|
| `line.name` | `Section` / `SC` | Falls back to `"Line {id}"` if unnamed |
| shot credit (#329) | `Explorer` / `EX` | Per ROW, from the shot's own surveyors and explorers. Always the entity-escaped element form `<Explorer>e1, e2</Explorer><Surveyor>s1, s2</Surveyor>` when anyone is credited, a role with nobody left empty (#719): a plain single value is read as a surveyor by this importer but shown as the explorer by Ariane and read as explorers by openspeleo-lib, so only the element form names the role for every reader. `"unknown"` when nobody is credited. Within a role names are joined with `", "`: the import reads every Ariane field as a name list, with no `Last, First` rule. `<` and `>` are dropped from exported names so no name can move people between roles. A `,` or `;` inside one name cannot round-trip (Ariane separates people with commas). The import reads every row back to the same people on the same shot. A START row takes the credit of the shot it leads into; a CLOSURE row takes the credit of the row it hangs off. |
| shot credit date | `Date` / `DT` | Per row, the shot's own survey date; `"2000-01-01"` if unset. A CLOSURE row takes the date of the row it hangs off, see [Loop closures](#loop-closures) |
| `line.color` (`#RRGGBB`) | `Color` / `CL` | Converted to Ariane format `0xRRGGBBff` |

### Shot-level fields

| Internal field | TML element | Notes |
|----------------|-------------|-------|
| Station ID of the station the row ends at | `ID` | A START's is the station it stands on; CLOSURE rows and GPS-fix STARTs are numbered above the highest Station ID. See [ID assignment](#id-assignment) |
| Station ID of the station the row starts from | `FromID` / `FRID` | Chains the rows of a line; a line entered at a junction hangs off the junction's row; START has -1 |
| `shot.azimuth_in` | `Azimut` / `AZ` | Front-sight azimuth |
| `shot.azimuth_out` | - | Not exported. Ariane does not encode backsight azimuth. |
| `shot.length` | `Length` / `LG` | |
| `shot.depth_out` | `Depth` / `DP` | Absolute station depth |
| `shot.depth_in` | `DepthIn` / `DPI` | -1.0 sentinel when equal to previous `depth_out` |
| `shot.pitch_in` | `Inclination` / `INC` | 0.0 if unset. Collides with a genuinely measured level shot (`Some(0.0)`) the same way LRUD does; see the "Inclination zero/null collision" entry in [Ariane import](ariane-import.md) for the import-side rule that keeps this from fabricating validator warnings on re-import (#482). |
| LRUD | `Left`/`Right`/`Up`/`Down` | `0.0` for `None` - Ariane's format has no null, so an UNMEASURED passage dimension and one MEASURED as zero are written as the same `0.0`. See [The LRUD zero/null collision](#the-lrud-zeronull-collision). |
| `shot.comment` | `Comment` / `CM` | Empty string if unset. Also carries the comment of the station the row ends at (`Station.comment`), joined with the shot comment via the ZWSP marker - see [Station-comment co-encoding](#station-comment-co-encoding). |
| `shot.kind` | `Type` / `TY` | Both `Jump` and `Reference` export as `VIRTUAL`; `Normal` as `REAL`. Ariane has a single virtual category, so the #27 distinction is lossy on export by design (q9) and is preserved locally instead. |
| `shot.uuid` | `UUID` / `UU` | Emitted for every block that stands for a real project shot - the forward blocks and the reversed / pre-zero-datum legs alike; omitted on the blocks the exporter synthesizes (START, CLOSURE, and the SpeleoDB VIRTUAL padding shot), and on a sidecar shot whose value is a per-document repeat or not a v4. See [Shot UUIDs](#shot-uuids). |
| - | `Locked` / `LCK` | `false` for normal shots, `true` for START |
| - | `Name` / `NM` | `"START"` for START, `"CLOSURE:CLOSURE"` for CLOSURE, empty otherwise |
| - | `Profiletype` / `PT` | Always `VERTICAL` |
| - | `Excluded` / `EXC` | Always `false` |

## Shot UUIDs

Every real forward shot carries its persistent `shot.uuid` into the export as
`<UUID>` (`<UU>` in `.tmlu`), so a re-export of an unchanged project is
**byte-identical**. Every shot of the model holds a uuid (the model mints one
when a line is built without it), so nothing is minted per export;
`export_uuid` still mints for an empty value, a fallback that keeps the writer
total.

A shot UUID is unique across the whole project. It is an invariant of the
model (`Project::validate`, invariant 1 of the
[survey model](survey-model.md)):

- **Doors re-mint repeats.** An import (`LineBuilder::claim_uuid` in the import
  funnel), Add .stalx and Merge Projects bring ids from outside. A newcomer
  whose id the project already holds gets a derived replacement
  (`reminted_shot_uuid_avoiding`, a v5 hash of the line and the old id
  re-stamped as a v4, so the SpeleoDB sidecar keeps it); the project keeps
  its own. Importing the same file twice therefore gives the second copy
  derived ids, and only the first copy round-trips to the file's `<UUID>`s.
- **Older files are healed when they are converted.** A legacy (SQLite) file
  saved before this can hold one id in several lines. The frozen 0.6.0 reader
  keeps it in the lowest line and gives the others the derived replacement
  (`make_shot_uuids_project_unique` in `crates/legacy`), so the migrated
  project holds each id once; a legacy file's undo history is not kept (see
  [Undo/Redo](undo-redo.md#persisted-entries)). Derived, so every open of an
  unsaved file agrees.
- **A version 2 file that repeats a shot uuid is refused** at load (it breaks
  the invariant), with the repair offer.
- **Every edit and every history step is validated** (an edit that would break
  an invariant is rolled back; a history step read from a file is rolled back
  from a copy of the project, [Undo/Redo](undo-redo.md)), so a project the
  app holds never repeats a uuid.
- **The export keeps a safety net** that a valid project never trips: after
  the rows are built, the user export re-mints a later occurrence of a uuid
  (`remint_duplicate_uuids`), and the SpeleoDB sidecar drops it
  (`harden_for_speleodb`), because a repeat would alias two shots onto one
  GeoJSON feature id.

Keys stored OUTSIDE the project against an id a heal renamed - a dive-plan
anchor, a legacy file's ignored warning while it is converted - still find
their shot through the derived id (`live_shot_uuid_for_stored`); the
conversion re-keys an ignore to the shot's final uuid. They try the first
derived candidate only. A replacement is stepped past it only when the file
itself already holds that derived value on another shot of the line, and then
a key stored against the old id names that other shot - one the same file
could have named directly.

So a consumer may key a shot on `<UUID>` alone, within one document and across
re-exports.

The importer reads the element back (#423): a `<UUID>`/`<UU>` on a `<SurveyData>`
row becomes that shot's identity verbatim, so an Ariane-authored file that leaves
and returns keeps the same per-shot ids instead of getting a fresh set. The value
is not validated or coerced into a canonical v4 - coercing would recreate exactly
the identity churn the carry exists to prevent - so `Shot::uuid` is no longer
guaranteed to be a v4 we minted. Two consequences follow: an empty element means
"no identity" and gets a fresh mint, and a non-v4 value is dropped from the
SpeleoDB sidecar by `speleodb_uuid`, costing that shot its GeoJSON feature id in
the server preview (Ariane writes v4s, so this bites only hand-edited or
third-party files). A file that repeats a `<UUID>` inside one line is
repaired, not rejected: `Line::new` keeps the FIRST occurrence and mints a
fresh id for the rest, restoring the in-line uniqueness `resolve_key` needs.

Ariane 26.3.0's own Save fills any row missing `<UUID>` with a fresh v4
(verified 2026-07-31), so the synthesized-shot gaps above do not survive a
round-trip through Ariane itself. Every consumer of the file *as we export
it* - SpeleoDB, the GeoJSON feature ids - still sees the omissions.
It exists for the SpeleoDB sidecar profile and for any consumer that wants a
stable per-shot key. Ariane 26+ round-trips it; Ariane 25 and older drop it silently,
which is harmless here.

## Topology

Ariane represents survey data as a directed graph rooted at START shots - one
per entrance, matching what Ariane itself writes for multi-entrance caves.

### START shots - one per entrance

The **root line** and the first line of every other connected piece each get
a synthesized START shot with `FromID = -1` (see
[Line connectivity](#line-connectivity) for which line roots a piece). A START carries the GPS of the datum on its own station; the
entrance split puts the START on that station. A START with no datum of its own
carries the GPS of where the raw survey puts its station, relative to the frame
datum (see [no two STARTs on one point](#no-two-starts-on-one-point)). An
unanchored START would land at lat/lon 0, and the view would open on the
Atlantic. A component's
entrance is a line holding a **placement** datum; a GPS fix never roots one.
All other lines connect to the line they were reached from via `FromID` on
their first row, which hangs off the junction's row.

### Every datum is written as a START (#741)

Ariane reads every START's lat/lon as a **hard fix**. When two STARTs disagree
with the survey between them, it bends the loop to fit and shows the loop
error (measured 2026-11-13 with a synthetic two-entrance file). That is close
to the Closures Computed View, except that Ariane is hard where ours is
weighted by the datum's accuracy.

So every resolvable datum, placement and GPS fix alike, gets a START carrying
its own GPS. That includes a GPS fix whose Entrance box is unticked (for
example a radiolocation point mid-passage): Ariane has no other way to hold a
GPS position, so its GPS is kept at the cost of Ariane labelling it START. In
Stalagmight itself such a fix is not an entrance for Plan mode or penetration.


- A datum on an entrance's station is written on that entrance's START.
- A datum anywhere else - a GPS fix part-way through a line, or on a
  junction a child line starts from - gets a
  **zero-length START of its own at its GPS, tied to the fix station by a
  zero-length CLOSURE** (`FromID` = the fix station's row,
  `ClosureToID` = the new START). That is the shape Ariane's own files use for
  a loop that ends on an entrance, and it adds no station: the START is a leaf
  and the CLOSURE carries no length. Both are minted after every other row and
  take the line name, colour, date and people of the row the tie hangs off
  (the one row that ends at the fix station: at a junction, the row of the
  line the walk reached it on). The START sits at that row's depth.

Datums claim Ariane nodes in list order, so the placement datum always claims
its node first. A datum is **unwritten**, and the export returns the
`gps-anchors-not-written` note, when it no longer resolves, when its station
has no row in the file, when an earlier datum already holds the same node
(two GPS positions for one point cannot be carried), or when its GPS is not a
point on the globe (not finite, or outside |lat| <= 90, |lon| <= 180). A
loaded `.stalx` can carry such a value, since persistence builds the struct
without `GpsCoordinates::new`'s checks. Such a datum claims no node, and an
entrance it stood on is placed like a datum-less one.

The SpeleoDB sidecar leaves the fix STARTs out. Its converter drops CLOSURE
rows, so a fix START would reach the map preview as a lone anchor with nothing
hanging off it. The sidecar counts the fixes as unwritten in its log.

A file whose frame datum's GPS is not a point on the globe leaves the fixes
out too, and counts them unwritten. That frame's entrance stays at `0,0`, and
a fix START at its real GPS tied into the same survey would have Ariane bend
the survey across the globe to reach it.

A file that holds an orphan root (see below) also leaves every fix out and
counts it unwritten. Ariane blanks its map for an orphan alongside **any**
CLOSURE row, and a fix's tie is one; nothing shows a CLOSURE onto a START is
exempt. Writing the fixes would turn a file that drew into a blank one, or
have the orphan guard refuse it over closures the user never made.

On re-import, the in-repo importer drops START rows and skips a closure whose
target is a START with no line hanging off it, so the fix rows come back as
the same stations and legs. The fix's GPS is not read back as a datum. The
import receipt carries up to two counted warnings, each one line for all the
fixes together:

- "N loop closures referenced excluded stations and were skipped", with `N`
  the number of fixes. The tie's target is a START nothing hangs off, which
  the importer keeps no trace of, so it cannot tell it from a station the
  exclusion filter removed.
- "N lines had no shots after filtering and were dropped", with `N` the
  number of fixes part-way along a line. Such a tie hangs off a row the
  next row also hangs off, which the parser reads as a branch: a one-row
  line holding only the CLOSURE, dropped empty. Any loop closure from a
  station part-way along a line does the same, fix or not. A fix at the
  end of a line continues that line and raises no such receipt entry.

Both blame filtering the user never did. The importer would need to keep the
file's START ids (`TmlFile` is an IPC type, so that is not a local change) to
count the ties in its entrance bucket instead; tracked as a follow-up.

### No two STARTs on one point

Two STARTs with the same lat/lon are pinned on top of each other, and Ariane
collapses any loop between them (`Err=100%`, measured 2026-11-13). A START with
no datum of its own used to repeat the frame datum's coordinates, so Ariane,
and a re-import of the file, put a datum-less piece of survey on top of the
frame's entrance, wherever the map showed it.

The app does not let such a piece keep a loop closure back to the survey: after
every topology edit, a closure that is the only link between two pieces
becomes a junction (the orphan-closure sweep, `models::survey::sweep`), so
"merge, close the loop, detach" ends with the piece joined again. The collapse is therefore reachable
only from a project built outside the app, but the misplaced piece is not: any separate
piece with no datum of its own exported onto the frame's entrance.

Such a START now carries the GPS of where the raw survey puts its station:
its raw offset from the frame datum's station, converted with `local_to_gps`
against the frame datum. That station is meant to sit at local `(0, 0)`, but
a legacy project need not hold that, so the offset is taken from the
station's raw position rather than from the origin. Two STARTs therefore
share coordinates only when the survey puts them on the same point. The frame
datum's coordinates remain the fallback for a START whose station has no raw
position. A project with no datum at all still writes every START at `0,0`.
When the frame datum's GPS is not a point on the globe, every START without a
placeable datum of its own stays at `0,0` and the GPS fixes are left out (see
above).

A parentless shot whose type is **not** START is an orphan root. Ariane
tolerates it in isolation, but a file containing both an orphan root and any
CLOSURE row makes Ariane abandon the loop solve and draw a completely blank
map with a fully populated data table (#379). Two hard facts keep this
invariant on the exporter alone:

- **Ariane never repairs it.** Re-saving such a file in Ariane 26.3.0
  preserves every orphan root untouched (verified 2026-07-31, 17/17 on a
  pre-fix export), and every Ariane-authored file examined in the #379
  investigation typed all of its parentless roots START. Nothing downstream
  compensates.
- `ensure_rooted_chain` in `src-tauri/crates/export/src/export/tml.rs` therefore refuses to
  write an orphan root alongside closures (user profile), and warns when a
  file carries orphans without closures.

### Line connectivity

The walk reads the station graph (plan Deviations X2, `export/tml_survey.rs`):

- **Roots.** The frame datum's line roots the document; with no datum, the
  first line in list order that shares no station with a line of lower id.
  Every other connected piece is rooted at its first line in list order that
  holds a placement datum, else its first line.
- **Children.** Lines are visited breadth first from each root through their
  junctions: a line's children are the unvisited lines it shares a station
  with, in line-id order, each entered at the station the two share (one, in
  a valid project: two would close a loop). A junction is one station, so every line through it
  is adjacent to every other; the walk expands each station once, so it is
  linear in the slots however many lines meet at one station.
- **Rows.** Each line is walked once, outwards from the station it was entered
  at: its shots after that station forwards, the ones before it reversed
  (`"<line> (reversed)"`). A child line's first row has its `FromID` set to the
  junction's Station ID, which is the id of the parent's row ending there.

When a root line is split at a non-zero zero datum, the generated pre-zero segment is exported in reverse direction (`"<line> (pre-zero-datum)"`). Its azimuth is always synthesized as `azimuth_in + 180°` (normalized), not from `azimuth_out`.

### Loop closures

Every loop closure (`Tie`s included: a migrated cycle-closing merge is a loop like any other) is exported as a CLOSURE-type shot from its first station's row, with `ClosureToID` / `CID` pointing to the second station's Station ID. These encode the same loop-closure metadata that Ariane uses internally.

A CLOSURE row carries the date and people of the row it hangs off (its
`FromID`). A CLOSURE off a station part-way along a line re-imports as a
one-row branch line of its own, and the importer evaluates the declination
at the earliest survey date in the file. Written undated, the row took the
`2000-01-01` stand-in, which dragged that epoch back and turned the whole
re-imported survey (1.8 degrees for a synthetic 2015 survey).

### ID assignment

IDs are Station IDs, so they are stable across exports and need not start at
0 or be contiguous (plan Deviations X1):

- a REAL or VIRTUAL row's `ID` is the Station ID of the station it ends at,
  its `FromID` that of the station it starts from;
- a START's `ID` is the Station ID of the station it stands on;
- CLOSURE rows, then each GPS fix's START and its tying CLOSURE (one pair per
  fix), are numbered from the highest Station ID + 1, in that order.

In a valid project every `ID` is unique: each line is walked once, from the
one station it shares with the lines walked before it (the forest invariant),
so each station ends exactly one row (or stands under one START), and the
synthesized rows are numbered above every station. `harden_for_speleodb`
still checks the sidecar for a repeated `ID` (it would fail the whole file
on the server), and logs one as an error.

## Color Format Conversion

Internal `#RRGGBB` hex colors are converted to Ariane's `0xRRGGBBff` format (8-digit hex with alpha suffix). On import, the reverse conversion strips the alpha channel.

## The LRUD zero/null collision

`ExportShot.left` and its three siblings are plain `f64`, filled with
`shot.left.unwrap_or(0.0)` (`line_rows` in `export/tml_survey.rs`, forward
and reversed rows alike), because Ariane's `Left`/`Right`/`Up`/`Down` elements
have no null. So the file cannot distinguish these two facts about a passage:

- **unmeasured** - nobody put a tape across it (`None` in the model);
- **measured as zero** - somebody did, and it is zero (`Some(0.0)`).

Both are written `0.0`, and [the importer](ariane-import.md) maps `0.0` back to
`None` **per field** (`parsers/tml/mod.rs:1902-1908`). The round trip is
therefore lossless for an unmeasured dimension (`None -> 0.0 -> None`) and lossy
for a measured zero (`Some(0.0) -> 0.0 -> None`).

**This is intended, and it is a property of the format rather than a defect in
either end.** `0.0` IS Ariane's absent sentinel; the alternatives are to invent
a spelling Ariane does not define, or to stop treating `0.0` as absent on read
and hand every Ariane file in the world four measured zeros per shot. Per-field
rather than all-four also matters: a shot measured `left=0, right=3` comes back
`null/3`, not `0/3`.

Measured, so the scale is on record rather than guessed: on a 532-shot survey
carrying 261 all-null and 271 all-zero shots, the round trip nulls exactly those
271 and changes nothing else - no length, azimuth, depth or comment moves.
`export-roundtrip-fuzz`'s X6 oracle cites this section by name for that reason;
it is the one difference it excuses, and it excuses it because the format says
so, not because the oracle was losing.

## Station-comment co-encoding

Ariane's `.tml` format has one `<Comment>` field per shot and no per-station
comment field, so a Stalagmight station comment otherwise has nowhere to live
in `.tml`. To round-trip both `shot.comment` and the station comment
(`Station.comment`) through Ariane, the exporter packs both into the shot's
`<Comment>`:

- Each station comment is attached to the one REAL or VIRTUAL row that ends at
  the station (its `ID` is the station's), so a junction's one comment is
  written once (plan Deviations X4).
- Shot vs station lines inside the resulting Comment are distinguished by a
  zero-width-space (`U+200B`) prefix: lines prefixed with `ZWSP` are shot
  comments; lines without the prefix are station comments.
- The marker goes on **every** exported shot comment, whether or not a station
  comment rides along. An unmarked line is indistinguishable from text an
  Ariane user typed, and the default import routes it to the station - before
  #564 a lone shot comment therefore silently became a station comment on a
  default re-import.
- The non-default import setting is one-way by design: importing with
  "comments on stations" OFF concatenates station lines into the shot comment,
  and the next export marks all of them as shot lines - station text imported
  that way stays shot text. The default round trip is the side-stable one.
- The marker is invisible in any editor, including Ariane's, so a reader sees
  a single readable Comment block.

Caveats:

The fold walks the emitted REAL/VIRTUAL shots and skips START (its `"START"`
content is hardcoded) and CLOSURE (no `to` station in the create-station sense).
A station whose only row is a START therefore loses its comment on export:

- **An entrance's station.** The station an entrance line's START stands on
  (station 0 of an unsplit root line, or the datum's station when the line is
  split at a mid-line datum) has no REAL row ending at it, so a comment on it
  is dropped. Add the note one shot downstream if you need it to survive. A
  junction is one station, so a child line starting on that station does not
  bring the comment back.

Comments that survive but move:

- **Stations before the zero-datum split index.** Those stations are emitted on
  the reverse `"<line name> (pre-zero-datum)"` leg, so their comments are folded
  onto that synthetic leg's shots. The text survives, but it re-imports onto a
  *fabricated* line that never existed in the project - not onto the line
  the surveyor wrote it in.

Every drop emits a `tracing::debug!` naming the station id, so a lost comment is
diagnosable from a debug-level log, and an export that dropped any returns the
`station-comments-not-written` note, which the app shows as a warning
("Comments on entrance stations were left out"; 0.6.0 said nothing, demo
issue N-14).

The same split logic is applied in reverse on `.tml` import
(`models::comment_split::split`) when the import dialog's "comments on
stations" option is on (the default). Text typed in Ariane carries no marker
and therefore routes to the **station** on import - matching how Ariane
visually centers Comment text on the station, not along the shot. A bare
Ariane file has no markers at all, so every Comment line routes to the
station.

## The SpeleoDB sidecar profile

Two things read our `.tml`: Ariane, and the `ariane.tml` sidecar SpeleoDB
converts to GeoJSON for its map preview (see [SpeleoDB dev testing](speleodb-dev-testing.md)).
The converter (`openspeleo_lib`) enforces rules Ariane never looks at, and when
one is broken it either fails the whole file or silently skips the map - the
upload still returns 200, so nothing surfaces to the user.

`export_tml` and `export_tml_for_speleodb` therefore build the same document
under different profiles. No push renders the sidecar today: since the move to
the SpeleoDB SDK a push uploads the project file only. The profile is kept,
and its tests pin it, for when the co-push returns. The **user** profile is what it has always been, down
to the byte. The **sidecar** profile additionally:

| Concession | Why |
|------------|-----|
| Rounds a repaired `Length` up to the next micrometre | The server re-derives `abs(Depth - parent.Depth) <= Length` from the DECIMAL STRINGS, and the six-decimal write can round a repaired tape back below the drop it was minted to cover |
| Never emits exactly one `<SurveyData>` (pads with a VIRTUAL root) | A lone element parses as a dict in the server's decoder and crashes the conversion; zero is handled cleanly, two or more is the normal path |
| Re-emits every `<Date>` as an ISO calendar date (`YYYY-MM-DD`), year clamped to `[1900, 2035]` | A missing, empty or non-ISO date fails the whole file; outside IGRF's range the declination silently becomes 0.0. Only `YYYY-MM-DD` is parsed - anything else, including a full ISO 8601 datetime, falls back to the placeholder and loses its real survey date |
| Emits `<UUID>` only for a v4 not already used in the file | The field is typed `UUID4`; a repeat aliases two shots onto one GeoJSON feature (the model keeps shot UUIDs unique; this is the safety net) |
| Nudges a zero latitude or longitude by a micro-degree, and clamps out-of-range ones | Both components must exceed `1e-7` or the map is skipped, so a survey on the equator or the prime meridian gets none. No datum still means no anchor |
| Replaces U+220E in line and explorer names with a space | The server groups shots into lines by hashing the line-identity fields joined with that character |
| Normalises `Azimut` for every shot type | Only REAL shots are normalised server-side |

None of this touches the project: the transformations run on the export
representation, and only for the throwaway sidecar. Past 25,000 shots the
sidecar render warns that the server may fail to build the preview - nothing is
dropped or capped.

## Implementation

| Component | File | Role |
|-----------|------|------|
| Rows | `src-tauri/crates/export/src/export/tml_survey.rs` | The walk over the station graph, Station IDs, comments, CLOSURE rows; `export_tml` and `export_tml_for_speleodb` (re-exported as `export::survey`) |
| Export logic | `src-tauri/crates/export/src/export/tml.rs` | XML generation, ZIP packaging, the GPS stamp, the geometry repair, the orphan guard, the two profiles |
| Export command | `src-tauri/src/commands/export.rs` | `export_tml` IPC handler |
| SpeleoDB sidecar | `export_tml_for_speleodb` | The sidecar profile; nothing in a push calls it today (see above) |
| Store method | `src/store/projectStore.ts` | `exportTml()` frontend binding |
| Export dialog | `src/components/Dialogs/ExportDialog.tsx` | UI with TML format option |

## Limitations

- Shape/cross-section geometry (`Shape`/`SH` elements) is not exported (not stored in the internal model)
- GPS coordinates are not written per-shot: latitude and longitude are `0.0`
  everywhere except the START shots, which carry their own datum's GPS or
  the GPS of their surveyed position (see Topology); without any datum they
  are `0.0` there too. Only the project-level elevation reference is otherwise set
- A GPS fix's accuracy is not exported: Ariane has no field for it and pins
  every START exactly
