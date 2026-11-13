# Per-shot survey credit: surveyors, explorers and date as people

Status: IMPLEMENTED (UI wired). Issues #328, #329. Companion: `2026-11-12_merge-split-sections.md`.

## Goal

- Surveyor stops being free text. Surveyors and Explorers are lists of people, entered as pills.
- Surveyors, explorers and the survey date live on every SHOT. A section shows the aggregate:
  the union of people and a date or date range.
- SpeleoDB collaborators of the linked project are suggested, matched on name and email.
- Imports (Ariane, Compass, Walls, ...) produce separate people, in the right role.
- Old files keep their meaning. Values that we know came from the Ariane importer
  (`"explorers / surveyors"`) are split into the two roles.

## Model (`crates/models/src/credit.rs`)

```rust
pub struct Person { pub name: String, pub email: Option<String> }   // by value, no registry ids
pub struct SurveyCredit { pub surveyors: Vec<Person>, pub explorers: Vec<Person>, pub date: Option<NaiveDate> }
```

- `Shot.credit` and `ShotData.credit` hold a `SurveyCredit` (`#[serde(default)]`, TS-optional on
  `ShotData`). Every copy of a shot (split, reverse, undo, project append, insert) carries it.
- **Person identity.** Names are compared after lowercasing and collapsing whitespace. Emails are
  compared lowercased. Two people are the same when their names and emails are both equal, and an
  email of `None` only matches `None`. Lists keep their order and drop duplicates.
- **`Section` keeps no surveyor or date.** `Section::credit_summary()` computes the aggregate: each
  person with their shot count, plus the minimum and maximum date. The frontend computes the same
  thing from the shots it holds.
- **Legacy inputs.** These keep their wire shape:
  - `SectionData.surveyor/date` and `Command::CreateSection.surveyor/date`: a template, applied to
    every shot whose credit is empty.
  - `SectionUpdates.surveyor/date`: set on every shot, the surveyors parsed with the legacy
    splitter.

  So old persisted undo entries and every existing test literal keep working. `Section` keeps a
  deserialize-only `surveyor`/`date` (`skip_serializing`, `ts(skip)`), and the migration below
  absorbs it whenever an old `Project` snapshot is restored.
- **Legacy splitter** (`credit::legacy_people(text) -> (explorers, surveyors)`):
  - Text with exactly two `" / "` halves came from the Ariane importer: the first half is the
    explorers, the second the surveyors. Any other text is surveyors only.
  - Each half is split on `;`. Each piece is then split on `,`, unless the whole half looks like
    `Last, First` (exactly two comma parts, each one word).
  - Pieces are trimmed. Empty pieces and the placeholders `unknown`, `none` and `?` are dropped,
    and duplicates are removed.

## New commands (one undo step each)

`Command::EditShotCredit { targets: CreditTargets, edit: CreditEdit }`:

- `CreditTargets`: a whole section (`section_id`), or explicit shots (`[(section_id, shot_id)]`).
- `CreditEdit` holds `add_surveyors`, `remove_surveyors`, `add_explorers`, `remove_explorers`,
  `set_surveyors: Option<Vec<Person>>`, `set_explorers` and `set_date: Option<Option<NaiveDate>>`.
- Add appends the person when missing, remove drops them, and set replaces the list.
- The undo data is the before-credit of every touched shot, keyed by shot uuid.
- It is not a geometry change, and it is refused in read-only mode, like every other mutation.

## New shots inherit credit

`AppState::create_shot` (append and insert) copies the credit of the neighbouring shot, the one
before it, or the next one when inserting at the start. It does this only when the incoming
`ShotData.credit` is empty. Splitting a shot copies its credit to both pieces.

## Storage: schema stays v18

SpeleoDB reads `.stalx` through openspeleo-lib 0.0.22, which uses stalagmight-lib 0.0.1. That
reader:
- requires `PRAGMA user_version` and `project.schema_version` to both be 18;
- ignores unknown tables and columns;
- validates `sections.surveyor` as `str | None` and `sections.date` as one ISO date.

How this feature fits:
- New side tables, keyed by section and shot uuid:
  - `credit_people (id, name, email)`
  - `shot_credits (section_id, shot_uuid, date, surveyors, explorers)`, keyed by
    `(section_id, shot_uuid)`, where the people columns are comma-joined ids from `credit_people`.

  - `credit_sections (section_id, surveyor, date)`: the legacy pair this build wrote. A section
    whose `sections.surveyor`/`date` no longer match it (an older build retyped them) ignores its
    rows, so the retyped text wins.

  A shot's row says that its credit is explicit, even when the lists are empty. A shot with no row
  falls back to the legacy section columns through the splitter. That makes old files and shots
  saved by older builds work unchanged.
