/**
 * The module stand-ins and async helpers of #576 E15-NF1 behave like the
 * copies they replaced: `t` echoes keys (with options when asked) and is fresh
 * per `useTranslation()` call; `telemetryWith` keeps the real exports and
 * `telemetryStubs` has none but the stubs; the Tauri module answers as told;
 * the dialog frame renders exactly the parts asked for; the waits take exactly
 * the turns they name.
 */
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { i18nKeys, translationStub } from '../i18n';
import { telemetryStubs, telemetryWith } from '../telemetry';
import { tauriCore, tauriCoreResolving } from '../tauri';
import { buttonStub, dialogWrapperStub } from '../components';
import { deferred, microtasks, nextTimer, timerTurns } from '../async';
import { actMicrotasks, actNextTimer } from '../act';

describe('react-i18next stand-ins', () => {
  it('echo the key, or key and options, and hand out a fresh t per call', async () => {
    const real = await i18nKeys();
    const { t } = real.useTranslation();
    expect(t('a.b', { count: 2 })).toBe('a.b');
    expect(real.useTranslation().t).not.toBe(t);
    expect(typeof real.Trans).toBe('function');

    const withOptions = translationStub('keyAndOptions');
    expect(withOptions.useTranslation().t('a.b', { count: 2 })).toBe('a.b:{"count":2}');
    expect(withOptions.useTranslation().t('a.b')).toBe('a.b');
    expect(Object.keys(withOptions).sort()).toEqual(['initReactI18next', 'useTranslation']);
  });
});

describe('utils/telemetry stand-ins', () => {
  it('telemetryWith keeps the real exports and replaces trackDialogComplete and the stubs', async () => {
    const setSource = vi.fn();
    const mod = await telemetryWith({ setSource });
    expect(mod.setSource).toBe(setSource);
    expect(vi.isMockFunction(mod.trackDialogComplete)).toBe(true);
    expect(vi.isMockFunction(mod.trackAction)).toBe(false);
    expect(typeof mod.consumeSource).toBe('function');
  });

  it('telemetryStubs has trackDialogComplete and the stubs only', () => {
    const mod = telemetryStubs({ clearProjectActionSource: vi.fn() });
    expect(Object.keys(mod).sort()).toEqual(['clearProjectActionSource', 'trackDialogComplete']);
  });
});

describe('@tauri-apps/api/core stand-ins', () => {
  it('resolve the given answer, or wrap the test spy', async () => {
    await expect(tauriCoreResolving({}).invoke('any')).resolves.toEqual({});
    await expect(tauriCoreResolving(null).invoke('any')).resolves.toBeNull();
    const spy = vi.fn();
    expect(tauriCore(spy).invoke).toBe(spy);
  });
});

describe('DialogWrapper and Button stand-ins', () => {
  it('render the children, a render-prop child, and only the parts asked for', () => {
    const { DialogWrapper } = dialogWrapperStub();
    const { container } = render(
      <DialogWrapper title="T" data-testid="d">
        {({ requestClose }) => <span onClick={requestClose}>body</span>}
      </DialogWrapper>,
    );
    expect(container.innerHTML).toBe('<div><span>body</span></div>');
  });

  it('render the title and the test id when asked', () => {
    const spy = vi.fn();
    const Titled = dialogWrapperStub({ title: 'testid' }).DialogWrapper;
    const Headed = dialogWrapperStub({ title: 'h2', testId: true, spy }).DialogWrapper;
    const a = render(<Titled title="Hello">x</Titled>);
    expect(a.container.innerHTML).toBe('<div><div data-testid="dialog-title">Hello</div>x</div>');
    const b = render(
      <Headed title="Hi" data-testid="dlg" isDirty>
        y
      </Headed>,
    );
    expect(b.container.innerHTML).toBe('<div data-testid="dlg"><h2>Hi</h2>y</div>');
    expect(spy).toHaveBeenLastCalledWith({ title: 'Hi', 'data-testid': 'dlg', isDirty: true });
  });

  it('Button is a plain button with the props passed through', () => {
    const { Button } = buttonStub();
    render(<Button data-testid="b">Go</Button>);
    expect(screen.getByTestId('b').outerHTML).toBe('<button data-testid="b">Go</button>');
  });
});

describe('async helpers', () => {
  it('deferred settles by hand, both ways', async () => {
    const ok = deferred<number>();
    ok.resolve(3);
    await expect(ok.promise).resolves.toBe(3);
    const bad = deferred();
    bad.reject(new Error('no'));
    await expect(bad.promise).rejects.toThrow('no');
  });

  it('microtasks(n) takes as many turns as the inline copies it replaced, and no more', async () => {
    // How far a chain of queued callbacks has got when the wait returns.
    const reached = async (wait: () => Promise<void>) => {
      const seen: number[] = [];
      let p: Promise<void> = Promise.resolve();
      for (let i = 1; i <= 12; i += 1) {
        const n = i;
        p = p.then(() => {
          seen.push(n);
        });
      }
      await wait();
      const at = seen.length;
      await p;
      return at;
    };
    async function inlineThree() {
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    }
    expect(await reached(() => microtasks(3))).toBe(await reached(inlineThree));
    expect(await reached(() => microtasks(4))).toBe((await reached(() => microtasks(3))) + 1);
  });

  it('nextTimer and timerTurns wait for zero-delay timers', async () => {
    const order: string[] = [];
    setTimeout(() => order.push('first'), 0);
    await nextTimer();
    expect(order).toEqual(['first']);
    setTimeout(() => setTimeout(() => order.push('nested'), 0), 0);
    await timerTurns(2);
    expect(order).toEqual(['first', 'nested']);
  });

  it('the act variants run the same waits', async () => {
    const order: string[] = [];
    setTimeout(() => order.push('t'), 0);
    await actNextTimer();
    expect(order).toEqual(['t']);
    void Promise.resolve().then(() => order.push('m'));
    await actMicrotasks(1);
    expect(order).toEqual(['t', 'm']);
  });
});
