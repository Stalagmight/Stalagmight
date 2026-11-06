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
| Derived from the sections' north references | `useMagneticAzimuth` | See "Azimuth convention" below (#438) |
| `project.surface_elevation` | `firstStartAbsoluteElevation` | 0.0 if unknown |

### Azimuth convention (#438)

The flag is file-level and the references are per section, so the exporter picks
one of three shapes:

Only a section that DECLARES something votes. A section whose correction source
is `unknown` - every `.dmp`, and any `.tml` with no georeference - is raw
magnetic with nothing on record; it constrains the document to be magnetic and
otherwise abstains. Counting it as "correction 0" would make the ordinary case
(a georeferenced `.tml` plus `.dmp` files from the same cave) mixed, rewriting
every bearing and declaring true north about readings known to be magnetic.

| Project | `useMagneticAzimuth` | Azimuths written |
|---------|----------------------|------------------|
| Nothing declared, or every declared section magnetic with one correction | `true` | exactly as stored - Ariane applies its own declination |
| Every section true north | `false` | exactly as stored |
| Declared corrections disagree, or any section is grid-referenced, AND every section has a correction on record | `false` | each section's correction applied, so the document is uniformly true north |
| Any of those, but at least one section has NO correction on record | `true` | as stored, with an export note: a section whose offset from true north is unknown cannot be rotated into true north, so the rewrite is refused rather than shipping an unrotated bearing under a true-north flag |

