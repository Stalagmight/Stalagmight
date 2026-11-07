/** #576 W51: the four groups of print and write sites, and the directories scanned. */
import { GROUP as LIB } from './allowLib';
import { GROUP as RUNNERS } from './allowRunners';
import { GROUP as SCENARIOS } from './allowScenarios';
import { GROUP as SCRIPTS } from './allowScripts';
import type { SiteGroup } from './rules';

export const GROUPS: readonly SiteGroup[] = [RUNNERS, LIB, SCENARIOS, SCRIPTS];

/** Everything that prints or writes on the harness's or the tooling's behalf. */
export const SCANNED_DIRS = ['e2e/agent', 'e2e/lib', 'scripts'] as const;
