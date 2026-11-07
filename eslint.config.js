// ESLint flat config (#576 D1). The gate is scripts/check-eslint.mjs
// (pnpm lint:eslint): errors fail, warnings are ratcheted per file in
// eslint-baseline.json. Policy and how to extend it: docs/linting.md
// "Frontend: ESLint".
//
// Scope: the frontend (src/) and the two test-automation trees (e2e-playwright/,
// e2e/). The parser is typescript-eslint's, with no type-aware rules (no
// `parserOptions.project`), so a run needs no TypeScript program and stays fast.
//
// Rules:
//   - typescript-eslint's `recommended` set (not `recommended-type-checked`),
//     everywhere;
//   - the two React hook rules, in src/ only (nothing else renders React):
//     rules-of-hooks (error) - a hook behind `||`, a condition or an early
//     return changes the hook order between renders and React throws;
//     exhaustive-deps (warn, ratcheted) - a missing effect/memo dependency can
//     be a stale-closure bug, but the tree carried ~100 findings when this
//     landed, so they are frozen in the baseline rather than fixed in one go.
//   - stalagmight/no-render-ref-write (error, src/ only; scripts/eslint/): no
//     `ref.current = ...` while a component or hook renders - use useLatest.
//   - stalagmight/require-store-selector (error, src/ only, zero tolerance): no
//     zustand store hook call without a selector (`useProjectStore()`), or with
//     one that subscribes to the whole store anyway (identity, a fresh object or
//     array without useShallow, useShallow over `{ ...s }`), which re-renders its
//     component on every store update.
//   - in the test-automation trees, no-unused-vars is a warning, ratcheted the
//     same way: the recommended set found ~260 findings there when it landed
//     (236 `any` across 22 Playwright specs, 25 unused names). The `any`s are
//     gone, so no-explicit-any is an error there too (#576 NF-W7F-5). src/ had
//     three findings, fixed, and keeps every recommended rule an error.
// An `eslint-disable` needs a reason after ` -- ` (stalagmight/require-disable-reason,
// error, src and both test-automation trees), and an unused or unknown-rule
// directive is an error (reportUnusedDisableDirectives).
import tseslint from 'typescript-eslint';
import reactHooks from 'eslint-plugin-react-hooks';
import stalagmight from './scripts/eslint/plugin.mjs';

const SRC = ['src/**/*.{ts,tsx}'];
const E2E = ['e2e-playwright/**/*.ts', 'e2e/**/*.ts'];
const TS = [...SRC, ...E2E];

// `_`-prefixed names are deliberately unused (tsc's noUnusedParameters
// convention, which the tree already follows).
const UNUSED_VARS = {
  argsIgnorePattern: '^_',
  varsIgnorePattern: '^_',
  caughtErrorsIgnorePattern: '^_',
  destructuredArrayIgnorePattern: '^_',
  ignoreRestSiblings: true,
};

export default [
  // Generated ts-rs output (CLAUDE.md "IPC Types Are Generated"): never hand-written.
  { ignores: ['src/types/ipc/**', 'src/types/ipc.generated.ts'] },
  { linterOptions: { reportUnusedDisableDirectives: 'error' } },
  ...tseslint.configs.recommended.map((config) => ({ ...config, files: TS })),
  {
    files: TS,
    languageOptions: { parserOptions: { ecmaFeatures: { jsx: true } } },
    rules: {
      '@typescript-eslint/no-unused-vars': ['error', UNUSED_VARS],
    },
  },
  {
    files: SRC,
    plugins: { 'react-hooks': reactHooks, stalagmight },
    rules: {
      'react-hooks/rules-of-hooks': 'error',
      'react-hooks/exhaustive-deps': 'warn',
      'stalagmight/no-render-ref-write': 'error',
      'stalagmight/require-store-selector': 'error',
    },
  },
  {
    // [#576 RV5-LS-4] Every linted tree: the reason after ` -- ` is enforced.
    files: TS,
    plugins: { stalagmight },
    rules: {
      'stalagmight/require-disable-reason': 'error',
    },
  },
  {
    files: E2E,
    rules: {
      '@typescript-eslint/no-unused-vars': ['warn', UNUSED_VARS],
    },
  },
];
