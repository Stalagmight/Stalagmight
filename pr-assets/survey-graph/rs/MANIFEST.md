# Kept and archived stations (RS): real-app screenshots and usability review

Build under test: branch `sg/rs`, head `7953325ba` (FX8: fixes for N1-N7 of the second round;
FX9: the /pre-pr fixes). Worktree `.claude/worktrees/sg-rs`. 55 screenshots of fully synthetic
data. The shots whose surface FX8/FX9 changed were retaken on `7953325ba`: 03, 04, 05, 10, 14,
15, 16, 17, 19, 20, 21, 22, 29, 31, 33, 34, 53, 54, 55. The other 36 come from the `14ee871fb`
round. Nothing FX8/FX9 changed is visible in them; the RS-FX8 and RS-FX9 notes and the diff
list only the surfaces above.

**Third round (this head).** Same setup as below: the harness rebuilt this tree's debug binary
in `~/.cargo_build_rs_a`; the stamp and `session.start` both report `repoRoot .../worktrees/sg-rs`,
`gitHead 7953325ba`; the doctor's self-test panic was the only finding; the same synthetic survey
was seeded the same way (the Loop merge again through the Merge tool); no upgrade files were
needed. Sweeps after every flow were clean; the app log's only ERROR line is the self-test panic.

## How it was run (14ee871fb round; the 7953325ba round repeated it, see above)

- **Build and provenance.** The harness (`e2e/agent` of this worktree) rebuilt the debug
  binary in `CARGO_TARGET_DIR=$HOME/.cargo_build_rs_a`: the binary there was stamped for
  `51b5191db`, so `build: 'auto'` rebuilt it. The new stamp reads `repoRoot .../worktrees/sg-rs`,
  `gitHead 14ee871fb`, `viteE2E true`; `session.start` reported `builtFrom {repoRoot: .../sg-rs,
  gitHead: 14ee871fb}`; the running app was `$HOME/.cargo_build_rs_a/debug/stalagmight`.
- **Driver.** A tsx REPL in the worktree (`.agent-artifacts/rsdemo/repl.ts`, gitignored, deleted
  afterwards): `createSession()` + `session.start({build: 'auto', freshProfile: true})`, function
  bodies POSTed to it. Under `dbus-run-session` on the free Xvfb display `:176`, `NO_AT_BRIDGE=1`,
  `TMPDIR=/tmp/rsd/t`, `STALAGMIGHT_AGENT_BASE_PORT=4844`, Node 24.14. Window 1440x900 (one shot at
  1024x640). Satellite basemap off (config and per project; the 0.6.0 file turns it on at open,
  so it was switched off before any capture), autosave off, Full highlighting (the default).
- **Harness doctor.** `debug_trigger_panic` then a full sweep reported the `fatal crash-marker`
  and its `log-error` line; the app kept responding. Both findings are the self-test only.
