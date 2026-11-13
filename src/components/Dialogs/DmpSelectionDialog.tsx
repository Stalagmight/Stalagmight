import { useState, useMemo, useCallback, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { DialogWrapper } from './DialogWrapper';
import { Button } from '../common/Button';
import { Panel } from '../common/Panel';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { extractFilename } from '../../utils/path';
import { defaultConfig } from '../../types/config';
import { formatLength, formatDepthRange } from '../../utils/formatStats';
import { isImportableDmpShot } from '../../hooks/useImportState';
import type { DmpFile } from '../../types/commands';
import type { Person } from '../../types/ipc.generated';
import { CreditPeopleFields } from '../common/CreditFields';
import { OptionalDateInput } from '../common/OptionalDateInput';
import { listPeopleProps, type CreditLists } from '../common/SectionMetadataForm';
import { personKey } from '../../utils/people';

/**
 * Configuration produced by DmpSelectionDialog (Step 1)
 * and passed to the DMP import wizard.
 */
export interface DmpSelectionConfig {
  /** Indices of selected sections from the DMP file */
  selectedIndices: Set<number>;
  /** Default surveyors for every section (a section may change its own) */
  defaultSurveyors: Person[];
  /** Default explorers for every section */
  defaultExplorers: Person[];
  /** Optional date override to apply to all sections */
  defaultDate: string | null;
  /** SHA256 hash of the source DMP file, null if computation failed */
  fileHash: string | null;
  /** Content hashes of selected sections (for section-level duplicate detection) */
  sectionHashes: string[];
}

interface DmpSelectionDialogProps {
  /** Parsed DMP file data */
  dmpFile: DmpFile;
  /** Path to the DMP file */
  filePath: string;
  /** Called when user cancels the dialog */
  onCancel: () => void;
  /** Called when user clicks Continue to proceed to Step 2 */
  onContinue: (config: DmpSelectionConfig) => void;
}

/**
 * Calculate DMP file metadata for header display based on selected sections.
 */
function calculateMetadata(dmpFile: DmpFile, selectedIndices: Set<number>) {
  const sections = dmpFile.sections;
  const sectionCount = sections.length;

  // Count only importable shots from selected sections
  let totalShots = 0;
  let totalLength = 0;
  let minDepth: number | null = null;
  let maxDepth: number | null = null;

  for (let i = 0; i < sections.length; i++) {
    if (!selectedIndices.has(i)) continue;

    const section = sections[i];
    if (!section) continue;

    const standardShots = section.shots.filter(isImportableDmpShot);
    totalShots += standardShots.length;

    for (const shot of standardShots) {
      // Exclude negative lengths from total (data corruption guard)
      if (Number.isFinite(shot.length) && shot.length >= 0) {
        totalLength += shot.length;
      }

      // Calculate depth range from depth_in and depth_out
      for (const depth of [shot.depth_in, shot.depth_out]) {
        if (Number.isFinite(depth)) {
          if (minDepth === null || depth < minDepth) minDepth = depth;
          if (maxDepth === null || depth > maxDepth) maxDepth = depth;
        }
      }
    }
  }

  return {
    sectionCount,
    totalShots,
    totalLength,
    minDepth,
    maxDepth,
  };
}

/** Same people in the same order, per role. */
function samePeopleLists(a: CreditLists, b: CreditLists): boolean {
  const same = (x: readonly Person[], y: readonly Person[]) =>
    x.length === y.length && x.every((p, i) => personKey(p) === personKey(y[i]!));
  return same(a.surveyors, b.surveyors) && same(a.explorers, b.explorers);
}

/**
 * Step 1 of DMP Import: Select sections and set default values.
 *
 * This dialog displays:
 * - DMP file metadata header (version, sections, shots, length, date range)
 * - Default values section (surveyors, explorers, date override)
 * - Bulk selection controls (All, None, Invert)
 * - Section table with checkboxes
 *
 * Empty sections (0 importable shots) are displayed but grayed out and disabled.
 */
export default function DmpSelectionDialog({
  dmpFile,
  filePath,
  onCancel,
  onContinue,
}: DmpSelectionDialogProps) {
  const { t } = useTranslation();
  const importSecondaryButtonClass = 'ring-1 ring-inset ring-border hover:bg-background-50';

  // Extract filename from path
  const fileName = useMemo(() => {
    return extractFilename(filePath);
  }, [filePath]);

  // Get config for unit conversion (fallback to defaults)
  const config = useProjectStore((state) => state.config);
  const lengthUnit = config?.units?.length ?? defaultConfig.units.length;
  const depthUnit = config?.units?.depth ?? defaultConfig.units.depth;

  // Get hash functions from project store
  const calculateFileHash = useProjectStore((state) => state.calculateFileHash);
  const checkDuplicateImport = useProjectStore((state) => state.checkDuplicateImport);
  const checkSectionHashes = useProjectStore((state) => state.checkSectionHashes);

  // File hash and duplicate detection state
  const [fileHash, setFileHash] = useState<string | null>(null);
  const [isDuplicate, setIsDuplicate] = useState(false);

  // Track whether hash computation has settled (resolved or failed)
  const [hashSettled, setHashSettled] = useState(false);

  // Section-level duplicate detection state
  const [duplicateSectionIndices, setDuplicateSectionIndices] = useState<Set<number>>(new Set());
  const [sectionHashesSettled, setSectionHashesSettled] = useState(false);

  // Check section content hashes for duplicates on mount
  useEffect(() => {
    let cancelled = false;
    setSectionHashesSettled(false);
    setDuplicateSectionIndices(new Set());
    (async () => {
      try {
        const hashes = dmpFile.sections.map(s => s.content_hash);
        const results = await checkSectionHashes(hashes);
        if (cancelled) return;
        const dupes = new Set<number>();
        results.forEach((isDup, idx) => {
          if (isDup) dupes.add(idx);
        });
        setDuplicateSectionIndices(dupes);
        // Auto-deselect duplicate sections
        if (dupes.size > 0) {
          setSelectedIndices(prev => {
            const next = new Set(prev);
            for (const idx of dupes) {
              next.delete(idx);
            }
            return next;
          });
        }
      } catch {
        // Graceful degradation
      } finally {
        if (!cancelled) {
          setSectionHashesSettled(true);
        }
      }
    })();
    return () => { cancelled = true; };
  }, [dmpFile.sections, checkSectionHashes]);

  // Compute file hash and check for duplicates on mount / filePath change
  useEffect(() => {
    let cancelled = false;
    // Reset state when filePath changes to avoid stale values
    setFileHash(null);
    setIsDuplicate(false);
    setHashSettled(false);
    (async () => {
      try {
        const hash = await calculateFileHash(filePath);
        if (cancelled) return;
        setFileHash(hash);
        if (cancelled) return;
        const isDup = await checkDuplicateImport(hash);
        if (cancelled) return;
        setIsDuplicate(isDup);
      } catch {
        // Graceful degradation: proceed without warning
      } finally {
        if (!cancelled) {
          setHashSettled(true);
        }
      }
    })();
    return () => { cancelled = true; };
  }, [filePath, calculateFileHash, checkDuplicateImport]);

  // The people the last section was created or imported with this session
  const setLastUsedCredit = useUIStore((state) => state.setLastUsedCredit);

  // Default values state - the people start as the last used ones
  const [initialPeople] = useState<CreditLists>(() => {
    const last = useUIStore.getState().lastUsedCredit;
    return { surveyors: [...last.surveyors], explorers: [...last.explorers] };
  });
  const [defaultPeople, setDefaultPeople] = useState<CreditLists>(initialPeople);
  // A name typed into a people field but not added yet is a change too.
  const [peopleTextPending, setPeopleTextPending] = useState(false);
  const [defaultDate, setDefaultDate] = useState('');

  // Track initial state for isDirty calculation
  const [initialDate] = useState('');

  const peopleProps = listPeopleProps(
    defaultPeople,
    (role, next) => setDefaultPeople((prev) => ({ ...prev, [role]: next(prev[role]) })),
    setPeopleTextPending,
  );

  // Section selection state - default to sections with at least 2 importable shots
  const [selectedIndices, setSelectedIndices] = useState<Set<number>>(() => {
    const indices = new Set<number>();
    dmpFile.sections.forEach((section, index) => {
      const standardShots = section.shots.filter(isImportableDmpShot);
      if (standardShots.length > 1) {
        indices.add(index);
      }
    });
    return indices;
  });

  // Track initial selection for isDirty calculation
  const [initialSelection] = useState(() => {
    const indices = new Set<number>();
    dmpFile.sections.forEach((section, index) => {
      const standardShots = section.shots.filter(isImportableDmpShot);
      if (standardShots.length > 1) {
        indices.add(index);
      }
    });
    return indices;
  });

  // Check if form has changes
  const isDirty = useMemo(() => {
    // Check people/date changes
    if (peopleTextPending) return true;
    if (!samePeopleLists(defaultPeople, initialPeople)) return true;
    if (defaultDate !== initialDate) return true;

    // Check selection changes
    if (selectedIndices.size !== initialSelection.size) return true;
    for (const idx of selectedIndices) {
      if (!initialSelection.has(idx)) return true;
    }

    return false;
  }, [peopleTextPending, defaultPeople, initialPeople, defaultDate, initialDate, selectedIndices, initialSelection]);

  // Calculate metadata for header based on selected sections (updates in real-time)
  const metadata = useMemo(
    () => calculateMetadata(dmpFile, selectedIndices),
    [dmpFile, selectedIndices]
  );

  // Bulk selection handlers
  const handleSelectAll = useCallback(() => {
    const indices = new Set<number>();
    dmpFile.sections.forEach((section, index) => {
      const standardShots = section.shots.filter(isImportableDmpShot);
      if (standardShots.length > 0) {
        indices.add(index);
      }
    });
    setSelectedIndices(indices);
  }, [dmpFile.sections]);

  const handleSelectNone = useCallback(() => {
    setSelectedIndices(new Set());
  }, []);

  const handleInvertSelection = useCallback(() => {
    const inverted = new Set<number>();
    dmpFile.sections.forEach((section, index) => {
      const standardShots = section.shots.filter(isImportableDmpShot);
      if (standardShots.length > 0 && !selectedIndices.has(index)) {
        inverted.add(index);
      }
    });
    setSelectedIndices(inverted);
  }, [dmpFile.sections, selectedIndices]);

  // Toggle individual section
  const handleToggleSection = useCallback((index: number) => {
    setSelectedIndices(prev => {
      const next = new Set(prev);
      if (next.has(index)) {
        next.delete(index);
      } else {
        next.add(index);
      }
      return next;
    });
  }, []);

  // Handle Continue button click
  const handleContinue = useCallback(() => {
    // Remember the people for the next import (and New Section) this session.
    setLastUsedCredit({ surveyors: defaultPeople.surveyors, explorers: defaultPeople.explorers });

    const sectionHashes: string[] = [];
    for (const idx of selectedIndices) {
      const section = dmpFile.sections[idx];
      if (section) {
        sectionHashes.push(section.content_hash);
      }
    }

    const config: DmpSelectionConfig = {
      selectedIndices,
      defaultSurveyors: defaultPeople.surveyors,
      defaultExplorers: defaultPeople.explorers,
      defaultDate: defaultDate.trim() || null,
      fileHash,
      sectionHashes,
    };
    onContinue(config);
  }, [selectedIndices, defaultPeople, defaultDate, fileHash, dmpFile.sections, onContinue, setLastUsedCredit]);

  // Calculate section counts for display
  const nonEmptySectionCount = useMemo(() => {
    return dmpFile.sections.filter(
      section => section.shots.filter(isImportableDmpShot).length > 0
    ).length;
  }, [dmpFile.sections]);

  return (
    <DialogWrapper
      title={t('dialogs.dmpSelection.title')}
      onClose={onCancel}
      size="2xl"
      isDirty={isDirty}
      data-testid="dmp-selection-dialog"
    >
      <div className="space-y-4">
        {/* File metadata header */}
        <div className="bg-background-50 rounded p-3 text-sm" data-testid="dmp-file-overview">
          <div className="font-medium text-text-primary mb-1" data-testid="dmp-filename">
            {t('dialogs.dmpSelection.fileLine', { fileName, version: dmpFile.version })}
          </div>
          <div
            className="text-text-secondary"
            data-testid="selection-stats"
            aria-label={t('dialogs.dmpSelection.statsAria', {
              shots: metadata.totalShots,
              length: formatLength(metadata.totalLength, lengthUnit),
              depthRange: formatDepthRange(metadata.minDepth, metadata.maxDepth, depthUnit),
            })}
          >
            <span data-testid="stats-shot-count">{t('dialogs.dmpSelection.shots', { count: metadata.totalShots })}</span>
            <span className="mx-2">|</span>
            <span data-testid="stats-total-length">{formatLength(metadata.totalLength, lengthUnit)}</span>
            <span className="mx-2">|</span>
            <span data-testid="stats-depth-range">{t('dialogs.dmpSelection.depth', { range: formatDepthRange(metadata.minDepth, metadata.maxDepth, depthUnit) })}</span>
          </div>
        </div>

        {/* Duplicate hash warning banner */}
        {isDuplicate && hashSettled && (
          <div
            className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm"
            data-testid="duplicate-hash-warning"
          >
            {'\u26A0'} {t('dialogs.dmpSelection.duplicateHashWarning')}
          </div>
        )}

        {/* Section-level duplicate banner */}
        {sectionHashesSettled && duplicateSectionIndices.size > 0 && (
          <div
            className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm"
            data-testid="duplicate-sections-warning"
          >
            {'\u26A0'} {t('dialogs.dmpSelection.duplicateSectionsWarning', {
              duplicates: duplicateSectionIndices.size,
              total: dmpFile.sections.length,
            })}
          </div>
        )}

        {/* Default values section */}
        <Panel title={t('dialogs.dmpSelection.defaultValues')}>
          <div className="space-y-3" data-testid="dmp-default-values">
            <CreditPeopleFields {...peopleProps} layout="columns" testIdPrefix="default" />
            <div className="w-1/2 pr-1.5">
              <label className="block text-sm text-text-secondary mb-1">
                {t('dialogs.dmpSelection.dateOverrideLabel')}
              </label>
              <OptionalDateInput
                value={defaultDate}
                onChange={setDefaultDate}
                onClear={() => setDefaultDate('')}
                label={t('dialogs.dmpSelection.dateOverrideLabel')}
                inputClassName="w-full"
                testId="default-date-input"
              />
            </div>
          </div>
        </Panel>

        {/* Bulk selection controls */}
        <div className="flex gap-2" data-testid="dmp-bulk-selection-controls">
          <Button
            size="sm"
            onClick={handleSelectAll}
            className={importSecondaryButtonClass}
            data-testid="select-all-button"
          >
            {t('dialogs.dmpSelection.selectAll')}
          </Button>
          <Button
            size="sm"
            onClick={handleSelectNone}
            className={importSecondaryButtonClass}
            data-testid="select-none-button"
          >
            {t('dialogs.dmpSelection.selectNone')}
          </Button>
          <Button
            size="sm"
            onClick={handleInvertSelection}
            className={importSecondaryButtonClass}
            data-testid="invert-selection-button"
          >
            {t('dialogs.dmpSelection.invert')}
          </Button>
          <span className="ml-auto text-sm text-text-secondary self-center">
            {t('dialogs.dmpSelection.selectedCount', {
              selected: selectedIndices.size,
              total: nonEmptySectionCount,
            })}
          </span>
        </div>

        {/* Section table */}
        <div className="border border-border rounded overflow-hidden" data-testid="dmp-section-table">
          <div className="max-h-64 overflow-auto">
            <table className="w-full text-sm">
              <thead className="bg-background-50 sticky top-0">
                <tr>
                  <th className="w-10 px-3 py-2 text-left"></th>
                  <th className="px-3 py-2 text-left">{t('dialogs.dmpSelection.table.name')}</th>
                  <th className="px-3 py-2 text-left">{t('dialogs.dmpSelection.table.date')}</th>
                  <th className="px-3 py-2 text-left">{t('dialogs.dmpSelection.table.direction')}</th>
                  <th className="px-3 py-2 text-right">{t('dialogs.dmpSelection.table.shots')}</th>
                </tr>
              </thead>
              <tbody>
                {dmpFile.sections.map((section, index) => {
                  const standardShots = section.shots.filter(isImportableDmpShot);
                  const shotCount = standardShots.length;
                  const isEmpty = shotCount === 0;
                  const isSelected = selectedIndices.has(index);
                  const isDupeSection = duplicateSectionIndices.has(index);
                  const dateStr = section.date?.split(/[T ]/)[0] ?? '-';

                  return (
                    <tr
                      key={index}
                      className={`
                        border-t border-border
                        ${isEmpty ? 'opacity-50 bg-background-30' : ''}
                        ${isSelected && !isEmpty ? 'bg-accent-10' : ''}
                      `}
                      data-testid={`section-row-${index}`}
                    >
                      <td className="px-3 py-2">
                        <input
                          type="checkbox"
                          checked={isSelected}
                          disabled={isEmpty}
                          onChange={() => handleToggleSection(index)}
                          data-testid={`section-checkbox-${index}`}
                          aria-label={t('dialogs.dmpSelection.selectSectionLabel', { name: section.name })}
                        />
                      </td>
                      <td className="px-3 py-2">
                        {section.name}
                        {isDupeSection && (
                          <span className="ml-2 text-xs text-warning" data-testid={`section-duplicate-${index}`}>
                            {t('dialogs.dmpSelection.alreadyImported')}
                          </span>
                        )}
                      </td>
                      <td className="px-3 py-2">{dateStr}</td>
                      <td className="px-3 py-2 capitalize">{section.direction}</td>
                      <td className="px-3 py-2 text-right tabular-nums">
                        {isEmpty ? (
                          <span className="text-text-secondary">{t('dialogs.dmpSelection.empty')}</span>
                        ) : (
                          shotCount
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>

        {/* Actions */}
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={onCancel} className={importSecondaryButtonClass} data-testid="cancel-button">
            {t('dialogs.dmpSelection.cancel')}
          </Button>
          <Button
            variant="primary"
            onClick={handleContinue}
            disabled={selectedIndices.size === 0 || !hashSettled || !sectionHashesSettled}
            data-testid="continue-button"
          >
            {!hashSettled || !sectionHashesSettled
              ? t('dialogs.dmpSelection.checkingDuplicates')
              : t('dialogs.dmpSelection.continue')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
