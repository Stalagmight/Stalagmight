# Stalagmight Project Structure

## Tech Stack

| Layer | Technology |
|-------|------------|
| Framework | Tauri 2.x |
| Backend | Rust (stable) |
| Frontend | React 18 + TypeScript (strict mode) |
| State | Zustand |
| Map | MapLibre GL JS |
| Database | SQLite (rusqlite) |
| Styling | Tailwind CSS |
| Logging | tracing + tracing-appender |
| E2E Testing | Playwright (mocked IPC, the CI tier), the `e2e/agent` harness (full app, via the webdriverio library) |
| Unit / Component Testing | Vitest (+ Testing Library) |
| Backend Testing | Rust built-in tests (`cargo test`) |

## Directory Tree

```
stalagmight/
├── src-tauri/                    # Rust backend: cargo workspace root + the app crate
│   ├── Cargo.toml                # Hybrid workspace root (see note below)
│   ├── src/                      # The thin `stalagmight` app crate
│   │   ├── main.rs               # Tauri entry point
│   │   ├── lib.rs                # Library root, Tauri setup, the single generate_handler!
│   │   ├── commands/             # Tauri IPC command handlers
│   │   ├── tile_attribution.rs   # Tile-provider attribution fetch
│   │   └── webkit_memory.rs      # Linux WebKit memory-pressure env setup
│   ├── examples/                 # gen_ipc_types (dev-only, feature-gated)
│   ├── tests/                    # App-crate integration tests
│   ├── crates/                   # The 8 workspace member crates
│   │   ├── models/               # Data structures (Project, Section, Shot, geo, app_dirs)
│   │   ├── telemetry/            # Anonymous counters + event registry, logging/ (tracing setup)
│   │   ├── parsers/              # Import parsers: dmp (mNemo, versions 2-5),
│   │   │                         #   tml (Ariane), compass, gpx, walls/
│   │   ├── persistence/          # SQLite operations (.stalx, .plnx), tile caches
│   │   ├── export/               # CSV, JSON, image, TML export
│   │   ├── speleodb/             # SpeleoDB integration (auth, dump, sync)
│   │   ├── tiles/                # Map-tile engine (fetch, HTTP caching, overzoom)
│   │   └── core/                 # Business logic (AppState, command executors, undo,
│   │                             #   config/ I/O, tile_providers)
│   ├── capabilities/             # Tauri 2.x permissions
│   └── icons/                    # App icons
│
├── src/                          # TypeScript frontend
│   ├── main.tsx                  # React entry point
│   ├── App.tsx                   # Root component
│   ├── components/
│   │   ├── GlobalView/           # Landing/overview view
│   │   │   ├── GlobalView.tsx        # Container component
│   │   │   ├── GlobalViewMap.tsx     # MapLibre map integration
│   │   │   ├── GlobalSurveyLayer.ts  # Multi-system geometry rendering
│   │   │   ├── ViewModeToggle.tsx    # Survey/markers toggle
│   │   │   ├── MetadataPanel.tsx     # System metadata display
│   │   │   └── SystemCard.tsx        # System list item
│   │   ├── SingleSystemView/     # Main editing view
│   │   ├── Map/                  # MapLibre integration
│   │   │   ├── MapCanvas.tsx         # MapLibre map initialization
│   │   │   ├── SurveyLayer.ts        # Survey rendering on map
│   │   │   └── GridOverlay.tsx       # Theme-aware coordinate grid overlay
│   │   ├── Dialogs/              # All dialog components
│   │   │   ├── KeyboardShortcutsDialog.tsx  # Shortcuts overlay (? key)
│   │   │   └── settings/        # Settings dialog tabs
│   │   │       ├── AppearanceSettings.tsx   # Theme mode & color theme selection
│   │   │       ├── ThemePreviewCard.tsx     # Mini app mockup preview cards
│   │   │       ├── AdvancedSettings.tsx     # Placement suggestion constants
│   │   │       └── ...
│   │   └── common/               # Shared primitives (see below)
│   ├── theme/                    # Color theme system
│   │   └── tokens.ts                 # 10 color theme token definitions
│   ├── store/                    # Zustand stores
│   │   ├── projectStore.ts           # Project state
│   │   ├── uiStore.ts                # UI state (incl. resolvedTheme)
│   │   └── globalViewStore.ts        # Global View state
│   ├── hooks/                    # Custom React hooks
│   │   ├── useFocusTrap.ts           # Focus trap for modal dialogs
│   │   └── ...
│   ├── utils/                    # Coordinate/color utilities
│   │   ├── dialogStack.ts            # Nested dialog stack manager
│   └── types/                    # TypeScript type definitions
│
├── e2e/                          # Full-app tests + harnesses (real Tauri app)
│   ├── agent/                    # LLM-agent harness (MCP + WebdriverIO)
│   ├── fixtures/                 # Test data (DMP files)
│   └── lib/                      # Shared helpers
│
├── e2e-playwright/               # Playwright E2E tests (mocked IPC, 100+ spec files)
│   ├── mocks/                    # Tauri IPC mocks
│   │   ├── tauri-ipc.ts          # Mock command handlers (the core)
│   │   └── ipc/                  # Its command families (config, Plan mode, DMP, tiles, SpeleoDB)
│   ├── fixtures/                 # Test data
│   ├── dmp-import.spec.ts        # Import DMP dialog tests
│   ├── project-crud.spec.ts      # Project operations tests
│   ├── theme.spec.ts             # Color theme switching tests
│   ├── settings-shortcuts.spec.ts # Shortcut configuration tests
│   ├── ui-components.spec.ts     # UI interaction tests
│   └── ...                       # the rest of the spec files
│
├── scripts/                      # Dev + CI scripts: the lint gates, dev.sh,
│                                 #   release.sh, mac build, replay generators
│
├── docs/                         # Documentation
│   ├── SPEC.md                   # Full specification document
│   ├── TESTING.md                # Testing guide
│   ├── architecture.md           # System design
│   ├── project-structure.md      # This file
│   └── ...                       # every page indexed in docs/README.md
│
└── CLAUDE.md                     # Agent instructions (AGENTS.md symlinks to it)
```

`src-tauri/Cargo.toml` is the workspace root - a hybrid one: it is both the
`stalagmight` app package's manifest and the `[workspace]` declaration for the
eight members under `src-tauri/crates/`. The lockfile (`Cargo.lock`), the wry
`[patch.crates-io]`, the build profiles, `[workspace.dependencies]` and the
`[workspace.lints]` policy all live there; each member inherits the lints via
`[lints] workspace = true`. See the workspace-layout table in
[architecture.md](architecture.md) for what each crate contains.

## Shared UI primitives

`src/components/common/` holds the shared primitives - about 28 of them, among
which: `Button` / `IconButton`, `Panel` / `ResizablePanel`, `Select`, `Toggle`,
`DatePicker`, `Tooltip` / `HelpTooltip`, `ToastStack`, `ElidedPath`,
`ContextMenu`, the icon set in `icons.tsx`, and `SpreadsheetGrid` - the in-house,
library-free survey data grid (documented in `SpreadsheetGrid/README.md`;
`react-datasheet-grid` is not a dependency). Shot editing inside dialogs uses
`ShotTableEditor` instead. For which primitive to reach for and how it should
look, see [style-guide.md](style-guide.md).