- **Data, all synthetic.**
  - "Synthetic Cave", built in the app with `createLine` (8 lines): Main `#d62728` (14 x 10 m,
    stations 0..14, entrance datum on Main / 0), Side `#f4f4f2` (near-white, 14 x 5 m, stations
    0..14, so it has its own 12), Loop `#141414` (near-black, from Main / 3 round to beside Main /
    8, end renamed L8), Lower `#1f77b4` and Upper `#2ca02c` (a free piece, Upper / 0 merged into
    Lower / 3), Annex `#ff7f0e` and Spur `#9467bd` (a free piece, Spur / 0 merged into Annex / 3),
    Twig `#17becf`; ten station comments. Saved as `/tmp/rsd/files/synthetic-cave.stalx`.
  - 0.6.0 SQLite file: a scratch Rust crate outside the repo, path-depending on
    `stalagmight-legacy` with feature `writer`, builds the survey with the frozen 0.6.0 model
    (`add_line`, `merge_stations`, `create_loop_closure`, `set_zero_datum`, `station_comments`)
    and writes it with the frozen 0.6.0 save `write_v18`: Main 80 m with datum; Side merged at
    Main / 3; Upper 120 m (longer than Main) merged at Main / 6; Lower merged at Main / 8; two
    closures on Side / 4 (the second one's archive refused by C1); five comments.
  - Earlier version 2 file: the same crate writes the exact shape of the persistence test helper
    `earlier_file` (`stalx_v2/format_v2_upgrade_tests.rs`, copied verbatim, `requires []`, a U1
    alias row `[9,"S0",1]` on junction M2 with a U1-joined comment): Main 20 m, Side 60 m, Annex
    from M4 with a crafted native repeat (two `A1`, which U1 forbade, to show the letter clause),
    two closures on M4. Both files lived in `/tmp/rsd/files` and were deleted.
- **Sweeps.** The full oracle sweep (`runSweep(..., {full: true})`, what `check_problems` runs)
  after every flow; see "Sweep results".

### What did not go through the UI

- Building the main survey: `createLine` x 8, `setZeroDatum`, `updateStationComment` x 10,
  `setStationName` (Loop / 8 to L8; for shot 54, Loop / 7 to L8), `moveLine` (five pieces), the
  setup merges `Upper / 0` into `Lower / 3`, `Spur / 0` into `Annex / 3` and `Twig / 0` into `Side
  / 6` (explicit keep), and the re-merge keeping Main / 12 after flow 2. The Loop merge, now shot
  53, went through the Merge tool.
- One shot edit (`updateShot`, Side's first shot depth) for the warning and history shots, undone
  right after.
- Theme switches (`updateConfig`), display toggles (`showSatellite`, `showStationNames`,
  `showLineNames` and `showStationLabels` for the label probe), map framing (`jumpTo`), the
  Computed View switch (`setGeometryMode`).
- Plan's "Export text": WebKitGTK has no `navigator.clipboard`, so the REPL stubbed `writeText`.
- The map label probe reads `window.__stalagmight_labels__()` (the e2e hook) to count the label
  descriptors the declutter kept.
- Everything else (Merge tool clicks, the dialogs and their radios, context menus, the Also on
  detach button, data-table cell edits, the rename prompts, Delete Shot, the Plan context menu,
  Open through the app menu with the harness file-dialog hook, Ctrl+S, a real pointer click on
  Back to Map) went through the real UI.

## Shots

| File | What it shows / proves |
|---|---|
| 01-merge-dialog-keep-picker-light.png | Merge tool, Side / 0 then Main / 12: Keep radios as pills with superscript Station IDs, default Main / 12, "Side will show Main / 12.", "Side / 0 is archived with its comment.", the new repeat sentence "Side already has a station 12 (Station ID 27); it keeps its name, and Side shows 12 twice, the junction with Main's dot.", depths "7.8 m (Main, 2 readings)". |
| 02-merge-dialog-keep-picker-dark.png | The same dialog in the dark theme; the Sections panel now rings the near-black Loop dot. |
| 03-merge-result-map-junction-labelled-light.png | After the merge keeping Main / 12 (7953325ba): the junction is labelled 12 again; Side's own 12 further south is labelled too (label probe: two '12' labels kept); the near-white 'Side' name now anchors on Side / 1 instead of the junction. |
| 04-merge-result-data-table-side-two-12s-light.png | Side's stations at the new 112 px Name column (7953325ba): row 0 reads '12 (Main)' in full, Main's home dot in the pill (tooltip 'Belongs to Main', screen-reader '(Main)'), Station ID 12, the kept comment; Side's own 12 (Station ID 27) plain. |
| 05-merge-result-data-table-side-two-12s-dark.png | The same table in the dark theme (7953325ba). |
| 06-merge-result-station-details-also-on-light.png | Details of Side's junction slot: "12 belongs to Main.", Station ID 12, "Y: 0.00 m" (no minus), Also on: Main, Archived here: 0 (Station ID 15), Side's own station, with its comment. |
| 07-warnings-tab-dotted-shot-pill-light.png | A warning on Side's first shot: the pill 12(dot) -> 1 on Side; backend text "Shot Side: 12 (Main) → 1 ...". |
| 08-undo-history-plain-text-12-main-light.png | History rows "Update shot Side: 12 (Main) → 1" and, wrapped instead of cut, "Merge Side / 0 into Main / 12 (keeps 12)". |
| 09-merge-dialog-keep-the-other-side-0-light.png | Keeping Side / 0 (the moving piece): "Main will show Side / 0.", "Main / 12 is archived with its comment.", "Main already has a station 0 (Station ID 0); it keeps its name, and Main shows 0 twice, the junction with Side's dot." |
| 10-keep-other-result-map-junction-reads-0-light.png | Keeping Side / 0 instead (7953325ba): the junction is now labelled 0 on the map; details list Main / 12 under Archived here with its comment. Geometry bit-identical to the keep-Main merge (checked in the 14ee871fb round, 62 of 62 slots). |
| 11-keep-other-result-data-table-main-two-0s-light.png | Main row 12 reads "0 (Side)" with Side's ring mark and Station ID 15, beside the entrance 0. |
| 12-detach-setup-side-reuses-archived-name-0.png | Side / 14 renamed 0 in the Name column (allowed; Side's own 0 is archived). |
| 13-detach-entry-context-menu.png | "Detach Line from Station..." in the junction slot's context menu. |
| 14-detach-dialog-collision-answers-by-role-light.png | Detach Side, default answer (7953325ba): 'it gets back 0a (Station ID 15)'; answers 'Rename the station that comes back (Side / 0, Station ID 15) to 0a' and 'Rename the 0 that is on Side now (Station ID 29) to 0a'. |
| 15-detach-dialog-other-answer-lead-follows-light.png | The other answer chosen (7953325ba): the lead reads 'it gets back 0 (Station ID 15)'. |
| 16-detach-result-side-gets-0-back-own-0-is-0a.png | Result as the dialog said (7953325ba): Station ID 15 back as 0 with its depth and comment; Station ID 29 is 0a. |
| 17-merge-a-junction-again-dialog-light.png | Merging the Lower / Upper junction onto Side / 6 (7953325ba): 'Lower / 3 is archived with its comment.', then 'Detach Line from Station brings Lower / 3 back.', then 'Upper / 0 (Station ID 46), archived inside Lower / 3 by an earlier merge, stays inside it: a second Detach brings it back.', the repeat sentence for Lower's own 6, depths by line. |
| 18-junction-merged-again-details-archived-here-nested.png | Side / 6: Also on Lower, Upper, Twig; Archived here Lower / 3 with Upper / 0 indented inside, and Twig / 0. |
| 19-detach-dialog-lines-come-off-together-light.png | Detach Upper (7953325ba): 'It gets back 3 (Station ID 42)', 'Upper / 0 (Station ID 46) stays archived inside it: detach Upper again to get it back.', 'Lower comes off with Upper', the After Detach list. |
| 20-detach-remaining-junction-picker-light.png | The other lines together, default pick (7953325ba): 'They get back 3 (Station ID 42)', and both stations that go with them are named: 'Upper / 0 ... stays archived inside it' and 'Twig / 0 (Station ID 59) stays archived inside it: detach Twig again to get it back.'; the picker draws Lower: 3 and Twig: 0 as pills. |
| 21-detach-remaining-junction-twig-picked-light.png | Picker on Twig / 0 (7953325ba): 'They get back 0 (Station ID 59)', and now 'Lower / 3 (Station ID 42) stays archived inside it, with Upper / 0 (Station ID 46) archived inside Lower / 3: detach Lower again to get it back.' |
| 22-detach-together-result-upper-shows-lower-3.png | After detaching Upper (7953325ba): Upper's start shows 3 (belongs to Lower, Station ID 42) with Lower / 3's comment; Upper's own 0 (46) still inside it. |
| 23-create-loop-closure-both-ends-show-light.png | Create Loop Closure: "Both ends show:" Loop: L8 (38), Main: 8 (8, default), "Keep both names"; what is hidden and how it comes back; pinned buttons. |
| 24-create-loop-closure-both-ends-show-dark.png | The same in the dark theme. |
| 25-loop-closure-result-raw-view-light.png | Raw Data View: Loop labelled 1..7, no label at its loop end. |
| 26-loop-closure-raw-view-zoomed-two-points-one-label-light.png | Max zoom, Raw: two points 0.45 m apart joined by the tie; only Main / 8 is labelled. |
| 27-loop-closure-computed-view-light.png | Closures Computed View: the loop end on Main / 8, labelled once. |
| 28-loop-closure-computed-view-zoomed-light.png | Max zoom, Computed View: one point, one label. |
| 29-loop-end-station-details-and-table-mark-light.png | Loop end (7953325ba): 'Loop closure end, shown as Main / 8 / Its own station L8 (Station ID 38) is hidden'; 'This end's own position:'; 'Loop / L8, the loop end, is shown as Main / 8.'; the Name cell now reads '8 (loop end)' in full. |
| 30-loop-end-station-details-and-table-mark-dark.png | The same in the dark theme. |
| 31-remove-closure-dialog-what-comes-back-light.png | Remove Closure, no collision (7953325ba): '(L8 comes back)', 'Loop / L8 comes back: Loop shows it again instead of Main / 8.', 'Its comment comes back too'; pinned buttons. |
| 32-remove-closure-result-l8-back-with-comment-light.png | L8 back with its comment; at max zoom its label L8 is back. |
| 33-rename-through-loop-end-prompt-light.png | Typing 8E in the loop end's Name cell (7953325ba): 'Shown as Main / 8 through a loop closure.', 'A new name changes it on every line it is on.', 'Rename Main / 8 to 8E on Main and Loop' / 'Remove the closure' / 'Cancel'; the lines are named once (Cancel verified). |
| 34-rename-through-dotted-slot-prompt-light.png | Typing 12J in Side's junction cell (7953325ba): '12 belongs to Main.', 'Rename Main / 12 to 12J on Main and Side' / 'Detach Side to get back its own station 0' / 'Cancel'; the grid cursor stays on the edited row (Cancel verified). |
| 35-delete-shot-preview-restore-and-hidden-comment-light.png | Delete Shot on Annex 2 -> 3: "Spur / 0 (Station ID 55) is shown again. Annex / 3's comment ("Annex end: belay on a flake") is hidden: Undo brings it back." |
| 36-delete-shot-result-spur-0-shown-again-light.png | Spur / 0 shown again; Archived here "3 (Station ID 54), no longer on any line"; the notice repeats the preview. |
| 37-plan-route-across-closure-tie-light.png | Plan, endpoint Loop / 7: route Main / 0 -> Main / 8, across the tie, to Loop / 7. |
| 38-plan-endpoint-pin-on-loop-end-light.png | Plan pin on the loop end: "Loop / L8 (loop end, shown as Main / 8)"; Export text has one "turn" row at 80.0 m. |
| 39-pills-near-black-line-light.png | Loop (near-black): dotted start 3 with Main's dot, Also on Main, shot pills; the Sections panel rings near-white Side and near-black Loop. |
| 40-pills-near-black-line-dark.png | The same in the dark theme: the Loop dot is a ring in pills and in the Sections panel. |
| 41-pills-near-white-line-dark.png | Side / 6 in the dark theme: near-white pills and Side's home mark on Lower / Upper / Twig shot pills. |
| 42-upgrade-060-conversion-notice-light.png | 0.6.0 file: "Converted from the old file format. 4 stations ... 3 stations' comments ... 1 loop closure keeps both names."; "Old format: converted when you save". |
| 43-upgrade-060-conversion-notice-details-light.png | Details, now naming the second closure as made: "between Main / 7 and Side / 4 (now shown as Lower / 5) keeps both names". |
| 44-upgrade-060-expanded-table-under-banner-light.png | Expanded table with the banner up: it starts under the toolbar (picker, view toggle, headers visible); row 6 "0 (Upper)". |
| 45-upgrade-060-back-to-map-works-under-banner-light.png | A real click on Back to Map (hit-tested: the button) returns to the map with the banner still up. |
| 46-upgrade-060-map-junction-and-loop-end-light.png | Upgraded map: Main reads 0 1 2 3 4 5 0 7 8; Side's end is a loop end with two ties. |
| 47-upgrade-060-after-save-saved-in-new-format-light.png | After Ctrl+S: "Saved", banner "Saved in the new format ..."; the file requires archived-stations. |
| 48-upgrade-earlier-v2-notice-light.png | Earlier version 2 file: lead "Upgraded from an earlier version 2 file." then "3 stations ...; 1 station now has a letter suffix; 1 station's comment is now hidden; 1 loop closure keeps both names". |
| 49-upgrade-earlier-v2-notice-details-light.png | Details: "Main / M2 is now shown as Side / S0", "the comment of M4 on Main and Annex is hidden" (once), "Annex / A1 is now shown as A1a", "the loop closure between Main / M4 (now shown as Side / S4) and Side / S3 keeps both names". |
| 50-upgrade-earlier-v2-after-save-light.png | After Ctrl+S: Saved; the file requires archived-stations. |
| 51-upgrade-earlier-v2-junction-details-light.png | Upgraded junction: "S0 belongs to Side", Station ID 9, the U1-joined comment, Archived here: M2 (Station ID 2). |
| 52-detach-dialog-at-1024x640-buttons-pinned.png | Detach at 1024x640: the body scrolls, Cancel / Detach stay pinned (y 559-595 of 640). |
| 53-merge-dialog-name-repeat-sentence-light.png | Merge Loop / 0 onto Main / 3 through the Merge tool (7953325ba): 'Loop / 0 is archived.', 'Detach Line from Station brings Loop / 0 back.', 'Loop already has a station 3 (Station ID 33); it keeps its name, and Loop shows 3 twice, the junction with Main's dot.' |
| 54-remove-closure-collision-lead-follows-answer-light.png | Remove Closure with a collision (Loop / 7 renamed L8 while L8 was hidden), default answer (7953325ba): the row and sentence now say '(L8a comes back)' and 'Loop / L8a comes back'; the other answer reads 'Rename the L8 that is on Loop now (Station ID 37) to L8a' and, when picked, the text goes back to 'L8 comes back'. |
| 55-map-line-names-off-junction-and-twig-0-labelled-light.png | Line names toggled off with no rebuild (7953325ba): the junction keeps its 12, and Twig's first station 0, which the 'Twig' name had covered, is now labelled (the shadowed label comes back on the toggle). |

## Fix check (51b5191db issues)

Old issues from the `51b5191db` round, each against its new shot.

| Old issue | New shot(s) | Result |
|---|---|---|
| P1-1 Detach collision answers identical | 14, 15, 54 | Fixed: "the station that comes back (Side / 0, Station ID 15)" vs (since FX8) "the 0 that is on Side now (Station ID 29)". |
| P2-2 Detach lead ignores the chosen answer | 14, 15, 16 | Fixed in Detach ("gets back 0a" / "gets back 0", result matches). Same bug still in Remove Closure: N2. |
| P2-3 Earlier version 2 banner without lead | 48 | Fixed: "Upgraded from an earlier version 2 file." |
| P2-4 Expanded table under a banner, clicks swallowed | 44, 45 | Fixed: table starts under the toolbar; Back to Map hit-tests to the button and works. |
| P2-5 Dotted station marked only by a 5 px dot | 04, 05, 11, 29 | Fixed: Name column "12 (Main)" / "0 (Side)" / "8 (loop end)", mark tooltip "Belongs to Main" and sr-only "(Main)" (DOM). The qualifier truncates at the default width: N3. |
| P2-6 Merge dialog silent about a name shown twice | 01, 09, 17, 53 | Fixed: one repeat sentence per line. |
| P2-7 Hidden comment named by Station ID only | 35, 36 | Fixed: "Annex / 3's comment ("...") is hidden"; history record "no longer on any line". |
| P2-8 Notice names closures by shown ends | 43, 49 | Fixed: "Side / 4 (now shown as Lower / 5)", "Main / M4 (now shown as Side / S4)". |
| P3-9 One hidden comment counted twice | 48, 49 | Fixed: "1 station's comment", "the comment of M4 on Main and Annex is hidden". |
| P3-10 "Main / 8 (loop end)" ambiguous | 38, 29 | Fixed: "Loop / L8 (loop end, shown as Main / 8)", "Loop / L8, the loop end, is shown as Main / 8." |
| P3-11 Rename prompts hide the new name, two wordings | 33, 34 | Fixed: "Rename Main / 8 to 8E on Main and Loop" in both. Residual nits N6. |
| P3-12 Junction merged again under-explained | 17, 19, 20 | Fixed: Merge and Detach say what stays inside. Ordering nit N4, Twig pick N7. |
| P3-13 Letters note on every Detach | 14, 19, 20 | Fixed: not shown when no letter exists (the case with a letter was not set up). |
| P3-14 Loop-end details mix two stations | 29 | Fixed: "This end's own position:". |
| P3-15 Picker plain text, "It" ambiguous | 20, 21 | Fixed: pills, "They get back". |
| P3-16 History truncates "(keeps 12)" | 08 | Fixed: rows wrap. |
| P3-17 Sections dots vanish (near-white / near-black) | 01, 24, 39, 40, 41 | Fixed: ringed in both themes. |
| P3-18 Detach buttons below the fold at 1024x640 | 52 (also 14, 19, 20, 23, 31) | Fixed: pinned footer. |
| P3-19 Not RS-made: depth list repeats, "-0.00 m", red error beside refused Create | 01, 17, 06 | Depth list and "-0.00" fixed; the refused-closure error colour left as is (FX6 says so), not re-shot. |

## Evaluation as a surveyor (14ee871fb round)

The dialogs now explain themselves well. Merge says when a line will show a name twice
and what travels inside an archived junction. Detach names each answer by role and
Station ID, and its lead follows the choice. Delete names the hidden comment in words.
The upgrade notices name closures as the user made them. In the data table the dotted
12 reads "12 (Main)" next to Side's plain 12. In pills it carries a dot and a tooltip.
In plain text it reads "12 (Main)". The two 12s can now be told apart everywhere except
on the map, where they differ only by position. The map has a worse problem, N1 below:
the junction often loses its label altogether.

## Issues found on 14ee871fb (N1-N7, fixed in FX8: see "Fix check N1-N7")

### P1

None found.

### P2

- **N1. A junction loses its map label to the branch's line name.** With Station names labels
  and line names on (the defaults), the junction Main / 12 drew no label after the merge, at
  zooms 18-21.5, and again after keeping Side / 0 (shots 03, 10). The label probe shows why:
  with line names on, the label set holds only Side's own "12"; the "Side" line name (reveal
  z10) sits on the junction, which is Side's first station; with line names off and a full
  rebuild (Station IDs then Station names) the junction's "12" is back (shot 55, two "12"
  descriptors). Turning line names off alone does not bring it back. On the light map the
  near-white "Side" name is invisible, so the junction looks unnamed. Not RS-made (no map code
  changed since `51b5191db`, where the same junction happened to win the race), but RS makes a
  branch starting on the kept station the normal shape, so every such junction is at risk; the
  image export (no declutter) still labels it. Mechanism (code-analyst, confirmed by the probe):
  a line name anchors at the line's first non-reference station (`src/components/Map/surveyLayer/
  customRender/labelTessellate.ts:498-536`); its box overlaps the station label's at every zoom
  (`labelLayout.ts:68-69`); line names place first (`labelReveal.ts:207-210, 505-526`), so the
  station label is `REVEAL_NEVER` and dropped (`labelController.ts:672-674`), and toggling a kind
  only reruns the reveal over the emitted set (`labelController.ts:898-931`). Fix: anchor a line
  name whose first slot is a junction away from it (its first own station or first shot's
  midpoint), or give names and station labels non-overlapping offsets; keep `REVEAL_NEVER`
  losers so a toggle can bring them back (or rebuild on a kind toggle).
- **N2. Remove Closure's "comes back" ignores the collision answer.** With the default answer
  ("Rename the station that comes back (Loop / L8, Station ID 38) to L8a") the dialog still says
  "(L8 comes back)" and "Loop / L8 comes back: Loop shows it again instead of Main / 8."; the
  removal gave L8a. Shot 54. The FX6 fix for Detach (P2-2) was not applied here. Code:
  `src/components/Dialogs/RemoveClosureDialog.tsx` (the option suffix and the comes-back sentence
  read the preview's name). Fix: name the returning station as the chosen answers leave it, as
  Detach now does.

### P3

- **N3. The Name column's qualifier is cut at the default width:** "8 (loop..." (shot 29); a
  longer home line ("North Passage") would cut "12 (Main)"-style text too. Fix: size the
  column for the qualifier, or let the qualifier wrap or abbreviate.
- **N4. The "it" in the Merge dialog now points at the wrong station.** The new "Upper / 0 ...
  stays inside it: a second Detach brings it back." sits between "Lower / 3 is archived with
  its comment." and "Detach Line from Station brings it back.", so that last "it" reads as
  Upper / 0 (shot 17). Fix: move the inside sentence after it, or write "brings Lower / 3
  back".
- **N5. "Side's own 0" also fits the station that comes back.** The returning station was
  Side's own 0 too (shots 14, 15; "Loop's own L8" in 54). "The 0 already on Side (Station ID
  29)" says it better.
- **N6. The rename prompt says the lines twice, and the cursor moves on.** "A new name changes
  it on every line it is on: Main and Side." comes right before "Rename Main / 12 to 12J on
  Main and Side". The grid cursor has already moved to the next row while the prompt is
  open (shots 33, 34).
- **N7. Picking Twig / 0 drops the note about Lower / 3.** In the remaining-junction picker,
  Lower / 3 (with Upper / 0 inside) stays archived in the new station, but the dialog no
  longer says so, as it did for the default pick (shots 20, 21).
- Nits, not RS-made: Remove Closure shows the internal closure number ("#1 -"); the Delete Shot
  dialog uses an ASCII "<->"; near-white line names are invisible on the light map.

## Fix check N1-N7 (7953325ba)

| Issue | Shot(s) | Result |
|---|---|---|
| N1 junction label lost to the branch's line name | 03, 10, 55 | Fixed. The junction reads 12 (and 0 after keeping Side / 0) at 18.75 and at 20.5 (the probe keeps two '12' labels). A line name now anchors on its first own station, so 'Side' sits on Side / 1. Turning line names off brings back a label a name had covered (Twig's 0 in 55) with no rebuild. |
| N2 Remove Closure ignores the collision answer | 54, 31 | Fixed: the default answer reads '(L8a comes back)' / 'Loop / L8a comes back'; the other answer gives back 'L8'. |
| N3 Name qualifier cut at the default width | 04, 05, 29 | Fixed: the column is 112 px; '12 (Main)' and '8 (loop end)' show whole. |
| N4 The 'it' in Merge pointing at the wrong station | 17, 53 | Fixed: 'Detach Line from Station brings Lower / 3 back.' comes right after the archive sentence, then what stays inside. |
| N5 'Side's own 0' also fit the returning station | 14, 54 | Fixed: 'Rename the 0 that is on Side now (Station ID 29) to 0a' and 'the L8 that is on Loop now (Station ID 37)'. |
| N6 Lines named twice; cursor moves on | 33, 34 | Fixed: the lines are named once, on the button; the grid cursor stays on the edited row while the prompt is open. |
| N7 Twig pick drops what stays archived | 20, 21 | Fixed: either pick names every station that goes with the lines; nested ones are named too ('with Upper / 0 (Station ID 46) archived inside Lower / 3', from FX9). |

## Short usability pass on the re-shot surfaces (7953325ba)

The re-shot surfaces read well. The junctions are labelled. Every collision answer says
which station it renames, in Remove Closure too. Every lead follows the answer. Merge and
Detach say what goes where, including stations nested inside others. Nothing found rises
to P1 or P2.

- **P3, R1. Many line names are never placed on the map.** At the views shot here (z16.5,
  17.4, 18.75, 20.5), the label probe keeps only the Main, Loop and Twig names. Side, Lower,
  Upper, Annex and Spur are never placed, also after full rebuilds. (The near-white Side name
  would be invisible on the light map anyway.) This is not FX8-made. The 14ee871fb probes
  already lacked Lower, Upper and Annex at these zooms. The reveal ladder lets a name that only
  clears its neighbours after z14 lose to station labels for good (`labelReveal.ts`, the ladder
  around lines 505-526). FX8's aim that a branch keeps its name is therefore not visible
  here, though the Sections panel names every line. A fix could admit names a few rungs
  earlier for the lines in view, or prefer a name over the station labels of its own line.
- **P3, R2. The remaining-junction picker comes after its consequences.** In 20 and 21 the
  lead ('They get back ...'), the archived-inside notes and the After Detach list all depend
  on the pick 'The lines that come off show:', which sits below them, just above the buttons.
  Moving the picker under the line choices would read top to bottom.
- **Nit.** 'A new name changes it on every line it is on.' (33, 34) now names no lines, so
  the sentence adds little next to the button that names them; it could go.

## Sweep results (14ee871fb round; the 7953325ba round was clean after every flow too)

| After | Result |
|---|---|
| Doctor (deliberate panic) | `fatal crash-marker` + `error log-error` for the self-test only |
| 1 Merge (incl. warning, history) | clean |
| 2 Keep the other | clean |
| 3 Detach with collision | clean |
| 3 Detach together, picker | clean |
| 4 Create Loop Closure | clean |
| 5 Remove Closure | clean |
| 6 Rename prompts (incl. the shot 54 setup) | clean |
| 7 Delete restore | clean |
| 8 Plan mode | clean |
| 9 Pills | clean |
| 10 0.6.0 upgrade (after save) | clean |
| 10 earlier version 2 upgrade (after save) | clean |
| 1024x640 and final | clean |

The app log holds one ERROR line, the self-test panic, and no WARN lines.

## Not covered

- The app has no station search; "Go to station" (a warning resolution) was not opened; the
  map has no hover tooltip.
- The UI-zoom (WebKit setZoom) matrix and 1920x1080 were not run; one dialog at 1024x640.
- The letters note (P3-13) was only checked where no letter exists.
- No replay or repro report: the REPL drove the session directly.

## Cleanup

After each round (the last one on 7953325ba), app, tauri-driver and Xvfb `:176` were stopped
through the session's own stop (SIGTERM path); nothing is left running. Deleted: `/tmp/rsd`
(raw shots, synthetic files, scripts), the scratch generator crate and its target dir, and
`.agent-artifacts` (REPL and harness session dir) in the sg-rs worktree; `~/.cargo_build_rs_a`
holds this tree's 7953325ba debug build. Nothing committed.
