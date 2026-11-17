# Stalagmight v1.0 Roadmap

> Historical record of the v1.0 feature roadmap. Each item was specified as a
> detailed PRD (specifications, UI mockups, acceptance criteria, test cases).
> **All planned v1.0 features are implemented.** Ongoing work is tracked in the
> GitHub issue tracker.

## Phase 1: Import Dialog Enhancements

| Task | Status | Description |
|------|--------|-------------|
| 01 | Done | Add L/R/U/D fields to Import Dialog |
| 02 | Done | Add pitch_in/pitch_out fields |
| 03 | Done | Add comment field per shot |
| 04 | Done | Live stats in Import Dialog footer |
| 05 | Done | Inline validation warnings |

## Phase 2: Map Display Features

| Task | Status | Description |
|------|--------|-------------|
| 06 | Done | Wire scale bar toggle to MapLibre |
| 07 | Done | Section name labels at midpoints |
| 08 | Done | Station index labels on map |

## Phase 3: GPS & Placement

| Task | Status | Description |
|------|--------|-------------|
| 09 | Done | GPS input with all format support (MGRS / What3words validate but do not convert - needs a library / API) |
| 10 | Done | Per-section adjustment dialog |
| 11 | Done | Interactive map placement mode |
| 12 | Done | Live SVG preview in Import Dialog |

## Phase 4: GlobalView Enhancements

| Task | Status | Description |
|------|--------|-------------|
| 13 | Done | GPS-based map view with tabs |
| 14 | Done | Side panel with project details |
| 15 | Done | Multi-select and merge projects |

## Phase 5: Polish

| Task | Status | Description |
|------|--------|-------------|
| 16 | Done | IN/OUT direction in DMP import |
| 17 | Done | Export preview thumbnail |

## Post-Roadmap Features

These features were implemented after the original roadmap was completed:

| Feature | Description |
|---------|-------------|
| Color Themes | 10 color themes (dark, light, cave, dracula, one_dark, solarized_light, catppuccin_latte, nord, catppuccin_mocha, kaleido_eink) with independent day/night palettes |
| Keyboard Shortcuts Overlay | `?` key opens a filterable shortcuts reference dialog |
| Grid Overlay | Theme-aware coordinate grid with user-selected spacing (1-100m) |
| Placement Suggestion Tuning | Heuristic constants exposed in Advanced Settings |

## Backend Changes

All backend changes have been implemented:

- `create_section` (now `create_line`) returns new section ID via `CommandResult.section_id` (now `line_id`)
- `ProjectMetadata` has `gps_center` and `gps_bounds` fields
- `merge_projects` command implemented in `commands/project.rs`
- `Section` model (now `Line`) has `direction: SurveyDirection` field
