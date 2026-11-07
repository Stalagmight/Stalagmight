/**
 * MapLibre `error` event boundary-filter tests. Moved here with
 * `shouldSuppressMapError` when it was extracted from the deleted stock
 * `vectorTileRegistry.ts`.
 */
import { describe, expect, it } from 'vitest';
import { shouldSuppressMapError, renderFatalMessage } from '../mapErrors';
import i18n from '../../../../i18n';

describe('shouldSuppressMapError (boundary filter)', () => {
  const PREFIX = 'survey-data';
  const reloadErr = new Error(
    'Should not be trying to reload a tile that was never loaded or has been removed',
  );

  it('suppresses a reload-removed error for a survey stalx source', () => {
    expect(shouldSuppressMapError({ error: reloadErr, sourceId: 'survey-data-shots' }, PREFIX)).toBe(true);
    expect(shouldSuppressMapError({ error: reloadErr, sourceId: 'survey-data-station-labels' }, PREFIX)).toBe(true);
  });

  it('passes through the same message from a non-survey source', () => {
    expect(shouldSuppressMapError({ error: reloadErr, sourceId: 'satellite-tiles' }, PREFIX)).toBe(false);
    expect(shouldSuppressMapError({ error: reloadErr, sourceId: 'other-source' }, PREFIX)).toBe(false);
  });

  it('passes through a different-message error from a survey source', () => {
    expect(shouldSuppressMapError({ error: new Error('worker parse failed'), sourceId: 'survey-data-shots' }, PREFIX)).toBe(false);
  });

  it('does not throw and returns false for events with missing or malformed fields', () => {
    expect(shouldSuppressMapError({ error: reloadErr }, PREFIX)).toBe(false); // no sourceId
    expect(shouldSuppressMapError({ sourceId: 'survey-data-shots' }, PREFIX)).toBe(false); // no error
    expect(shouldSuppressMapError({}, PREFIX)).toBe(false);
    expect(shouldSuppressMapError(null, PREFIX)).toBe(false);
    expect(shouldSuppressMapError(undefined, PREFIX)).toBe(false);
    // Non-string sourceId short-circuits; a non-Error error is stringified, not dereferenced.
    expect(shouldSuppressMapError({ error: 'Should not be trying to reload', sourceId: 42 }, PREFIX)).toBe(false);
    expect(shouldSuppressMapError({ error: 42, sourceId: 'survey-data-shots' }, PREFIX)).toBe(false);
    // A non-Error error object whose stringified message matches is still classified.
    expect(shouldSuppressMapError({ error: 'Should not be trying to reload', sourceId: 'survey-data-shots' }, PREFIX)).toBe(true);
  });
});

describe('renderFatalMessage', () => {
  it('maps a font failure to a labels-only message', () => {
    expect(renderFatalMessage('label-font-unavailable')).toBe(
      'Map labels unavailable (font failed to load)',
    );
  });
  it('maps a measure-path fault to a ruler-tint message', () => {
    expect(renderFatalMessage('measure-path: gl-error-0x502')).toBe(
      'Measured-path highlight unavailable (GPU)',
    );
  });
  it('falls back to the survey-wide GPU message for geometry / unknown reasons', () => {
    expect(renderFatalMessage('gl-error-0x502')).toBe('Survey rendering unavailable (WebGL2/GPU)');
    expect(renderFatalMessage('program-link-failed')).toBe(
      'Survey rendering unavailable (WebGL2/GPU)',
    );
  });

  // [#576 RV4-FE-MF-4] Toast copy goes through i18n (docs/notifications.md): the
  // render-fatal toasts and the lost-context toast read these keys.
  it('reads every message from the locale', () => {
    const keys = { labels: 'label-font-unavailable', measurePath: 'measure-path: gl-error-0x502', survey: 'webgl-context-lost' };
    const original = Object.fromEntries(Object.keys(keys).map((k) => [k, i18n.t(`map.renderFatal.${k}`)]));
    try {
      for (const k of Object.keys(keys)) i18n.addResource('en', 'translation', `map.renderFatal.${k}`, `TEST ${k}`);
      for (const [k, reason] of Object.entries(keys)) expect(renderFatalMessage(reason)).toBe(`TEST ${k}`);
    } finally {
      for (const [k, text] of Object.entries(original)) i18n.addResource('en', 'translation', `map.renderFatal.${k}`, text);
    }
  });
});
