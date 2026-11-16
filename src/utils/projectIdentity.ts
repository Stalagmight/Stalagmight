/** Stable identity for a reply guard: a distinct project file - even a duplicate
 *  sharing created_at/modified_at - and any edit (modified_at) yield a different
 *  key, so an in-flight reply issued against one state can't paint over another
 *  (#26). Shared by the ruler's along-line query and the zero-datum preview. */
export function projectIdentityKey(
  project: { created_at?: string; modified_at?: string; file_path?: string | null } | null | undefined
): string {
  return `${project?.created_at ?? ''}|${project?.modified_at ?? ''}|${project?.file_path ?? ''}`;
}
