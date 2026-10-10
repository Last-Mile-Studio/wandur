# Tweaking the UI

Where every colour, font, size, spacing, panel layout and skin lives, how to change each one
quickly, and how to see the result in a screenshot. Paths are under `crates/wandur-app/src/`
unless they say otherwise.

The short version:

1. Change a value in the file the table below names.
2. Run `scripts/capture-scenes.sh .superpowers/shots/try session-play main-fleet-dark` (or any
   scene names) and open the PNGs.
3. Compare with the C# reference of the same name in `.superpowers/ref-shots/` (or the copies in
   `docs/screens/`).

## Two kinds of change

- **No code at all.** Colours of the chrome and the terminal can be changed in the running app:
  Settings > Appearance > Create a copy makes a custom theme from any preset, and every one of its
  eighteen chrome colours and sixteen terminal colours is then editable (hex or swatch). The
  skin is chosen apart from the theme (View > Skin, or the Skin button on the title bar). Text
  size is Settings > Terminal (11 to 28 points). Custom themes are saved in `settings.json` in the
  data directory, so this is the quickest way to try a palette before putting it in code.
- **In code.** Everything below. The app is plain Rust with egui: a value changed in a source file
  shows up after a rebuild (`cargo build --release -p wandur-app`, or a debug build, which is
  quicker to build and fine for looking).

## Colours

