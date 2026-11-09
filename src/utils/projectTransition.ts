import { confirmSpeleoFileOpen } from './confirmSpeleoOpen';
import { getErrorMessage } from './ipc';
import { clearProjectActionSource } from './telemetry';
import { useUIStore } from '../store/uiStore';
import type { LoadProjectOutcome } from '../store/projectStore';

/**
 * Coordinate whole-project switches with in-place SpeleoDB tree replacements.
 * Existing switches finish before a replacement starts; switches requested during
 * the replacement wait until its frontend reconciliation is complete.
 */
let requestGeneration = 0;
let mutationRequest = 0;

/** Reserve ownership before any asynchronous preflight or replacement wait. */
export function reserveProjectRequest(): number {
  return ++requestGeneration;
}

export function isProjectRequestCurrent(request: number): boolean {
  return request === requestGeneration;
}

/** A confirmed operation owns backend completion until another mutation begins. */
export function beginProjectMutation(request: number): boolean {
  if (!isProjectRequestCurrent(request) && mutationRequest !== request) return false;
  mutationRequest = request;
  return true;
}
export function isProjectMutationCurrent(request: number): boolean {
  return mutationRequest === request;
}

let replacementTail: Promise<void> = Promise.resolve();
let switchTail: Promise<void> = Promise.resolve();

export function runProjectSwitch<T>(work: (request: number) => Promise<T>, request = reserveProjectRequest(), preflightOnly = false): Promise<T> {
  const result = replacementTail.then(() => {
    if (!preflightOnly) beginProjectMutation(request);
    return work(request);
  });
  switchTail = Promise.allSettled([switchTail, result]).then(() => undefined);
  return result;
}

export function runTreeReplacement<T>(work: () => Promise<T>): Promise<T> {
  const previousReplacement = replacementTail;
  const switchesAlreadyIssued = switchTail;
  let releaseReplacement!: () => void;
  const replacementFinished = new Promise<void>((resolve) => {
    releaseReplacement = resolve;
  });

  replacementTail = previousReplacement.then(() => replacementFinished);
  return (async () => {
    await previousReplacement;
    await switchesAlreadyIssued;
    try {
      return await work();
    } finally {
      releaseReplacement();
    }
  })();
}


/** Confirm access before any destructive part of a file-open transition. */
export function runConfirmedProjectSwitch(
  path: string,
  currentPath: () => string | null | undefined,
  accessConfirmed: boolean | undefined,
  work: () => Promise<LoadProjectOutcome>,
  request = reserveProjectRequest(),
): Promise<LoadProjectOutcome> {
  const current = () => isProjectRequestCurrent(request) || (accessConfirmed === true && isProjectMutationCurrent(request));
  const superseded: LoadProjectOutcome = { success: false, superseded: true };
  return runProjectSwitch(async () => {
    if (!current()) return superseded;
    if (!accessConfirmed && currentPath() !== path) {
      try {
        if (!await confirmSpeleoFileOpen(path, current)) {
          if (!current()) return superseded;
          clearProjectActionSource();
          return { success: false };
        }
      } catch (error) {
        if (!current()) return superseded;
        clearProjectActionSource();
        useUIStore.getState().openDialog('error', { title: 'Open Failed', message: getErrorMessage(error) });
        return { success: false };
      }
    }
    if (!current() || !beginProjectMutation(request)) return superseded;
    return work();
  }, request, true);
}
