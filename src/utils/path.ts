/**
 * Extract filename from a path, optionally stripping an extension.
 * Works with both forward slashes and backslashes (cross-platform).
 */
export function extractFilename(path: string, stripExtension?: RegExp): string {
  const name = path.split(/[/\\]/).pop() ?? path;
  return stripExtension ? name.replace(stripExtension, '') : name;
}

/**
 * True for a path in Windows shape: a drive letter followed by a separator
 * (`C:\caves`, `C:/caves`), or a leading `\\` (a network path `\\srv\share`, or a
 * verbatim path `\\?\C:\caves`). Only such a path treats `\` as a separator: on
 * Linux and macOS `\` is an ordinary file-name character.
 */
export function isWindowsPath(path: string): boolean {
  return /^[a-zA-Z]:[/\\]/.test(path) || path.startsWith('\\\\');
}

/** Index of the separator before the file name, or -1 (see {@link isWindowsPath}). */
export function lastSeparatorIndex(path: string): number {
  const slash = path.lastIndexOf('/');
  return isWindowsPath(path) ? Math.max(slash, path.lastIndexOf('\\')) : slash;
}

/**
 * The file `name` in the folder holding `path`, joined with the separator that
 * precedes `path`'s own file name: `C:\caves\CaveB.stalx` next to
 * `C:\caves\CaveA.stalx`. A POSIX path is split on `/` alone and always joined
 * with `/`, so a bare file name gives `/name`.
 */
export function siblingPath(path: string, name: string): string {
  const at = lastSeparatorIndex(path);
  return `${path.substring(0, at)}${path[at] ?? '/'}${name}`;
}
