# Stalagmight Configuration

The reference for `config.toml`. Ground truth is the `Config` struct in
`src-tauri/crates/core/src/config/types.rs` plus the defaults in
`src-tauri/crates/core/src/config/defaults.rs` - change those and this file together.

## Config File Location

The directory is Tauri's per-app config dir, named for the bundle identifier
(`com.stalagmight.desktop`).

| Platform | Path |
|----------|------|
| Linux | `~/.config/com.stalagmight.desktop/config.toml` |
| macOS | `~/Library/Application Support/com.stalagmight.desktop/config.toml` |
| Windows | `%APPDATA%\com.stalagmight.desktop\config.toml` |

## SpeleoDB configuration

All SpeleoDB settings and the API token are stored in a separate TOML file:

| Platform | Path |
|----------|------|
| Linux | `~/.config/com.stalagmight.desktop/speleodb/speleodb.conf` |
| macOS | `~/Library/Application Support/com.stalagmight.desktop/speleodb/speleodb.conf` |
| Windows | `%APPDATA%\com.stalagmight.desktop\speleodb\speleodb.conf` |

```toml
endpoint = "https://www.speleodb.org"
email = "you@example.com"
token = "<API token>"
insecure_http = false   # development builds only; ignored by release builds
undo_mode = "full"
```

The endpoint and email are optional until configured. The token is optional until
sign-in and is removed on disconnect; disconnect before changing instances. Saving a
new instance clears the previous instance's email and token, so the old account is
never paired with the new server (not even when the Connect that follows fails). There
is one instance and one saved account. Tokens are plaintext, with atomic replacement and
owner-only file permissions (0600) on Unix; Windows uses the user's profile ACLs.
Passwords are never saved. Tokens remain backend-only and are omitted from IPC
settings payloads and debug output.

A malformed or unreadable SpeleoDB file leaves general preferences intact and the
app signed out, with a redacted diagnostic in the log. Unrelated preference saves
preserve that file; signing in or explicitly changing SpeleoDB settings can replace
it. If saving general preferences fails after writing the SpeleoDB file, the old
credential bytes (or prior absence) are restored with private permissions. A failed
rollback is reported explicitly; two files cannot provide crash-atomic updates.

When `speleodb/speleodb.conf` is genuinely absent, the legacy `[speleodb]` table
supplies endpoint, email, `insecure_http`,
`poll_interval_seconds` and `undo_mode`. Normalization still applies, including the
polling floor. Passwords, tokens and obsolete credential-storage options are ignored;
sign in again to store a token. The separate file always takes precedence, including
when damaged or unreadable. Malformed legacy preferences are logged without source
values and retained on unrelated saves for recovery. A successful two-file save
persists migrated preferences and removes the legacy table; a failed general save
restores the prior separate file or its absence.
Signing out also deletes the current account's obsolete app-owned plaintext token
file from `speleodb/tokens`, without reading or importing it. Other accounts are
untouched; cleanup failures are reported. No keychain access is restored.
`DEV_SPELEODB_URL` still overrides the instance for development, but a saved token
is restored only when the resolved instance matches its configured endpoint.
Reloading configuration refuses a different endpoint while connected. Same-instance
credential reloads reset the edit gate; unrelated preference reloads retain the session.
Projects refresh at startup and immediately after sign-in. Connect in Settings first
asks what the account change needs (the sign-out flow when another account is signed
in, and the question about unsent SpeleoDB drafts the change would drop; Cancel changes
nothing, not even the staged settings), then saves the staged settings, then signs in
over the connection on screen - in a development build including the "Allow insecure
HTTP" switch on screen (never the saved value). On success the backend saves that
endpoint and `insecure_http` with the token, and Settings closes and shows the
project listing. A failed sign-in saves no credentials. Refreshes display a spinner until
they finish, including failed requests. Use the refresh button
on the home screen to reload the listing, or Refresh in the open project's
SpeleoDB workspace. No periodic SpeleoDB refresh runs in the background.

