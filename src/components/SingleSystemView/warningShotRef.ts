import { formatShotId } from '../../utils/formatters';
import type { ProjectWarning } from '../../types/ipc/ProjectWarning';

/**
 * [#576 NF-W7E-3] The rest of a warning's message once its OWN shot reference -
 * "Shot <section>:<position>", built from the warning's `sectionId` and
 * `shotIndex` - is taken off the front, for the warnings list to show the shot
 * pill in its place. Null when the message does not lead with that reference
 * (another shot's, a longer number, a message about no shot): it is then shown as
 * it is.
 *
 * The list used to find the reference with a regex over the English text, so it
 * cut ANY leading shot reference - including one naming a different shot, whose
 * text then disappeared behind a pill for the warning's own shot.
 */
export function warningTextAfterOwnShot(
  warning: Pick<ProjectWarning, 'message' | 'sectionId' | 'shotIndex'>,
): string | null {
  const own = `Shot ${formatShotId(warning.sectionId, warning.shotIndex + 1)}`;
  if (!warning.message.startsWith(own)) return null;
  const rest = warning.message.slice(own.length);
  // A whole reference only: "Shot 1:1" is not the start of "Shot 1:12".
  if (rest !== '' && !/^\s/.test(rest)) return null;
  return rest.trimStart();
}
