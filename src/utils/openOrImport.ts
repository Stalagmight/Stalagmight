/**
 * Unified "Open / Import" routing.
 *
 * A single entry point that accepts any file the app can open or import and
 * dispatches to the right flow based on the file extension:
 *
 * - `.stalx`            -> open the native project (loadProject)
 * - `.gpx`              -> GPX import (new project only; rejected if one is open)
 * - `.tml` / `.tmlu`    -> Ariane import (always a new project)
 * - `.mak` / `.dat`     -> Compass import (always a new project)
 * - `.wpj` / `.srv`     -> Walls import (always a new project)
 * - `.dmp`              -> DMP import (add to current project, or new project)
 * - anything else       -> unsupported-file error (NOT silently treated as DMP)
 *
 * Importing INTO an open project while editing is blocked (the closure-adjusted
 * view, the Plan/Export workspaces, or a SpeleoDB project held without the
 * write lock) is rejected here, so every import entry point shares that guard.
 * This only applies to `.dmp` (the sole additive in-project import, #391) - the
 * other formats always close the current project first (via handleCloseProject,
 * which carries its own SpeleoDB hand-back and unsaved-changes guards) rather
 * than mutating it, so the read-only guard doesn't apply to them.
 *
 * ONE ROUTER (#407): every open/import entry point routes the chosen file
 * through here - the hamburger Open / Import and Import rows, the EmptyState
 * card, the "open it from file" hint link, Ctrl+O, Ctrl+I, drag-drop, and the
 * OS file-launch channel - so the dispatch, the guards and the error text are
 * defined once. What differs between doors is only which files their PICKER
 * offers: the Open doors show everything (`openOrImportFilters`), while the
 * in-project Import doors show `.dmp` alone (`dmpImportFilters`, #508), since
 * that is the only format additive to an open project. Narrowing a picker
 * narrows nothing here - a file that arrives by any other route (drag-drop, a
 * typed filename, the OS) is still classified by extension below.
 * Stores are read via `getState()` so this is callable outside React (e.g.
 * from a raw keydown handler).
 */
import { useProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';
import { useSpeleoStore, selectSpeleoReadOnly } from '../store/speleoStore';
import { useUpdateStore, selectUpdateInstalling } from '../store/updateStore';
import { projectSessionEpoch } from '../store/projectSession';
import { isEditingBlockedNow } from '../hooks/useEditingBlocked';
import { getErrorMessage } from './ipc';
import { extractFilename } from './path';
import { openDialog, openOrImportFilters, DMP_EXTENSION } from './fileDialogs';
import { clearProjectActionSource } from './telemetry';
import { editsSettledForExit, editsSettledForHandBack, needsSpeleoHandBackPrompt } from './quit';
import { openPendingImport, type PendingImport } from './pendingImport';
import { TOAST_KEYS } from './toastKeys';
import i18n from '../i18n';

/**
 * Filename to show in the "Unsupported file type" dialog. A path ending in a
 * separator (e.g. a directory dropped by mistake) makes `extractFilename`
 * return '' - falling back to the full `path` in that case would print the
 * absolute path (home dir, username) into a dialog, so trailing separators
 * are stripped first and there is no path fallback.
 */
function unsupportedFileDisplayName(path: string): string {
  return extractFilename(path.replace(/[/\\]+$/, ''));
}

/**
 * Await one of the six import parsers (parseTml/parseGpx/parseCompass/
 * parseWalls/importDmp), converting a THROWN rejection into the error dialog
 * with `hideReportHint: true`.
 *
 * Scoped to exactly the parse call - not the whole `routeOpenedFile` try block
 * - because a parser rejection is almost always a malformed or unreadable user
 * file, not an app defect, so the "report this as a bug" hint would send a
 * damaged-file report where none is warranted (#447). Anything else that
 * throws inside `routeOpenedFile` (a routing/store bug) falls through to the
 * function's own catch, which keeps the hint - that failure mode IS worth a
 * bug report.
 *
 * Returns null on a caught rejection, matching the existing `if (!file)
 * return` abandon path every caller already has for a parser that resolves to
 * null (no dialog, quiet abort) - the two null cases end up in the same
 * no-op branch, only one of them opened a dialog first.
 *
 * [#576 HFI-4-D1] `stillWanted` is read when the parse settles: a result nobody
 * is waiting for any more - another project opened meanwhile, or the
 * import-as-new confirm was cancelled - is dropped, success or failure alike,
 * so neither a late dialog nor a late error lands over what the user did since.
 */
async function parseForImport<T>(
  promise: Promise<T>,
  errorTitle: string,
  stillWanted: () => boolean,
): Promise<T | null> {
  try {
    const parsed = await promise;
    return stillWanted() ? parsed : null;
  } catch (err) {
    if (!stillWanted()) return null;
    useUIStore.getState().openDialog('error', {
      title: errorTitle,
      message: getErrorMessage(err, 'Failed to open file'),
      hideReportHint: true,
    });
    return null;
  }
}

/**
 * Default project name for a new-project import: prefer the name the file
 * itself carries (TML/Compass's cave name, Walls's project name, GPX's track
 * name - each shown in NewProjectForImportDialog's summary immediately above
 * the name field) over the filename, so "Naharon.tml" imports as "Naharon"
 * rather than as the filename. Falls back to `fallback` (the filename-derived
 * default) when the file carries no such name, or only whitespace - trimmed
 * so a name that is whitespace-only doesn't win over a real fallback. One
 * helper for all four formats that carry an in-file name (DMP does not).
 */
function projectNameFromCaveOrFile(fileName: string | null | undefined, fallback: string): string {
  const trimmed = fileName?.trim();
  return trimmed || fallback;
}

/**
 * Open the "import as new project?" confirm dialog (#391) for a new-project-only
 * format (GPX / Ariane / Compass / Walls) picked while a project is already
 * open. Replaces the old close-first dead end (`IMPORT_SURVEY_CLOSE_FIRST`):
 * confirming re-routes the same path with `importAsNew`, which parses the file
 * while the project stays open and closes it (honoring its own unsaved-changes
 * guard) only once the parse succeeded (#576 HFI-4-D1).
 */
function openImportAsNewProject(ui: ReturnType<typeof useUIStore.getState>, path: string): void {
  ui.openDialog('import-as-new-project', { path, fileName: extractFilename(path) });
}

/**
 * The dialogs that own an IN-FLIGHT import flow - one that has declared an
 * `importContext` and has not yet reached its `saveAfterImport`. Mirrors the
 * import-flow dialogs mounted in App.tsx, in flow order.
 *
 * Three import-adjacent dialogs are deliberately NOT here:
 * - `import-receipt` is post-save (the flow's `saveAfterImport` already ran), so
 *   re-routing behind it can lose nothing, and "finish the current import" would
 *   be false advice for a dialog whose only action is Close.
 * - `import-blocked` and `import-as-new-project` are PRE-flow prompts that own no
 *   context and re-enter this router themselves; both `closeDialog()` first, so
 *   they would pass the guard anyway, but listing them would be a trap.
 */
const IMPORT_FLOW_DIALOGS = new Set([
  'new-project-for-import',
  'dmp-selection',
  'zero-datum-selection',
  'zero-datum-config',
  'dmp-import-new',
  'tml-import',
  'compass-import',
  'walls-import',
  'gpx-selection',
  'gpx-placement',
  'orphan-placement',
]);

/**
 * True while an import flow owns the screen and still needs its `importContext`.
 *
 * Two shapes, because the flow is not always behind a dialog. The DMP wizard's
 * MANUAL PLACEMENT phase hands the map to the user: it calls
 * `startPlacementMode(...)` and then `closeDialog()` (DmpNewImportDialog.tsx,
 * `startSegmentPlacement` -> "Close dialog so user can interact with the map"),
 * so `activeDialog` is null for the whole phase while the flow is very much
 * in-flight and its `saveAfterImport` is still to come. `placementMode` is the
 * signal that covers it: `startPlacementMode` sets it (uiStore) and BOTH exits
 * clear it - `confirmPlacement()` and `cancelPlacementMode()` each
 * `set({ placementMode: null })` - so it is false again the moment placement
 * ends, whether the user placed the segment or cancelled back to the wizard.
 *
 * It reads slightly wider than "import": the paste-sections dialog uses
 * placement too. That is the right side to err on - the rest of the app already
 * treats placement as modal-equivalent (the toolbar disables, SingleSystemView
 * dims and drops pointer events, useKeyboardShortcuts allowlists a handful of
 * actions), so routing a dropped file into it was never wanted either.
 */
function importFlowOwnsScreen(ui: ReturnType<typeof useUIStore.getState>): boolean {
  if (ui.activeDialog && IMPORT_FLOW_DIALOGS.has(ui.activeDialog)) return true;
  return ui.placementMode !== null;
}

/**
 * Dispatch an already-chosen path to the matching open/import flow.
 *
 * @param path       Absolute path to the selected file.
 * @param errorTitle Title for the error dialog if the flow throws.
 * @param opts.residualReadOnlyToast  Set by ImportBlockedDialog's "Switch to
 *   Survey" re-route: the user just confirmed intent there, so a read-only
 *   cause that REMAINS after the switch (e.g. the restored survey view being
 *   closure-adjusted) is reported as a non-blocking toast instead of a second
 *   modal - informational, no click owed. Never set on first-touch routes,
 *   where the modal (or the import-blocked choice) is the right interruption.
 * @param opts.swapConfirmed  Set by LaunchOpenConfirmDialog's "Open" button
 *   when it re-routes the same path after the user confirmed swapping out a
 *   clean, already-open project: skips that same confirm on this second pass,
 *   so confirming doesn't ask again.
 * @param opts.importAsNew  Set by ImportAsNewProjectDialog's confirm, which
 *   stays on screen (showing the parse in progress) while this runs: "parse
 *   first, then switch" (#576 HFI-4-D1). The project stays open while the file
 *   parses; only a successful parse closes it, through `closeCurrent` (the same
 *   guarded close as File > Close), and hands over to `new-project-for-import`.
 *   A failed parse leaves the project untouched. A close that must ask first
 *   (unsaved changes, SpeleoDB hand-back) asks only after a good parse, carrying
 *   the parsed file (#576 HFI-4-D2): its own close then opens the new-project
 *   dialog with it, and its Cancel keeps the project and drops the file.
 * @param opts.allow  Restricts which extensions this call may route. `'project-
 *   only'` refuses everything but `.stalx` with the same unsupported-file
 *   dialog used for an unrecognized extension. Recents entries are read from
 *   persisted config (a hand-edited or legacy entry could carry any extension),
 *   so every recents open call site passes this - it must never reach the six
 *   import parsers, which the OS-launch route (`launch.rs` `LAUNCHABLE_EXTENSIONS`)
 *   also restricts to `.stalx`/`.tml`/`.tmlu`. Unset (the default) keeps the
 *   general router's full dispatch for the picker, drag-drop, and Ctrl+O/Ctrl+I.
 */
export async function routeOpenedFile(
  path: string,
  errorTitle = 'Open Failed',
  opts?: {
    residualReadOnlyToast?: boolean;
    swapConfirmed?: boolean;
    allow?: 'project-only';
    importAsNew?: { closeCurrent: () => Promise<{ success: boolean }> };
  },
): Promise<void> {
  const lowerPath = path.toLowerCase();
  const store = useProjectStore.getState();
  const ui = useUIStore.getState();
  const hasProject = !!store.project;
  const importAsNew = opts?.importAsNew;

  // [#576 HFI-4-D1] The version guard every parse below settles against, in the
  // shape of the `projectIdentityKey` guards (useMapTools, useUnwindTarget): the
  // project session the file was picked in, and - for an import-as-new run - the
  // confirm dialog instance it was confirmed in (`openDialog` always installs a
  // fresh `dialogData`, so its identity names the instance). A parse that settles
  // after either moved is a late result and is dropped.
  const session = projectSessionEpoch();
  const confirmDialogData = importAsNew ? ui.dialogData : null;
  const confirmStillShowing = (): boolean => {
    const now = useUIStore.getState();
    return now.activeDialog === 'import-as-new-project' && now.dialogData === confirmDialogData;
  };
  const stillWanted = (): boolean =>
    projectSessionEpoch() === session && (!importAsNew || confirmStillShowing());
  // A dropped result whose confirm dialog is still up (the project changed under
  // it, not by a cancel) takes the dialog down too: it names the old project.
  const dropLateResult = (): void => {
    if (importAsNew && !stillWanted() && confirmStillShowing()) useUIStore.getState().closeDialog();
  };
  // After a successful parse, switch to the new project. Over an open project the
  // close comes first. A close that must ask (a SpeleoDB hand-back, then unsaved
  // changes: `handleCloseProject`'s order) asks NOW, after the good parse, and
  // carries the parsed file (#576 HFI-4-D2): its own close opens the new-project
  // dialog with it (`pendingImport`), its Cancel keeps the project and drops the
  // file. A close with nothing to ask goes through the guarded close as before.
  // True when a dialog took the screen (it owns the entry-point source from here);
  // false when the close was refused and the import ends.
  //
  // [#576 W85] Both checks read state an edit sets only once its IPC and the refresh
  // after it have returned, so they run once the edits queued so far have settled,
  // as the close itself does (`editsSettledForExit`): an edit still in flight read
  // as a clean project, the close below found it and asked WITHOUT the file, and the
  // parsed import was dropped. One that has not settled in time counts as unsaved
  // work. The confirm can be cancelled during the wait, which drops the file.
  const handOverParsed = async (pending: PendingImport): Promise<boolean> => {
    if (importAsNew && useProjectStore.getState().project) {
      const editsSettled = await editsSettledForExit();
      if (!stillWanted()) {
        dropLateResult();
        return false;
      }
      if (needsSpeleoHandBackPrompt(useSpeleoStore.getState().link)) {
        ui.openDialog('speleodb-close', { pendingImport: pending });
        return true;
      }
      if (useProjectStore.getState().isDirty || !editsSettled) {
        ui.openDialog('unsaved-changes', { pendingImport: pending });
        return true;
      }
      if (!(await importAsNew.closeCurrent()).success) return false;
    }
    openPendingImport(pending);
    return true;
  };

  // A dialog (or the terminal command it triggers) now owns the pending entry-
  // point source's lifetime - leave it set so that flow can apply it on
  // completion (or drop it on cancel via checkDialogCancel). EVERY other path
  // (abandon, guard-rejected, parse-null, unsupported, error) drops the source
  // in the finally below, so it can't mis-attribute a later, unrelated project
  // command. `setView('single')` clears it on a successful open/create.
  let handedOff = false;

  try {
    // [#579 x #576] An update is installing and the project is held still until it
    // ends. The install overlay covers every DOM entry point, but a file dropped on
    // the window arrives as a native webview event: it must not open, import or
    // raise a prompt over the install. The overlay already says why nothing acts.
    if (selectUpdateInstalling(useUpdateStore.getState())) return;

    if (opts?.allow === 'project-only' && !lowerPath.endsWith('.stalx')) {
      ui.openDialog('error', {
        title: errorTitle,
        message: `Unsupported file type: ${unsupportedFileDisplayName(path)}`,
      });
      return;
    }

    // One import at a time (#411). Drag-drop routes unconditionally - the drop
    // listener has no modal check (Ctrl+O/Ctrl+I do: useKeyboardShortcuts returns
    // early for every non-Escape key while a dialog is open) - so a second file
    // dropped mid-flow would re-enter here and the reset below would null the
    // context the in-flight flow's `saveAfterImport` is about to read, losing its
    // save. Refuse instead, and say so; the running flow keeps the screen and its
    // context. Guarded in the router, not in the drop handler, so every present
    // and future entry point inherits it. Inside the `try` so the abandoned
    // route's telemetry source is dropped by the `finally`, like every other
    // guard-rejected path.
    if (importFlowOwnsScreen(ui)) {
      ui.showToast(i18n.t('importGate.inProgress'), TOAST_KEYS.IMPORT_IN_PROGRESS, 'warning');
      return;
    }

    // Any OTHER dialog also owns the screen and must not be silently replaced -
    // a dropped (or OS-launched, or recents-opened) file used to call
    // loadProject/openDialog straight over settings, an error dialog, or a
    // pending unsaved-changes prompt, because `importFlowOwnsScreen` above only
    // recognizes the import wizard's OWN dialogs. Mirrors
    // `useKeyboardShortcuts`' `activeDialog && event.key !== 'Escape'` gate,
    // which already refuses Ctrl+O/Ctrl+I the same way.
    //
    // The one deliberate exception is `swapConfirmed`: `LaunchOpenConfirmDialog`
    // re-routes the SAME path through here while `launch-open-confirm` is still
    // the active dialog (its own handleConfirm routes BEFORE closing, to keep
    // the dialog slot - and `screenBusy()` - occupied for the whole flow; see
    // there). Every other dialog that re-enters this router closes itself
    // first, so `activeDialog` is already null by the time it calls back in -
    // except `import-as-new-project`, which (#576 HFI-4-D1) stays up showing the
    // parse in progress until the new-project dialog replaces it.
    const reentryOwnsScreen =
      opts?.swapConfirmed === true ||
      (importAsNew !== undefined && ui.activeDialog === 'import-as-new-project');
    if (ui.activeDialog && !reentryOwnsScreen) {
      ui.showToast(i18n.t('importGate.inProgress'), TOAST_KEYS.IMPORT_IN_PROGRESS, 'warning');
      return;
    }

    // `importContext` tells `saveAfterImport` whether the completion handlers must
    // write the .stalx (a brand-new project's initial save is deferred) or only mark
    // dirty. It has no auto-reset, so a value left behind by a previous flow -
    // abandoned parse, cancelled dialog, or a completed import - would decide the
    // NEXT one, making the save non-deterministic across a session (#411). Reset it
    // here: this router is the single entry point of every import flow, and each
    // branch below re-declares its own context once it commits to a flow.
    ui.setImportContext(null);

    // Importing into an open project adds sections (a mutation) - but only DMP
    // does that; every other importable format instead CLOSES the current
    // project and imports as a new one (openImportAsNewProject below), which
    // doesn't mutate it and carries its own guards via handleCloseProject. So
    // this read-only guard is scoped to `.dmp` only, not every importable
    // extension - the read-only map (#70 closure-adjusted view, #243
    // Plan/Export, or a locked remote SpeleoDB project) rejects only the
    // additive DMP-into-open-project path. Centralized here so every caller
    // (toolbar, Ctrl+I, drag-drop) shares it.
    const isDmp = lowerPath.endsWith(`.${DMP_EXTENSION}`);
    if (isDmp && hasProject && isEditingBlockedNow()) {
      // Tailor the fix-it advice to the active read-only cause(s) - read the
      // individual flags separately (isEditingBlockedNow() reports only the
      // combined boolean, and isMapReadOnly folds the closure-adjusted and
      // Plan/Export causes into one) so a compound state (e.g. a
      // locked remote project also viewed in Plan mode) mentions BOTH causes
      // instead of silently dropping the SpeleoDB lock from the message.
      const speleoReadOnly = selectSpeleoReadOnly(useSpeleoStore.getState());
      const nonSurveyMode = ui.appMode !== 'survey';
      const closureAdjusted = ui.geometryMode === 'closureAdjusted';
      if (!speleoReadOnly) {
        // Every non-SpeleoDB cause is one safe click from fixed, so it's a
        // choice, not an error: the import-blocked prompt offers the switch
        // (Survey mode and/or the Raw Data View) and CONTINUES this import,
        // or abandons it. `cause` picks the wording; mode wins when both
        // apply because its switch also drops the view. The dialog owns the
        // pending telemetry source from here (complete-on-switch /
        // cancel-on-stay).
        ui.openDialog('import-blocked', { path, cause: nonSurveyMode ? 'mode' : 'view' });
        handedOff = true;
        return;
      }
      // A missing SpeleoDB write lock is the one cause no local switch can
      // fix, so it keeps the plain error dialog (compound states included -
      // switching mode/view alone would still leave the import blocked).
      let message: string;
      if (nonSurveyMode) {
        message = 'Switch to Survey mode and acquire the SpeleoDB write lock to import.';
      } else if (closureAdjusted) {
        message = 'Switch to the Raw Data View and acquire the SpeleoDB write lock to import.';
      } else {
        message = 'Acquire the SpeleoDB write lock to import.';
      }
      if (opts?.residualReadOnlyToast) {
        // The user just confirmed a switch on the import-blocked prompt -
        // don't demand a second click for a cause the switch could not fix;
        // inform and get out of the way.
        ui.showToast(message, 'import-read-only-residual', 'warning');
      } else {
        ui.openDialog('error', { title: 'Import', message });
      }
      return;
    }

    if (lowerPath.endsWith('.stalx')) {
      // Native project: guard against silently discarding unsaved edits. Opening
      // a .stalx replaces the current project, so if one is open and dirty, defer
      // to the unsaved-changes dialog (it loads on discard) instead of clobbering
      // it. Centralized here so EVERY .stalx-open path (Ctrl+O, drag-drop, the
      // unified Open/Import button) shares the guard, not just the menu item.
      //
      // #270: this is one of the four ways to leave an open project, and it used to
      // gate on the DISK-dirty bit alone - so a remote project saved to disk with a
      // held lock or with edits SpeleoDB has never seen was swapped out with no
      // notice, while closing the same project prompted. Ordered SpeleoDB-first for
      // the same reason `guardQuit` is: the SpeleoDB prompt carries its own "save
      // changes locally" checkbox and so subsumes the disk one, while the reverse
      // order would let "Discard Changes" walk straight past the lock and the
      // unpushed edits. The prompt carries the chosen path and performs the open
      // itself once the SpeleoDB side is resolved, so the user's Ctrl+O is not
      // silently swallowed by the prompt it raised.
      //
      // [#576 W84] Over an open project the guards run once the edits queued so far
      // have settled, as at quit (`editsSettledForExit`): an edit still in flight
      // reads as a clean project, and `loadProject` waits for it to land on this
      // project and then replaces it, unsaved. One that has not settled in time
      // counts as unsaved work. The state is read again after the wait. [#576 W85] An
      // open asked for while another hand-back exit waits (a double-click on a Recents
      // entry) is dropped: that exit owns the answer.
      const editsSettled = store.project ? await editsSettledForHandBack() : true;
      if (editsSettled === null) return;
      const current = useProjectStore.getState();
      if (current.project && needsSpeleoHandBackPrompt(useSpeleoStore.getState().link)) {
        ui.openDialog('speleodb-close', { pendingOpenPath: path });
        handedOff = true;
      } else if (current.project && (current.isDirty || !editsSettled)) {
        ui.openDialog('unsaved-changes', { pendingOpenPath: path });
        handedOff = true;
      } else if (current.project && !opts?.swapConfirmed) {
        // A clean, locally-owned project is open - dirty and SpeleoDB already
        // prompt above, but a clean project used to fall straight through to
        // loadProject and get swapped out with no notice at all. Reuses the
        // confirm this branch already raises for a `.stalx` the OS FORWARDS
        // over a clean project (`launchWithFile.ts`) - it's the identical
        // question ("swap the open project for this file?") asked by a
        // different trigger, so every `.stalx`-open entry point (picker,
        // drag-drop, Ctrl+O, and the launch channel) now shares one dialog.
        // Confirming re-enters here with `swapConfirmed: true` so the second
        // pass falls through to loadProject instead of asking again.
        ui.openDialog('launch-open-confirm', { path, fileName: extractFilename(path) });
        handedOff = true;
      } else {
        // loadProject handles view switching and error display, and consumes the
        // source itself (setView on success / its error paths on failure).
        await store.loadProject(path);
      }
    } else if (lowerPath.endsWith('.gpx')) {
      // GPX import creates a new project; importing into an open one isn't
      // supported (DMP is the additive in-project import). Mirror the TML/Compass
      // guard so a GPX opened/dropped while a project is open is rejected, not
      // silently added to the current project.
      if (store.project && !importAsNew) {
        openImportAsNewProject(ui, path);
        handedOff = true;
        return;
      }
      const gpxFile = await parseForImport(store.parseGpx(path), errorTitle, stillWanted);
      if (!gpxFile) {
        dropLateResult();
        return;
      }
      handedOff = await handOverParsed({
        gpxFile,
        filePath: path,
        defaultName: projectNameFromCaveOrFile(
          gpxFile.name,
          extractFilename(path, /\.gpx$/i) || 'Imported',
        ),
        importType: 'gpx',
      });
    } else if (lowerPath.endsWith('.tml') || lowerPath.endsWith('.tmlu')) {
      // TML/TMLU import always creates a new project.
      if (store.project && !importAsNew) {
        openImportAsNewProject(ui, path);
        handedOff = true;
        return;
      }
      const tmlFile = await parseForImport(store.parseTml(path), errorTitle, stillWanted);
      if (!tmlFile) {
        dropLateResult();
        return;
      }
      // Mirror the DMP branch: the project about to be created exists only for
      // this import (`openPendingImport` sets the 'new-project' import context the
      // import dialog reads to offer to discard it again, #411 C2).
      handedOff = await handOverParsed({
        tmlFile,
        filePath: path,
        defaultName: projectNameFromCaveOrFile(
          tmlFile.cave_name,
          extractFilename(path, /\.tmlu?$/i) || 'Imported',
        ),
        importType: 'tml',
      });
    } else if (lowerPath.endsWith('.mak') || lowerPath.endsWith('.dat')) {
      // Compass import always creates a new project.
      if (store.project && !importAsNew) {
        openImportAsNewProject(ui, path);
        handedOff = true;
        return;
      }
      const compassFile = await parseForImport(store.parseCompass(path), errorTitle, stillWanted);
      if (!compassFile) {
        dropLateResult();
        return;
      }
      handedOff = await handOverParsed({
        compassFile,
        filePath: path,
        defaultName: projectNameFromCaveOrFile(
          compassFile.cave_name,
          extractFilename(path, /\.(mak|dat)$/i) || 'Imported',
        ),
        importType: 'compass',
      });
    } else if (lowerPath.endsWith('.wpj') || lowerPath.endsWith('.srv')) {
      // Walls import always creates a new project.
      if (store.project && !importAsNew) {
        openImportAsNewProject(ui, path);
        handedOff = true;
        return;
      }
      const wallsFile = await parseForImport(store.parseWalls(path), errorTitle, stillWanted);
      if (!wallsFile) {
        dropLateResult();
        return;
      }
      handedOff = await handOverParsed({
        wallsFile,
        filePath: path,
        defaultName: projectNameFromCaveOrFile(
          wallsFile.project_name,
          extractFilename(path, /\.(wpj|srv)$/i) || 'Imported',
        ),
        importType: 'walls',
      });
    } else if (isDmp) {
      // mNemo DMP: import into the current project, or seed a new one.
      const dmpFile = await parseForImport(store.importDmp(path), errorTitle, stillWanted);
      if (!dmpFile) return;
      // Re-read project state fresh after the parse - the existing/new-project
      // decision must reflect the project as it is NOW, not the snapshot taken
      // before the await. A switch during the parse no longer gets this far
      // (#576 HFI-4-D1: `stillWanted` dropped the result - the file was picked
      // for the project that was open then), so this reads that same project.
      if (useProjectStore.getState().project) {
        ui.setImportContext('existing-project');
        ui.openDialog('dmp-selection', { dmpFile, filePath: path });
      } else {
        ui.setImportContext('new-project');
        ui.openDialog('new-project-for-import', {
          dmpFile,
          filePath: path,
          defaultName: extractFilename(path, /\.dmp$/i) || 'Imported',
        });
      }
      handedOff = true;
    } else {
      // Unknown/unsupported extension - don't blindly assume DMP (that fed
      // arbitrary files chosen via an "All Files" filter to the DMP parser).
      ui.openDialog('error', {
        title: errorTitle,
        message: `Unsupported file type: ${unsupportedFileDisplayName(path)}`,
      });
    }
  } catch (err) {
    // Every parser rejection is caught locally by `parseForImport` (with
    // `hideReportHint: true`) before it can reach here, so anything landing in
    // THIS catch is a routing/store bug - loadProject() throwing unexpectedly,
    // a guard's own code faulting, etc. - which IS worth a bug report, so the
    // hint stays on by default (#447).
    useUIStore.getState().openDialog('error', {
      title: errorTitle,
      message: getErrorMessage(err, 'Failed to open file'),
    });
  } finally {
    if (!handedOff) clearProjectActionSource();
  }
}

/**
 * Open a single-file picker with the given filters, then route the chosen file
 * to the matching open/import flow. On cancel, drop the entry-point source so it
 * can't attribute a later, unrelated open/create/import. Callers set the source
 * before calling.
 *
 * Shared by the toolbar "Open / Import" button, the in-project import button,
 * and the Ctrl+I keyboard shortcut (GlobalView + in-project), which differ only
 * in the picker filters and the error-dialog title.
 */
export async function pickAndRoute(
  filters: Array<{ name: string; extensions: string[] }>,
  errorTitle = 'Open Failed',
): Promise<void> {
  let selected: string | string[] | null;
  try {
    selected = await openDialog({ multiple: false, filters });
  } catch (err) {
    // The native picker itself failed (plugin/permission error) - `open()`
    // rejects rather than resolving null. Surface it and drop the entry-point
    // source so it can't attribute a later, unrelated open/create/import (the
    // source has no auto-reset; only a completed flow or this clear drops it).
    clearProjectActionSource();
    useUIStore.getState().openDialog('error', {
      title: errorTitle,
      message: getErrorMessage(err, 'Failed to open the file picker'),
    });
    return;
  }

  let path: string | null = null;
  if (typeof selected === 'string') {
    path = selected;
  } else if (Array.isArray(selected) && (selected as string[]).length > 0) {
    path = (selected as string[])[0] ?? null;
  }

  if (path) {
    await routeOpenedFile(path, errorTitle);
  } else {
    clearProjectActionSource();
  }
}

/**
 * Open the unified picker (native projects + all importable formats), then
 * route the chosen file to the matching flow.
 */
export async function openOrImport(): Promise<void> {
  await pickAndRoute(openOrImportFilters());
}
