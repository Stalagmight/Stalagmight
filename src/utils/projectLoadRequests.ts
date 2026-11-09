import type { LoadProjectOutcome } from '../store/projectStore';
import { isProjectRequestCurrent, reserveProjectRequest } from './projectTransition';

const loads = new Map<string, { request: number; promise: Promise<LoadProjectOutcome> }>();

/** Share current same-path opens while letting A-B-A start a fresh final A. */
export function runDeduplicatedProjectLoad(
  path: string,
  suppliedRequest: number | undefined,
  work: (request: number) => Promise<LoadProjectOutcome>,
): Promise<LoadProjectOutcome> {
  const existing = loads.get(path);
  if (existing && isProjectRequestCurrent(existing.request)) return existing.promise;
  const request = suppliedRequest ?? reserveProjectRequest();
  loads.clear();
  const promise = work(request);
  loads.set(path, { request, promise });
  const cleanup = () => { if (loads.get(path)?.promise === promise) loads.delete(path); };
  void promise.then(cleanup, cleanup);
  return promise;
}
