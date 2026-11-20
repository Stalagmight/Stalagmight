# The `.stalx` file format, version 2

This is the public specification of the Stalagmight project file, version 2:
canonical, line-oriented JSON. It is written for anyone who reads or writes
these files outside Stalagmight (SpeleoDB's Python reader, scripts, other
survey software). Stalagmight's own reader and writer live in
`src-tauri/crates/persistence/src/persistence/stalx_v2/`; the design is
[plan section 6](plans/2026-11-20_station-graph-model.md) and the model behind
it is [the survey model guide](survey-model.md).

Files written before version 2 are SQLite databases with the same `.stalx`
extension ("legacy"). Section 1 tells the two apart.

Contents:

1. [Recognising a file](#1-recognising-a-file)
2. [Encoding rules](#2-encoding-rules)
3. [Versioning](#3-versioning)
4. [Members](#4-members)
5. [Member reference](#5-member-reference)
6. [What makes a survey valid](#6-what-makes-a-survey-valid)
7. [Unknown content](#7-unknown-content)
8. [Canonical form](#8-canonical-form)
9. [Deriving positions](#9-deriving-positions)
10. [Reading a file in Python](#10-reading-a-file-in-python)
11. [Limits](#11-limits)
12. [How Stalagmight saves](#12-how-stalagmight-saves)
13. [A complete example](#13-a-complete-example)

## At a glance

- One UTF-8 JSON object, no byte order mark. It starts with
  `{"format":"stalx","version":2,` and its last member is `"end":true`.
- One record per physical line: each station, each line header, each shot,
  each datum, closure and history entry. A one-shot edit changes a handful
  of lines, so the files diff and merge well in git.
- The file stores observations and user decisions only. Station positions,
  closure misclosures, loop alignment and every other derived value are
  recomputed by the reader (section 9).
- Stations are entities with stable integer ids. A line (survey run) is a
  chain of stations: its `start`, then the `to` of each shot. A station on two
  or more lines is a junction (what older versions called a merge).
- Floats are written in their shortest exact form, `-0.0` included; NaN and
  infinity are never written. Times are RFC 3339 UTC with nanoseconds.
- `requires` names the features a reader must understand; `conventions` fixes
  units and sign conventions.

## 1. Recognising a file

Look at the first bytes:

| First bytes | Format |
|---|---|
| `SQLite format 3` followed by a zero byte | legacy SQLite `.stalx` (versions 0 to 18) |
| `{"format":"stalx","version":` | version 2 or later |
| anything else | not a Stalagmight project |

Stalagmight writes the version 2 prefix exactly as shown. A reader should also
accept insignificant JSON whitespace between those tokens (a pretty-printed
file), but the tokens themselves are spelled literally: no escapes in
`"format"`, `"stalx"` and `"version"`. A file that starts with a UTF-8 byte
order mark (`EF BB BF`) is refused.

The `version` integer follows the prefix, so a reader can refuse a newer major
version before parsing the rest (section 3).

## 2. Encoding rules

**JSON.** The file is one JSON object (RFC 8259) in UTF-8. A key twice in one
object this specification defines is an error. Every array and object has a
fixed meaning given below; positions in arrays are significant.

**Floating-point numbers** (measurements, coordinates, thresholds) are IEEE
754 doubles.

- Only finite values: `NaN`, `Infinity` and out-of-range literals such as
  `1e400` make the file invalid.
- A reader parses every float field as a double, whatever its spelling: `1`,
  `1.0`, `1e0` and `10e-1` are the same value. `-0.0` is a distinct value and
  must survive a read and write unchanged.
- Stalagmight writes the shortest spelling that reads back to the same 64
  bits (the "ryu" algorithm): `12.5`, `1.0`, `-0.0`, `0.30000000000000004`,
  `1e16`, `1.5e-7`, `5e-324`. Whole numbers carry `.0`, so a float is never
  mistaken for an integer and the sign of zero survives readers that parse
  `-0` as the integer 0.

**Integers** (ids, counters, counts, the marker index, the IGRF epoch) are
written in decimal without a fraction or exponent and are at most
`9007199254740991` (2^53 - 1), so every JSON reader holds them exactly.
Station and Section ids are below 2^31; line ids are at most 42948; closure
ids are below 2^53 - 1.

**Strings** are any Unicode text. Stalagmight escapes only what JSON requires:
`"` as `\"`, `\` as `\\`, and the control characters U+0000 to U+001F (`\b`,
`\f`, `\n`, `\r`, `\t` in short form, the others as `\u00xx` in lower-case
hex). Everything else, including non-ASCII text, is written as UTF-8.
Readers accept any valid JSON escape.

**Times** are RFC 3339 strings, written in UTC with nine fractional digits and
`Z`: `"2025-11-20T12:00:00.123456789Z"`. A reader accepts any RFC 3339 offset.
Times are clamped into the years 0001 to 9999 and never hold a leap second, so
common date libraries parse them. Python's `datetime.fromisoformat` does from
Python 3.11 on (3.10 refuses the `Z`), but keeps only six fractional digits:
a script that writes a time back must keep the original string. The
nanoseconds matter: a dive plan (`.plnx`) is matched to its project by the
exact creation instant.

**Dates** (survey credit dates) are `"YYYY-MM-DD"` strings, clamped like
times into 0001-01-01 to 9999-12-31.

**Absent versus empty.** An optional member that is unset is left out; an
empty list is left out. A member that is present holds a value, even an empty
string: `"comment":""` is a comment that is empty, which is not the same as no
comment.

## 3. Versioning

| Member | Meaning | A reader... |
|---|---|---|
| `version` | The major format version: `2`. | refuses a higher value (made with a newer Stalagmight) and any value below 2. |
| `solver` | The version of the formulas that derive positions (section 9): `1`. | derives positions with its own formulas; a different value alone is informative (below). |
| `requires` | Names of features a reader must understand to use the file correctly. Sorted, no duplicates. | that meets a name it does not know must not modify the file's survey; Stalagmight opens it read-only. |
| `conventions` | Units and sign conventions, fixed for version 2 (below). | refuses any other value, a missing key or an extra key. |
| `generator` | Who wrote the survey, e.g. `"Stalagmight 0.6.0"`. | may show it; nothing depends on it. |

Version 2.0 defines no `requires` features; files written by this version have
`"requires":[]`. A later writer adds a feature name whenever it stores
something an older reader would misread (a new meaning, a member that
references ids, or a relaxed rule of section 6), and bumps `version` only for
changes no older reader could open at all. A reader that meets an unknown
feature reads the rest as far as it can: when the survey it reads then breaks
a rule of section 6, the feature may be what relaxes the rule, so the file is
reported as made with a newer Stalagmight, never as damaged (Stalagmight
refuses it with its "saved by a newer version" message and no repair offer).

**The solver rule.** Positions, depths, closure figures and the other derived
values are not stored (section 9), so `solver` is the only trace of the
formulas the author saw them with. A writer bumps it whenever it changes a
formula. A different value is surfaced to the user only when it changes what
the user sees, and only the writer can know that: a writer whose formulas
would make a reader with an older `solver` show the file's survey differently
(another position, depth or closure figure) must also add a `requires`
feature, so that reader opens the file read-only with its "made with a newer
Stalagmight" banner (Stalagmight does). A bump without a feature tells a
reader that nothing it shows changes (a faster or more exact solve, a fix of
a case the file does not have); the reader opens the file normally and may
log the difference. On save a reader writes its own `solver`: the value names
the formulas that derived what the user saw last. Solver 1 is the first; a
later Stalagmight that reads a file of an older solver applies the same rule
from its side, knowing both formulas.

**Conventions** (the object is always exactly this):

| Key | Value | Meaning |
|---|---|---|
| `length` | `"m"` | Lengths, depths, LRUD, coordinates in metres. |
| `angles` | `"deg"` | Angles in degrees. |
| `azimuth` | `"cw-from-north"` | Azimuths clockwise from north, as measured (see `azimuth` references). |
| `inclination` | `"+down"` | Pitch (inclination) is positive downwards. |
| `depth` | `"+down"` | Depth is positive downwards, metres below the surface. |
| `local` | `"enu-m"` | Local coordinates are east / north metres on a tangent plane. |
| `gps` | `"wgs84-deg"` | Latitude and longitude in decimal degrees on WGS84. |
| `temperature` | `"degC"` | Water temperature in degrees Celsius. |

## 4. Members

The top-level members, in the order Stalagmight writes them. "Tier" is how a
reader treats keys it does not know inside the member (section 7).

| Member | Required | Tier | Content |
|---|---|---|---|
| `format` | yes | - | Always `"stalx"`. First member. |
| `version` | yes | - | Major version, `2`. Second member. |
| `solver` | yes | - | Solver version, `1`. |
| `requires` | no (`[]`) | - | Required feature names. |
| `conventions` | yes | refused | The fixed conventions object. |
| `generator` | no | - | The writing program. |
| `summary` | no | 2 | A cache for project lists (5.1). Never authoritative. |
| `project` | yes | 2 | Name, location, creation and modification times (5.2). |
| `settings` | no | 2 | Validation thresholds, surface elevation, default azimuth reference (5.3). |
| `counters` | no | 1 | Id allocators (5.4). |
| `speleodb` | no | 2 | The SpeleoDB link identity (5.5). |
| `local` | no | 3 | Per-machine state (5.6). |
| `sections` | no | 1 | Cave Sections, one per line (5.7). |
| `stations` | no | 1 | Stations, one per line, ascending id (5.8). |
| `lines` | no | 1 | Line headers, each followed by its shot rows (5.9). |
| `datums` | no | 1 | Zero datums (GPS ties), list order significant (5.10). |
| `placements` | no | 1 | Anchors of pieces without a datum (5.11). |
| `closures` | no | 1 | Loop closures (5.12). |
| `ignored_warnings` | no | 1 | Validation warnings the user dismissed (5.13). |
| `imports` | no | 1 | Hashes of imported files and lines (5.14). |
| (unknown) | - | 2 | Members a newer writer added; kept in file order. |
| `history` | no | 3 | Undo and redo history (5.15). |
| `end` | yes | - | Always `true`. Last member. |

A reader should not depend on the member order (except `format` and `version`
first). A reader that wants only the header (a project list, a link check)
may stop at the first survey member (`sections` onwards) once it has met
every header member it needs; otherwise a member it needs may still follow
the body. Stalagmight's header read does exactly that: a file it wrote,
which always puts the header first, is read from its first few kilobytes,
and a reordered file is read to its end (its project list alone stops at the
body, so it may list such a file without a link). A file without
`"end":true` is incomplete and refused, which, with the closing brace, makes
a truncated file impossible to mistake for a project.

## 5. Member reference

Key tables list keys in the order Stalagmight writes them. "opt" marks keys
left out when unset.

### 5.1 `summary`

A cache so a project list can show a file without parsing the survey. The
writer always recomputes it; a reader that has parsed the survey must ignore
it. A `summary` that is not an object holds nothing: Stalagmight ignores it,
leaves it out of a local-only rewrite or a rename (section 12), and writes a
new one on the next save.

| Key | Type | Meaning |
|---|---|---|
| `name` | string | The project name (as `project.name`). |
| `lines` | integer | Number of lines. |
| `stations` | integer | Number of stations (a junction counts once). |
| `shots` | integer | Number of shots. |
| `length` | float, opt | Surveyed length in metres: the sum of shot lengths, Reference shots excluded. Left out if the sum is not finite. |
| `center` | `[lon, lat]`, opt | The first datum's position. |
| `bounds` | `[west, south, east, north]`, opt | Box around every datum; `east < west` when it crosses the antimeridian. |
| `speleodb` | string, opt | The linked SpeleoDB project id. |

### 5.2 `project`

| Key | Type | Meaning |
|---|---|---|
| `name` | string | Project name. |
| `location` | string, opt | Free-text location. |
| `created` | time | Creation instant (identifies the project's dive plans). |
| `modified` | time | Last edit. A save without an edit does not change it. |

### 5.3 `settings`

| Key | Type | Meaning |
|---|---|---|
| `validation` | object | Warning thresholds, all floats: `warn_shot_length_min`, `warn_shot_length_max`, `warn_depth_change_per_shot_max`, `warn_azimuth_deviation_max`, `azimuth_diff_threshold`, `pitch_diff_threshold`, `inclination_depth_diff_threshold`. Always written in full; a missing one reads as Stalagmight's default. |
| `surface_elevation` | float, opt | Entrance elevation, metres above sea level (+up). |
| `azimuth` | azimuth reference, opt | The reference of every line that declares none. |

An **azimuth reference** says how a line's stored azimuths relate to true
north. Stored azimuths are always as measured; geometry adds `correction`.

| Key | Type | Meaning |
|---|---|---|
| `north` | `"magnetic"`, `"true"` or `"grid"` | What the readings are measured against. Descriptive. Read as `"magnetic"` when absent. |
| `correction` | float | Degrees to add to a stored azimuth to get a true bearing (declination, plus grid convergence for grid). Read as 0.0 when absent. |
| `source` | `"unknown"`, `"file"`, `"manual"` or `"igrf"` | Where the correction came from. Read as `"unknown"` when absent. |
| `epoch` | integer, only with `"igrf"` | The model year of an IGRF-derived declination. |

A line or project without a reference uses `{"north":"magnetic","correction":0.0,"source":"unknown"}`.

### 5.4 `counters`

The next id each allocator hands out. Ids are never reused, so a counter is
above every id of its kind ever issued, deleted ones and aliases included.

| Key | Type | Meaning |
|---|---|---|
| `station` | integer | Next station id. |
| `closure` | integer | Next loop closure id (starts at 1). |
| `section` | integer | Next Section id. |

There is no line counter: the next line id is one above the highest line id
in the file. A counter left out reads as one above the highest id of its kind.

### 5.5 `speleodb`

Present when the project is a SpeleoDB-managed copy. It rides every upload.

| Key | Type | Meaning |
|---|---|---|
| `endpoint` | string | The SpeleoDB server. |
| `project` | string | The SpeleoDB project id. |
| `name` | string | The SpeleoDB project name when it was linked. |
| `undo_mode` | `"full"` or `"disabled"`, opt | Whether the undo history rides uploads. Any other value reads as `"disabled"`. |

### 5.6 `local`

Per-machine state. Never in an upload, never in a detached copy, and lenient:
a reader drops a value of a known key it cannot read, and keeps a key it does
not know (another build's per-machine or sync state, at `local` or inside
`local.speleodb`) verbatim, written back after the known keys.

| Key | Type | Meaning |
|---|---|---|
| `display` | object of strings, opt | View toggles, key to stored text. |
| `speleodb` | object, opt | Sync state: `base` (string, the server revision this copy is based on), `unpushed` (`true` when there are edits not yet uploaded), `last_edit` and `last_verify` (times), `unwound_from` (string, the revision an Unwind started from), `inherited_through` (time: undo entries at or before it came with a downloaded revision). Every key optional. |

### 5.7 `sections`

Cave Sections (#327): named, coloured groupings of lines. One object per
physical line, in panel order.

| Key | Type | Meaning |
|---|---|---|
| `id` | integer | Section id. |
| `name` | string | Unique within the project. |
| `color` | string | `#rgb` or `#rrggbb`. |

### 5.8 `stations`

One row per station, in ascending id order:

```json
[2,"A2",{"comment":"tie-off on a rock","aliases":[[7,"B0"]]}]
```

| Position | Type | Meaning |
|---|---|---|
| 0 | integer | Station id: the "Station ID" users see. Unique, never reused. |
| 1 | string | Station name: unique among the stations of every line the station is on. |
| 2 | object, opt | Extras, written only when one is set. |

Extras keys: `comment` (string), `aliases` (ids merged into this station, each
`[id]` or `[id, "name"]`, oldest first; an alias id is never a live station's
id), `external` (string: the identity an importer gave the station, e.g. a
Compass or Walls station name or an Ariane station ID; an attribute only).

### 5.9 `lines`

A flat array in which each **line header** (an object) is followed by that
line's **shot rows** (arrays), then the next header:

```json
"lines":[
{"id":0,"name":"Main","color":"#ff6b6b","direction":"in","quality":1.0,"start":0},
["5f0c2a1e-0000-4000-8000-000000000001",1,12.5,90.0,90.0,3.0,4.5],
["5f0c2a1e-0000-4000-8000-000000000002",2,8.25,92.5,92.5,4.5,6.0],
{"id":1,"name":"Side","color":"#95e1d3","direction":"out","quality":1.0,"start":2},
["5f0c2a1e-0000-4000-8000-000000000004",4,6.0,0.0,0.0,6.0,7.0]
],
```

Lines are in display order (list order is significant). A row before the
first header is an error.

**Line header:**

| Key | Type | Meaning |
|---|---|---|
| `id` | integer | Line id, unique. |
| `name` | string, opt | Display name. |
| `color` | string | Display colour. |
| `direction` | `"in"` or `"out"` | Surveyed going into the cave or coming out. Read as `"in"` when absent. |
| `quality` | float | Weight of the line's shots in a least-squares adjustment (1.0 normal). Read as 1.0 when absent. |
| `section` | integer, opt | The Section the line belongs to. |
| `azimuth` | azimuth reference, opt | Overrides `settings.azimuth` for this line. |
| `surveyor` | string, opt | Legacy free-text surveyor (older files; a fallback for lines without shot credits). |
| `date` | date, opt | Legacy survey date, likewise. |
| `credit` | credit, opt | The credit of every shot of the line whose row does not say otherwise. |
| `start` | integer | Station id of station 0 of the chain. |
| `hand` | array, opt | Legacy hand moves: `[[station, dx, dy, dz], ...]`, ascending station id, metres added to that station's position on this line only. Only converted old files have them. |

**Shot row:** a shot is an edge from the previous station of the chain (the
line's `start` for the first row) to `to`.

| Position | Type | Meaning |
|---|---|---|
| 0 | string | Shot uuid: the shot's persistent identity, unique in the project, 1 to 128 characters. Usually a UUID; imported ids are kept verbatim. |
| 1 | integer | `to`: the station id the shot ends at. |
| 2 | float | Slope length (m). |
| 3 | float | Azimuth in (deg). |
| 4 | float | Azimuth out (deg). Both are forward readings; geometry uses their circular mean. |
| 5 | float | Depth at the start (m, +down). |
| 6 | float | Depth at the end (m, +down). |
| 7 to 10 | float or `null`, opt | Left, right, up, down passage dimensions (m). Trailing unset ones are left out; an unset one before a set one is `null`. |
| last | object, opt | Extras, written only when one is set. Recognised by its type: it is the only object in the row. |

Row extras keys:

| Key | Type | Meaning |
|---|---|---|
| `kind` | `"jump"` or `"reference"`, opt | Absent: a normal surveyed leg. `"jump"`: a swum shortcut between lines. `"reference"`: a survey tie, not swimmable, not counted in lengths. |
| `pitch_in`, `pitch_out` | float, opt | Recorded inclinations (+down). |
| `temperature` | float, opt | Water temperature (degC). |
| `marker` | integer, opt | Marker index from the dive computer. |
| `comment` | string, opt | Shot comment. |
| `credit` | credit or `null`, opt | This shot's credit when it differs from the header's; `null` means the shot has none. |

**Credit inheritance:** a shot's credit is its row's `credit` when the key is
present (`null` meaning none), else the line header's `credit`, else none.

**Credit object:** `surveyors` and `explorers` (arrays of people, each
`{"name": string, "email": string (opt)}`, in order; left out when empty) and
`date` (date, opt). `{}` is a credit that names nobody, which is distinct from
no credit.

### 5.10 `datums`

Zero datums tie a station to a GPS position. List order is significant: the
first datum whose station is on a line is the **frame** (the origin of local
coordinates), and the first datum of each piece of the survey places that
piece; later datums of a piece are GPS fixes (checks).

| Key | Type | Meaning |
|---|---|---|
| `station` | integer | The station. A station may hold several datums. |
| `via` | integer, opt | The line the datum was set on (it passes through `station`): kept so a later detach gives the datum back to that line. |
| `lat`, `lon` | float | WGS84 position (degrees). |
| `depth` | float | Depth of the station below the surface at that point (m). Read as 0.0 when absent. |
| `accuracy` | float, opt | GPS accuracy (m); absent is a hard pin. |
| `entrance` | boolean, opt | Overrides whether this datum is shown as an entrance. |
| `correction` | `[dx, dy]`, opt | For the datum that places its piece: the horizontal offset (m, local east / north) between where the piece sits and where its GPS says. Kept so that nothing moves when the frame changes. Absent is zero. |

### 5.11 `placements`

Where each piece of the survey that holds no datum sits: exactly one per such
piece.

| Key | Type | Meaning |
|---|---|---|
| `station` | integer | A station of the piece. |
| `x`, `y` | float | Its local east / north position (m). |

### 5.12 `closures`

Loop closures between two stations, in list order (significant).

| Key | Type | Meaning |
|---|---|---|
| `id` | integer | Closure id, unique, never reused. |
| `a`, `b` | integer | The two stations (different). |
| `a_via`, `b_via` | integer, opt | The line each end was set on (it passes through that end). |
| `strategy` | string | Read as `"batch"` when absent. `"batch"`: a soft loop closure, adjusted only by the least-squares view. `"move"`: a snap; rigid while its ends are less than 1 cm apart. `"tie"`: a rigid tie (converted old files only). `"least_squares_weighted"`, `"proportional_by_length"`, `"equal_distribution"`: legacy names, read like `"batch"`. |

Closure metrics (misclosure, error percentage, path length) are not stored;
they are derived.

### 5.13 `ignored_warnings`

Validation warnings the user dismissed, sorted by shot then code, one per
(shot, code) pair. An entry for a shot the project no longer has is dropped
by readers and writers.

| Key | Type | Meaning |
|---|---|---|
| `shot` | string | The shot's uuid. |
| `code` | string | Warning code, e.g. `"SHORT_SHOT"`. |
| `at` | time | When it was dismissed. |
| `comment` | string, opt | The user's note. |

### 5.14 `imports`

`{"files": [...], "lines": [...]}`: hashes of imported files and lines, used to
warn before importing the same data twice. Each list is optional; order is
kept.

### 5.15 `history`

`{"undo": [...], "redo": [...]}`, one entry per physical line. Each list is a
stack whose last element is the next step (the most recent edit for undo, the
most recently undone one for redo). Entries are Stalagmight's own and may
change between versions; the only rule others can rely on is that an entry is
a JSON object whose `"t"` member, when present, is the RFC 3339 time it was
recorded, non-decreasing along the undo list. A reader that cannot read the
history drops it whole; a third-party writer that edits the survey should
remove `history` (an undo entry that no longer matches the survey is refused
when the user tries it).

### 5.16 `end`

Always `true`, always last.

## 6. What makes a survey valid

Stalagmight refuses to open a file whose survey breaks these rules (and offers
a repair that keeps the damaged file as a backup copy, section 12), and never
writes one. A file that also requires a feature Stalagmight does not know is
refused as made with a newer version instead, without the repair (section 3).
The rules:

1. Ids are unique: station ids, line ids, shot uuids (project-wide), closure
   ids, Section ids. Alias ids are not live station ids and appear once.
   Station ids, alias ids included, and Section ids are below 2^31; line ids
   are at most 42948; closure ids are below 2^53 - 1; a line holds at most
   99,999 shots (Stalagmight numbers a line's shots in its session by
   `line id * 100000 + index`).
2. Every reference resolves: a line's `start`, a shot's `to`, every datum,
   placement and closure station, every `via` line (which must pass through
   that station), every hand-move station (on that line), every line's
   `section`.
3. A line never passes the same station twice.
4. Every station is on at least one line (a line with no shots holds just its
   `start`).
5. Stations and shots form a forest: there is no loop made of shots alone.
   Every loop in a cave is an explicit closure.
6. Each piece (connected group of stations) has exactly one anchor: its first
   datum in list order, or else exactly one placement. A piece with a datum
   has no placement.
7. A closure joins two different stations; its ends may be in different
   pieces only if both pieces hold a datum.
8. Station names are unique among the stations of each line, and Section
   names are unique in the project (compared exactly).
9. Counters are above every id in use; the station and Section counters
   are at most 2^31 and the closure counter at most 2^53 - 1 (at the bound
   the ids are used up, and Stalagmight refuses an edit that needs another).
10. Every number is finite.

## 7. Unknown content

A newer Stalagmight may write keys this specification does not list. What a
reader does depends on where they are:

1. **Survey records** (stations, line headers, shot rows, datums,
   placements, closures, Sections, credits, people, azimuth references,
   counters, ignored warnings, imports) and unknown values of enumerated
   fields (`kind`, `direction`, `strategy`, `north`, `source`): the file
   opens, read-only. A reader may display the survey but must not rewrite
   it. Unknown enumerated values read as a safe fallback for display: an
   unknown `kind` as `"reference"` (never a swimmable leg), `strategy` as
   `"batch"`, `direction` as `"in"`, `north` as `"magnetic"`, `source` as
   `"unknown"`.
2. **Header objects** (`project`, `settings`, `settings.validation`,
   `speleodb`, `summary`) and unknown top-level members: kept verbatim and
   written back after the known keys, in their original order.
3. **`local` and `history`**: a value of a known `local` key that cannot
   be read is dropped; an unknown key of `local` or `local.speleodb` is kept
   verbatim and written back after the known keys, by a save and by a
   local-only rewrite, so a newer build's per-machine and sync state survives
   an older build's display toggle (a detached copy and an upload carry no
   per-machine state, so neither has them). A `history` this build cannot
   read is not loaded (the project opens with nothing to undo) but stays in
   the file until a save of an edit writes the new history: a local-only
   rewrite copies it byte for byte. The project always opens.

Arrays have a fixed number of positions; a row with extra positions is an
error, not unknown content. New fields go in the objects.

A writer that adds anything that changes the meaning of existing data, or
that references ids (which other programs would not keep up to date), must
also add a `requires` feature name.

## 8. Canonical form

Stalagmight writes every file in one canonical form, so the same survey
always gives the same bytes and an edit changes few lines:

- The members and keys in the order of the tables above; unset optional
  members and empty lists left out.
- No whitespace except newlines: after the first line (which holds
  `format`, `version`, `solver` and `requires`), after each top-level member,
  and around the records of each record list and history list (after the
  opening bracket, between records, before the closing bracket). The file
  ends with `"end":true}` and a newline.
- Stations in ascending id order (a new station is appended at the end);
  every other list in its stored order.
- Numbers, strings and times as in section 2; display settings sorted by key;
  `requires` sorted.
- A line header's `credit` is the credit most of its shots carry (no credit
  counts as a value); ties go to the one that comes first.
- Preserved unknown content and history entries are kept with their key
  order and number spellings, without whitespace.

Diff sizes measured on a synthetic 5,000-shot survey, counted as lines
removed plus lines added: a save without an edit 0; a shot's azimuth 2 (its
row); a shot's length 4 (the row and the summary); a station rename 2;
appending or inserting a shot 8 (the row, its new station, the station
counter, the summary); adding a closure 5; moving a placement 2; renaming the
project 4. An edit in the app also updates `project.modified` (2 more).

A reader may accept any equivalent spelling; on its next save Stalagmight
writes the canonical form.

## 9. Deriving positions

Positions are not stored. This section summarises solver version 1, which
Stalagmight uses to lay out a survey (the reference implementation is
`models::survey::Project::raw_positions`; see [the survey model
guide](survey-model.md) for the details and the least-squares view).

1. **Chains.** Line `L`'s stations are `[start, row0.to, row1.to, ...]`.
2. **Shot vector.** For a shot from station `s` to station `t`:
   - `dz = depth_out - depth_in`; horizontal length
     `h = sqrt(length^2 - dz^2)`, or 0 when that is not positive.
   - Circular mean of the two azimuths:
     `d = norm(az_out - az_in + 540) - 180`, `a = norm(az_in + d / 2)`, where
     `norm(x)` is `x` modulo 360 into `[0, 360)`.
   - The line's reference (its own, else `settings.azimuth`, else none):
     when its `correction` is non-zero, `a = norm(a + correction)`.
   - Vector `(east, north) = (h * sin(a), h * cos(a))`, `a` in radians.
3. **Pieces.** Stations joined by shots form pieces (a junction is one
   station, so its lines are one piece). Closures do not join pieces.
4. **Frame.** The first datum (list order) whose station is on a line. The
   local position of a GPS point is its east / north offset from the frame's
   GPS on the tangent plane: both points converted to WGS84 Earth-centred
   coordinates at height 0, subtracted, and rotated into east / north / up at
   the frame (up dropped).
5. **Anchor of a piece.** Its first datum in list order puts that datum's
   station at `local(gps) + correction` (the frame's own station at
   `(0, 0) + correction`). A piece without a datum puts its placement's
   station at `(x, y)`.
6. **Walk.** From the anchor station, breadth first: for each station
   reached, take its lines in ascending line id, and walk each line not yet
   walked outwards from that station, adding the shot vectors going forwards
   along the chain and subtracting them going backwards. A station takes the
   first position it is given. With hand moves, a line's chain position of a
   station is its shown position minus that line's hand move, and the
   station is shown at chain position plus hand move.
7. **Depth.** A station's depth is the mean of every depth reading at it:
   the `depth_out` of each shot ending there and the `depth_in` of each shot
   starting there, over every line through it, plus the readings at stations
   tied to it by a `"tie"` closure or an aligned `"move"` closure
   (transitively). A `"move"` closure is aligned when its ends are less than
   1 cm apart horizontally and it does not join two pieces that each hold a
   datum.

## 10. Reading a file in Python

A minimal reader, standard library only. It refuses what a reader must refuse
(section 3: another major version, other conventions), tells the caller when
the file requires features it does not know (then the caller must not modify
the survey), and reads an unknown shot `kind` as `"reference"`, never as a
swimmable leg (section 7).

```python
import json
import math

CONVENTIONS = {
    "length": "m", "angles": "deg", "azimuth": "cw-from-north",
    "inclination": "+down", "depth": "+down", "local": "enu-m",
    "gps": "wgs84-deg", "temperature": "degC",
}
KNOWN_FEATURES = set()                    # version 2.0 defines none
KINDS = {"normal", "jump", "reference"}

def read_stalx(path):
    with open(path, "rb") as f:
        raw = f.read()
    if raw.startswith(b"SQLite format 3\x00"):
        raise ValueError("a legacy (SQLite) .stalx file")
    def no_constants(name):
        raise ValueError(f"{name} is not allowed in a .stalx file")
    def finite(text):
        value = float(text)
        if math.isinf(value):
            raise ValueError(f"{text} is out of range")
        return value
    def no_duplicates(pairs):
        keys = [k for k, _ in pairs]
        if len(keys) != len(set(keys)):
            raise ValueError("duplicate key")
        return dict(pairs)
    doc = json.loads(raw.decode("utf-8"), parse_constant=no_constants,
                     parse_float=finite, object_pairs_hook=no_duplicates)
    version = doc.get("version")
    if doc.get("format") != "stalx" or type(version) is not int:
        raise ValueError("not a .stalx file")
    if version > 2:
        raise ValueError("made with a newer Stalagmight")
    if version < 2:
        raise ValueError("not a version 2 file")
    if doc.get("end") is not True:
        raise ValueError("incomplete file")
    if doc.get("conventions") != CONVENTIONS:
        raise ValueError("other units or sign conventions than version 2's")
    # Not empty: made with a newer Stalagmight; read it, never rewrite it.
    unknown_features = [f for f in doc.get("requires", [])
                        if f not in KNOWN_FEATURES]

    stations = {}
    for row in doc.get("stations", []):
        extras = row[2] if len(row) > 2 else {}
        stations[row[0]] = {"name": row[1], **extras}

    lines = []
    for element in doc.get("lines", []):
        if isinstance(element, dict):            # a line header
            line = dict(element, shots=[])
            lines.append(line)
            continue
        uuid, to, length, az_in, az_out, depth_in, depth_out, *tail = element
        extras = tail.pop() if tail and isinstance(tail[-1], dict) else {}
        left, right, up, down = (tail + [None] * 4)[:4]
        previous = line["shots"][-1]["to"] if line["shots"] else line["start"]
        kind = extras.get("kind", "normal")
        line["shots"].append({
            "uuid": uuid, "from": previous, "to": to,
            "length": float(length), "azimuth_in": float(az_in),
            "azimuth_out": float(az_out), "depth_in": float(depth_in),
            "depth_out": float(depth_out),
            "lrud": (left, right, up, down),
            "kind": kind if kind in KINDS else "reference",
            "comment": extras.get("comment"),
            "credit": extras["credit"] if "credit" in extras else line.get("credit"),
        })
    return doc, stations, lines, unknown_features
```

Python's `json` parses `-0` (no fraction) as the integer 0. Stalagmight always
writes `-0.0`, which Python reads as the float `-0.0`.

## 11. Limits

Stalagmight refuses, while it reads (before holding them in memory), files
over 256 MiB and files holding more than 2,100,000 stations or aliases or
hand moves or datums, 100,000 lines, placements or Sections, 2,000,000 shots,
ignored warnings, import hashes or credited people, 50,000 closures, 10,000
preserved unknown keys and members (tier 2, section 7, and `requires` names)
in all, or an object with more than 10,100 members (known and unknown keys
together). Unknown keys of survey records (tier 1) are not held, so they count
only against their object's 10,100: a newer writer that adds a key to every
shot gives a file that opens read-only at any size. Up to 10,000 unknown
`local` keys are kept; more are dropped (tier 3 never refuses). The id and per-line bounds of section 6 (rule 1 and
rule 9) are validity rules: a file breaking them is refused after reading,
with the repair offer, and is never written.
Preserved unknown content nested deeper than 128 levels is refused. A history
of more than 65,536 entries is dropped (not refused). Servers should apply
caps that fit their own deadlines.

## 12. How Stalagmight saves

For context; nothing here changes the format. The app's side is
[saving.md](saving.md).

- **Opening reads the file once** and keeps it in memory; Stalagmight holds no
  handle on it while the project is open. Opening never writes: a file is
  rewritten only by a save or by one of the narrow rewrites below.
- **A save** validates the survey (section 6) and refuses to write an invalid
  one, writes `<name>.stalx.tmp` beside the file, flushes it to disk, checks
  that the file on disk is still the one it last read or wrote (size and
  SHA-256), renames the temp over it and flushes the folder. The file on disk
  is always either the old or the new version, and a torn temp never reads as
  a project. A file that another program changed (or moved or deleted) since
  Stalagmight read it is not overwritten: the user is asked to keep their
  version, save under another name, or reload the file's version. Unknown
  members and keys of tier 2 (section 7) survive a save; a file this build
  would lose something of (tier 1, an unknown `requires`) opens read-only and
  is never saved.
- **An upload to SpeleoDB** writes the same survey without `local` and without
  `redo`, with `undo` only when the project's undo mode is `full` (never when
  `speleodb.undo_mode` is `"disabled"`): entries at or before
  `local.speleodb.inherited_through` are left out, then the oldest entries
  until the file fits the 5 MiB upload limit. An Unwind position recorded
  without a watermark ships no undo at all (the inherited extent is
  unknown). It is sent as `application/json`, named `project.stalx`.
- **"Save a Copy" of a SpeleoDB project** writes no `speleodb`, no
  `local.speleodb` and no undo entries inherited from the server.
- **Local-only rewrites.** Changes to the link, the sync state, display
  settings and the history alone rewrite those members and copy every other
  member byte for byte, even when the file is open read-only. When the
  history did not load (unreadable entries, a newer history format, a
  read-only file), the rewrite copies `history` byte for byte too, so a
  display toggle never erases what another build can still undo; unknown
  keys of `local` are written back as read (section 7). Renaming a project
  from its Recent Projects card rewrites `project.name` and `summary.name`
  the same way.
- **A legacy (SQLite) file** is converted in memory when opened; its first save
  writes version 2 over it. Before that write the original is made whole (a hot
  `-journal` an old build left is rolled back into it, a `-wal` checkpointed,
  as SQLite's own open would), every `-journal`, `-wal` or `-shm` beside it is
  removed, so no older build can roll a stale journal into the JSON file, and
  the original is kept in the app's data folder
  (`backups/<stem>.<path key>.<UTC stamp>.pre-v2.stalx`, one self-contained file; the path key is a short hash of the file's full path, so same-named projects in different folders never share an original).
  Until that save the file is untouched. Its undo history is not converted.
- **Repair** of a damaged version 2 file copies it to `<name>.stalx.backup` (or
  the next free `.backup.N`) first, then writes the salvaged survey in its
  place, without history.
- A symbolic link at the file's path is replaced by the saved file, not
  written through. On Unix the saved file keeps the old file's permissions.

## 13. A complete example

A project with two lines meeting at a junction (station 2), a datum, a second
piece placed by a placement, a closure, a Section, a credit shared by most of
a line, an alias, a SpeleoDB link and one undo entry. This is byte for byte
what Stalagmight writes for it (a test keeps it so).

<!-- example:begin -->

```json
{"format":"stalx","version":2,"solver":1,"requires":[],
"conventions":{"length":"m","angles":"deg","azimuth":"cw-from-north","inclination":"+down","depth":"+down","local":"enu-m","gps":"wgs84-deg","temperature":"degC"},
"generator":"Stalagmight 0.6.0",
"summary":{"name":"Cenote Example","lines":3,"stations":7,"shots":5,"length":33.75,"center":[-87.25,20.5],"bounds":[-87.25,20.5,-87.25,20.5],"speleodb":"0f6c1d2e-3b4a-4c5d-8e9f-a0b1c2d3e4f5"},
"project":{"name":"Cenote Example","location":"Synthetic, nowhere","created":"2025-11-20T12:00:00.123456789Z","modified":"2025-11-20T13:00:00.000000000Z"},
"settings":{"validation":{"warn_shot_length_min":0.3,"warn_shot_length_max":50.0,"warn_depth_change_per_shot_max":10.0,"warn_azimuth_deviation_max":30.0,"azimuth_diff_threshold":20.0,"pitch_diff_threshold":10.0,"inclination_depth_diff_threshold":15.0}},
"counters":{"station":8,"closure":2,"section":1},
"speleodb":{"endpoint":"https://speleodb.example","project":"0f6c1d2e-3b4a-4c5d-8e9f-a0b1c2d3e4f5","name":"Cenote Example","undo_mode":"full"},
"local":{"display":{"grid":"true"},"speleodb":{"base":"3a7bd3e2360a3d29eea436fcfb7e44c735d117c4","unpushed":true,"last_edit":"2025-11-20T13:00:00.000000000Z"}},
"sections":[
{"id":0,"name":"Upstream","color":"#4ecdc4"}
],
"stations":[
[0,"A0"],
[1,"A1"],
[2,"A2",{"comment":"tie-off on a rock","aliases":[[7,"B0"]]}],
[3,"A3"],
[4,"B1"],
[5,"C0"],
[6,"C1"]
],
"lines":[
{"id":0,"name":"Main","color":"#ff6b6b","direction":"in","quality":1.0,"section":0,"credit":{"surveyors":[{"name":"A. Diver"}],"date":"2026-03-14"},"start":0},
["5f0c2a1e-0000-4000-8000-000000000001",1,12.5,90.0,90.0,3.0,4.5,1.5,2.0,null,0.5],
["5f0c2a1e-0000-4000-8000-000000000002",2,8.25,92.5,92.5,4.5,6.0],
["5f0c2a1e-0000-4000-8000-000000000003",3,4.0,180.0,180.0,6.0,-0.0,{"kind":"jump","comment":"restriction, \"silt\"","credit":null}],
{"id":1,"name":"Side","color":"#95e1d3","direction":"out","quality":1.0,"azimuth":{"north":"magnetic","correction":-1.5,"source":"igrf","epoch":2026},"credit":{"surveyors":[{"name":"A. Diver"}],"date":"2026-03-14"},"start":2},
["5f0c2a1e-0000-4000-8000-000000000004",4,6.0,0.0,0.0,6.0,7.0,{"pitch_in":-10.0,"temperature":24.5}],
{"id":2,"name":"Unconnected","color":"#abc","direction":"in","quality":1.0,"credit":{"surveyors":[{"name":"A. Diver"}],"date":"2026-03-14"},"start":5},
["5f0c2a1e-0000-4000-8000-000000000005",6,3.0,45.0,45.0,2.0,2.0]
],
"datums":[
{"station":0,"via":0,"lat":20.5,"lon":-87.25,"depth":0.0,"accuracy":3.0,"entrance":true,"correction":[0.25,-0.125]}
],
"placements":[
{"station":5,"x":40.0,"y":-12.5}
],
"closures":[
{"id":1,"a":4,"a_via":1,"b":3,"strategy":"batch"}
],
"ignored_warnings":[
{"shot":"5f0c2a1e-0000-4000-8000-000000000003","code":"SHORT_SHOT","at":"2025-11-20T12:50:00.000000000Z","comment":"checked on the dive"}
],
"imports":{"files":["9f86d081884c7d65"]},
"history":{"undo":[
{"t":"2025-11-20T12:59:00.000000000Z","k":"update_shot","label":"Edit shot A2-A3","ops":[]}
]},
"end":true}
```

Reading it: line `Main` runs A0 -> A1 -> A2 -> A3; line `Side` starts at the
junction A2 (station 2) and runs to B1. The third shot of `Main` has no credit
(`"credit":null`); the others take the header's. Station 2 used to be two
stations, and the one merged into it had id 7 and the name `B0`. The datum on
A0 is the frame and places the first piece; the second piece (C0 -> C1) holds
no datum, so a placement puts C0 at 40 m east, 12.5 m south. The closure ties
B1 to A3 as a soft loop closure.
