export const themeTokens = {
  dark: {
    // UI Chrome
    '--cc-background': '#1a1a2e',
    '--cc-panel': '#16213e',
    '--cc-border': '#0f3460',
    '--cc-text-primary': '#f5f5f5',
    '--cc-text-secondary': '#a0a0a0',
    '--cc-accent': '#4ecdc4',
    '--cc-accent-on': '#1a1a2e',
    '--cc-warning': '#ffcb77',
    '--cc-error': '#ff6b6b',
    '--cc-success': '#95e1d3',
    '--cc-lock-other': '#5aa9ff',
    '--cc-hover': '#1a1a2e',
    '--cc-focus-ring': '#4ecdc4',
    '--cc-focus-glow': 'rgba(78, 205, 196, 0.5)',
    '--cc-scrollbar-track': '#16213e',
    '--cc-scrollbar-thumb': '#0f3460',
    '--cc-scrollbar-hover': '#4ecdc4',
    '--cc-input-bg': '#16213e',
    '--cc-input-border': '#0f3460',
    '--cc-outside-month': '#666666',
    '--cc-selection-gradient': 'rgba(78, 205, 196, 0.3)',
    '--cc-overlay': 'rgba(0, 0, 0, 0.5)',

    // MapLibre Controls
    '--cc-map-ctrl-bg': '#16213e',
    '--cc-map-ctrl-border': '#0f3460',
    '--cc-map-ctrl-hover': '#1a1a2e',
    '--cc-map-ctrl-active': '#0f3460',
    '--cc-map-ctrl-shadow': 'rgba(15, 52, 96, 0.3)',
    '--cc-map-scale-bg': 'transparent',
    // Was #0f3460 (same as --cc-border, tuned for the panel, not the map canvas):
    // 1.36:1 on --cc-map-background, invisible. Lifted to 3.22:1 - #357 contrast
    // audit finding 2. Hue/saturation preserved.
    '--cc-map-scale-border': '#1f6bc5',
    '--cc-map-scale-text': '#f5f5f5',

    // Map Visualization (see mapVizTokenNames - light and kaleido_eink override these)
    '--cc-map-background': '#1a1a2e',
    '--cc-snap-color': '#ff6b6b',
    '--cc-hover-highlight': '#ffcb77',
    '--cc-select-highlight': '#4ecdc4',
    '--cc-stroke-color': '#ffffff',
    '--cc-stroke-light': 'rgba(255, 255, 255, 0.25)',
    '--cc-text-halo': '#1a1a2e',
    '--cc-text-halo-alt': 'rgba(0, 0, 0, 0.7)',
    '--cc-station-default': '#a0a0a0',
    '--cc-gray-line': '#666666',
    '--cc-gray-text': '#999999',
    '--cc-grid-default': '#333366',
    '--cc-zero-datum': '#ffd700',
    '--cc-compass-color': '#ffffff',
    // DEPTH RAMP CONTRACT - applies to every theme that sets these two tokens.
    //
    // Green (shallower) -> red (deeper). Three things must hold, and the
    // "depth gradient legibility" suite in __tests__/tokens.test.ts measures all
    // three over the whole 41-step sweep, not just at the endpoints:
    //
    // 1. HUE PATH. `getDepthColor` interpolates along the SHORTEST HSL hue path,
    //    so the endpoints alone do not determine what gets drawn. Shallow in
    //    ~90..150 and deep in ~340..20 makes the short path run DOWNWARD through
    //    yellow (60) and orange (30) - the readable depth scale. A shallow hue at
    //    or above 180 (teal, cyan, spring green) flips it onto the upward path
    //    through blue and magenta instead, which is unreadable AND is what put
    //    one_dark at 2.60:1 (#357 item 8).
    // 2. FLOOR. Worst contrast anywhere on the sweep >= 3:1 on the theme's own
    //    `--cc-map-background` (WCAG's floor for a non-text graphical object).
    // 3. LIGHTNESS TRAVEL. The ramp ENDS in the classic red/green confusable
    //    pair, so it must also carry its information in lightness or it collapses
    //    to a flat band under deuteranopia / protanopia. Every theme therefore
    //    runs LIGHT cyan -> DARKER red, monotonically. Two floors enforce it: >= 1.6:1
    //    end-to-end contrast here, and CIEDE2000 separation of the CVD-simulated
    //    endpoints in __tests__/depthRampLegibility.test.ts. Note HSL lightness is
    //    a poor proxy on its own - yellow and blue at equal HSL L differ ~5x in
    //    relative luminance - which is why both floors are perceptual, not HSL.
    //
    // This ramp: hue 173 -> 3, worst 3.49:1 at #d92a20 on #1a1a2e, ends 3.54:1 apart,
    // HSL L 0.69 -> 0.49, deuteranopia 40.88 / protanopia 47.42 dE2000.
    // "solarized_light" and "catppuccin_latte" declare no map-viz tokens, so they
    // inherit this ramp AND this canvas.
    '--cc-depth-shallow': '#6ff0e0',
    '--cc-depth-deep': '#d92a20',
    '--cc-depth-text': '#f5f5f5',

    // Canvas Preview (dark only)
    '--cc-preview-bg': '#1a1a2e',
    '--cc-preview-text': '#f5f5f5',

    // Opacity Variants: accent
    '--cc-accent-5': 'rgba(78, 205, 196, 0.05)',
    '--cc-accent-10': 'rgba(78, 205, 196, 0.1)',
    '--cc-accent-20': 'rgba(78, 205, 196, 0.2)',
    '--cc-accent-30': 'rgba(78, 205, 196, 0.3)',
    '--cc-accent-50': 'rgba(78, 205, 196, 0.5)',
    '--cc-accent-70': 'rgba(78, 205, 196, 0.7)',
    '--cc-accent-80': 'rgba(78, 205, 196, 0.8)',
    '--cc-accent-90': 'rgba(78, 205, 196, 0.9)',

    // Opacity Variants: background
    '--cc-background-30': 'rgba(26, 26, 46, 0.3)',
    '--cc-background-50': 'rgba(26, 26, 46, 0.5)',
    '--cc-background-80': 'rgba(26, 26, 46, 0.8)',

    // Opacity Variants: panel
    '--cc-panel-20': 'rgba(22, 33, 62, 0.2)',
    '--cc-panel-50': 'rgba(22, 33, 62, 0.5)',
    '--cc-panel-90': 'rgba(22, 33, 62, 0.9)',

    // Opacity Variants: error
    '--cc-error-10': 'rgba(255, 107, 107, 0.1)',
    '--cc-error-20': 'rgba(255, 107, 107, 0.2)',
    '--cc-error-30': 'rgba(255, 107, 107, 0.3)',
    '--cc-error-80': 'rgba(255, 107, 107, 0.8)',
    '--cc-error-90': 'rgba(255, 107, 107, 0.9)',

    // Opacity Variants: warning
    '--cc-warning-5': 'rgba(255, 203, 119, 0.05)',
    '--cc-warning-10': 'rgba(255, 203, 119, 0.1)',
    '--cc-warning-20': 'rgba(255, 203, 119, 0.2)',
    '--cc-warning-30': 'rgba(255, 203, 119, 0.3)',

    // Opacity Variants: success
    '--cc-success-5': 'rgba(149, 225, 211, 0.05)',
    '--cc-success-10': 'rgba(149, 225, 211, 0.1)',
    '--cc-success-30': 'rgba(149, 225, 211, 0.3)',
  },
  light: {
    // UI Chrome
    '--cc-background': '#f5f0e8',
    '--cc-panel': '#ffffff',
    '--cc-border': '#d4c5a9',
    '--cc-text-primary': '#1a120b',
    '--cc-text-secondary': '#6b5b4a',
    '--cc-accent': '#3a8a82',
    '--cc-accent-on': '#ffffff',
    '--cc-warning': '#b8860b',
    '--cc-error': '#c0392b',
    '--cc-success': '#27764e',
    '--cc-lock-other': '#1f6feb',
    '--cc-hover': '#ede6db',
    '--cc-focus-ring': '#3a8a82',
    '--cc-focus-glow': 'rgba(58, 138, 130, 0.4)',
    '--cc-scrollbar-track': '#ede6db',
    '--cc-scrollbar-thumb': '#c1a07a',
    '--cc-scrollbar-hover': '#3a8a82',
    '--cc-input-bg': '#ffffff',
    '--cc-input-border': '#d4c5a9',
    '--cc-outside-month': '#b0a090',
    '--cc-selection-gradient': 'rgba(58, 138, 130, 0.2)',
    '--cc-overlay': 'rgba(0, 0, 0, 0.5)',

    // MapLibre Controls
    '--cc-map-ctrl-bg': '#ffffff',
    '--cc-map-ctrl-border': '#d4c5a9',
    '--cc-map-ctrl-hover': '#ede6db',
    '--cc-map-ctrl-active': '#d4c5a9',
    '--cc-map-ctrl-shadow': 'rgba(0, 0, 0, 0.15)',
    '--cc-map-scale-bg': 'transparent',
    // Was #d4c5a9 (same as --cc-border, tuned for the panel, not the map canvas):
    // 1.59:1 on --cc-map-background, invisible. Darkened to 3.23:1 - #357
    // contrast audit finding 2. Hue/saturation preserved.
    '--cc-map-scale-border': '#a38651',
    '--cc-map-scale-text': '#1a120b',

    // Map Visualization - warm paper canvas, dark ink outlines
    '--cc-map-background': '#faf7f2',
    '--cc-snap-color': '#c0392b',
    '--cc-hover-highlight': '#b8860b',
    '--cc-select-highlight': '#3a8a82',
    '--cc-stroke-color': '#1a120b',
    '--cc-stroke-light': 'rgba(26, 18, 11, 0.25)',
    '--cc-text-halo': '#faf7f2',
    '--cc-text-halo-alt': 'rgba(250, 247, 242, 0.85)',
    '--cc-station-default': '#6b5b4a',
    '--cc-gray-line': '#9c8b78',
    '--cc-gray-text': '#6b5b4a',
    '--cc-grid-default': '#ddd0bb',
    '--cc-zero-datum': '#b8860b',
    '--cc-compass-color': '#1a120b',
    // Cyan -> green -> yellow -> red per the depth-ramp contract on `dark`,
    // tuned for paper: on a
    // light canvas the ramp's own yellow MIDPOINT is the worst case (it is the
    // brightest thing on the sweep), so both ends run dark enough to hold the
    // mid at 2.10:1. hue 175 -> 5, worst 2.10:1 at #b2b311 on #faf7f2, ends
    // 2.23:1 apart, HSL L 0.38 -> 0.38, deuteranopia 34.99 / protanopia
    // 39.52 dE2000. Held to a 2:1 floor, not 3:1 - see RAMP_FLOOR_LIGHT_CANVAS
    // in tokens.test.ts for why, and for the measured cost of the alternatives.
    '--cc-depth-shallow': '#1aa89c',
    '--cc-depth-deep': '#b81a0c',
    '--cc-depth-text': '#1a120b',

    // Canvas Preview
    '--cc-preview-bg': '#faf7f2',
    '--cc-preview-text': '#1a120b',

    // Opacity Variants: accent
    '--cc-accent-5': 'rgba(58, 138, 130, 0.05)',
    '--cc-accent-10': 'rgba(58, 138, 130, 0.1)',
    '--cc-accent-20': 'rgba(58, 138, 130, 0.2)',
    '--cc-accent-30': 'rgba(58, 138, 130, 0.3)',
    '--cc-accent-50': 'rgba(58, 138, 130, 0.5)',
    '--cc-accent-70': 'rgba(58, 138, 130, 0.7)',
    '--cc-accent-80': 'rgba(58, 138, 130, 0.8)',
    '--cc-accent-90': 'rgba(58, 138, 130, 0.9)',

    // Opacity Variants: background
    '--cc-background-30': 'rgba(245, 240, 232, 0.3)',
    '--cc-background-50': 'rgba(245, 240, 232, 0.5)',
    '--cc-background-80': 'rgba(245, 240, 232, 0.8)',

    // Opacity Variants: panel
    '--cc-panel-20': 'rgba(255, 255, 255, 0.2)',
    '--cc-panel-50': 'rgba(255, 255, 255, 0.5)',
    '--cc-panel-90': 'rgba(255, 255, 255, 0.9)',

    // Opacity Variants: error
    '--cc-error-10': 'rgba(192, 57, 43, 0.1)',
    '--cc-error-20': 'rgba(192, 57, 43, 0.2)',
    '--cc-error-30': 'rgba(192, 57, 43, 0.3)',
    '--cc-error-80': 'rgba(192, 57, 43, 0.8)',
    '--cc-error-90': 'rgba(192, 57, 43, 0.9)',

    // Opacity Variants: warning
    '--cc-warning-5': 'rgba(184, 134, 11, 0.05)',
    '--cc-warning-10': 'rgba(184, 134, 11, 0.1)',
    '--cc-warning-20': 'rgba(184, 134, 11, 0.2)',
    '--cc-warning-30': 'rgba(184, 134, 11, 0.3)',

    // Opacity Variants: success
    '--cc-success-5': 'rgba(39, 118, 78, 0.05)',
    '--cc-success-10': 'rgba(39, 118, 78, 0.1)',
    '--cc-success-30': 'rgba(39, 118, 78, 0.3)',
  },
  cave: {
    // UI Chrome - very dark, desaturated, faint warm tint
    '--cc-background': '#0e0d0c',
    '--cc-panel': '#171615',
    '--cc-border': '#2a2826',
    '--cc-text-primary': '#d5d0ca',
    '--cc-text-secondary': '#858280',
    '--cc-accent': '#a89060',
    '--cc-accent-on': '#0e0d0c',
    '--cc-warning': '#b89848',
    '--cc-error': '#b85048',
    '--cc-success': '#48986a',
    '--cc-lock-other': '#6f9ccf',
    '--cc-hover': '#171615',
    '--cc-focus-ring': '#a89060',
    '--cc-focus-glow': 'rgba(168, 144, 96, 0.35)',
    '--cc-scrollbar-track': '#171615',
    '--cc-scrollbar-thumb': '#2a2826',
    '--cc-scrollbar-hover': '#a89060',
    '--cc-input-bg': '#171615',
    '--cc-input-border': '#2a2826',
    '--cc-outside-month': '#585654',
    '--cc-selection-gradient': 'rgba(168, 144, 96, 0.2)',
    '--cc-overlay': 'rgba(0, 0, 0, 0.5)',

    // MapLibre Controls
    '--cc-map-ctrl-bg': '#171615',
    '--cc-map-ctrl-border': '#2a2826',
    '--cc-map-ctrl-hover': '#0e0d0c',
    '--cc-map-ctrl-active': '#2a2826',
    '--cc-map-ctrl-shadow': 'rgba(42, 40, 38, 0.3)',
    '--cc-map-scale-bg': 'transparent',
    // Was #2a2826 (same as --cc-border, tuned for the panel, not the map canvas):
    // 1.32:1 on --cc-map-background, invisible. Lifted to 3.21:1 - #357 contrast
    // audit finding 2. Hue/saturation preserved.
    '--cc-map-scale-border': '#66625d',
    '--cc-map-scale-text': '#d5d0ca',

    // Map Visualization
    '--cc-map-background': '#0e0d0c',
    '--cc-snap-color': '#b85048',
    '--cc-hover-highlight': '#b89848',
    '--cc-select-highlight': '#a89060',
    '--cc-stroke-color': '#d5d0ca',
    '--cc-stroke-light': 'rgba(213, 208, 202, 0.25)',
    '--cc-text-halo': '#0e0d0c',
    '--cc-text-halo-alt': 'rgba(0, 0, 0, 0.7)',
    '--cc-station-default': '#858280',
    '--cc-gray-line': '#585654',
    '--cc-gray-text': '#787674',
    '--cc-grid-default': '#2a2826',
    '--cc-zero-datum': '#b8a868',
    '--cc-compass-color': '#d5d0ca',
    // Was tan #a89060 -> salmon #b85048: 35 degrees of hue travel across the whole
    // depth range, so a survey drew as one flat salmon and depth was effectively
    // not encoded at all. Sage green -> brick in cave's muted register instead:
    // hue 173 -> 3, worst 3.98:1 at #d92a20 on #0e0d0c, ends 3.54:1 apart,
    // HSL L 0.69 -> 0.49, deuteranopia 40.88 / protanopia 47.42 dE2000.
    '--cc-depth-shallow': '#6ff0e0',
    '--cc-depth-deep': '#d92a20',
    '--cc-depth-text': '#d5d0ca',

    // Canvas Preview
    '--cc-preview-bg': '#0e0d0c',
    '--cc-preview-text': '#d5d0ca',

    // Opacity Variants: accent
    '--cc-accent-5': 'rgba(168, 144, 96, 0.05)',
    '--cc-accent-10': 'rgba(168, 144, 96, 0.1)',
    '--cc-accent-20': 'rgba(168, 144, 96, 0.2)',
    '--cc-accent-30': 'rgba(168, 144, 96, 0.3)',
    '--cc-accent-50': 'rgba(168, 144, 96, 0.5)',
    '--cc-accent-70': 'rgba(168, 144, 96, 0.7)',
    '--cc-accent-80': 'rgba(168, 144, 96, 0.8)',
    '--cc-accent-90': 'rgba(168, 144, 96, 0.9)',

    // Opacity Variants: background
    '--cc-background-30': 'rgba(14, 13, 12, 0.3)',
    '--cc-background-50': 'rgba(14, 13, 12, 0.5)',
    '--cc-background-80': 'rgba(14, 13, 12, 0.8)',

    // Opacity Variants: panel
    '--cc-panel-20': 'rgba(23, 22, 21, 0.2)',
    '--cc-panel-50': 'rgba(23, 22, 21, 0.5)',
    '--cc-panel-90': 'rgba(23, 22, 21, 0.9)',

    // Opacity Variants: error
    '--cc-error-10': 'rgba(184, 80, 72, 0.1)',
    '--cc-error-20': 'rgba(184, 80, 72, 0.2)',
    '--cc-error-30': 'rgba(184, 80, 72, 0.3)',
    '--cc-error-80': 'rgba(184, 80, 72, 0.8)',
    '--cc-error-90': 'rgba(184, 80, 72, 0.9)',

    // Opacity Variants: warning
    '--cc-warning-5': 'rgba(184, 152, 72, 0.05)',
    '--cc-warning-10': 'rgba(184, 152, 72, 0.1)',
    '--cc-warning-20': 'rgba(184, 152, 72, 0.2)',
    '--cc-warning-30': 'rgba(184, 152, 72, 0.3)',

    // Opacity Variants: success
    '--cc-success-5': 'rgba(72, 152, 106, 0.05)',
    '--cc-success-10': 'rgba(72, 152, 106, 0.1)',
    '--cc-success-30': 'rgba(72, 152, 106, 0.3)',
  },
  dracula: {
    // UI Chrome
    '--cc-background': '#282a36',
    '--cc-panel': '#21222c',
    '--cc-border': '#44475a',
    '--cc-text-primary': '#f8f8f2',
    // Lifted from #6272a4 (3.03:1 on --cc-panel/--cc-background/--cc-hover/--cc-input-bg,
    // the worst of the four) to 4.61:1 minimum across all four - #357 contrast audit
    // finding 1. Hue/saturation preserved, lightness only.
    '--cc-text-secondary': '#8592b8',
    '--cc-accent': '#bd93f9',
    '--cc-accent-on': '#282a36',
    '--cc-warning': '#ffb86c',
    '--cc-error': '#ff5555',
    '--cc-success': '#50fa7b',
    '--cc-lock-other': '#6d9eeb',
    '--cc-hover': '#282a36',
    '--cc-focus-ring': '#bd93f9',
    '--cc-focus-glow': 'rgba(189, 147, 249, 0.5)',
    '--cc-scrollbar-track': '#21222c',
    '--cc-scrollbar-thumb': '#44475a',
    '--cc-scrollbar-hover': '#bd93f9',
    '--cc-input-bg': '#21222c',
    '--cc-input-border': '#44475a',
    '--cc-outside-month': '#6272a4',
    '--cc-selection-gradient': 'rgba(189, 147, 249, 0.3)',
    '--cc-overlay': 'rgba(0, 0, 0, 0.5)',

    // MapLibre Controls
    '--cc-map-ctrl-bg': '#21222c',
    '--cc-map-ctrl-border': '#44475a',
    '--cc-map-ctrl-hover': '#282a36',
    '--cc-map-ctrl-active': '#44475a',
    '--cc-map-ctrl-shadow': 'rgba(68, 71, 90, 0.3)',
    '--cc-map-scale-bg': 'transparent',
    // Was #44475a (same as --cc-border, tuned for the panel, not the map canvas):
    // 1.56:1 on --cc-map-background, invisible. Lifted to 3.20:1 - #357 contrast
    // audit finding 2. Hue/saturation preserved.
    '--cc-map-scale-border': '#717694',
    '--cc-map-scale-text': '#f8f8f2',

    // Map Visualization
    '--cc-map-background': '#282a36',
    '--cc-snap-color': '#ff5555',
    '--cc-hover-highlight': '#ffb86c',
    '--cc-select-highlight': '#bd93f9',
    '--cc-stroke-color': '#f8f8f2',
    '--cc-stroke-light': 'rgba(248, 248, 242, 0.25)',
    '--cc-text-halo': '#282a36',
    '--cc-text-halo-alt': 'rgba(0, 0, 0, 0.7)',
    '--cc-station-default': '#6272a4',
    '--cc-gray-line': '#44475a',
    '--cc-gray-text': '#6272a4',
    '--cc-grid-default': '#44475a',
    '--cc-zero-datum': '#f1fa8c',
    '--cc-compass-color': '#f8f8f2',
    // Cyan -> green -> yellow -> red per the depth-ramp contract on `dark`. The
    // shallow end is a Dracula-register cyan rather than the palette's #50fa7b
    // green: starting at hue 172 puts the whole green/yellow arc INSIDE the ramp
    // instead of at its start, which is what widens both the endpoint separation
    // and the deuteranopia margin. Ends spread apart in lightness for the same
    // reason. hue 172 -> 3, worst 3.18:1 at #e0342a on #282a36, ends 3.34:1 apart,
    // HSL L 0.72 -> 0.52, deuteranopia 38.94 / protanopia 45.01 dE2000.
    '--cc-depth-shallow': '#7ff2e2',
    '--cc-depth-deep': '#e0342a',
    '--cc-depth-text': '#f8f8f2',

    // Canvas Preview
    '--cc-preview-bg': '#282a36',
    '--cc-preview-text': '#f8f8f2',

    // Opacity Variants: accent
    '--cc-accent-5': 'rgba(189, 147, 249, 0.05)',
    '--cc-accent-10': 'rgba(189, 147, 249, 0.1)',
    '--cc-accent-20': 'rgba(189, 147, 249, 0.2)',
    '--cc-accent-30': 'rgba(189, 147, 249, 0.3)',
    '--cc-accent-50': 'rgba(189, 147, 249, 0.5)',
    '--cc-accent-70': 'rgba(189, 147, 249, 0.7)',
    '--cc-accent-80': 'rgba(189, 147, 249, 0.8)',
    '--cc-accent-90': 'rgba(189, 147, 249, 0.9)',

    // Opacity Variants: background
    '--cc-background-30': 'rgba(40, 42, 54, 0.3)',
    '--cc-background-50': 'rgba(40, 42, 54, 0.5)',
    '--cc-background-80': 'rgba(40, 42, 54, 0.8)',

    // Opacity Variants: panel
    '--cc-panel-20': 'rgba(33, 34, 44, 0.2)',
    '--cc-panel-50': 'rgba(33, 34, 44, 0.5)',
    '--cc-panel-90': 'rgba(33, 34, 44, 0.9)',

    // Opacity Variants: error
    '--cc-error-10': 'rgba(255, 85, 85, 0.1)',
    '--cc-error-20': 'rgba(255, 85, 85, 0.2)',
    '--cc-error-30': 'rgba(255, 85, 85, 0.3)',
    '--cc-error-80': 'rgba(255, 85, 85, 0.8)',
    '--cc-error-90': 'rgba(255, 85, 85, 0.9)',

    // Opacity Variants: warning
    '--cc-warning-5': 'rgba(255, 184, 108, 0.05)',
    '--cc-warning-10': 'rgba(255, 184, 108, 0.1)',
    '--cc-warning-20': 'rgba(255, 184, 108, 0.2)',
    '--cc-warning-30': 'rgba(255, 184, 108, 0.3)',

    // Opacity Variants: success
    '--cc-success-5': 'rgba(80, 250, 123, 0.05)',
    '--cc-success-10': 'rgba(80, 250, 123, 0.1)',
    '--cc-success-30': 'rgba(80, 250, 123, 0.3)',
  },
  one_dark: {
    // UI Chrome
    '--cc-background': '#282c34',
    '--cc-panel': '#21252b',
    '--cc-border': '#3e4451',
    '--cc-text-primary': '#abb2bf',
    // Lifted from #5c6370 (2.16:1 worst of panel/background/hover/input) to
    // 4.55:1 minimum - #357 contrast audit finding 1. Hue/saturation preserved.
    '--cc-text-secondary': '#939aa6',
    '--cc-accent': '#61afef',
    '--cc-accent-on': '#282c34',
    '--cc-warning': '#e5c07b',
    '--cc-error': '#e06c75',
    '--cc-success': '#98c379',
    '--cc-lock-other': '#5c7cfa',
    '--cc-hover': '#2c313a',
    '--cc-focus-ring': '#61afef',
    '--cc-focus-glow': 'rgba(97, 175, 239, 0.5)',
    '--cc-scrollbar-track': '#21252b',
    '--cc-scrollbar-thumb': '#3e4451',
    '--cc-scrollbar-hover': '#61afef',
    '--cc-input-bg': '#21252b',
    '--cc-input-border': '#3e4451',
    '--cc-outside-month': '#5c6370',
    '--cc-selection-gradient': 'rgba(97, 175, 239, 0.3)',
    '--cc-overlay': 'rgba(0, 0, 0, 0.5)',

    // MapLibre Controls
    '--cc-map-ctrl-bg': '#21252b',
    '--cc-map-ctrl-border': '#3e4451',
    '--cc-map-ctrl-hover': '#282c34',
    '--cc-map-ctrl-active': '#3e4451',
    '--cc-map-ctrl-shadow': 'rgba(62, 68, 81, 0.3)',
    '--cc-map-scale-bg': 'transparent',
    // Was #3e4451 (same as --cc-border, tuned for the panel, not the map canvas):
    // 1.43:1 on --cc-map-background, invisible. Lifted to 3.20:1 - #357 contrast
    // audit finding 2. Hue/saturation preserved.
    '--cc-map-scale-border': '#6e7990',
    '--cc-map-scale-text': '#abb2bf',

    // Map Visualization
    '--cc-map-background': '#282c34',
    '--cc-snap-color': '#e06c75',
    '--cc-hover-highlight': '#e5c07b',
    '--cc-select-highlight': '#61afef',
    '--cc-stroke-color': '#abb2bf',
    '--cc-stroke-light': 'rgba(171, 178, 191, 0.25)',
    '--cc-text-halo': '#282c34',
    '--cc-text-halo-alt': 'rgba(0, 0, 0, 0.7)',
    '--cc-station-default': '#5c6370',
    '--cc-gray-line': '#3e4451',
    '--cc-gray-text': '#5c6370',
    '--cc-grid-default': '#3e4451',
    '--cc-zero-datum': '#e5c07b',
    '--cc-compass-color': '#abb2bf',
    // Cyan -> green -> yellow -> red per the depth-ramp contract on `dark`. This
    // is the theme #357 item 8 was filed against, and the distinction matters:
    // the defect was never "cyan", it was hue 187 - the WRONG SIDE of the 180
    // line, where the shortest path to red runs UP through indigo (it bottomed
    // out at #5f5ccd = 2.60:1). The shallow end is cyan again here, at hue 169.8,
    // which descends through green and yellow instead. Ends spread apart in
    // lightness so the ramp survives deuteranopia.
    // hue 170 -> 4, worst 3.05:1 at #dd3327 on #282c34, ends 3.35:1 apart, HSL L 0.71 -> 0.51,
    // deuteranopia 37.92 / protanopia 44.59 dE2000.
    '--cc-depth-shallow': '#7af0dc',
    '--cc-depth-deep': '#dd3327',
    '--cc-depth-text': '#abb2bf',

    // Canvas Preview
    '--cc-preview-bg': '#282c34',
    '--cc-preview-text': '#abb2bf',

    // Opacity Variants: accent
    '--cc-accent-5': 'rgba(97, 175, 239, 0.05)',
    '--cc-accent-10': 'rgba(97, 175, 239, 0.1)',
    '--cc-accent-20': 'rgba(97, 175, 239, 0.2)',
    '--cc-accent-30': 'rgba(97, 175, 239, 0.3)',
    '--cc-accent-50': 'rgba(97, 175, 239, 0.5)',
    '--cc-accent-70': 'rgba(97, 175, 239, 0.7)',
    '--cc-accent-80': 'rgba(97, 175, 239, 0.8)',
    '--cc-accent-90': 'rgba(97, 175, 239, 0.9)',

    // Opacity Variants: background
    '--cc-background-30': 'rgba(40, 44, 52, 0.3)',
    '--cc-background-50': 'rgba(40, 44, 52, 0.5)',
    '--cc-background-80': 'rgba(40, 44, 52, 0.8)',

    // Opacity Variants: panel
    '--cc-panel-20': 'rgba(33, 37, 43, 0.2)',
    '--cc-panel-50': 'rgba(33, 37, 43, 0.5)',
    '--cc-panel-90': 'rgba(33, 37, 43, 0.9)',

    // Opacity Variants: error
    '--cc-error-10': 'rgba(224, 108, 117, 0.1)',
    '--cc-error-20': 'rgba(224, 108, 117, 0.2)',
    '--cc-error-30': 'rgba(224, 108, 117, 0.3)',
    '--cc-error-80': 'rgba(224, 108, 117, 0.8)',
    '--cc-error-90': 'rgba(224, 108, 117, 0.9)',

    // Opacity Variants: warning
    '--cc-warning-5': 'rgba(229, 192, 123, 0.05)',
    '--cc-warning-10': 'rgba(229, 192, 123, 0.1)',
    '--cc-warning-20': 'rgba(229, 192, 123, 0.2)',
    '--cc-warning-30': 'rgba(229, 192, 123, 0.3)',

    // Opacity Variants: success
    '--cc-success-5': 'rgba(152, 195, 121, 0.05)',
    '--cc-success-10': 'rgba(152, 195, 121, 0.1)',
    '--cc-success-30': 'rgba(152, 195, 121, 0.3)',
  },
  solarized_light: {
    // UI Chrome
    '--cc-background': '#fdf6e3',
    '--cc-panel': '#eee8d5',
    '--cc-border': '#d3cbb7',
    '--cc-text-primary': '#586e75',
    // Darkened from #93a1a1 (2.18:1 worst of panel/background/hover/input) to
    // 4.60:1 minimum - #357 contrast audit finding 1. Hue/saturation preserved.
    '--cc-text-secondary': '#5c6a6a',
    '--cc-accent': '#268bd2',
    '--cc-accent-on': '#fdf6e3',
    '--cc-warning': '#b58900',
    '--cc-error': '#dc322f',
    '--cc-success': '#859900',
    '--cc-lock-other': '#1a6496',
    '--cc-hover': '#eee8d5',
    '--cc-focus-ring': '#268bd2',
    '--cc-focus-glow': 'rgba(38, 139, 210, 0.4)',
    '--cc-scrollbar-track': '#eee8d5',
    '--cc-scrollbar-thumb': '#d3cbb7',
    '--cc-scrollbar-hover': '#268bd2',
    '--cc-input-bg': '#fdf6e3',
    '--cc-input-border': '#d3cbb7',
    '--cc-outside-month': '#93a1a1',
    '--cc-selection-gradient': 'rgba(38, 139, 210, 0.2)',
    '--cc-overlay': 'rgba(0, 0, 0, 0.5)',

    // MapLibre Controls
    '--cc-map-ctrl-bg': '#eee8d5',
    '--cc-map-ctrl-border': '#d3cbb7',
    '--cc-map-ctrl-hover': '#fdf6e3',
    '--cc-map-ctrl-active': '#d3cbb7',
    '--cc-map-ctrl-shadow': 'rgba(0, 0, 0, 0.15)',
    '--cc-map-scale-bg': 'transparent',
    '--cc-map-scale-border': '#d3cbb7',
    // Lifted from #586e75 (3.17:1 - tuned for the light panel, but this text sits
    // on the DARK map canvas this chrome-only theme inherits from :root) to
    // 4.64:1 on #1a1a2e - #357 contrast audit finding 3. Hue/saturation preserved.
    '--cc-map-scale-text': '#6e8a92',

    // Opacity Variants: accent
    '--cc-accent-5': 'rgba(38, 139, 210, 0.05)',
    '--cc-accent-10': 'rgba(38, 139, 210, 0.1)',
    '--cc-accent-20': 'rgba(38, 139, 210, 0.2)',
    '--cc-accent-30': 'rgba(38, 139, 210, 0.3)',
    '--cc-accent-50': 'rgba(38, 139, 210, 0.5)',
    '--cc-accent-70': 'rgba(38, 139, 210, 0.7)',
    '--cc-accent-80': 'rgba(38, 139, 210, 0.8)',
    '--cc-accent-90': 'rgba(38, 139, 210, 0.9)',

    // Opacity Variants: background
    '--cc-background-30': 'rgba(253, 246, 227, 0.3)',
    '--cc-background-50': 'rgba(253, 246, 227, 0.5)',
    '--cc-background-80': 'rgba(253, 246, 227, 0.8)',

    // Opacity Variants: panel
    '--cc-panel-20': 'rgba(238, 232, 213, 0.2)',
    '--cc-panel-50': 'rgba(238, 232, 213, 0.5)',
    '--cc-panel-90': 'rgba(238, 232, 213, 0.9)',

    // Opacity Variants: error
    '--cc-error-10': 'rgba(220, 50, 47, 0.1)',
    '--cc-error-20': 'rgba(220, 50, 47, 0.2)',
    '--cc-error-30': 'rgba(220, 50, 47, 0.3)',
    '--cc-error-80': 'rgba(220, 50, 47, 0.8)',
    '--cc-error-90': 'rgba(220, 50, 47, 0.9)',

    // Opacity Variants: warning
    '--cc-warning-5': 'rgba(181, 137, 0, 0.05)',
    '--cc-warning-10': 'rgba(181, 137, 0, 0.1)',
    '--cc-warning-20': 'rgba(181, 137, 0, 0.2)',
    '--cc-warning-30': 'rgba(181, 137, 0, 0.3)',

    // Opacity Variants: success
    '--cc-success-5': 'rgba(133, 153, 0, 0.05)',
    '--cc-success-10': 'rgba(133, 153, 0, 0.1)',
    '--cc-success-30': 'rgba(133, 153, 0, 0.3)',
  },
  catppuccin_latte: {
    // UI Chrome
    '--cc-background': '#eff1f5',
    '--cc-panel': '#e6e9ef',
    '--cc-border': '#ccd0da',
    '--cc-text-primary': '#4c4f69',
    // Darkened from #6c6f85 (4.06:1 worst of panel/background/hover/input) to
    // 4.63:1 minimum - #357 contrast audit finding 1. Hue/saturation preserved.
    '--cc-text-secondary': '#64667b',
    '--cc-accent': '#1e66f5',
    '--cc-accent-on': '#eff1f5',
    '--cc-warning': '#df8e1d',
    '--cc-error': '#d20f39',
    '--cc-success': '#40a02b',
    '--cc-lock-other': '#7287fd',
    '--cc-hover': '#e6e9ef',
    '--cc-focus-ring': '#1e66f5',
    '--cc-focus-glow': 'rgba(30, 102, 245, 0.4)',
    '--cc-scrollbar-track': '#e6e9ef',
    '--cc-scrollbar-thumb': '#ccd0da',
    '--cc-scrollbar-hover': '#1e66f5',
    '--cc-input-bg': '#e6e9ef',
    '--cc-input-border': '#ccd0da',
    '--cc-outside-month': '#9ca0b0',
    '--cc-selection-gradient': 'rgba(30, 102, 245, 0.2)',
    '--cc-overlay': 'rgba(0, 0, 0, 0.5)',

    // MapLibre Controls
    '--cc-map-ctrl-bg': '#e6e9ef',
    '--cc-map-ctrl-border': '#ccd0da',
    '--cc-map-ctrl-hover': '#eff1f5',
    '--cc-map-ctrl-active': '#ccd0da',
    '--cc-map-ctrl-shadow': 'rgba(0, 0, 0, 0.15)',
    '--cc-map-scale-bg': 'transparent',
    '--cc-map-scale-border': '#ccd0da',
    // Lifted from #4c4f69 (2.14:1 - tuned for the light panel, but this text sits
    // on the DARK map canvas this chrome-only theme inherits from :root) to
    // 4.61:1 on #1a1a2e - #357 contrast audit finding 3. Hue/saturation preserved.
    '--cc-map-scale-text': '#7f83a2',

    // Opacity Variants: accent
    '--cc-accent-5': 'rgba(30, 102, 245, 0.05)',
    '--cc-accent-10': 'rgba(30, 102, 245, 0.1)',
    '--cc-accent-20': 'rgba(30, 102, 245, 0.2)',
    '--cc-accent-30': 'rgba(30, 102, 245, 0.3)',
    '--cc-accent-50': 'rgba(30, 102, 245, 0.5)',
    '--cc-accent-70': 'rgba(30, 102, 245, 0.7)',
    '--cc-accent-80': 'rgba(30, 102, 245, 0.8)',
    '--cc-accent-90': 'rgba(30, 102, 245, 0.9)',

    // Opacity Variants: background
    '--cc-background-30': 'rgba(239, 241, 245, 0.3)',
    '--cc-background-50': 'rgba(239, 241, 245, 0.5)',
    '--cc-background-80': 'rgba(239, 241, 245, 0.8)',

    // Opacity Variants: panel
    '--cc-panel-20': 'rgba(230, 233, 239, 0.2)',
    '--cc-panel-50': 'rgba(230, 233, 239, 0.5)',
    '--cc-panel-90': 'rgba(230, 233, 239, 0.9)',

    // Opacity Variants: error
    '--cc-error-10': 'rgba(210, 15, 57, 0.1)',
    '--cc-error-20': 'rgba(210, 15, 57, 0.2)',
    '--cc-error-30': 'rgba(210, 15, 57, 0.3)',
    '--cc-error-80': 'rgba(210, 15, 57, 0.8)',
    '--cc-error-90': 'rgba(210, 15, 57, 0.9)',

    // Opacity Variants: warning
    '--cc-warning-5': 'rgba(223, 142, 29, 0.05)',
    '--cc-warning-10': 'rgba(223, 142, 29, 0.1)',
    '--cc-warning-20': 'rgba(223, 142, 29, 0.2)',
    '--cc-warning-30': 'rgba(223, 142, 29, 0.3)',

    // Opacity Variants: success
    '--cc-success-5': 'rgba(64, 160, 43, 0.05)',
    '--cc-success-10': 'rgba(64, 160, 43, 0.1)',
    '--cc-success-30': 'rgba(64, 160, 43, 0.3)',
  },
  nord: {
    // UI Chrome
    '--cc-background': '#2e3440',
    '--cc-panel': '#3b4252',
    '--cc-border': '#4c566a',
    '--cc-text-primary': '#d8dee9',
    // Lifted from #616e88 (1.96:1 worst of panel/background/hover/input, the
    // worst pairing in the repo) to 4.61:1 minimum - #357 contrast audit
    // finding 1. Hue/saturation preserved.
    '--cc-text-secondary': '#a7b0c0',
    '--cc-accent': '#88c0d0',
    '--cc-accent-on': '#2e3440',
    '--cc-warning': '#ebcb8b',
    '--cc-error': '#bf616a',
    '--cc-success': '#a3be8c',
    '--cc-lock-other': '#6c92d8',
    '--cc-hover': '#3b4252',
    '--cc-focus-ring': '#88c0d0',
    '--cc-focus-glow': 'rgba(136, 192, 208, 0.5)',
    '--cc-scrollbar-track': '#3b4252',
    '--cc-scrollbar-thumb': '#4c566a',
    '--cc-scrollbar-hover': '#88c0d0',
    '--cc-input-bg': '#3b4252',
    '--cc-input-border': '#4c566a',
    '--cc-outside-month': '#616e88',
    '--cc-selection-gradient': 'rgba(136, 192, 208, 0.3)',
    '--cc-overlay': 'rgba(0, 0, 0, 0.5)',

    // MapLibre Controls
    '--cc-map-ctrl-bg': '#3b4252',
    '--cc-map-ctrl-border': '#4c566a',
    '--cc-map-ctrl-hover': '#2e3440',
    '--cc-map-ctrl-active': '#4c566a',
    '--cc-map-ctrl-shadow': 'rgba(76, 86, 106, 0.3)',
    '--cc-map-scale-bg': 'transparent',
    // Was #4c566a (same as --cc-border, tuned for the panel, not the map canvas):
    // 1.69:1 on --cc-map-background, invisible. Lifted to 3.22:1 - #357 contrast
    // audit finding 2. Hue/saturation preserved.
    '--cc-map-scale-border': '#74829c',
    '--cc-map-scale-text': '#d8dee9',

    // Map Visualization
    '--cc-map-background': '#2e3440',
    '--cc-snap-color': '#bf616a',
    '--cc-hover-highlight': '#ebcb8b',
    '--cc-select-highlight': '#88c0d0',
    '--cc-stroke-color': '#d8dee9',
    '--cc-stroke-light': 'rgba(216, 222, 233, 0.25)',
    '--cc-text-halo': '#2e3440',
    '--cc-text-halo-alt': 'rgba(0, 0, 0, 0.7)',
    '--cc-station-default': '#616e88',
    '--cc-gray-line': '#4c566a',
    '--cc-gray-text': '#616e88',
    '--cc-grid-default': '#434c5e',
    '--cc-zero-datum': '#ebcb8b',
    '--cc-compass-color': '#d8dee9',
    // Cyan -> green -> yellow -> red per the depth-ramp contract on `dark`. This
    // canvas is the lightest of the dark set, so the deep end is lifted off
    // Nord's Aurora red #bf616a, which was the whole ramp's worst case at 3.05:1
    // - clearing the floor by 1.8%. The light shallow end is on-register for Nord
    // (cf. Snow Storm) and is what buys the lightness travel.
    // hue 170 -> 4, worst 3.30:1 at #ea4a3e on #2e3440, ends 2.76:1 apart, HSL L 0.71 -> 0.58,
    // deuteranopia 34.22 / protanopia 37.91 dE2000.
    '--cc-depth-shallow': '#7af0dc',
    '--cc-depth-deep': '#ea4a3e',
    '--cc-depth-text': '#d8dee9',

    // Canvas Preview
    '--cc-preview-bg': '#2e3440',
    '--cc-preview-text': '#d8dee9',

    // Opacity Variants: accent
    '--cc-accent-5': 'rgba(136, 192, 208, 0.05)',
    '--cc-accent-10': 'rgba(136, 192, 208, 0.1)',
    '--cc-accent-20': 'rgba(136, 192, 208, 0.2)',
    '--cc-accent-30': 'rgba(136, 192, 208, 0.3)',
    '--cc-accent-50': 'rgba(136, 192, 208, 0.5)',
    '--cc-accent-70': 'rgba(136, 192, 208, 0.7)',
    '--cc-accent-80': 'rgba(136, 192, 208, 0.8)',
    '--cc-accent-90': 'rgba(136, 192, 208, 0.9)',

    // Opacity Variants: background
    '--cc-background-30': 'rgba(46, 52, 64, 0.3)',
    '--cc-background-50': 'rgba(46, 52, 64, 0.5)',
    '--cc-background-80': 'rgba(46, 52, 64, 0.8)',

    // Opacity Variants: panel
    '--cc-panel-20': 'rgba(59, 66, 82, 0.2)',
    '--cc-panel-50': 'rgba(59, 66, 82, 0.5)',
    '--cc-panel-90': 'rgba(59, 66, 82, 0.9)',

    // Opacity Variants: error
    '--cc-error-10': 'rgba(191, 97, 106, 0.1)',
    '--cc-error-20': 'rgba(191, 97, 106, 0.2)',
    '--cc-error-30': 'rgba(191, 97, 106, 0.3)',
    '--cc-error-80': 'rgba(191, 97, 106, 0.8)',
    '--cc-error-90': 'rgba(191, 97, 106, 0.9)',

    // Opacity Variants: warning
    '--cc-warning-5': 'rgba(235, 203, 139, 0.05)',
    '--cc-warning-10': 'rgba(235, 203, 139, 0.1)',
    '--cc-warning-20': 'rgba(235, 203, 139, 0.2)',
    '--cc-warning-30': 'rgba(235, 203, 139, 0.3)',

    // Opacity Variants: success
    '--cc-success-5': 'rgba(163, 190, 140, 0.05)',
    '--cc-success-10': 'rgba(163, 190, 140, 0.1)',
    '--cc-success-30': 'rgba(163, 190, 140, 0.3)',
  },
  catppuccin_mocha: {
    // UI Chrome
    '--cc-background': '#1e1e2e',
    '--cc-panel': '#181825',
    '--cc-border': '#313244',
    '--cc-text-primary': '#cdd6f4',
    // Lifted from #6c7086 (3.36:1 worst of panel/background/hover/input) to
    // 4.61:1 minimum - #357 contrast audit finding 1. Hue/saturation preserved.
    '--cc-text-secondary': '#83879b',
    '--cc-accent': '#89b4fa',
    '--cc-accent-on': '#1e1e2e',
    '--cc-warning': '#f9e2af',
    '--cc-error': '#f38ba8',
    '--cc-success': '#a6e3a1',
    '--cc-lock-other': '#74c7ec',
    '--cc-hover': '#181825',
    '--cc-focus-ring': '#89b4fa',
    '--cc-focus-glow': 'rgba(137, 180, 250, 0.5)',
    '--cc-scrollbar-track': '#181825',
    '--cc-scrollbar-thumb': '#313244',
    '--cc-scrollbar-hover': '#89b4fa',
    '--cc-input-bg': '#181825',
    '--cc-input-border': '#313244',
    '--cc-outside-month': '#6c7086',
    '--cc-selection-gradient': 'rgba(137, 180, 250, 0.3)',
    '--cc-overlay': 'rgba(0, 0, 0, 0.5)',

    // MapLibre Controls
    '--cc-map-ctrl-bg': '#181825',
    '--cc-map-ctrl-border': '#313244',
    '--cc-map-ctrl-hover': '#1e1e2e',
    '--cc-map-ctrl-active': '#313244',
    '--cc-map-ctrl-shadow': 'rgba(49, 50, 68, 0.3)',
    '--cc-map-scale-bg': 'transparent',
    // Was #313244 (same as --cc-border, tuned for the panel, not the map canvas):
    // 1.30:1 on --cc-map-background, invisible. Lifted to 3.20:1 - #357 contrast
    // audit finding 2. Hue/saturation preserved.
    '--cc-map-scale-border': '#686b91',
    '--cc-map-scale-text': '#cdd6f4',

    // Map Visualization
    '--cc-map-background': '#1e1e2e',
    '--cc-snap-color': '#f38ba8',
    '--cc-hover-highlight': '#f9e2af',
    '--cc-select-highlight': '#89b4fa',
    '--cc-stroke-color': '#cdd6f4',
    '--cc-stroke-light': 'rgba(205, 214, 244, 0.25)',
    '--cc-text-halo': '#1e1e2e',
    '--cc-text-halo-alt': 'rgba(0, 0, 0, 0.7)',
    '--cc-station-default': '#6c7086',
    '--cc-gray-line': '#313244',
    '--cc-gray-text': '#6c7086',
    '--cc-grid-default': '#313244',
    '--cc-zero-datum': '#f9e2af',
    '--cc-compass-color': '#cdd6f4',
    // Cyan -> green -> yellow -> red per the depth-ramp contract on `dark`. The
    // deep end is pulled off Mocha's pink-leaning Red #f38ba8 (hue 343) toward a
    // truer red so the ramp ends in red rather than magenta, stopping at hue 354
    // to keep some Mocha character.
    // hue 170 -> 354, worst 3.57:1 at #dd3040 on #1e1e2e, ends 3.35:1 apart, HSL L 0.71 -> 0.53,
    // deuteranopia 35.78 / protanopia 42.71 dE2000.
    '--cc-depth-shallow': '#7af0dc',
    '--cc-depth-deep': '#dd3040',
    '--cc-depth-text': '#cdd6f4',

    // Canvas Preview
    '--cc-preview-bg': '#1e1e2e',
    '--cc-preview-text': '#cdd6f4',

    // Opacity Variants: accent
    '--cc-accent-5': 'rgba(137, 180, 250, 0.05)',
    '--cc-accent-10': 'rgba(137, 180, 250, 0.1)',
    '--cc-accent-20': 'rgba(137, 180, 250, 0.2)',
    '--cc-accent-30': 'rgba(137, 180, 250, 0.3)',
    '--cc-accent-50': 'rgba(137, 180, 250, 0.5)',
    '--cc-accent-70': 'rgba(137, 180, 250, 0.7)',
    '--cc-accent-80': 'rgba(137, 180, 250, 0.8)',
    '--cc-accent-90': 'rgba(137, 180, 250, 0.9)',

    // Opacity Variants: background
    '--cc-background-30': 'rgba(30, 30, 46, 0.3)',
    '--cc-background-50': 'rgba(30, 30, 46, 0.5)',
    '--cc-background-80': 'rgba(30, 30, 46, 0.8)',

    // Opacity Variants: panel
    '--cc-panel-20': 'rgba(24, 24, 37, 0.2)',
    '--cc-panel-50': 'rgba(24, 24, 37, 0.5)',
    '--cc-panel-90': 'rgba(24, 24, 37, 0.9)',

    // Opacity Variants: error
    '--cc-error-10': 'rgba(243, 139, 168, 0.1)',
    '--cc-error-20': 'rgba(243, 139, 168, 0.2)',
    '--cc-error-30': 'rgba(243, 139, 168, 0.3)',
    '--cc-error-80': 'rgba(243, 139, 168, 0.8)',
    '--cc-error-90': 'rgba(243, 139, 168, 0.9)',

    // Opacity Variants: warning
    '--cc-warning-5': 'rgba(249, 226, 175, 0.05)',
    '--cc-warning-10': 'rgba(249, 226, 175, 0.1)',
    '--cc-warning-20': 'rgba(249, 226, 175, 0.2)',
    '--cc-warning-30': 'rgba(249, 226, 175, 0.3)',

    // Opacity Variants: success
    '--cc-success-5': 'rgba(166, 227, 161, 0.05)',
    '--cc-success-10': 'rgba(166, 227, 161, 0.1)',
    '--cc-success-30': 'rgba(166, 227, 161, 0.3)',
  },
  kaleido_eink: {
    // UI Chrome - pure white bg, pure black text, maximum e-ink contrast
    '--cc-background': '#ffffff',
    '--cc-panel': '#ffffff',
    '--cc-border': '#555555',
    '--cc-text-primary': '#000000',
    '--cc-text-secondary': '#333333',
    '--cc-accent': '#0050a0',
    '--cc-accent-on': '#ffffff',
    '--cc-warning': '#cc7700',
    '--cc-error': '#cc0000',
    '--cc-success': '#007a2f',
    '--cc-lock-other': '#0066cc',
    '--cc-hover': '#e8e8e8',
    '--cc-focus-ring': '#0050a0',
    '--cc-focus-glow': '#99bbdd',
    '--cc-scrollbar-track': '#ffffff',
    '--cc-scrollbar-thumb': '#999999',
    '--cc-scrollbar-hover': '#0050a0',
    '--cc-input-bg': '#ffffff',
    '--cc-input-border': '#555555',
    '--cc-outside-month': '#999999',
    '--cc-selection-gradient': '#ccdcec',
    '--cc-overlay': '#666666',

    // MapLibre Controls
    '--cc-map-ctrl-bg': '#ffffff',
    '--cc-map-ctrl-border': '#555555',
    '--cc-map-ctrl-hover': '#e8e8e8',
    '--cc-map-ctrl-active': '#cccccc',
    '--cc-map-ctrl-shadow': '#aaaaaa',
    '--cc-map-scale-bg': 'transparent',
    '--cc-map-scale-border': '#555555',
    '--cc-map-scale-text': '#000000',

    // Map Visualization - white background, black outlines
    '--cc-map-background': '#ffffff',
    '--cc-snap-color': '#cc0000',
    '--cc-hover-highlight': '#cc7700',
    '--cc-select-highlight': '#0050a0',
    '--cc-stroke-color': '#000000',
    '--cc-stroke-light': '#888888',
    '--cc-text-halo': '#ffffff',
    '--cc-text-halo-alt': '#ffffff',
    '--cc-station-default': '#000000',
    '--cc-gray-line': '#888888',
    '--cc-gray-text': '#555555',
    '--cc-grid-default': '#cccccc',
    '--cc-zero-datum': '#cc7700',
    '--cc-compass-color': '#000000',
    // Cyan -> green -> yellow -> red per the depth-ramp contract on `dark`,
    // tuned for the e-ink
    // white canvas: as on `light`, the ramp's yellow midpoint is the worst case,
    // so both ends run dark. hue 175 -> 5, worst 2.25:1 at #b2b311 on #ffffff,
    // ends 2.23:1 apart, HSL L 0.38 -> 0.38, deuteranopia 34.99 /
    // protanopia 39.52 dE2000. 2:1 floor, as on `light`.
    '--cc-depth-shallow': '#1aa89c',
    '--cc-depth-deep': '#b81a0c',
    '--cc-depth-text': '#000000',

    // Canvas Preview
    '--cc-preview-bg': '#ffffff',
    '--cc-preview-text': '#000000',

    // Opacity Variants: accent (pre-composited against #ffffff)
    '--cc-accent-5': '#f2f6fa',
    '--cc-accent-10': '#e6edf5',
    '--cc-accent-20': '#ccdcec',
    '--cc-accent-30': '#b3cae2',
    '--cc-accent-50': '#80a8d0',
    '--cc-accent-70': '#4d85bd',
    '--cc-accent-80': '#3374b3',
    '--cc-accent-90': '#1a62aa',

    // Opacity Variants: background (white on white - all white)
    '--cc-background-30': '#ffffff',
    '--cc-background-50': '#ffffff',
    '--cc-background-80': '#ffffff',

    // Opacity Variants: panel (white on white - all white)
    '--cc-panel-20': '#ffffff',
    '--cc-panel-50': '#ffffff',
    '--cc-panel-90': '#ffffff',

    // Opacity Variants: error (pre-composited against #ffffff)
    '--cc-error-10': '#fce6e6',
    '--cc-error-20': '#f5cccc',
    '--cc-error-30': '#ebadad',
    '--cc-error-80': '#d63333',
    '--cc-error-90': '#d11a1a',

    // Opacity Variants: warning (pre-composited against #ffffff)
    '--cc-warning-5': '#fef8f0',
    '--cc-warning-10': '#fdf2e0',
    '--cc-warning-20': '#fae4c2',
    '--cc-warning-30': '#f8d7a3',

    // Opacity Variants: success (pre-composited against #ffffff)
    '--cc-success-5': '#f0faf3',
    '--cc-success-10': '#e0f5e7',
    '--cc-success-30': '#b3e2c3',
  },
} as const;

// Map-visualization tokens. Chrome-only light themes ("solarized_light",
// "catppuccin_latte") deliberately leave these unset so the canvas inherits the
// dark values from :root; full light themes ("light", "kaleido_eink") override
// them all. Tests enforce both halves. Named for what the tokens are, not for
// which themes set them: "light" moved into the full set in #332, so the older
// name darkOnlyTokenNames no longer described it.
export const mapVizTokenNames = [
  '--cc-map-background',
  '--cc-snap-color',
  '--cc-hover-highlight',
  '--cc-select-highlight',
  '--cc-stroke-color',
  '--cc-stroke-light',
  '--cc-text-halo',
  '--cc-text-halo-alt',
  '--cc-station-default',
  '--cc-gray-line',
  '--cc-gray-text',
  '--cc-grid-default',
  '--cc-zero-datum',
  '--cc-compass-color',
  '--cc-depth-shallow',
  '--cc-depth-deep',
  '--cc-depth-text',
  '--cc-preview-bg',
  '--cc-preview-text',
] as const;

export type ThemeTokenName = keyof typeof themeTokens.dark;