| What | Where | How to change |
|---|---|---|
| The eleven preset themes (Hull, Ember, Moonlight, Forest, Midnight, Slate, Rose, Paper, Parchment, Daylight, Linen): shell, panel, terminal, text, muted, accent, border, map background and grid, the 16 terminal colours | `theme.rs`, `const PRESETS` (top of the file), one line per preset in `0xRRGGBB` | Edit the hex. Hull is the default (`DEFAULT_THEME` in `crates/wandur-core/src/settings.rs`). The order of `PRESETS` is the order in the menus. |
| Colours derived from a preset: terminal text on Hull, ok, warn, error, selection, the card plate, the second accent | `theme.rs`, `Theme::preset` | The mixes are written out (`mix(a, b, t)` blends two colours). For example `selection: mix(terminal, accent, 0.35)`. |
| The light map on Hull (the C# map is dark) | `theme.rs`, `Theme::preset`, the `(map_background, map_grid)` block | Delete the `if p.light && !is_light(...)` branch to get the C# dark map back. The map's own ink (route, current room, search) then follows from `map_view.rs`, `Ink::of`, which already has a dark set. |
| How egui widgets look: buttons, fields, hover and press shades, the selection, rounding of corners (4 points, windows 6) | `theme.rs`, `Theme::visuals` | Every widget state is set there from the theme's colours. Corner radius is `CornerRadius::same(4)`. |
| Skin paint: title band, toolbar, panel headers, footer, the plate, lamps, the device edge, rim highlights, title text | `skin.rs`, `Chrome::new` | Each surface is a `Gradient` built from the theme (`shaded`, `matte`, `lit`, `toward`). Hull with no world theme paints the reference metal (`theme.reference`). |
| Script code editor colours per preset (background and text) | `theme.rs`, `const EDITOR` | One pair per preset, in `PRESETS` order. Token colours (keywords, strings, numbers, `mud`) are in `script_editor.rs`, the `pick(dark, light)` calls. |
| Goal editor (Markdown) colours | `markdown_editor.rs`, the colour table at the top | Light and dark pairs per token kind. |
| Vitals bar colours (health, mana, movement and so on) | `vitals_view.rs`, `color_for` | Hex per mapped key, the C# values. |
| Map terrain colours and symbols | `map_palette.rs`, `const STYLES` | One line per terrain: key, label, colour, symbol. |
| Map ink (route, current room, glow, search highlight, grid tiles) | `map_view.rs`, `Ink::of` | A light set and a dark set, picked by the map background. |
| Channel panel colour codes | `theme.rs`, `Theme::panel_ansi` | Uses the theme's terminal palette, or Linen's or Ember's when the panel's lightness differs. |
| Contrast rule for the chrome | `skin.rs`, test `every_preset_keeps_its_chrome_legible_in_every_skin` | A colour change that drops a painted text pair below 4.5:1 fails `cargo test`. That is on purpose: fix the colour or, if you accept it, loosen the test. |

## Fonts

| What | Where | How to change |
|---|---|---|
| Terminal font (JetBrains Mono Regular and Bold, bundled) | `fonts.rs` (`REGULAR`, `BOLD`, `install`) and `crates/wandur-app/assets/fonts/` | Drop another monospace `.ttf` in `assets/fonts/` with its licence file (it must be OFL or Apache) and point `include_bytes!` at it. The grid assumes a monospace face. |
| Interface font | egui's built-in proportional font (no file of ours) | To use Inter as C# does: add `Inter-Regular.ttf`, `Inter-SemiBold.ttf` and `OFL.txt` to `assets/fonts/`, insert them in `fonts::install` with `defs.font_data.insert(...)` and put them first in `defs.families[&FontFamily::Proportional]`. A semibold weight needs a named family, as `BOLD_FAMILY` does for the terminal. |
| Terminal text size | `settings.json` `font_size` (default 15, 11 to 28), Settings > Terminal, or `wandur --font-size 14` | Default and limits: `DEFAULT_FONT_SIZE` and `FONT_SIZES` in `crates/wandur-core/src/settings.rs`. |
| Interface text sizes | Each view sets its own with `FontId::proportional(n)`: most in `world_form.rs`, `map_view.rs`, `diagnostics_view.rs`, `widgets.rs`, `terminal_view.rs` (`grep -rn "FontId::proportional" crates/wandur-app/src`) | Change the number at the place. There is no central text scale. To add one, set `ctx.all_styles_mut(|s| s.text_styles = ...)` in `Theme::apply` (`theme.rs`), which runs on every theme change, and use `TextStyle` names in the views. |
| Title on the plate | `skin.rs`, `TitleBarMetrics::FLEET` and `::ARMORED` (`title_font_size`, `title_letter_spacing`) | Change the number. |

## Sizes and spacing

| What | Where |
|---|---|
| Title band, plate, toolbar padding, title buttons for Fleet and Armored | `skin.rs`, `TitleBarMetrics::FLEET` and `::ARMORED` (each field has a comment) |
| Frame around the content, panel header height (30 in every skin, as C# since October 2026) | `skin.rs`, `WindowSkin::FLEET`, `::ARMORED`, `::SYSTEM` (`frame`, `dock_header_height`) |
| Armored rails and foot, the System toolbar height, caption button widths | `skin.rs`, the constants at the top (`ARMORED_RAIL_WIDTH`, `ARMORED_FOOT_HEIGHT`, `SYSTEM_TOOLBAR_HEIGHT`, `CAPTION_BUTTONS_WIDTH`) |
| Session tabs: the narrowest tab before the strip scrolls (120), the widest (220), padding, dot, close button, the strip's buttons, title size | `session_tabs.rs`, the constants at the top (`TAB_MIN`, `TAB_MAX`, `PAD`, `DOT`, `CLOSE`, `BUTTON`, `FONT`); the strip's height and surface follow the panel headers (`header_look` in `app.rs`) |
| The undo toast: how long it shows (7 seconds), how far above the dock's bottom (92, clear of a session's composer and footer) | `toast.rs`, `SHOWN_FOR`, `MAIN_LIFT`, `BOTTOM_GAP` |
| Composer row (46) and footer (28) heights, scrollbar width, live-view divider | `terminal_view.rs`, `COMPOSER_HEIGHT`, `FOOTER_HEIGHT`, `SCROLLBAR`, `DIVIDER` |
| Limit text width: the default (100 columns) and the range (60 to 240) | `crates/wandur-core/src/settings.rs`, `DEFAULT_TEXT_WIDTH_COLUMNS`, `TEXT_WIDTH_COLUMNS`; the centring in `terminal_view.rs`, `limit_width` |
| Word wrapping: the hanging indent (2 columns when Indent wrapped lines is on), where breaks may go, the longest logical line rewrapped (64 grid rows) | `terminal_view.rs`, `WRAP_INDENT`; `crates/wandur-term/src/wrap.rs`, `wrap_line` and `MAX_WRAP_ROWS` |
| The Map page's output strip (5 rows, 1 to 20) and Play and Map side by side (the map's share 0.5, 0.2 to 0.8; divider 5) | `terminal_view.rs`, `DEFAULT_STRIP_ROWS`, `STRIP_ROWS`, `DEFAULT_SPLIT_SHARE`, `SPLIT_SHARES`, `SPLIT_DIVIDER` |
| The mini map's starting zoom (0.6: the current room and a ring or two of neighbours) | `map_view.rs`, `MINI_ZOOM` |
| Session rail width (260 at least, wider to fit its widest table up to 420, past which table cells wrap), folded width, the gap between table columns | `panel_view.rs`, `RAIL_WIDTH`, `RAIL_MAX_WIDTH`, `COLLAPSED_WIDTH`, `TABLE_GAP`; how columns share a narrow rail: `column_widths` |
| Vitals card height | `vitals_view.rs`, `CARD_HEIGHT` |
| Map zoom range and scale | `map_view.rs`, `SCALE`, `MIN_ZOOM`, `MAX_ZOOM` |
| Gap around the dock | `app.rs`, the `CentralPanel` frame with `.inner_margin(4)` just before `DockArea::new` |
| Spacing between widgets | egui's defaults, adjusted locally with `ui.spacing_mut().item_spacing` in each view. A global change goes in `Theme::apply` with `ctx.all_styles_mut(|s| s.spacing.item_spacing = egui::vec2(8.0, 6.0))` (also `button_padding`, `interact_size`). |
| Toolbar layout and icons | `app.rs`, `fn toolbar`; icons are drawn as vectors in `widgets.rs`, `enum Icon` and `paint_icon` |
| Status bar | `shell.rs`, `status_bar` |
| World editor and Settings windows: title, default and minimum size (C# 1100 by 780, at least 780 by 560; 880 by 740, at least 740 by 560) | `world_form.rs` and `settings_dialog.rs`, the `dialog_window::Spec` in `show`; the window itself (native viewport, embedded fallback, the main window's card) in `dialog_window.rs` |
| Find a MUD: control height, gaps, corner radius, the search box's narrowest width, the filter grid's narrowest cell, the card breakpoints and heights | `directory_view.rs`, the constants at the top (`CONTROL_H`, `CONTROL_GAP`, `CONTROL_RADIUS`, `SEARCH_MIN`, `FILTER_CELL_MIN`, `STACKED_BELOW`, `SPLIT_BELOW`) and `card_geometry` |
| Selects (Find a MUD, Settings, the world editor): field colour, border, hover and focus, the list's rows | `select.rs` (`paint_field`, `Select`); the field colour is the theme's text field colour (`Theme::visuals`, `text_edit_bg_color`) |
| The map editor: toolbar height and buttons, the hint plate | `map_view/editor/toolbar.rs` (`HEIGHT`, `BUTTON`, `button`, `hint`) |
| The map editor's inspector: label column, field height, colour swatches, section headers, the exit rows | `map_view/editor/inspector.rs` (`LABEL`, `FIELD`, `SWATCHES`, `section`, `exit_row`); its width range and default in `crates/wandur-core/src/settings.rs` (`MAP_INSPECTOR_WIDTHS`, `DEFAULT_MAP_INSPECTOR_WIDTH`) |
| The map editor's canvas: selection and hover rings, the marquee, the rubber band, the context menu | `map_view.rs` (`paint`, the `edit` block) and `map_view/editor/canvas.rs` (`paint_overlay`, `context_menu`, `MENU_WIDTH`, `EXIT_REACH`) |
| Pack script notice and the read-only editor background | `world_form.rs`, `pack_notice`; `script_editor.rs`, `read_only_background` |

## Panel layout

| What | Where |
|---|---|
| The session tabs: their look (the shown tab joins the document under it with an accent line on top; others on the header surface with quiet separators) | `session_tabs.rs`, `show` |
| A new install's layout (Panels on the left: one column of 0.2, Map 0.32, Saved worlds 0.26, Channels the rest; the Workspace closed) and the other View > Layout presets | `workspace.rs`, `NEW_INSTALL`, `Preset`, `preset_layout`, `COLUMN_SHARE`, `COLUMN_ROWS` |
| The C# layout (Both sides; scenes use it): left column 0.18 (Workspace 0.35 over Saved worlds), documents 0.59, right column 0.23, Map 0.58 over Channels | `workspace.rs`, `default_layout` and `layout_with_shares` |
| Where a reopened panel goes | `workspace.rs`, `open_panel` |
| Dock tab bar look (height, line colour, separators, the narrowest panel) | `app.rs`, the `egui_dock::Style::from_egui` block just before `DockArea::new` |
| Panel headers: grip and title positions, title size, line colours, which skins hide the options, pin and close buttons | `app.rs`, `header_look` (one `HeaderLook` per skin); drawing in `panel_header.rs` (`header`, `action_row`, `RIGHT_MARGIN`, `DOCK_PITCH`, `MINIMUM_TITLE_CHARACTERS`) |
| A panel's header actions | `saved_worlds_panel::header_actions`, `map_view::header_actions` (the mini map's in the dock header, the full map's in a bar over its canvas) |
| Drop preview: tile size, fill and edge opacity, the window-edge share | `dock_drop.rs`, the constants at the top (`TILE`, `FILL_ALPHA`, `EDGE_ALPHA`, `EDGE_SHARE`) and `paint` |
| The menu button and its dropdown | `menus.rs` (`button_entries`, `MenuButton`), placed in `app.rs` (`draw_menu_button` and its callers); the macOS menu bar in `native_menu.rs` |
| Pin and auto-hide strips | `autohide.rs` (state) and `app.rs` (drawing) |
| Saved layout | `layout.json` in the data directory (version 2; version 1 files get Saved worlds under the Workspace); a preset chosen in View > Layout is saved there at once; delete it, or View > Restore Panels, to get the new-install layout again |

## Skins

The three skins (Fleet, Armored, System) are geometry in `skin.rs` (`TitleBarMetrics`,
`WindowSkin`), paint in `skin.rs` (`Chrome::new`, and the painters further down that draw the band,
plate, frame and Armored plates into baked meshes), and controls in `title_bar.rs` (Skin, palette
and full screen buttons, caption buttons on Windows and Linux, dragging). Armored's wear texture is
`assets/skins/armored-wear-384.png`; the plate icon is `assets/skins/app-icon-64.png`. Baked meshes
are rebuilt when the window size, plate or colours change, so a paint change shows on the next
frame. The skin is chosen by `settings.json` `skin` or View > Skin.

World themes from the directory replace the palette and may paint the skin (`Theme::resolve` in
`theme.rs`, `SkinPaint` in `skin.rs`); they never change geometry.

## Strings

Labels are not in the views: they come from `crates/wandur-core/locale/*.resx` (the C# tables,
byte for byte) and `Rust*.resx` (strings only this client has), generated into Rust by
`python3 scripts/generate-localization.py`. To reword a label, edit the `.resx` for each language
and run the generator; `cargo test` checks that every language has every key and that no English
text is written straight into a view.

## Script formatting

Scripts in the world editor are shown formatted. The style is set in
`crates/wandur-format/src/lib.rs`: `INDENT_WIDTH` (2), `LINE_WIDTH` (100), and the options built in
`format_js` (Biome's, which follow Prettier); `MAX_SOURCE` and `BUDGET` say when a script is shown
as stored instead. After a change, `WANDUR_BLESS=1 cargo test -p wandur-format` rewrites the golden
outputs in `crates/wandur-format/tests/golden/`; read the diff before committing. The scenes
`world-editor-scripts-pack-dense`, `world-editor-scripts-formatted` and
`world-editor-scripts-unparsed` show the result.

## Screenshots

Every screen that has a C# reference has a scene: a canned setup of the app rendered headless
(wgpu, no window) against loopback bench servers.

```sh
# All 91 scenes (about 3 minutes, release build first):
scripts/capture-scenes.sh .superpowers/shots/try

# Just the ones you are tweaking:
scripts/capture-scenes.sh .superpowers/shots/try session-play settings-appearance

# The list of scenes with a line about each:
target/release/wandur --scene list

# At another window size (the dialogs reflow to fit):
WANDUR_CAPTURE_WINDOW=1000x660 scripts/capture-scenes.sh .superpowers/shots/try world-editor-scripts

# Sharper shots (2 pixels per point, as a Retina screen):
WANDUR_SCREENSHOT_SCALE=2 scripts/capture-scenes.sh .superpowers/shots/try session-play
```

The script starts the loopback MUD pages and the fixture directory, gives each scene a throwaway
data directory under `.superpowers/scene-data/`, and fails if any request leaves loopback. The C#
references are `.superpowers/ref-shots/<scene>.png`. Note that the C# captures have red and blue
swapped for colours set in code (Hull's teal accent shows as brown there), so compare layout and
hue family rather than exact pixels.

To look at a scene in a real window and click around (it saves nothing):

```sh
target/release/wandur-bench mud-server --port 4400 --page lantern &
target/release/wandur --data-dir .superpowers/try-data --scene session-play
```

Find a MUD at a chosen panel width: `WANDUR_SCENE_CENTRE=520 WANDUR_CAPTURE_WINDOW=1300x820
scripts/capture-scenes.sh .superpowers/shots/try directory-dark directory-light-filters` (the
`directory-*` scenes; `-filters` opens the advanced filters, `-sort-open` and `-filters-open` show
a select's list).

Scenes are defined in `scene.rs` (`SCENES`, then the setup per scene); a new screen to check
needs a new entry there.

The map editor's screens: `full-map-edit-system` and `-fleet` (Edit on, nothing selected),
`map-edit-room-*`, `map-edit-exit-*`, `map-edit-multi-*`, `map-edit-connect-*` and
`map-edit-menu-*` (System with Linen and Fleet with Hull), and `map-edit-room-armored`
(Armored with Midnight). They read best at a larger window:
`WANDUR_CAPTURE_WINDOW=1600x1000 scripts/capture-scenes.sh .superpowers/shots/try map-edit-room-system`.

## Checks after a UI change

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

`cargo test` includes the contrast check, the string checks, the accessibility walk over every
scene (every control needs a name) and a headless capture of every scene. A purely visual change
should pass all of them; if the contrast test fails, the change made some text hard to read.
