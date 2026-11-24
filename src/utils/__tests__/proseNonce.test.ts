/**
 * RS-PROSE delta fix 7: the token nonce survives a module reload (Vite HMR
 * re-evaluates `proseTokenCore.ts` in dev). A token already held in a store
 * (a toast's message, a dialog's sentence) was minted with the old nonce, so
 * after a reload it rendered as raw words. The nonce now lives on
 * `globalThis`, minted once per page. Injection: mint it at module scope
 * again.
 */
import { describe, expect, it, vi } from 'vitest';

describe('the prose nonce (RS-PROSE delta fix 7)', () => {
  it('is the same after the module is evaluated again', async () => {
    const first = await import('../proseTokenCore');
    vi.resetModules();
    const second = await import('../proseTokenCore');
    expect(second).not.toBe(first);
    expect(second.NONCE).toBe(first.NONCE);
    expect(first.NONCE).toMatch(/^[0-9a-f]{16}$/);
  });
});