## Key Config Options

```toml
[general]
auto_save = true                    # Enable auto-save after modifications
auto_save_interval_seconds = 30     # Debounce delay before an auto-save fires
undo_history_limit = 100            # In-memory undo stack depth
undo_persist_limit = 20             # Undo entries persisted into the .stalx on save
recent_projects = []                # List of recently opened project paths
recent_projects_max = 20            # Maximum recent projects to track
locale = "system"                   # "system" or a specific locale tag like "en-US"
number_format = "locale"            # "locale" (follow language) or a locale tag like "de-DE"
date_format = "locale"              # "locale" (follow language) or a locale tag like "en-GB"
update_track = "stable"             # "stable" (finals) or "beta" (candidates, then finals); see Update Track

[units]
length = "meters"                   # meters, feet
depth = "meters"                    # meters, feet (independent of length)
angle = "degrees"                   # degrees, gradians
# Display decimals per quantity (length 1, coordinates 2, depth 1, azimuth 1,
# inclination 1, gps 6); any subset may be overridden:
decimal_precision = { length = 1, coordinates = 2, depth = 1, azimuth = 1, inclination = 1, gps = 6 }

[display]
grid_color = "#333366"              # Grid overlay line color
grid_spacing = 10.0                 # Grid spacing in meters
font_size = 12                      # Map label font size
font_color = "#f5f5f5"              # Map label color
measure_path_color = "#22d3ee"      # Along-line tint for the measure tool's path

[section_colors]
# Colors handed to new sections, in order; wraps around when all are used
palette = ["#4ecdc4", "#ff6b6b", "#95e1d3", "#f38181",
           "#aa96da", "#fcbad3", "#a8d8ea", "#ffcb77"]

[appearance]
theme = "dark"                      # "dark", "light", or "auto" (follows OS preference)
day_color_theme = "light"           # Color palette for light/day mode
night_color_theme = "cave"          # Color palette for dark/night mode
# Available color themes:
#   dark, light, cave, dracula, one_dark, solarized_light,
#   catppuccin_latte, nord, catppuccin_mocha, kaleido_eink

[syntactic_highlighting]
level = "full"                      # "off", "toolbar_only", "icons", or "full" (default)
preset = "default"                  # "default", "deuteranopia", "protanopia", "tritanopia"
# Per-preset, per-role color overrides (sparse; only customized colors are stored).
# Roles: segment, section, shot, station, comment, delete, move,
#        select, measure, merge, pan, zoom
# [syntactic_highlighting.overrides.default]
# station = "#36b96a"

[import]
# Which data wins where an imported section overlaps existing shots:
#   "import_data" (use the orphan's), "existing_data" (use the target's),
#   "interpolate" (average), "keep_both" (mark the duplicate as reference)
overlap_mode = "import_data"

[placement_suggestion]
azimuth_sigma = 18.0                # Sigma for azimuth error (degrees)
length_sigma = 0.8                  # Sigma for length error (meters)
depth_sigma = 0.35                  # Sigma for depth error (meters)
azimuth_weight = 0.35               # Weight for azimuth score component
length_weight = 0.25                # Weight for length score component
depth_in_weight = 0.20              # Weight for depth_in score component
depth_out_weight = 0.20             # Weight for depth_out score component
min_shot_score = 0.15               # Minimum per-shot score to continue matching
min_aggregate_score = 0.45          # Minimum aggregate score to include a suggestion
max_reference_shots = 5             # Maximum reference shots to try from each leaf
# Further cutoffs and penalties (max_suggestions, min_partial_ratio,
# single_shot_penalty, short_shot_threshold, max_azimuth_error_*,
# max_depth_error, max_length_error_*) are in `src-tauri/crates/models/src/config/mod.rs`.

[map]
satellite_provider = "esri"         # Tile provider (selects from backend registry via get_tile_providers IPC)
satellite_enabled = true            # Show satellite imagery
# GPS origin for coordinate alignment (required for satellite tiles)
satellite_gps_origin = { latitude = 24.55, longitude = -81.78 }
zoom_default = 15                   # Currently unread - the map always opens at 15 (hard-coded in uiStore)
zoom_min = 0                        # Zoom range clamp
zoom_max = 22
zoom_step = 1.0                     # Zoom delta per +/- keypress

[map.controls]
show_navigation_tools = false       # Put the Pan/Zoom tools back on the toolbar, with their P/Z shortcuts
pan_speed = 1.0                     # Ctrl+drag and middle-drag pan multiplier (0.1-5.0); plain drag stays 1:1
zoom_speed = 0.55                   # Zoom levels per wheel notch (0.1-0.95); 0.95 is the engine's practical ceiling
invert_scroll = false               # Reverse wheel zoom direction

[map.station]
radius = 4                          # Visual circle radius in pixels (1-50)
hit_area = 10                       # Clickable/hoverable area radius in pixels (1-50, >= radius)

[map.shot]
width = 2                           # Visual line width in pixels (0.5-20)
hit_area = 8                        # Clickable/hoverable area width in pixels (0.5-20, >= width)

[map.snap]
threshold = 10                      # Snap detection radius in pixels (1-50)
enabled_merge = true                # Enable snap-to-station in merge tool
enabled_measure = true              # Enable snap-to-station in measure tool

[cache]
tile_cache_enabled = true           # The one on/off switch for tile caching
tile_cache_max_mb = 500             # Maximum cache size in MB
tile_ttl_days = 30                  # Default retention: days a tile stays on disk
# Per-provider retention override; omit to use tile_ttl_days.
# [cache.providers.esri]
# ttl_days = 7
#
# Imagery is never stored for offline use, or for any purpose of our own. These
# keys bound an ordinary HTTP cache; what may be kept, and for how long, is the
# provider's decision via its own Cache-Control.
#
# There is no per-provider on/off switch, because config is not what decides
# whether a tile is stored. `tile_cache_enabled` permits the cache to be used at
# all; after that a tile is written ONLY if its response carried a
# `Cache-Control` granting it (a `max-age`, or `no-cache` for "keep but always
# revalidate"). A response with no directive is served and dropped - silence is
# not read as consent, so the app holds third-party imagery only where the
# provider's own headers say it may, and only for as long as they say.
#
# TTL is RETENTION - how long a tile is kept at all. It is NOT how long a tile
# may be served as current: that is separately capped by the provider's own
# `Cache-Control: max-age` (Esri sends `max-age=86400`), recorded per tile when
# it is cached. Raising ttl_days therefore only lengthens how long a stored copy
# remains available to answer a request the network could not; it can never make
# the app show stale imagery as though it were fresh.
#
# Past max-age the tile is REVALIDATED, not re-downloaded: the request carries
# the stored ETag as If-None-Match, and Esri answers an unchanged tile with
# `304 Not Modified` and no body (~20 KB saved per tile over real imagery).
# A 304 re-stamps the tile as current, so a slow-changing basemap costs one
# body-less round trip per tile per day rather than a full re-fetch. The stored
# copy is only served without asking when the network call fails outright -
# `stale-if-error`, and only where that actually applies: a transport failure or
# a 5xx. A 403 or 404 is the provider answering, so the tile is reported missing
# rather than answered off the stored copy - and a 401/403/410 also stops the
# overzoom fallback, so a denial cannot come back as an upscaled crop of a parent
# tile. (A 404 still climbs: for most tile services that is how they say "no
# imagery at this zoom here", which is what overzoom is for. 410 Gone gets no
# such carve-out - it says the imagery was removed on purpose.)
#
# A `no-cache` response is stored, and IS revalidated - keeping its ETag is the
# whole reason to store it - but it is never served without a successful
# validation first. RFC 9111 5.2.2.4 requires that before reuse, which makes a
# failed revalidation the one case where a retained copy may not answer. And a
# `Cache-Control` we cannot parse grants nothing but revokes nothing either: the
# response is not cached, and a copy already held keeps the deadline it was
# stored under rather than being dropped over a header a proxy mangled.
#
# `max-age` is counted from when the ORIGIN made the response, not from when we
# received it, so any `Age` the response carries is subtracted before the
# deadline is recorded. Esri is behind CloudFront and does send it; without this
# a tile with an hour of its grant left would be treated as fresh for a further
# day.
#
# Retention is enforced at the point of use as well as by eviction. Eviction runs
# on a tile write, so an offline session - which never writes one - could
# otherwise be served a copy past `ttl_days` for as long as it lasted. Retired
# rows are also dropped when the cache opens, so the setting bounds what is kept
# on disk and not merely what is shown.

[global_view]
fps_threshold = 30                  # FPS below which auto-throttle activates
tolerance = 0.5                     # GeoJSON simplification tolerance (normal)
throttle_tolerance = 1.0            # GeoJSON simplification tolerance (throttled)

[validation]
warn_shot_length_min = 0.3              # Flag shots shorter than this (meters)
warn_shot_length_max = 50.0             # Flag shots longer than this (meters)
warn_depth_change_per_shot_max = 10.0   # Flag depth change over this per shot (meters)
warn_azimuth_deviation_max = 30.0       # Flag azimuth deviation over this (degrees)
azimuth_diff_threshold = 20.0           # Flag when the two (both forward) azimuth readings differ by more than this, after 360-degree wraparound (degrees)
pitch_diff_threshold = 10.0             # Flag when the two recorded inclination legs disagree by more (degrees)
inclination_depth_diff_threshold = 15.0 # Flag recorded inclination vs the one implied by length + depth change (degrees)

[closure_adjustment]
base_sigma_length_m = 0.05          # Per-shot length sigma feeding the least-squares weights
base_sigma_azimuth_deg = 1.0        # Per-shot azimuth sigma (degrees)
base_sigma_depth_m = 0.05           # Per-shot depth sigma (meters)

[panels]
sections_panel_width = 250          # Pixels (clamped to 150-400 on load)
detail_panel_width = 300            # Pixels (clamped to 200-500)
data_table_height = 200             # Pixels (clamped to 100-500)
wizard_map_panel_width = 480        # Import wizards' map column, px (clamped to 280-900)
sections_panel_open = true          # Pinned open at startup
sections_panel_mode = "pin"         # "pin" (click to open, stays open) or "hover"
detail_panel_open = true
detail_panel_mode = "pin"
data_table_open = true
data_table_mode = "pin"
# The data table's column layout is persisted here too:
# data_table_shot_column_order / data_table_station_column_order (empty = declaration
# order), data_table_shot_hidden_columns (default ["section_id", "shot_id"]),
# data_table_station_hidden_columns (default ["_x", "_y", "_lat", "_long"]),
# data_table_azimuth_mode ("avg" | "inout" | "off", default "avg") and
# data_table_inclination_mode (same values, default "off").
#
# Column widths are two sub-tables rather than scalars, so in a real file they appear
# AFTER every key above (TOML writes a table's own values before its child tables):
#
#   [panels.data_table_shot_column_widths]
#   length = 120                      # Pixels (clamped to 40-2000 on load)
#   comment = 480
#
#   [panels.data_table_station_column_widths]
#
# Only resized columns get an entry; the rest use their declared width. An empty
# table means no overrides. A junk entry costs that one column its width rather
# than failing the whole [panels] table, and nothing prunes an entry whose column
# no longer exists. At most 256 entries per table, keys up to 128 bytes.

[export]
default_image_format = "png"        # Default image format for exports
default_dpi = 300                   # Default raster export DPI
default_include_satellite = true    # Include satellite imagery
default_include_grid = false        # Include the grid overlay
default_include_labels = true       # Include labels

[logging]
level = "info"                      # trace, debug, info, warn, error
max_directory_size_mb = 128         # Log-dir cap; swept once per launch (see below)

[gas]
units = "metric"                    # "metric" (bar / litres) or "imperial" (psi / cuft)
default_reserve = "all_thirds"      # "all_thirds", "one_stage", "two_stage", or "free"
swim_speed_m_per_min = 10.0         # Exit-leg swim speed
survey_rmv_multiplier = 1.0         # Task-loading RMV bump on the survey-in leg (never below 1.0)
stressed_rmv_multiplier = 1.5       # Stressed-RMV multiplier for bailout / contingency
jump_rmv_multiplier = 1.5           # Extra RMV on jump legs; stacks multiplicatively with survey_rmv_multiplier
include_reference_legs = false      # Escape hatch: treat reference shots as swimmable again
conservative_tank_gas = true        # "Call it an 80" derate instead of precise fill x volume x Z
# survey_speed_m_per_min (unset = same speed both ways) and the dpv scooter
# profile (unset = no scooter) are optional; see `src-tauri/crates/models/src/config/gas.rs`.

[gas.reserve_profile]
stage_reserve_bar = 30.0            # Gas held back in each stage under Half-Minus
all_thirds_safety_margin_bar = 0.0  # Extra bar held back from every cylinder under All-thirds
recalc_hold_stage2_bar = 100.0      # Back-gas held pressures for the Phase-2 return-leg
recalc_hold_stage1_bar = 70.0       # recalc (carried now; that path is not implemented yet)

[map_palette]
# Depth / distance coloring palettes, chosen from the map legend's cog or the
# Export panel (see docs/color-palettes.md). Ids come from the frontend catalogue;
# an unknown id loads as-is and draws as "theme". The Levels range is session-only
# and not stored anywhere.
depth = { id = "theme", reversed = false }
distance = { id = "theme", reversed = false }
pinned = []                         # Palette ids pinned to the top of the picker

[debug.map]
show_hitboxes = false               # Show red overlay for interaction hitbox areas
log_hit_detection = false           # Log hit detection events to browser console
```

