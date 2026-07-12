import { useState, useEffect, useCallback, useMemo } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { invokeIpc, getErrorMessage } from '../../utils/ipc';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { getThemeColor } from '../../utils/themeColors';
import { ShotTableEditor, type EditableShot } from '../common/ShotTableEditor';
import { SectionMetadataForm } from '../common/SectionMetadataForm';
import type { Shot, ShotData } from '../../types/project';
import type { DeleteShotImpact } from '../../types/commands';
import { defaultConfig } from '../../types/config';
import DeleteShotImpactDetails from './DeleteShotImpactDetails';
import { setSource, trackDialogComplete } from '../../utils/telemetry';

export default function EditSectionDialog() {
  const { closeDialog, dialogData, openDialog } = useUIStore();
  const { updateSection, createShot, updateShot, deleteShot, project, isShotFrozen } = useProjectStore();

  const sectionId = dialogData.sectionId as number | undefined;
  const section = project?.sections.find((s) => s.id === sectionId);

  // Compute frozen shot indices for the ShotTableEditor
  const frozenShotIndices = useMemo(() => {
    if (sectionId === undefined || !section) return undefined;
    const frozen = new Set<number>();
    for (let i = 0; i < section.shots.length; i++) {
      if (isShotFrozen(sectionId, i)) {
        frozen.add(i);
      }
    }
    return frozen.size > 0 ? frozen : undefined;
  }, [sectionId, section, isShotFrozen]);

  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Form state
  const [name, setName] = useState('');
  const [surveyor, setSurveyor] = useState('');
  const [date, setDate] = useState('');
  const [color, setColor] = useState(getThemeColor('--cc-station-default'));
  const [direction, setDirection] = useState<'in' | 'out'>('in');
  const [quality, setQuality] = useState(1);

  // Shots state
  const [shots, setShots] = useState<EditableShot[]>([]);
  const [nextTempId, setNextTempId] = useState(-1);
  const [selectedShotKey, setSelectedShotKey] = useState<string | null>(null);
  const [pendingMidDelete, setPendingMidDelete] = useState<{ index: number; shotId: number } | null>(null);
  const [pendingImpact, setPendingImpact] = useState<DeleteShotImpact | null>(null);
  const [pendingImpactError, setPendingImpactError] = useState<string | null>(null);
  const shiftBlocked = pendingImpact?.shift_blocked ?? false;
  const shiftBlockedReasons = pendingImpact?.shift_blocked_reasons ?? [];

  // Initialize form with section data
  useEffect(() => {
    if (section) {
      setName(section.name ?? '');
      setSurveyor(section.surveyor ?? '');
      setDate(section.date ?? '');
      setColor(section.color);
      setDirection(section.direction ?? 'in');
      setQuality(section.quality ?? 1);
      setShots(section.shots.map(s => ({ ...s })));
    }
  }, [section]);

  const handleShotChange = useCallback((index: number, field: keyof EditableShot, value: number | string | boolean | null) => {
    setShots(prev => {
      const updated = [...prev];
      const shot = updated[index];
      if (!shot) return prev;
      updated[index] = {
        ...shot,
        [field]: value,
      } as EditableShot;
      return updated;
    });
  }, []);

  const handleAddShot = useCallback(() => {
    const newShot: EditableShot = {
      id: nextTempId,
      tempId: nextTempId,
      azimuth_in: 0,
      azimuth_out: 180,
      length: 1,
      depth_in: 0,
      depth_out: 0,
      pitch_in: null,
      pitch_out: null,
      left: null,
      right: null,
      up: null,
      down: null,
      comment: null,
      is_reference: false,
      isNew: true,
    };
    setShots(prev => [...prev, newShot]);
    setNextTempId(prev => prev - 1);
  }, [nextTempId]);

  const getShotKey = useCallback((shot: EditableShot): string => {
    if (shot.tempId !== undefined) return `t:${shot.tempId}`;
    return `i:${shot.id}`;
  }, []);

  const selectedRowIndex = useMemo(() => {
    if (!selectedShotKey) return null;
    const index = shots.findIndex((shot) => getShotKey(shot) === selectedShotKey);
    return index >= 0 ? index : null;
  }, [getShotKey, selectedShotKey, shots]);

  useEffect(() => {
    if (!selectedShotKey) return;
    const selectedShot = shots.find((shot) => getShotKey(shot) === selectedShotKey);
    if (!selectedShot || selectedShot.isDeleted) {
      setSelectedShotKey(null);
    }
  }, [getShotKey, selectedShotKey, shots]);

  const handleDeleteShot = useCallback((index: number) => {
    const shot = shots[index];
    if (!shot) return;
    const isSelected = selectedShotKey !== null && getShotKey(shot) === selectedShotKey;

    if (!shot.isNew && section) {
      const originalIndex = section.shots.findIndex((s) => s.id === shot.id);
      const totalShots = section.shots.length;
      const isMidShot = totalShots >= 3 && originalIndex > 0 && originalIndex < totalShots - 1;
      if (isMidShot) {
        setPendingMidDelete({ index, shotId: shot.id });
        return;
      }
    }

    setShots((prev) => {
      const current = prev[index];
      if (!current) return prev;
      if (current.isNew) {
        // New shot - just remove from array
        return prev.filter((_, i) => i !== index);
      }
      // Existing shot - mark as deleted
      const updated = [...prev];
      updated[index] = { ...current, isDeleted: true } as EditableShot;
      return updated;
    });

    // Clear selection if deleted row was selected
    if (isSelected) {
      setSelectedShotKey(null);
    }
  }, [getShotKey, section, selectedShotKey, shots]);

  useEffect(() => {
    if (pendingMidDelete && sectionId !== undefined) {
      invokeIpc<DeleteShotImpact>('preview_delete_shot', { sectionId, shotId: pendingMidDelete.shotId }, { purpose: 'preview mid-shot deletion impact' })
        .then((result) => {
          setPendingImpact(result);
          setPendingImpactError(null);
        })
        .catch((e) => {
          setPendingImpact(null);
          setPendingImpactError(getErrorMessage(e, 'Failed to load shot impacts'));
        });
    } else {
      setPendingImpact(null);
      setPendingImpactError(null);
    }
  }, [pendingMidDelete, sectionId]);

  const confirmShiftMidDelete = useCallback(() => {
    if (!pendingMidDelete) return;
    setShots((prev) => {
      const current = prev[pendingMidDelete.index];
      if (!current) return prev;
      if (current.isNew) {
        return prev.filter((_, i) => i !== pendingMidDelete.index);
      }
      const updated = [...prev];
      updated[pendingMidDelete.index] = { ...current, isDeleted: true } as EditableShot;
      return updated;
    });
    if (selectedShotKey !== null) {
      const pendingShot = shots[pendingMidDelete.index];
      if (pendingShot && getShotKey(pendingShot) === selectedShotKey) {
        setSelectedShotKey(null);
      }
    }
    setPendingMidDelete(null);
  }, [getShotKey, pendingMidDelete, selectedShotKey, shots]);

  const confirmSplitMidDelete = useCallback(() => {
    if (!pendingMidDelete || sectionId === undefined) return;
    const targetName = section?.name ?? `Section ${sectionId}`;
    setSource('dialog');
    openDialog('split-section', {
      sectionId,
      shotId: pendingMidDelete.shotId,
      sectionName: targetName,
    });
  }, [pendingMidDelete, sectionId, section, openDialog]);

  const handleInsertBefore = useCallback((index: number) => {
    const newShot: EditableShot = {
      id: nextTempId,
      tempId: nextTempId,
      azimuth_in: 0,
      azimuth_out: 180,
      length: 1,
      depth_in: 0,
      depth_out: 0,
      pitch_in: null,
      pitch_out: null,
      left: null,
      right: null,
      up: null,
      down: null,
      comment: null,
      is_reference: false,
      isNew: true,
    };
    setShots(prev => {
      const updated = [...prev];
      updated.splice(index, 0, newShot);
      return updated;
    });
    setNextTempId(prev => prev - 1);
  }, [nextTempId]);

  const handleInsertAfter = useCallback((index: number) => {
    const newShot: EditableShot = {
      id: nextTempId,
      tempId: nextTempId,
      azimuth_in: 0,
      azimuth_out: 180,
      length: 1,
      depth_in: 0,
      depth_out: 0,
      pitch_in: null,
      pitch_out: null,
      left: null,
      right: null,
      up: null,
      down: null,
      comment: null,
      is_reference: false,
      isNew: true,
    };
    setShots(prev => {
      const updated = [...prev];
      updated.splice(index + 1, 0, newShot);
      return updated;
    });
    setNextTempId(prev => prev - 1);
  }, [nextTempId]);

  const handleSave = async () => {
    if (sectionId === undefined) return;

    setIsLoading(true);
    setError(null);

    try {
      // Update section metadata
      const sectionResult = await updateSection(sectionId, {
        name: name.trim() || null,
        surveyor: surveyor.trim() || null,
        date: date.trim() || null,
        color,
        direction,
        quality,
      });

      if (!sectionResult.success) {
        setError(sectionResult.error ?? 'Failed to update section');
        return;
      }

      // Process shot changes
      const originalShots = section?.shots ?? [];

      // Delete removed shots (in reverse order to avoid index issues)
      const deletedShots = shots.filter(s => s.isDeleted && !s.isNew);
      for (const shot of deletedShots.reverse()) {
        const result = await deleteShot(sectionId, shot.id);
        if (!result.success) {
          setError(result.error ?? 'Failed to delete shot');
          return;
        }
      }

      // Update existing shots
      for (const shot of shots.filter(s => !s.isNew && !s.isDeleted)) {
        const original = originalShots.find(o => o.id === shot.id);
        if (original && hasChanges(original, shot)) {
          const result = await updateShot(sectionId, shot.id, {
            azimuth_in: shot.azimuth_in,
            azimuth_out: shot.azimuth_out,
            length: shot.length,
            depth_in: shot.depth_in,
            depth_out: shot.depth_out,
            pitch_in: shot.pitch_in,
            pitch_out: shot.pitch_out,
            left: shot.left,
            right: shot.right,
            up: shot.up,
            down: shot.down,
            comment: shot.comment,
            is_reference: shot.is_reference ?? false,
            marker_index: original.marker_index ?? null,
            temperature: original.temperature ?? null,
          });
          if (!result.success) {
            setError(result.error ?? 'Failed to update shot');
            return;
          }
        }
      }

      // Create new shots
      const newShots = shots.filter(s => s.isNew && !s.isDeleted);
      for (const shot of newShots) {
        const shotData: ShotData = {
          azimuth_in: shot.azimuth_in,
          azimuth_out: shot.azimuth_out,
          length: shot.length,
          depth_in: shot.depth_in,
          depth_out: shot.depth_out,
          pitch_in: shot.pitch_in,
          pitch_out: shot.pitch_out,
          left: shot.left,
          right: shot.right,
          up: shot.up,
          down: shot.down,
          comment: shot.comment,
          is_reference: shot.is_reference ?? false,
          marker_index: null,
          temperature: null,
        };
        const result = await createShot(sectionId, shotData);
        if (!result.success) {
          setError(result.error ?? 'Failed to create shot');
          return;
        }
      }

      trackDialogComplete('edit-section');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to update section'));
    } finally {
      setIsLoading(false);
    }
  };

  // Track if form has been modified
  const isDirty = section ? (
    name !== (section.name ?? '') ||
    surveyor !== (section.surveyor ?? '') ||
    date !== (section.date ?? '') ||
    color !== section.color ||
    direction !== (section.direction ?? 'in') ||
    quality !== (section.quality ?? 1) ||
    shots.some(s => s.isNew || s.isDeleted) ||
    shots.some(s => {
      const original = section.shots.find(o => o.id === s.id);
      return original && hasChanges(original, s);
    })
  ) : false;

  // Per-project validation thresholds (#57) so inline warnings match the
  // Warnings panel. project.validation is a non-null ValidationConfig (seeded
  // from the global config on load), so read it directly; fall back to the
  // shared TS defaults only when no project is open.
  const validation = project?.validation ?? defaultConfig.validation;
  const validationConfig = {
    azimuth_diff_threshold: validation.azimuth_diff_threshold,
    pitch_diff_threshold: validation.pitch_diff_threshold,
  };

  if (!section) {
    return (
      <DialogWrapper title="Edit Section" onClose={closeDialog} size="5xl" data-testid="edit-section-dialog">
        <div className="text-error">Section not found</div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper title="Edit Section" onClose={closeDialog} size="5xl" isDirty={isDirty} data-testid="edit-section-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        {/* Section metadata */}
        <SectionMetadataForm
          name={name}
          surveyor={surveyor}
          date={date}
          color={color}
          direction={direction}
          quality={quality}
          onNameChange={setName}
          onSurveyorChange={setSurveyor}
          onDateChange={setDate}
          onColorChange={setColor}
          onQualityChange={setQuality}
          onDirectionChange={setDirection}
          showDirection={true}
          showQuality={true}
          disabled={isLoading}
        />

        {/* Shots table using ShotTableEditor */}
        <div className="border-t border-border pt-4">
          <ShotTableEditor
            shots={shots}
            onShotChange={handleShotChange}
            onAddShot={handleAddShot}
            onDeleteShot={handleDeleteShot}
            onInsertBefore={handleInsertBefore}
            onInsertAfter={handleInsertAfter}
            validationConfig={validationConfig}
            selectedRowIndex={selectedRowIndex}
            onRowSelect={(rowIndex) => {
              const shot = shots[rowIndex];
              if (!shot) {
                setSelectedShotKey(null);
                return;
              }
              setSelectedShotKey(getShotKey(shot));
            }}
            {...(frozenShotIndices ? { frozenShotIndices } : {})}
            maxHeight="16rem"
            {...(sectionId !== undefined ? {
              sectionId,
              markers: (project?.markers ?? []).filter((m) => m.section_id === sectionId),
            } : {})}
          />
        </div>

        {pendingMidDelete && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-sm space-y-3">
            <p className="font-medium text-warning">This is a mid-section shot.</p>
            <p className="text-text-secondary">
              Split Section keeps geometry fixed but discards other unsaved edits. Shift Geometry deletes the shot and allows downstream stations to move.
            </p>

            {pendingImpactError && (
              <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
                {pendingImpactError}
              </div>
            )}

            {shiftBlocked && (
              <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
                <p className="font-medium">Shift Geometry is not available for this shot.</p>
                {shiftBlockedReasons.length > 0 && (
                  <ul className="list-disc list-inside">
                    {shiftBlockedReasons.map((reason, index) => (
                      <li key={`shift-blocked-${index}`}>{reason}</li>
                    ))}
                  </ul>
                )}
              </div>
            )}

            <DeleteShotImpactDetails
              impact={pendingImpact}
              title="If you choose Shift Geometry, these relationships will update:"
            />

            <div className="flex justify-end gap-2">
              <Button onClick={() => setPendingMidDelete(null)}>Cancel</Button>
              <Button onClick={confirmSplitMidDelete}>Split Section</Button>
              <Button variant="danger" onClick={confirmShiftMidDelete} disabled={shiftBlocked}>
                Shift Geometry
              </Button>
            </div>
          </div>
        )}

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading}>
            Cancel
          </Button>
          <Button variant="primary" onClick={handleSave} disabled={isLoading || pendingMidDelete !== null}>
            {isLoading ? 'Saving...' : 'Save'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}

function hasChanges(original: Shot, current: EditableShot): boolean {
  return (
    original.azimuth_in !== current.azimuth_in ||
    original.azimuth_out !== current.azimuth_out ||
    original.length !== current.length ||
    original.depth_in !== current.depth_in ||
    original.depth_out !== current.depth_out ||
    original.pitch_in !== current.pitch_in ||
    original.pitch_out !== current.pitch_out ||
    original.left !== current.left ||
    original.right !== current.right ||
    original.up !== current.up ||
    original.down !== current.down ||
    original.comment !== current.comment ||
    original.is_reference !== current.is_reference
  );
}
