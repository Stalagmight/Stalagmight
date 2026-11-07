/**
 * #576 NF-W12H-2: the environment a headless harness app runs in.
 *
 * Harness runs started together, each under its own `dbus-run-session`, died at
 * start with "invalid session id": every run's AT-SPI bus launcher serves the
 * same `$XDG_RUNTIME_DIR/at-spi/bus` path, and an app whose launcher lost the race
 * never finished starting. The app on a throwaway display now runs with the
 * accessibility bridge off, beside the existing X11 pin.
 */
import { describe, expect, it } from 'vitest';

import { headlessAppEnv } from './display';

describe('headlessAppEnv (#576 NF-W12H-2)', () => {
  const desktop: NodeJS.ProcessEnv = {
    PATH: '/usr/bin',
    DISPLAY: ':0',
    WAYLAND_DISPLAY: 'wayland-0',
    XDG_RUNTIME_DIR: '/run/user/1000',
    STALAGMIGHT_TEST_HOOKS: '1',
  };

  it('turns the accessibility bridge off, so parallel runs cannot race for the AT-SPI bus', () => {
    expect(headlessAppEnv(desktop, ':91').NO_AT_BRIDGE).toBe('1');
  });

  it('renders on the virtual display through X11, never onto the desktop compositor', () => {
    const env = headlessAppEnv(desktop, ':91');
    expect(env.DISPLAY).toBe(':91');
    expect(env.GDK_BACKEND).toBe('x11');
    expect(env).not.toHaveProperty('WAYLAND_DISPLAY');
  });

  it('keeps everything else and leaves the caller environment alone', () => {
    const before = { ...desktop };
    const env = headlessAppEnv(desktop, ':91');
    expect(env.PATH).toBe('/usr/bin');
    expect(env.XDG_RUNTIME_DIR).toBe('/run/user/1000');
    expect(env.STALAGMIGHT_TEST_HOOKS).toBe('1');
    expect(desktop).toEqual(before);
  });
});