`[webkit_memory]` exists only for Linux WebKit memory-pressure tuning. It is
backend-only (kept out of the frontend config types and out of `get_config`), so
it is not documented here - see the field comment in `src-tauri/crates/core/src/config/types.rs`.

## Keyboard Shortcuts

Shortcuts are configurable via the `[shortcuts]` section. Each action supports up to two key bindings (primary and secondary) as an array of strings.

### Shortcut Format

Shortcuts use the format `Modifier+Modifier+Key`, where:

- **Modifiers** (in order): `Ctrl`, `Alt`, `Shift`
- **Keys**: Single letters (uppercase), numbers, or special key names
- **Platform note**: On macOS, `Ctrl` in config maps to the Command key; on Windows/Linux it maps to Control

**Special key names:**
- `Space`, `Enter`, `Tab`, `Escape`
- `ArrowUp`, `ArrowDown`, `ArrowLeft`, `ArrowRight`
- `Plus`, `Minus`, `Equal`
- `Backspace`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`, `Insert`
- `F1` through `F12` (function keys)

### Configuration Example

```toml
[shortcuts]
# File - project and import/export operations
new_file = ["Ctrl+N"]
open_file = ["Ctrl+O"]
save = ["Ctrl+S"]
save_as = ["Ctrl+Shift+S"]
import_dmp = ["Ctrl+I"]
export = ["Ctrl+E"]
quit = ["Ctrl+Q"]

