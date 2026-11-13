# Inclination sign convention

One rule: **stored pitch is positive-DOWN.** Positive means the shot descends
and depth increases. It is the same frame `depth_in` / `depth_out` are already
in, which is what makes it the useful choice: a pitch and the depth change
beside it can be compared without either being flipped first.

Every external format we read is the opposite (positive-up). The conversion
happens exactly once per format, at its import boundary, and nowhere else.

Established in [#296](https://github.com/Stalagmight/Stalagmight/issues/296),
which fixed a DMP import that skipped the conversion.

## Where the flip happens

| Source | In-file convention | Where it is normalised |
|---|---|---|
| MNemo DMP | +up (device: `-90` = straight down) | `parse_section` (`src-tauri/crates/parsers/src/parsers/dmp.rs`) |
| Walls `.srv` | +up | `walls/srv.rs`, negates `inc_up` into `pitch_in`/`pitch_out` |
| Compass | +up | `compass.rs`, `pitch_in: Some(-inclination)` |
| Ariane `.tml` / `.tmlu` | +up (inferred, see below) | `parsers::tml::inclination_to_pitch`, inverse `export::tml::pitch_to_inclination` |

DMP normalises inside `parse_section`, not at the `ShotData` conversion, because
`DmpShot::to_shot_data` is not on the import path. Real DMP import returns the
parsed `DmpFile` to the frontend (`import_dmp`), and the import dialog builds
`ShotData` in TypeScript and sends it to `import_sections`. Normalising at the
parser is what keeps the preview table, the overlap merge against existing
project shots, and `to_shot_data` in one frame; a conversion that lived only in
`to_shot_data` would never run.

The flip is applied after `compute_section_hash`, which therefore still hashes
the device's own bytes, so re-import deduplication keeps recognising sections
imported by an older build.

`pitch_out` is a foresight in the same sense as `pitch_in`, not a reversed
backsight, which is why both legs are negated identically, why validation may
average them, and why a synthesized reverse leg negates the forward reading
rather than reusing `pitch_out` (`export::tml::reversed_leg_pitch`).

## Consumers

Anything deriving inclination from geometry must use `depth_out - depth_in` so
it lands in the same frame:

- `models::shot::pitch_from_components`: `atan2(dz, horizontal)`, +down
- `core::validation`: compares recorded against implied directly, no negation
- `core::state::command_move`: writes +down pitch when a move reshapes a shot
- `DetailPanel.computeInclination`, `WarningResolution.impliedInclination`,
  `resolutionPreview.lengthFixableFromInclination` (TypeScript)

The UI displays stored pitch **raw**. There is no display-boundary negation, so
what the user sees is what is in the file.

Reversing a shot (swapping its legs) negates pitch. That is true under either
convention and is not a convention conversion, see
`overlapHelpers.reverseTargetShot`.

## Ariane: an inference, not a measurement

Ariane's `<Inclination>` sign could not be established from data. `pytool_ariane_lib`
never computes with the field, and across every Ariane file available all 5,686
shots carry `Inclination` 0, because these are depth-gauge surveys and Ariane
does not populate it. Stalagmight treats it as +up because every format around
it is:
Walls and Compass both store +up in-file, and the MNemo reports +up.

On real files the flip is a no-op, so it only affects what we export. If a
counter-example ever turns up, an Ariane file with a non-zero inclination
alongside a known depth progression, it settles the question, and the two
helpers named in the table above are the only places to change.

The all-zero observation above held for the files it was written against; it no
longer holds in general (re-measured 2026-08-04: one file carries a non-zero
`Inclination` on 266 of its 538 shots). The convention is unchanged - it is
still the inference described here - but the flip is no longer a no-op on live
data, so the counter-example that would settle the question is now reachable.

## Migrating old projects

Schema v12 normalises pre-#296 files, which mix conventions: DMP-sourced shots
are +up, Walls/Compass-sourced shots already +down. No shot or section records
its importer, so `migrate_pitch_to_down_convention` infers it from geometry.

The verdict is **per section, never per shot**. A single shot is often
ambiguous: near-horizontal and very short shots carry almost no signal (1 cm
depth resolution swamps the implied angle), and on real data individual shots do
score better in the wrong direction. A section is decisive because the
convention is a property of the file it was imported from.

Per-section also protects real data: a shot whose inclination genuinely
contradicts its depths cannot flip itself into agreement. It inherits the
section verdict and stays flagged, which is the entire point of the
`INCLINATION_DEPTH_MISMATCH` check.

A section is only touched when the evidence is lopsided, and it is votes that
are counted, never summed errors: each usable shot casts one vote - to flip
when its flipped error is less than half its as-is error, to keep when its
as-is error is less than half its flipped error, and it abstains when neither
holds. A section flips only when at least 3 shots vote to flip and the flip
votes are more than twice the keep votes. That margin lets a real DMP section
carry one blunder without being held hostage by it, while an already-correct
section can never be flipped by an outlier.
Anything in between is left alone rather than guessed at. Out-of-range legs
(steeper than vertical) do not vote: validation skips the cross-check for them,
so a single blunder should not drag a section's verdict. Sections that had
signal but were not decisive are named in a `warn!` on open, because the v12
stamp means they are never re-examined.

### What else the migration has to discard

The migration rewrites the `shots` table. Two other stores keep pitch, or a
verdict derived from it, and neither is versioned, so a stale copy would
outlive the migration with no second pass to catch it. When at least one section
is flipped, `invalidate_pre_pitch_flip_state` drops both:

- **Persisted undo/redo.** Payloads carry absolute pitch and whole-project
  snapshots, so one undo of a pre-upgrade edit would write the +up value back
  into a file already stamped v12. `UNDO_FORMAT_SAFE_VERSION` is 7, so a v11
  file keeps its stacks by default.
- **Dismissed inclination warnings.** `resolved_warnings` is keyed by
  `(section_id, shot_id, warning_code)` with no value component, so a mismatch
  dismissed under the old convention would keep suppressing the check.

A project where nothing was flipped keeps its undo history.

## TML is always read and written as Ariane

There is no attempt to detect who wrote a `.tml`. Import always applies the
Ariane convention, and export always writes it, so a file Stalagmight produces
re-imports correctly as though Ariane had written it
(`tml_inclination_survives_a_file_level_round_trip` pins that round-trip).

The cost is that a standalone `.tml` exported by a **pre-#296** build carries
raw +down pitch in a field now read as +up, so its non-zero inclinations import
inverted, and unlike a `.stalx` there is no migration to repair it. Real
Ariane-authored files are unaffected: they carry `Inclination` 0.

## Adding a new importer

Normalise at the import boundary and say so in a comment naming the source's
convention. Put it in the function the real import path actually calls, which
is not necessarily the one that builds `ShotData`: for DMP the conversion has
to sit in the parser, because the dialog builds `ShotData` in TypeScript.
Do not add a negation to a consumer; if a consumer looks like it needs one, the
import is wrong. That is the bug #296 was.
