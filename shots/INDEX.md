# PR #745 round 2 screenshots

Build tags: M = master 63050ac1c, N = author head 19f1b5b80 (before review fixes), P = pr745-r3 a63ab3ec8 (with review fixes). Window 1440x960 unless SMALL. Synthetic data only.

| File | Caption |
|---|---|
| 001-W0-M-synthetic-survey-no-datums.png | Master. Synthetic survey: A Entrance (5 x 50 m E), B North (5 x 50 m N, B:0 merged to A:5), C Separate (3 x 40 m), no datums yet. |
| 002-W0-P-synthetic-survey-no-datums.png | PR with fixes. Same synthetic survey, no datums. |
| 003-W1-M-set-zero-datum-dialog-empty.png | Master. Right-click A:0 > Set Zero Datum: empty dialog. |
| 004-W1-M-set-zero-datum-gps-entered.png | Master. GPS 20.5, -87.3 typed; no role/preview (single-datum world). |
| 005-W1-M-after-first-datum-details.png | Master. After Save: A:0 drawn as the datum; Details "Zero datum station". |
| 006-W1-P-set-zero-datum-dialog-empty.png | PR. Same dialog before GPS is typed. |
| 007-W1-P-set-zero-datum-gps-entered-entrance-locked.png | PR. GPS typed: adds a ticked, disabled Entrance box "The zero datum that places this part of the survey is always an entrance"; no accuracy field (correct for a placement). |
| 008-W1-P-after-first-datum-details-placement.png | PR. After Save: Details now titled "Placement datum - Places this part of the survey on its GPS". |
| 009-W1-P-edit-station-stored-placement-role-line.png | PR. Edit Station on a stored placement datum: "Role: places this part of the survey on its GPS", no Accuracy field. |
| 010-W2-M-B5-second-datum-6m-no-preview.png | Master. Set Zero Datum on B:5 (same connected piece) with a GPS 6 m E: no preview of any kind. |
| 011-W2-M-B5-second-datum-40m-no-preview.png | Master. Same with a GPS 40 m NE: still no preview. |
| 012-W2-M-B5-second-datum-refused.png | Master. Save refused: "Cannot set zero datum: "B North" #5 is connected to "A Entrance" #0 which already has a zero datum". |
| 013-W2-P-B5-fix-preview-6m.png | PR. B:5 with GPS 6 m E: fix notice, "GPS is 6.0 m E of the surveyed position", Accuracy 0, Entrance ticked (end of section). Save/Cancel already cut off at the bottom at 1440x960. |
| 014-W2-P-B5-fix-preview-40m-warning-cut-off.png | PR. GPS 40 m NE: "GPS is 40 m NE", warning "more than the 25 m expected over 500 m of survey..."; the warning is cut and Save is below the fold. |
| 015-W2-P-B5-fix-preview-40m-scrolled.png | PR. Same dialog scrolled to the bottom: full warning, Cancel/Save. |
| 016-W2-P-after-save-raw-view-unchanged.png | PR. After Save: Raw Data View unchanged (B:5 still at 247.81, 247.95), B:5 drawn as a hollow ring, Warnings badge 1. |
| 017-W3-P-details-B5-gps-fix.png | PR. Details of B:5: "GPS fix", hint, "GPS is 40 m NE of the surveyed position", threshold box, "GPS reading (not the surveyed position)". No accuracy or Entrance shown. |
| 018-W3-P-details-A0-placement.png | PR. Details of A:0: "Placement datum". |
| 019-W3-P-map-closeup-fix-hollow-ring.png | PR. Map close-up: the GPS fix B:5 is a hollow gold ring. (Toast from the first autosave: "Older app versions drop this file's GPS fixes and accuracies on save".) |
| 020-W3-P-map-closeup-placement-filled.png | PR. Map close-up: the placement datum A:0 is a filled gold dot. |
| 021-W4-P-raw-view-hard-fix-40m.png | PR. Raw Data View with the 40 m hard fix (accuracy 0): survey unchanged. |
| 022-W4-P-display-menu-ccv-offered.png | PR. Display menu: Closures Computed View offered although there is no loop closure (a GPS fix is an adjustment input). |
| 023-W4-P-ccv-hard-fix-on-gps-notice.png | PR. Closures Computed View, hard fix: B:5 exactly on its GPS (0.000 m), A:3 dragged 16.6 m north; notice "Bent to a GPS fix at 1:5, 40 m from its surveyed position. Check it." |
| 024-W4-P-edit-station-accuracy-5m.png | PR. Edit Station B:5, Accuracy 5 (role line and gap readout visible). |
| 025-W4-P-ccv-soft-fix-5m-notice-overstates.png | PR. Computed view with accuracy 5 m: B:5 moved only 5.3 m (34.7 m still off its GPS) but the notice still says "Bent to a GPS fix at 1:5, 40 m..."; Details shows CCV X/Y 251.56/251.70. |
| 026-W4-P-plan-mode-same-notice.png | PR. Plan mode: same far-fix notice; the fix B:5 has an entrance star. |
| 027-W4-P-export-preview-no-notice.png | PR. Export preview: the drawing is bent toward the fix but there is NO far-fix notice in Export. |
| 028-W5-P-warnings-tab-datum-gps-gap.png | PR. Warnings tab: "The GPS position of this zero datum is 40.0 m from where the survey puts it (bearing 45 degrees), more than the 25.0 m expected..."; only a "B North 1" chip locates it. |
| 029-W5-P-resolution-options.png | PR. Resolve panel: Go to station / Edit zero datum / Ignore (keep value, don't warn again). |
| 030-W5-P-survey-issues-fix-ring-lost.png | PR. Survey Issues colouring (auto-on with the Warnings tab): the fix B:5 is a filled yellow dot, the hollow-ring role cue is gone. |
| 031-W5-P-ignored.png | PR. After Ignore + Apply: "0 warnings 1 resolved". |
| 032-W5-P-new-gps-45m-warning-back.png | PR. Edit Station: new GPS 45 m NE: the warning is back ("45.0 m"), badge 1. |
| 033-W5-P-undo-gps-ignore-back.png | PR. Undo the GPS edit: GPS back to the 40 m reading and its Ignore applies again ("0 warnings 1 resolved"). |
| 034-W6-M-before-merge-two-placed-pieces.png | Master. Two placed pieces: A+B (A:0) and C (C:0); C:3 is 12 m from A:3. |
| 035-W6-M-merge-redirected-to-closure.png | Master. Merge With C:3 then click A:3: redirected to "Create Loop Closure - Both sections have fixed coordinates". |
| 036-W6-P-before-merge-two-placed-pieces.png | PR. Same two placed pieces. |
| 037-W6-P-merge-dialog-datum-shift.png | PR. Merge dialog: "C Separate is placed by its own GPS. It moves 12 m to meet the other piece, and its zero datum at 2:0 becomes a GPS fix." (The "Checking merge compatibility..." toast is still up.) |
| 038-W6-P-after-merge-C-moved.png | PR. After Merge: C moved 12.044 m onto A:3, C:0 drawn as a hollow ring (now a GPS fix). |
| 039-W6-P-after-merge-C0-details.png | PR. Details of C:0: "GPS fix", "GPS is 12 m SE of the surveyed position". |
| 040-W6-P-after-undo-merge.png | PR. Undo: both pieces placed by their own datums again (max coordinate change 0.000 m). |
| 041-W7-N-before-remove-placement.png | Author head. A:0 placement (frame), B:5 hard fix 40 m NE, C placed by C:0. |
| 042-W7-N-remove-placement-dialog.png | Author head. Edit Station A:0 with Set as Zero Datum unticked: no word about what happens next. |
| 043-W7-N-after-remove-placement.png | Author head. After Save: B:5 promoted and on its GPS, C on its GPS (same as the PR). No toast. |
| 044-W7-P-before-remove-placement.png | PR. Same setup. |
| 045-W7-P-remove-placement-dialog.png | PR. Same dialog: no preview of the promotion or the 40 m move. |
| 046-W7-P-after-remove-placement-moved-40m-silently.png | PR. After Save: A+B moved 40 m NE onto B:5's GPS (C now crosses A at a different place); no toast, and the 40 m gap warning is gone (Warnings badge empty). |
| 047-W8-N-before-delete-first-shot.png | Author head. D Main: D:0 placement, D:1 fix 30 m E of its surveyed position; C placed separately. |
| 048-W8-N-delete-first-shot-confirm.png | Author head. Right-click shot 0:1 > Delete Shot: "Are you sure you want to delete this shot?" (nothing about the datum). |
| 049-W8-N-after-delete-first-shot-BUG.png | Author head (bug). The promoted datum is not placed: new D:0 at (39.95, 0) instead of the origin; C 69.95 m off its GPS. |
| 050-W8-P-before-delete-first-shot.png | PR. Same setup. |
| 051-W8-P-delete-first-shot-confirm.png | PR. Same confirm dialog: still nothing about the datum or the 30 m move. |
| 052-W8-P-after-delete-first-shot-fixed.png | PR (fixed). New D:0 on the origin (its GPS), C 0.001 m from its GPS. D moved 30 m E with no message. |
| 053-W9-N-fix-gps-copied-preview.png | Author head. Set Zero Datum on A:3 with the entrance's exact GPS 20.5, -87.3: preview only says "GPS is 148 m W". |
| 054-W9-N-before-remove-entrance.png | Author head. Before removal (C datum set before the fix: C becomes the frame). |
| 055-W9-N-after-remove-entrance-BUG.png | Author head (bug). Promoted A:3 not placed: 148.263 m off its GPS. |
| 056-W9-P-fix-gps-copied-preview.png | PR. Same preview; nothing says this GPS is identical to the zero datum at 0:0. |
| 057-W9-P-before-remove-entrance.png | PR. Before removal. |
| 058-W9-P-after-remove-entrance-fixed.png | PR (fixed). A:3 on its GPS (0.001 m), C at the origin. |
| 059-W9b-N-fix-gps-copied-preview.png | Author head, variant W9b (fix set before C, so the fix becomes the frame). |
| 060-W9b-N-before-remove-entrance.png | Author head, W9b before removal. |
| 061-W9b-N-after-remove-entrance-BUG.png | Author head (bug). C left 148.263 m off its GPS (C sits right above A instead of 148 m east). |
| 062-W9b-P-fix-gps-copied-preview.png | PR, W9b preview. |
| 063-W9b-P-before-remove-entrance.png | PR, W9b before removal. |
| 064-W9b-P-after-remove-entrance-fixed.png | PR (fixed). A:3 at the origin = its GPS, C 0.000 m from its GPS. |
| 065-W10-N-raw-view-one-shot.png | Author head. One-shot section E: E:0 placement, E:1 hard fix 40 m N. Raw Data View. |
| 066-W10-N-ccv-fix-not-applied-BUG.png | Author head (bug). Closures Computed View: E:1 still at its raw position, 40 m off its GPS, while the notice claims "Bent to a GPS fix at 0:1". |
| 067-W10-P-raw-view-one-shot.png | PR. Same Raw Data View. |
| 068-W10-P-ccv-fix-applied-fixed.png | PR (fixed). E:1 on its GPS (0.000 m); the 50 m shot is drawn about 64 m long, about 39 degrees off its azimuth. |
| 069-W11-P-merge-onto-station-holding-fix.png | PR. Merge E:0 (E Extension, placed by its own GPS) with B:5 (holds a GPS fix): dialog says E moves 5.0 m and its datum becomes a fix; nothing about B:5's own fix 40 m away from E's GPS. |
| 070-W11-P-combine-refused-reason-truncated.png | PR. Right-click B:5: "Combine Section With E Extension..." disabled, reason truncated "B North #5 and E Extension #0 each hold a ..." (full text only in the tooltip). |
| 071-W11-P-combine-refused-from-E0.png | PR. Right-click E:0: same refusal from the other side. |
| 072-W11-P-ccv-two-fixes-one-point-notice-wrong.png | PR. Computed view: the merged point sits on E:0's GPS (hard), B:5's fix is ignored (40 m off), yet the notice names 1:5. |
| 073-W12-N-save-notice.png | Author head. First explicit Ctrl+S: warning toast "Older app versions drop this file's GPS fixes and accuracies on save". |
| 074-W12-P-save-notice-once-per-project.png | PR. First explicit Ctrl+S on a new project: the same toast; a second save shows nothing. |
| 075-W12-P-demo-ctrl-s-no-second-notice.png | PR. Demo project: Ctrl+S after autosave already showed the notice: no repeat. |
| 076-W12-P-closed-global-view.png | PR. App menu > Close Project: Recent projects. |
| 077-W12-P-reopened-fix-details.png | PR. Reopened: B:5 is still a GPS fix, gap 45 m NE. |
| 078-W12-P-reopened-accuracy-entrance.png | PR. Reopened: Edit Station shows Accuracy 7.5 and Entrance ticked. |
| 079-W12-P-reopened-warning.png | PR. Reopened: the gap warning is still listed. |
| 080-W13-P-mid-passage-fix-entrance-off.png | PR. Set Zero Datum on A:3 (mid-passage, GPS 3 m S): Entrance unticked by default; gap 3.0 m, no warning. |
| 081-W13-P-penetration-250m-B5-entrance.png | PR. Project stats: Penetration 250.0 m while B:5 is an entrance. |
| 082-W13-P-B5-entrance-unticked.png | PR. Edit Station B:5: Entrance unticked. |
| 083-W13-P-penetration-500m-B5-not-entrance.png | PR. Penetration 500.0 m once B:5 is not an entrance. |
| 084-W13-P-plan-start-from-entrance.png | PR. Plan > Start from entrance: reach from A:0 and C:0; non-entrance fixes still carry stars. |
| 085-W13-P-plan-non-entrance-fix-star.png | PR. Plan close-up: the non-entrance fix A:3 has the same black entrance star (tooltip "Zero datum - click to start the plan here"). |
| 086-W13-P-export-stars-on-non-entrance-fixes.png | PR. Export preview: stars at A:3 and B:5 although neither is an entrance. |
| 087-W13-P-entrance-checkbox-states-crop.png | PR. Crop: the locked Entrance box of a placement (top, dimmed) vs the editable one of a fix (bottom). The disabled state is visible. |
| 088-W14-P-undo-history-identical-labels.png | PR. Undo history (hover): setting the fix and the accuracy-only edit are both "Set zero datum at 1:5". |
| 089-W14-P-remove-placement-promotes-moves-40m.png | PR. W10 project: removing placement E:0 promotes E:1; the section moves 40 m in the world, nothing on screen says so and the gap warning disappears. |
| 090-W15-P-accuracy-minus-1.png | PR. Accuracy "-1": inline "Enter an accuracy from 0 to 1000 m." |
| 091-W15-P-accuracy-abc.png | PR. Accuracy "abc" (real keystrokes): the number field shows empty, inline error. |
| 092-W15-P-accuracy-1000-ok.png | PR. Accuracy 1000: accepted. |
| 093-W15-P-accuracy-1001.png | PR. Accuracy 1001: inline error. |
| 094-W15-P-accuracy-5000.png | PR. Accuracy 5000: inline error. |
| 095-W15-P-accuracy-blank.png | PR. Blank: inline error (blank is refused, not read as a hard pin). |
| 096-W15-P-accuracy-decimal-comma-becomes-25.png | PR. Typed "2,5": the WebKit number field drops the comma and shows 25, no error. |
| 097-W15-P-accuracy-blank-save-refused.png | PR. Save with blank: refused, dialog stays open. |
| 098-W15-P-accuracy-abc-save-refused.png | PR. Save with "abc": refused. |
| 099-SMALL-P-1100x750-edit-station-top.png | PR at 1100x750. Edit Station for a GPS fix: role, Entrance, gap, warning and Save are all below the fold. |
| 100-SMALL-P-1100x750-edit-station-scrolled.png | PR at 1100x750. Same dialog scrolled: content fits, nothing clipped horizontally. |
| 101-SMALL-P-1100x750-ccv-notice-details.png | PR at 1100x750. Computed view notice wraps to two lines next to Show warnings; Details fit. |