The third case is the only one that changes a number on the way out, and it
changes only the FILE: the project keeps its measured readings. The third and
fourth both raise a toast on the export (the command returns a stable note code
per `AzimuthExport::note_code`, the copy is the frontend's) and log the long
form.

**Round-trip cost of that third case.** Re-importing such a file reads the
rotated bearings as measured true-north ones, so the per-section corrections are
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

### Section-level fields

These are written per-shot since Ariane stores section metadata on every shot row:

| Internal field | TML element | Notes |
|----------------|-------------|-------|
| `section.name` | `Section` / `SC` | Falls back to `"Section {id}"` if unnamed |
| `section.surveyor` | `Explorer` / `EX` | `"unknown"` if unset. Written VERBATIM, so a compound value the import built from Ariane's two-role field (`"nh / ds"` - see [Ariane import](ariane-import.md#per-section)) goes back out in the plain compound form real files use, and re-imports unchanged. A file that arrived in the entity-escaped `<Explorer>…</Explorer><Surveyor>…</Surveyor>` encoding is normalised to that plain form, not restored to the escaped one. |
| `section.date` | `Date` / `DT` | `"2000-01-01"` if unset, followed by the XML comment `<!--stalagmight:no-survey-date-->` so a person reading the file can tell it is a placeholder (#576 B2). Our importer reads every `2000-01-01` back as "no date", with or without the comment, so a survey genuinely dated 1 January 2000 also re-imports undated. The placeholder stays because SpeleoDB rejects a missing or empty date; the marker is a comment because no TML element is one Ariane ignores, and a comment is not document content to any XML reader. Without it, a dateless section re-imported dated 1 January 2000 and a magnetic, georeferenced file took its IGRF declination epoch from that year, rotating the survey. A file re-saved by Ariane loses the comment, and the SpeleoDB sidecar never writes it; both still re-import undated, because the importer goes by the date, not the comment. |
| `section.color` (`#RRGGBB`) | `Color` / `CL` | Converted to Ariane format `0xRRGGBBff` |

### Shot-level fields

| Internal field | TML element | Notes |
|----------------|-------------|-------|
| Sequential integer | `ID` | Assigned starting from 0, incrementing across all sections |
| Previous shot ID | `FromID` / `FRID` | Chains shots linearly within a section; START has -1 |
| `shot.azimuth_in`, `shot.azimuth_out` | `Azimut` / `AZ` | Their circular mean - the bearing the app draws (`models::average_azimuth_of`). The two are two FORWARD readings of the same line, not a foresight/backsight pair (see SPEC), and Ariane carries one azimuth per shot, so a pair that disagrees goes out as its mean and re-imports with both readings equal to it: the drawn geometry is unchanged. Before #576 NF-AN5-1 the export wrote `azimuth_in` alone, so a DMP shot read at 10/20 and drawn at 15 went out at 10. |
| `shot.length` | `Length` / `LG` | |
| `shot.depth_out` | `Depth` / `DP` | Absolute station depth |
| `shot.depth_in` | `DepthIn` / `DPI` | -1.0 sentinel when equal to previous `depth_out`, and on every shot refitted at a junction (see [Junction depths](#junction-depths)) |
| `shot.pitch_in` | `Inclination` / `INC` | 0.0 if unset. Collides with a genuinely measured level shot (`Some(0.0)`) the same way LRUD does; see the "Inclination zero/null collision" entry in [Ariane import](ariane-import.md) for the import-side rule that keeps this from fabricating validator warnings on re-import (#482). |
| LRUD | `Left`/`Right`/`Up`/`Down` | `0.0` for `None` - Ariane's format has no null, so an UNMEASURED passage dimension and one MEASURED as zero are written as the same `0.0`. See [The LRUD zero/null collision](#the-lrud-zeronull-collision). |
| `shot.comment` | `Comment` / `CM` | Empty string if unset. Also carries the **to-station's** `project.station_comments` entry, joined with the shot comment via the ZWSP marker - see [Station-comment co-encoding](#station-comment-co-encoding). |
| `shot.kind` | `Type` / `TY` | Both `Jump` and `Reference` export as `VIRTUAL`; `Normal` as `REAL`. Ariane has a single virtual category, so the #27 distinction is lossy on export by design (q9) and is preserved locally instead. |
| `shot.uuid` | `UUID` / `UU` | Emitted for every block that stands for a real project shot - the forward blocks and the reversed / pre-zero-datum legs alike; omitted on the blocks the exporter synthesizes (START, CLOSURE, and the SpeleoDB VIRTUAL padding shot), and on a sidecar shot whose value is a per-document repeat or not a v4. See [Shot UUIDs](#shot-uuids). |
| - | `Locked` / `LCK` | `false` for normal shots, `true` for START |
| - | `Name` / `NM` | `"START"` for START, `"CLOSURE:CLOSURE"` for CLOSURE, empty otherwise |
| - | `Profiletype` / `PT` | Always `VERTICAL` |
| - | `Excluded` / `EXC` | Always `false` |

## Shot UUIDs

Every real forward shot carries its persistent `shot.uuid` into the export as
`<UUID>` (`<UU>` in `.tmlu`). This is what makes a re-export of an unchanged
project **byte-identical**, which the SpeleoDB push depends on: the `.tml` is
rendered as a sidecar so the server can regenerate GeoJSON.
`Project::ensure_shot_uuids` settles the UUIDs before every save and before that
render, so nothing is minted per-export (a project loaded but never saved is the
one case where a UUID is minted on the fly, and it is persisted by the next
save).

Uniqueness lives at two different levels, and they are deliberately different:

- **Model level: per-section only.** `Project::ensure_unique_shot_uuids` resets
  its dedup set inside the section loop, and the companion
  `duplicate_shot_uuid` integrity check is section-scoped for the same reason -
  `resolve_key` is section-local, so a cross-section repeat cannot mis-resolve
  a station anchor and is not treated as a violation. Cross-section repeats are
  reachable in normal use since #423 (importing the same Ariane file twice, or
  merging two projects that each imported it) and are legal by design.
- **Export boundary: per-document, enforced.** `build_export_shots` settles
  repeats after assembling the full shot list, each profile its own way: the
  user export re-mints later occurrences (`remint_duplicate_uuids` - the first
  copy keeps its Ariane identity, later copies draw a fresh v4 *per export*,
  so byte-stability holds only for the first copy), and the SpeleoDB sidecar
  drops them (`harden_for_speleodb`), because a repeat would alias two shots
  onto one GeoJSON feature id.

So a consumer of either output may key on `<UUID>` within one document, but the
value guarantees **stability for settled identities, not project-wide model
uniqueness** - do not describe `Shot::uuid` itself as unique beyond its section.

The importer reads the element back (#423): a `<UUID>`/`<UU>` on a `<SurveyData>`
row becomes that shot's identity verbatim, so an Ariane-authored file that leaves
and returns keeps the same per-shot ids instead of getting a fresh set. The value
is not validated or coerced into a canonical v4 - coercing would recreate exactly
the identity churn the carry exists to prevent - so `Shot::uuid` is no longer
guaranteed to be a v4 we minted. Two consequences follow: an empty element means
"no identity" and gets a fresh mint, and a non-v4 value is dropped from the
SpeleoDB sidecar by `speleodb_uuid`, costing that shot its GeoJSON feature id in
the server preview (Ariane writes v4s, so this bites only hand-edited or
third-party files). A file that repeats a `<UUID>` inside one section is
repaired, not rejected: `Section::new` keeps the FIRST occurrence and mints a
fresh id for the rest, restoring the per-section uniqueness `resolve_key` needs.

Ariane 26.3.0's own Save fills any row missing `<UUID>` with a fresh v4
(verified 2026-07-31), so the synthesized-shot gaps above do not survive a
round-trip through Ariane itself. Every consumer of the file *as we export
it* - SpeleoDB, the GeoJSON feature ids - still sees the omissions.
It exists for the SpeleoDB render and for any consumer that wants a stable
per-shot key. Ariane 26+ round-trips it; Ariane 25 and older drop it silently,
which is harmless here.

## Topology

Ariane represents survey data as a directed graph rooted at START shots - one
per entrance, matching what Ariane itself writes for multi-entrance caves.

### START shots - one per entrance

The **root section** and any section left parentless after link resolution (a
genuinely detached component) each get a synthesized START shot with
`FromID = -1`, anchored at the project coordinates (every START repeats them -
an unanchored START lands at lat/lon 0 and the view opens on the Atlantic).
All other sections connect to their parent via `FromID` on their first
real/reference shot.

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

### Section connectivity

Parent-child relationships are derived from `station_merges`. When section B's entry station merges with a station in section A, section B's first shot has its `FromID` set to the Ariane ID of the corresponding station in section A.

When a root section is split at a non-zero zero datum, the generated pre-zero segment is exported in reverse direction. Its azimuth is always synthesized as the reciprocal (+180°, normalized) of the shot's drawn bearing - the same circular mean the forward legs carry. `azimuth_out` is never read as a backsight.

### Loop closures

`loop_closures` are exported as CLOSURE-type shots with `ClosureToID` / `CID` pointing to the target station's Ariane ID. These encode the same loop-closure metadata that Ariane uses internally.

### Junction depths

A station has one depth in Ariane's chain, and a consumer takes every shot's
start from its parent's `Depth` (SpeleoDB's `openspeleo_lib` never reads
`DepthIn`). Our model can hold several readings of one station: the main line
arrives at it with one, and every branch hung off it starts with its own. Where
they disagree the file still has to state one number (#576, decisions
TML-DEPTH-D1 and D2):

- **The junction is exported at the mean of its readings**: the main line's
  depth, plus one reading per section hung there - every section merged at
  that station, however many merges deep. A section counts once however many
  pieces it has there. One merged part-way along leaves the junction in both
  directions, a forward block and a reversed one, and its reading is the depth
  it arrives at the station with (the reversed block's start), as the main
  line's is. If its forward block starts from another depth, that is a second
  reading inside the section, and that block's first shot is refitted like any
  other (below).
- **Every other station keeps its true depth.** A shot after the first on a
  branch states its own absolute depth and carries no offset.
- **Only the shots touching the junction change, and only their slope.** Each
  shot into or out of an averaged junction, on the main line and on every
  branch, gets the length that keeps its measured horizontal run:
  `L' = sqrt(h^2 + dz'^2)`, with `h` from its own length and depth change. The
  plan stays where it is. `L'` is rounded up to the next written micrometre and
  never below the depth change as written, so `|Depth - parent.Depth| <=
  Length` holds on the file's six decimals in both profiles. A zero-length tie
  has no run, so its new length is its whole new depth change.
- Every refitted shot writes the `DepthIn` sentinel ("I start at my parent's
  `Depth`"), which our importer resolves to exactly that, so all three readers
  - Ariane, our importer and `openspeleo_lib` - see the same geometry.

Example: main-line station A reads 10.0 m and a branch's first shot starts at
12.0 m and ends at 12.5 m. A is exported at 11.0 m. The branch shot goes from
11.0 to 12.5 m (1.5 m) with a length that keeps its horizontal run; the
main-line shots into and out of A absorb +1.0 and -1.0 m the same way; every
later shot keeps its true depth.

The export toast says so: "N junctions had disagreeing depths (largest X m);
exported at their average, shot lengths adjusted." X is the widest
disagreement, highest reading minus lowest. Re-importing the file brings the
junction back at its mean - that is the loss.

This replaces the earlier **re-hang**, which started the branch at the
parent's depth, kept each shot's measured depth change, and so shifted every
later shot on the branch by the size of the mismatch.

A shot whose own drop already exceeds its own tape is broken data (decision
TML-DEPTH-Q2-D1). Both its depths stay true and it is exported as the vertical
shot it can at most be: no horizontal run (`h = 0`), so its length becomes its
drop, `L' = |dz|`, rounded up the same way. No station moves for it, so the
shot after it keeps its start and its tape. (This replaces an earlier clamp
onto the tape, which moved the station the shot ends at.)

A second reading of a station inside one section - a shot whose `DepthIn`
differs from where the shot before it ended, so it is written out explicitly -
is not a junction and is not averaged (decision TML-DEPTH-Q3-D1). It is kept
while its shot still satisfies the bound against its parent and neither end
moved. Otherwise that one shot is refitted the same way: it starts from its
parent's exported depth (the `DepthIn` sentinel), keeps its true end depth,
and takes the length that keeps its measured horizontal run. No station moves
for it. The same goes for a part-way-merged section's forward block that
starts from another depth than the one the section arrives with.

### ID assignment

IDs are assigned sequentially starting from 0:
- Root section: START (ID 0), then shots (ID 1, 2, …)
- Subsequent sections: shots continue the global counter
- CLOSURE shots are appended at the end

## Color Format Conversion

Internal `#RRGGBB` hex colors are converted to Ariane's `0xRRGGBBff` format (8-digit hex with alpha suffix). On import, the reverse conversion strips the alpha channel.

## The LRUD zero/null collision

`ExportShot.left` and its three siblings are plain `f64`, filled with
`shot.left.unwrap_or(0.0)` (`export/tml.rs:893-896` and the three reversed-leg
constructors beside it), because Ariane's `Left`/`Right`/`Up`/`Down` elements
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
in `.tml`. To round-trip both `shot.comment` and `project.station_comments`
through Ariane, the exporter packs both into the shot's `<Comment>`:

- Each station comment is attached to the shot whose **`to` station** matches
  it - the shot that "creates" that station, i.e. the one at `shot_index + 1`.
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
Every station whose Ariane id **is** the START shot's therefore loses its comment
on export. Three ways a station ends up there:

- **Root station 0.** In an unsplit root section, station 0 *is* the START shot,
  so a comment on it is dropped. Add the note one shot downstream if you need it
  to survive.
- **The primary zero datum's station.** When the root section is split at a
  non-zero zero datum, the datum's own station is aliased onto the synthesized
  START shot, so a comment on that station is dropped too - even though a real
  shot of the project sits at that index.
- **A station merged onto either of the above.** The parent-link pass maps a
  merged child station to the *parent's* Ariane id, so a child station merged
  into root station 0 (or into the aliased datum station) inherits the START id
  and is dropped with it.

Comments that survive but move:

- **Stations before the zero-datum split index.** Those stations are emitted on
  the reverse `"<Section> (pre-zero-datum)"` leg, so their comments are folded
  onto that synthetic leg's shots. The text survives, but it re-imports onto a
  *fabricated* section that never existed in the project - not onto the section
  the surveyor wrote it in.
- **Two StationRefs on one Ariane shot.** When a child section's station 0 is
  merged into a parent station that *is* a real shot, both refs alias that shot
  id, so both station comments are concatenated into its exported Comment
  (sorted by `(section_id, station_index)` for reproducibility).

A fourth, rarer drop: a station the Ariane topology does not represent at all
(nothing in the exporter's station-to-node map), which is what happens when the
shape of a merge leaves a station with no node of its own.

Every one of these drops emits a `tracing::debug!` naming the section id and
station index (plus the Ariane node id where there is one), so a lost comment is
diagnosable from a debug-level log rather than silent. Which comments get dropped
is deliberately unchanged - the logging exists so the loss is observable, not to
change the behaviour.

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
under different profiles. The **user** profile's bytes change only deliberately
(`tml_golden_tests.rs` pins the whole document). The **sidecar** profile
additionally:

| Concession | Why |
|------------|-----|
| Rounds a repaired `Length` up to the next micrometre | The server re-derives `abs(Depth - parent.Depth) <= Length` from the DECIMAL STRINGS, and the six-decimal write can round a repaired tape back below the drop it was minted to cover |
| Never emits exactly one `<SurveyData>` (pads with a VIRTUAL root) | A lone element parses as a dict in the server's decoder and crashes the conversion; zero is handled cleanly, two or more is the normal path |
| Re-emits every `<Date>` as an ISO calendar date (`YYYY-MM-DD`), year clamped to `[1900, 2035]` | A missing, empty or non-ISO date fails the whole file; outside IGRF's range the declination silently becomes 0.0. Only `YYYY-MM-DD` is parsed - anything else, including a full ISO 8601 datetime, falls back to the placeholder and loses its real survey date |
| Emits `<UUID>` only for a v4 not already used in the file | The field is typed `UUID4`; a repeat aliases two shots onto one GeoJSON feature (shot UUIDs are unique per SECTION only) |
| Nudges a zero latitude or longitude by a micro-degree, and clamps out-of-range ones | Both components must exceed `1e-7` or the map is skipped, so a survey on the equator or the prime meridian gets none. No datum still means no anchor |
| Replaces U+220E in section and explorer names with a space | The server hashes the section-identity fields joined with that character |
| Normalises `Azimut` for every shot type | Only REAL shots are normalised server-side |

None of this touches the project: the transformations run on the export
representation, and only for the throwaway sidecar. Past 25,000 shots the
sidecar render warns that the server may fail to build the preview - nothing is
dropped or capped.

## Implementation

| Component | File | Role |
|-----------|------|------|
| Export logic | `src-tauri/crates/export/src/export/tml.rs` | XML generation, ZIP packaging, topology resolution, the two profiles |
| Shared schema | `src-tauri/crates/parsers/src/parsers/tml/schema.rs` | The one statement of every element name (both spellings), the shot-type strings, the `DepthIn` sentinel and the inclination and colour conversions; the importer reads with the same table |
| Byte pins | `src-tauri/crates/export/src/export/tml_golden_tests.rs` | Whole-document goldens of the user and sidecar exports; a change to the written bytes updates them in the same commit |
| Export command | `src-tauri/src/commands/export.rs` | `export_tml` IPC handler |
| SpeleoDB sidecar | `src-tauri/crates/core/src/core/state/speleodb_push.rs` | `speleodb_render_tml` renders the co-push sidecar via `export_tml_for_speleodb` |
| Store method | `src/store/projectStore.ts` | `exportTml()` frontend binding |
| Export dialog | `src/components/Dialogs/ExportDialog.tsx` | UI with TML format option |

## Limitations

- Characters XML 1.0 cannot carry at all - C0 control characters other than tab,
  line feed and carriage return, and U+FFFE / U+FFFF - are written as U+FFFD
  (the replacement character) in every text element, in both profiles. They
  reach a project through imported files and pasted text, and a single one
  written raw made Ariane and SpeleoDB reject the whole document. The user
  export returns the `xml-chars-replaced` note, so the export toast says the
  file's text differs from the project's; the sidecar logs it. Line breaks,
  the U+200B shot-comment marker and C1 characters are legal XML and pass
  through unchanged.
- Shape/cross-section geometry (`Shape`/`SH` elements) is not exported (not stored in the internal model)
- GPS coordinates are not written per-shot: latitude and longitude are `0.0`
  everywhere except the START shots, which repeat the primary datum's anchor
  (see Topology); without a primary datum they are `0.0` there too. Only the
  project-level elevation reference is otherwise set