- The uuid is the one `write.rs` already mints at save time. Unreadable or dangling rows are
  skipped, never fatal. A credit table that cannot be read as a whole (a missing column, a view,
  a read failing part way, or `credit_people` missing beside `shot_credits`) gives no rows at all:
  every shot falls back to the legacy columns, with a warning that holds counts, never names
  (#717).
- The legacy columns are still written. `sections.surveyor` gets the distinct surveyor names
  joined with `", "`, or NULL. `sections.date` gets the earliest shot date, or NULL.
- A contract test pins that every table and column the Python reader reads is unchanged, apart
  from those two values. A live check runs against the internal SpeleoDB.

## Binary load envelope v7

Per section, a list of distinct credits (people by index into one project-wide people table)
plus one u32 credit index per shot. The TS decoder (`loadProjectBinary.ts`) rebuilds
`shot.credit`.

## Imports

Ariane sections are cut only where the COLOUR changes (a section holds one colour); people and
dates are per shot and never cut, so a section with mixed per-shot credit round-trips through
Ariane as one section. TML duplicate detection uses the file hash only, so it is unaffected.

| Format | Mapping |
|---|---|
| Ariane TML | Per shot. Ariane 26.3 and later write `XMLExplorer`/`XMLSurveyor`; those are preferred when non-empty. Otherwise the legacy `Explorer` field is used. In its element form (`<Explorer>..</Explorer><Surveyor>..</Surveyor>`), each half goes to its role. In the plain form, `a / b` gives explorer `a` and surveyor `b`, and a single value gives explorers, as Ariane's field name and openspeleo-lib say. Names inside a role are split on `,` and `;`. |
| Compass | `SURVEY TEAM:` gives surveyors, split. |
| Walls | `team:`/`surveyors:` gives surveyors, split. |
| DMP, GPX | No people. |
| Import dialogs | Surveyors and Explorers pills, plus a date, override every imported shot when set. |

## Exports

- **TML:** per row. If any surveyors are set, the `Explorer` field uses the element form;
  otherwise it holds the explorers as plain text, joined with `", "`. The round trip is stable.
- **JSON:** per-shot names and dates, plus section aggregates. No emails.
- **CSV:** unchanged.
- **stalx-reader crate:** exposes per-shot credit.

## SpeleoDB collaborators (linked project only)

New calls:
- `GET /api/v2/projects/{id}/permissions/user/` returns the emails and levels of users with access.
- `GET /api/v2/user/autocomplete/?query=<email>` returns `{email, name, label}`, to fill in names.
  It is authenticated, needs at least 3 characters and returns at most 10 results.

Results are cached in memory per project. The command is
`speleodb_project_collaborators(project_id)`.

## UI

- **`PeopleInput`.** Typing shows a suggestions dropdown, and Up/Down moves through it.
  - Enter or `;` adds the highlighted suggestion, or a new person from the typed text.
  - Each pill has an x button. Backspace in an empty input removes the last pill.
  - Hovering a pill shows the full name, email, source and shot count.
  - A pill shows the shortest text that tells same-named people apart.
  - A pill for someone on only some of the shots is dashed and reads "N of M shots".
- **Suggestions:** project people (any shot, either role), then the linked SpeleoDB project's
  collaborators (fetched in the background when signed in; a failure is silent), then recently
  entered people. The recent list lives in config.toml `[people] recent` (newest first, at most
  200, written only through `remember_people`), so it carries across projects. Typed text that
  names exactly one collaborator, by name or email, takes that collaborator's identity; a name two
  collaborators share is never guessed.
- **Where it appears:** Surveyors and Explorers fields everywhere Surveyor could be entered: New
  Section (prefilled with the session's last used people; the pills and date go on every shot
  entered), Edit Section, the mNemo wizard and its defaults, Zero Datum config, Suggestion
  settings. The Ariane preview shows each section's people read-only.
- **Edit Section:** the union of the shots' people with partial pills; adding, removing and
  promoting are a pending edit sent on Save as ONE `editShotCredit` on the section. A date that
  differs per shot shows its range until "Set for all shots".
- **Detail panel:** a shot's credit, editable, one undo step per change. Two or more selected shots
  (and nothing else) get a view that edits all of them at once, with the same union semantics. A
  section edits every shot through the section target. Read-only modes keep the pills only.
- **Data table:** optional Surveyors, Explorers and Survey date columns, hidden by default (and
  hidden once for configs saved before they existed). Enter, F2, Space, a double click or a typed
  letter opens a popover that commits one `editShotCredit` for the shot; paste, fill and clear
  write whole lists through the batch, one `edit_shot_credit` per shot.
