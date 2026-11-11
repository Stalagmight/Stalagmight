/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{js,ts,jsx,tsx}'],
  theme: {
    extend: {
      animation: {
        'pulse-shimmer': 'pulse-shimmer 1.5s ease-in-out infinite',
      },
      keyframes: {
        'pulse-shimmer': {
          '0%': { opacity: '0.5', transform: 'translateX(-100%)' },
          '50%': { opacity: '1' },
          '100%': { opacity: '0.5', transform: 'translateX(100%)' },
        },
      },
      colors: {
        // UI Chrome colors (CSS variable references)
        // NOTE: Tailwind opacity modifiers (e.g. bg-accent/10) do NOT work with
        // raw CSS variable color values. All slash-style usages must be migrated
        // to the dash-style opacity variant tokens below (e.g. bg-accent-10).
        // See "Complete Opacity Modifier Inventory" in the theme epic spec.
        background: 'var(--cc-background)',
        panel: 'var(--cc-panel)',
        border: 'var(--cc-border)',
        'text-primary': 'var(--cc-text-primary)',
        'text-secondary': 'var(--cc-text-secondary)',
        accent: 'var(--cc-accent)',
        'accent-on': 'var(--cc-accent-on)',
        hover: 'var(--cc-hover)',
        overlay: 'var(--cc-overlay)',
        warning: 'var(--cc-warning)',
        // Warning prose (text-warning-text): AA on the panel and the warning washes.
        'warning-text': 'var(--cc-warning-text)',
        // The same warning text dimmed toward the panel, for an idle tab that carries
        // a warning (the SpeleoDB tab with un-uploaded changes): 3:1 or better on the
        // panel in every theme (tokens.test.ts).
        'warning-text-dim': 'color-mix(in srgb, var(--cc-warning-text) 75%, var(--cc-panel))',
        // The `secondary` Button fill: the panel nudged 8% toward the text colour,
        // so a secondary button reads as a button instead of plain text on the panel
        // (the global `button { border: none }` reset hides its border). Hover takes
        // it to 14%. One rule for every theme; contrast pinned in tokens.test.ts.
        'button-secondary': 'color-mix(in srgb, var(--cc-panel) 92%, var(--cc-text-primary))',
        'button-secondary-hover': 'color-mix(in srgb, var(--cc-panel) 86%, var(--cc-text-primary))',
        error: 'var(--cc-error)',
        success: 'var(--cc-success)',

        // Section palette colors (hardcoded - per-project display settings, not theme-dependent)
        'section-teal': '#4ecdc4',
        'section-coral': '#ff6b6b',
        'section-mint': '#95e1d3',
        'section-salmon': '#f38181',
        'section-lavender': '#aa96da',
        'section-pink': '#fcbad3',
        'section-sky': '#a8d8ea',
        'section-peach': '#ffcb77',

        // Map colors (CSS variable references)
        grid: 'var(--cc-grid-default)',
        'zero-datum': 'var(--cc-zero-datum)',
        'loop-closure': 'var(--cc-hover-highlight)',
        // reference-shot stays as rgba literal (semi-transparent overlay, works on any background)
        'reference-shot': 'rgba(255, 255, 255, 0.25)',

        // Opacity variant tokens: 25 unique color entries covering all 29
        // slash-style usage patterns in the codebase. Some tokens serve
        // multiple utility prefixes (e.g. accent-20 maps to both
        // bg-accent/20 and border-accent/20). Use these dash-style tokens
        // instead of slash-style opacity modifiers with CSS variable colors.

        // Opacity variants: accent (8 entries)
        'accent-5': 'var(--cc-accent-5)',
        'accent-10': 'var(--cc-accent-10)',
        'accent-20': 'var(--cc-accent-20)',
        'accent-30': 'var(--cc-accent-30)',
        'accent-50': 'var(--cc-accent-50)',
        'accent-70': 'var(--cc-accent-70)',
        'accent-80': 'var(--cc-accent-80)',
        'accent-90': 'var(--cc-accent-90)',

        // Opacity variants: background (3 entries)
        'background-30': 'var(--cc-background-30)',
        'background-50': 'var(--cc-background-50)',
        'background-80': 'var(--cc-background-80)',

        // Opacity variants: panel (3 entries)
        'panel-20': 'var(--cc-panel-20)',
        'panel-50': 'var(--cc-panel-50)',
        'panel-90': 'var(--cc-panel-90)',

        // Opacity variants: error (5 entries)
        'error-10': 'var(--cc-error-10)',
        'error-20': 'var(--cc-error-20)',
        'error-30': 'var(--cc-error-30)',
        'error-80': 'var(--cc-error-80)',
        'error-90': 'var(--cc-error-90)',

        // Opacity variants: warning (4 entries)
        'warning-5': 'var(--cc-warning-5)',
        'warning-10': 'var(--cc-warning-10)',
        'warning-20': 'var(--cc-warning-20)',
        'warning-30': 'var(--cc-warning-30)',

        // Opacity variants: success (3 entries)
        'success-5': 'var(--cc-success-5)',
        'success-10': 'var(--cc-success-10)',
        'success-30': 'var(--cc-success-30)',
      },
    },
  },
  plugins: [],
};