# Tools - single letter keys for quick access
select = ["S"]
pan = ["P"]
zoom = ["Z"]
merge = ["M"]
measure = ["R"]
move_station = ["T"]
move_segment = ["V"]
move_shot = ["G"]
comment_tool = ["C"]
move_comment_tool = ["Shift+C"]

# Edit - standard modifier combinations
undo = ["Ctrl+Z"]
redo = ["Ctrl+Y", "Ctrl+Shift+Z"]   # Two bindings for redo

# View toggles
toggle_depth_gradient = ["Shift+D"]
toggle_grid = ["Shift+G"]
toggle_satellite = ["B"]
toggle_station_labels = ["L"]
toggle_shot_labels = ["Shift+L"]
toggle_section_labels = ["Ctrl+L"]
fit_to_bounds = ["F"]
zoom_in = ["Equal", "Plus"]         # = or + key
zoom_out = ["Minus"]                # - key

# Panel toggles
toggle_sections_panel = ["Ctrl+1"]
toggle_detail_panel = ["Ctrl+2"]
toggle_data_table = ["Ctrl+3"]
```

### All Configurable Actions

| Category | Action | Default | Description |
|----------|--------|---------|-------------|
| **File** | `new_file` | `Ctrl+N` | New project |
| | `open_file` | `Ctrl+O` | Open / Import (any supported file) |
| | `save` | `Ctrl+S` | Save project |
| | `save_as` | `Ctrl+Shift+S` | Save project as |
| | `import_dmp` | `Ctrl+I` | Import mNemo `.dmp` into the open project (the unified Open picker when none is open) |
| | `export` | `Ctrl+E` | Export project |
| | `quit` | `Ctrl+Q` | Quit the application |
| **Tools** | `select` | `S` | Select tool |
| | `pan` | `P` | Pan/hand tool (acts only when `map.controls.show_navigation_tools` is on) |
| | `zoom` | `Z` | Zoom tool (acts only when `map.controls.show_navigation_tools` is on) |
| | `merge` | `M` | Merge tool |
| | `measure` | `R` | Measure tool |
| | `move_station` | `T` | Move station tool |
| | `move_segment` | `V` | Move segment tool |
| | `move_shot` | `G` | Move shot tool |
| | `comment_tool` | `C` | Comment tool |
| | `move_comment_tool` | `Shift+C` | Move comment tool |
| **Edit** | `undo` | `Ctrl+Z` | Undo last action |
| | `redo` | `Ctrl+Y`, `Ctrl+Shift+Z` | Redo last undone action |
| **View** | `toggle_depth_gradient` | `Shift+D` | Toggle depth gradient coloring |
| | `toggle_grid` | `Shift+G` | Toggle grid overlay |
| | `toggle_satellite` | `B` | Toggle satellite imagery |
| | `toggle_station_labels` | `L` | Toggle station name labels |
| | `toggle_shot_labels` | `Shift+L` | Toggle shot distance labels |
| | `toggle_section_labels` | `Ctrl+L` | Toggle section name labels |
| | `fit_to_bounds` | `F` | Fit map to survey bounds |
| | `zoom_in` | `=`, `+` | Zoom in on map |
| | `zoom_out` | `-` | Zoom out on map |
| **Panels** | `toggle_sections_panel` | `Ctrl+1` | Toggle sections panel |
| | `toggle_detail_panel` | `Ctrl+2` | Toggle detail panel |
| | `toggle_data_table` | `Ctrl+3` | Toggle data table |

### Reserved Shortcuts

The following shortcuts are reserved and cannot be reassigned:

| Shortcut | Action |
|----------|--------|
| `Escape` | Close dialogs, clear selection, cancel operations |
| `Delete` / `Backspace` | Delete selected item (with confirmation) |
| `Tab` / `Enter` | UI navigation |
| `Ctrl+A` | Select all stations |
| `Ctrl+,` (`Cmd+,` on macOS) | Open Settings |
| `?` | Keyboard shortcuts overlay |

`Ctrl+=` / `Ctrl+-` / `Ctrl+0` zoom the interface in / out / reset, and the
Settings dialog lists them as reserved: `RESERVED_CTRL_KEYS` holds `Equal`,
`Plus`, `Minus`, `_` and `0` alongside `A` and `,`, so the shortcut editor
refuses to record them for an action. Only the `Ctrl` forms are blocked - the
bare `=` / `+` / `-` map-zoom defaults above stay bindable.

Two caveats on that refusal:

- **The block is deliberately broader than the zoom handler.** The listener in
  `src/hooks/useUiScale.ts` ignores Alt-modified presses, so `Ctrl+Alt+=` never
  zooms the interface - but the editor still refuses the whole `Ctrl` family for
  these keys rather than tracking the handler's modifier guards. This is the same
  conservative posture that already refuses `Ctrl+Alt+A`.
- **Enforcement covers newly recorded bindings only.** The check runs when the
  editor records a keypress; nothing re-validates bindings already persisted in
  `config.toml`. A binding saved before this enforcement existed, or hand-edited
  into the file, is not stripped on load - it stays active, and the UI-zoom
  listener fires alongside it. (Same pre-existing posture as `Ctrl+A` and
  `Ctrl+,`.)

### Shortcut Feedback

Control when toast notifications appear for shortcut actions via `general.shortcut_feedback`:

```toml
[general]
shortcut_feedback = "non_obvious"   # "none", "non_obvious", or "all"
```

| Value | Behavior |
|-------|----------|
| `"none"` | No toast notifications for any shortcuts |
| `"non_obvious"` | Toast for view/panel toggles only (default) |
| `"all"` | Toast for all shortcut actions |

### Backward Compatibility

The shortcut system accepts both the legacy single-string format and the new array format:

```toml
# Legacy format (still supported)
select = "S"

