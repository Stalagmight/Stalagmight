# Local modifications

This is a vendored fork of [`compass_data`](https://github.com/zheylmun/compass_data)
v0.0.7 by Zach Heylmun, used under its MIT OR Apache-2.0 license (see
`LICENSE-MIT` / `LICENSE-APACHE`). It is embedded in Stalagmight as a path
dependency so we can carry fixes the upstream release lacks.

Changes from upstream v0.0.7:

- **`Shot`**: added `back_azimuth` / `back_inclination: Option<f64>`. The shot
  parser now captures redundant backsight columns (when the survey format
  declares them) and the trailing `#|FLAGS#` token and free-form comment, instead
  of discarding the rest of the line. `Shot.flags` / `Shot.comment` are therefore
  populated.
- **`survey::parser`**: `parse_shot` takes a `has_backsights` flag (derived from
  the survey format) so backsight columns aren't mis-read as comment text; added
  `split_flags_and_comment`.
- **`SurveyFormat`**: added public accessors `has_backsights()`, `length_units()`,
  `inclination_units()` so consumers can interpret units, and
  `lrud_association()` (with `LRUDAssociation` re-exported from the crate root)
  so a consumer can tell a survey that records LRUDs at each shot's To station
  from the default From.
- **`project::Station`**: `name` / `location` fields made public so consumers can
  resolve georeference anchors from the `.MAK` link stations.
- **`DatFile::load`**: read `.DAT` files as latin1 (one byte per `char`)
  instead of UTF-8, so real files (e.g. a `0xB0` degree sign in a comment)
  don't fail to load. Stalagmight itself no longer goes through this reader;
  see the reader hook below.
- **Reader hook**: added `Project::from_text` (parse a `.MAK` from text),
  `DatFile::load_from_text` (parse a linked `.DAT` from text) and
  `Project::load_survey_files_with(read)`, which hands each linked file's path
  - as the project lists it - to the caller's `read` closure and skips a file
  the closure declines instead of failing the whole project. `Project::read`
  and `DatFile::load` delegate to the text entry points. Stalagmight reads the
  `.MAK` and every linked `.DAT` itself through this hook so its own policy
  applies: Windows-1252 decoding with a replacement warning, byte-order-mark
  stripping, which linked paths may be read, a size ceiling, regular files
  only, and skip-and-report for a missing file.
- **`survey::parser`**: the trailing survey form-feed terminator is now optional,
  so a hand-edited `.DAT` missing the final `0x0C` still parses.
- **`survey::parser` / `parser_utils`**: an impossible `SURVEY DATE:` is a parse
  ERROR rather than a panic. Upstream does
  `NaiveDate::from_ymd_opt(..).unwrap()` and `digit1.parse::<u32>().unwrap()`, so
  `SURVEY DATE: 2 29 1979` (or a field wider than a `u32`) aborts the thread.
  Stalagmight calls this from a `#[tauri::command]`, where a panic never reaches
  the user - the caller's promise simply never settles.

- **`Survey::parse_dat_file_with_remainder`**: added, and `parse_dat_file` now
  delegates to it. The file parser is a `many0`, which stops at the first survey
  it cannot read and returns the earlier ones without consuming the rest of the
  file; upstream throws that remainder away, so a truncated parse is
  indistinguishable from a short file. Stalagmight warns the user about the
  surveys it did not import, which needs the remainder.
- **`DatFile`**: carries that remainder for a `.DAT` reached through a `.MAK`,
  exposed as `unread()` / `unread_at()` (the tail, and the byte offset it starts
  at). `DatFile::load` does its own `parse_dat_file` call, so without this the
  `.MAK` path truncates just as silently as the direct one.
- **`Cargo.toml`**: dropped the `utm = "0.1"` dependency. Nothing in the crate
  (or in Stalagmight) uses it - the crate's own `UtmLocation` type is plain data
  - so it was compiled into the app and listed in the shipped third-party
  notices for nothing.
- **`survey::parser`**: a survey with no team reads. Compass writes the team on
  the line after `SURVEY TEAM:` and leaves it empty when there is none (so does
  this crate's own `serialize`); upstream skipped `multispace1` after the tag,
  which swallowed that empty line, read the `DECLINATION:` line as the team and
  then failed on the survey, dropping it and every survey after it. The team is
  now the next line (possibly empty), or the rest of the tag's own line when a
  writer put it there. Found by Stalagmight's parser fuzzing (#576 FZ-PARSE-6).

These should be contributed upstream where appropriate.
