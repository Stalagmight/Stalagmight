import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Select } from '../../common/Select';
import { SettingsSection } from './SettingsSection';
import { SettingRow } from './SettingRow';
import type { Config } from '../../../types/config';
import { isFieldChanged, createSelectHandler } from './types';
import type {
  SyntacticHighlightLevel,
  SyntacticHighlightPreset,
} from '../../../types/config';
import {
  SYNTACTIC_ROLES,
  resolveSyntacticColors,
  type SyntacticRole,
  type SyntacticRoleGroup,
} from '../../../theme/syntacticPalette';

interface SyntacticHighlightingSettingsProps {
  config: Config;
  onChange: (updates: Partial<Config>) => void;
  changedFields: Set<string>;
}

/**
 * Syntactic Highlighting settings: level (off / toolbar only / full), color
 * preset (default + colorblind-safe), and per-role color overrides scoped to the
 * active preset (with per-row and whole-preset reset).
 *
 * Rendered inline within the Appearance settings panel (not its own tab).
 */
export function SyntacticHighlightingSettings({
  config,
  onChange,
  changedFields,
}: SyntacticHighlightingSettingsProps) {
  const { t } = useTranslation();
  const sh = config.syntactic_highlighting;
  const preset = sh.preset;

  const levelOptions = useMemo(
    () => [
      { value: 'off', label: t('settings.syntactic.level.options.off') },
      { value: 'toolbar_only', label: t('settings.syntactic.level.options.toolbar') },
      { value: 'icons', label: t('settings.syntactic.level.options.icons') },
      { value: 'full', label: t('settings.syntactic.level.options.full') },
    ],
    [t],
  );

  const presetOptions = useMemo(
    () => [
      { value: 'default', label: t('settings.syntactic.preset.options.default') },
      { value: 'deuteranopia', label: t('settings.syntactic.preset.options.deuteranopia') },
      { value: 'protanopia', label: t('settings.syntactic.preset.options.protanopia') },
      { value: 'tritanopia', label: t('settings.syntactic.preset.options.tritanopia') },
    ],
    [t],
  );

  const handleLevelChange = createSelectHandler(onChange, (v) => ({
    syntactic_highlighting: { ...sh, level: v as SyntacticHighlightLevel },
  }));
  const handlePresetChange = createSelectHandler(onChange, (v) => ({
    syntactic_highlighting: { ...sh, preset: v as SyntacticHighlightPreset },
  }));

  // Effective colors for the active preset (base + overrides).
  const resolved = useMemo(
    () => resolveSyntacticColors(preset, sh.overrides),
    [preset, sh.overrides],
  );
  const presetOverrides = sh.overrides[preset] ?? {};

  const setOverride = (role: SyntacticRole, hex: string) => {
    const nextPresetOverrides = { ...presetOverrides, [role]: hex };
    onChange({
      syntactic_highlighting: {
        ...sh,
        overrides: { ...sh.overrides, [preset]: nextPresetOverrides },
      },
    });
  };

  const clearOverride = (role: SyntacticRole) => {
    const next = { ...presetOverrides };
    delete next[role];
    const overrides = { ...sh.overrides };
    if (Object.keys(next).length === 0) {
      delete overrides[preset];
    } else {
      overrides[preset] = next;
    }
    onChange({ syntactic_highlighting: { ...sh, overrides } });
  };

  const clearAllOverrides = () => {
    const overrides = { ...sh.overrides };
    delete overrides[preset];
    onChange({ syntactic_highlighting: { ...sh, overrides } });
  };

  const hasAnyOverride = Object.keys(presetOverrides).length > 0;

  const groups: { id: SyntacticRoleGroup; titleKey: string }[] = [
    { id: 'entity', titleKey: 'settings.syntactic.sections.entities' },
    { id: 'action', titleKey: 'settings.syntactic.sections.actions' },
    { id: 'tool', titleKey: 'settings.syntactic.sections.tools' },
  ];

  return (
    <div data-testid="settings-syntactic">
      <h3 className="text-base font-semibold mt-8 mb-3 pt-5 border-t border-border">
        {t('settings.syntactic.title')}
      </h3>

      <SettingsSection title={t('settings.syntactic.sections.level')} data-testid="section-syntactic-level">
        <SettingRow
          label={t('settings.syntactic.level.label')}
          helpText={t('settings.syntactic.level.help')}
          isChanged={isFieldChanged(changedFields, 'syntactic_highlighting.level')}
          data-testid="setting-syntactic-level"
        >
          <Select
            value={sh.level}
            onChange={handleLevelChange}
            options={levelOptions}
            className="w-40"
            aria-label={t('settings.syntactic.level.label')}
            data-testid="select-syntactic-level"
          />
        </SettingRow>
      </SettingsSection>

      <SettingsSection title={t('settings.syntactic.sections.preset')} data-testid="section-syntactic-preset">
        <SettingRow
          label={t('settings.syntactic.preset.label')}
          helpText={t('settings.syntactic.preset.help')}
          isChanged={isFieldChanged(changedFields, 'syntactic_highlighting.preset')}
          data-testid="setting-syntactic-preset"
        >
          <Select
            value={sh.preset}
            onChange={handlePresetChange}
            options={presetOptions}
            className="w-44"
            aria-label={t('settings.syntactic.preset.label')}
            data-testid="select-syntactic-preset"
          />
        </SettingRow>
      </SettingsSection>

      {groups.map((group) => {
        const roles = SYNTACTIC_ROLES.filter((r) => r.group === group.id);
        return (
          <SettingsSection
            key={group.id}
            title={t(group.titleKey)}
            data-testid={`section-syntactic-colors-${group.id}`}
          >
            {roles.map((role) => {
              const overridden = presetOverrides[role.id] !== undefined;
              return (
                <SettingRow
                  key={role.id}
                  label={t(role.labelKey)}
                  isChanged={isFieldChanged(
                    changedFields,
                    `syntactic_highlighting.overrides.${preset}.${role.id}`,
                  )}
                  data-testid={`setting-syntactic-color-${role.id}`}
                  onReset={() => clearOverride(role.id)}
                  isDefault={!overridden}
                >
                  <input
                    type="color"
                    value={resolved[role.id]}
                    onChange={(e) => setOverride(role.id, e.target.value)}
                    className="w-12 h-8 rounded border border-border cursor-pointer"
                    aria-label={t('settings.syntactic.colors.aria', { role: t(role.labelKey) })}
                    data-testid={`input-syntactic-color-${role.id}`}
                  />
                </SettingRow>
              );
            })}
          </SettingsSection>
        );
      })}

      <div className="mt-2">
        <button
          type="button"
          onClick={clearAllOverrides}
          disabled={!hasAnyOverride}
          className="text-sm text-accent hover:underline disabled:text-border disabled:cursor-not-allowed disabled:no-underline"
          data-testid="reset-syntactic-colors"
        >
          {t('settings.syntactic.colors.resetAll')}
        </button>
      </div>
    </div>
  );
}