# New format (preferred)
select = ["S"]
select = ["S", "Shift+S"]  # Primary + secondary binding
```

Missing shortcut fields are filled with defaults during config loading.

## Update Track

`general.update_track` picks which updater manifest the app reads
([releasing.md](releasing.md#updater-manifests)):

| Value | Gets | Reads |
|-------|------|-------|
| `"stable"` | final releases only | `latest-stable.json` |
| `"beta"` | release candidates, then the final they lead to | `latest-beta.json` |

The default is `"stable"`, except in a build from a release candidate, which is
compiled with `STALAGMIGHT_CHANNEL=beta` and defaults to `"beta"`. Switching from
Beta back to Stable does not downgrade:
an install on a candidate stays on it until the next final.

`"weekly"` is the legacy value 0.5.0 and 0.5.1 wrote for the track Beta replaces
(see the table below). It still loads, as `"beta"`, and is written back as `"beta"`.

## Deprecated Settings

The old top-level `[snap]` block is migrated on load (`Config::migrate_legacy`),
and only while `map.snap` is still at its defaults. The `display.*` keys were
removed outright: an old config still loads, but the values are ignored rather
than carried over.

| Old Setting | Replacement | Migrated on load |
|-------------|-------------|------------------|
| `display.station_size` | `map.station.radius` | no |
| `display.line_thickness` | `map.shot.width` | no |
| `display.depth_gradient.*` (never read) | `[map_palette]` and the legend's Levels range | no |
| `snap.enabled_merge` | `map.snap.enabled_merge` | yes |
| `snap.enabled_measure` | `map.snap.enabled_measure` | yes |
| `snap.threshold_pixels` | `map.snap.threshold` | yes |
| `general.update_track = "weekly"` | `general.update_track = "beta"` | yes (read as `"beta"`, saved as `"beta"`) |

## Settings Dialog

The Settings dialog provides a UI for common configuration changes. Access it by clicking the **Settings** button in the application header. After modifying `config.toml` manually, use the **Reload Config** button in the Settings dialog to apply changes without restarting the application.

### Configuring Shortcuts via UI

1. Click the **Settings** button in the application header
2. Select the **Shortcuts** tab
3. Use the search box to filter actions by name
4. Click on a shortcut field to record a new key combination
5. Press the desired keys to set the binding
6. Press `Escape` to cancel recording, or click the clear (x) button to remove the binding
7. Click **Reset** next to an action to restore its default binding
8. Click **Reset All Shortcuts** at the bottom to restore all shortcuts to defaults

**Conflict detection**: The UI warns when a shortcut conflicts with another action or reserved shortcut. Conflicting bindings are highlighted in the UI.

## Logging

Logs are written using the `tracing` crate with `tracing-appender` for file
output, as `stalagmight.log.YYYY-MM-DD`, rotated daily.

Unlike config, the log directory is **not** under the bundle-identifier dir:
`logging::init()` runs before Tauri's path API is initialised
(`src-tauri/src/lib.rs`), so `log_directory()` takes its `directories`-crate
fallback at init time and the files land here:

| Platform | Log Path |
|----------|----------|
| Linux | `~/.local/share/stalagmight/logs/` |
| macOS | `~/Library/Application Support/com.stalagmight.Stalagmight/logs/` |
| Windows | `%LOCALAPPDATA%\stalagmight\Stalagmight\data\logs\` |

Level and the directory size cap are the `[logging]` settings above.

### Directory size cap

`max_directory_size_mb` is enforced **once per launch**: the Tauri `setup` hook
calls `logging::cleanup_logs`, which deletes oldest-first - rotated
`stalagmight.log.*` files and `crash-*.json` panic markers alike - until the
directory fits under the cap. Two deliberate softenings: the newest log file
(the one the appender currently holds open) is never deleted, so the cap can be
overshot by that one file; and the configured value is floored to 1 MB, so a
hand-edited `0` cannot wipe the directory on every launch. Unrelated files in
the directory are left alone, and a failed sweep is logged at `warn` and never
blocks startup.

The sweep targets the directory `logging::init()` recorded in `ACTIVE_LOG_DIR`,
not whatever `log_directory()` resolves to when it runs. The two differ: once
`setup` has initialised `app_dirs`, `log_directory()` starts returning the
bundle-identifier path (`<local-data>/com.stalagmight.desktop/logs`), which is
not where any log file was written.

The obsolete lockless-edit preference is ignored when loading older files and is
omitted when saving. It cannot enable editing.

## Immediate SpeleoDB connection settings

Connect saves changed preferences and authenticates immediately. Success closes
Settings and refreshes the project listing. Disconnect first runs the sign-out flow
(the open project's un-uploaded work: Discard / Keep on this computer / Upload and
disconnect; other copies with changes are kept on this computer; held locks handed
back; Try again / Disconnect anyway when a hand-back fails). Cancel there
leaves you connected with Settings open. Once it goes ahead, Disconnect takes effect immediately,
clears the remote listing and stays on the SpeleoDB Settings tab. Neither action
requires a subsequent Save. While connected, the server endpoint and both reset
controls are disabled; backend requests to change instances are also refused.
Disconnect first, including when the current server is temporarily unavailable.

The SpeleoDB tab has a Close button instead of Save/Cancel. Closing validates and
saves preference changes, including changes in other categories, without connecting.
Connection and persistence errors keep Settings open. The ordinary Settings
unsaved-changes prompt offers only Cancel (red) and Discard Changes (blue).

HTTPS is required except for HTTP to the exact host `localhost`, the exact IPv4
address `127.0.0.1`, or valid IPv4 addresses in `192.168.0.0/16`. Valid ports are
allowed. For HTTP, other private ranges, IPv6, localhost aliases/subdomains, public hosts,
missing schemes and malformed addresses are rejected, without any DNS lookup. An
endpoint the policy refuses produces an error dialog when connecting, or when saving
a new server. `insecure_http` lifts only the HTTP rule (never certificate
verification), and only in a development build ([debug gates](debug-gates.md)): there the
Connection section has an "Allow insecure HTTP" switch, locked with the server while
connected. A release build - betas and release candidates included - has no control for
it and the backend ignores the stored value; the field stays in the file so existing
configurations load in every build. Certificate verification and redirect restrictions remain.
