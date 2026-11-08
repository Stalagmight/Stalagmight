import type { SpawnSyncOptions } from 'child_process';

export function foldLocalPaths(text: string): string;
export function execFileSyncFolded(file: string, args?: readonly string[], options?: SpawnSyncOptions): string | Buffer;
