# Screens: Rust next to C#

Every screen with a C# reference, captured at the final audit (2026-10-09, 9e55a7c) with
`scripts/capture-scenes.sh` against loopback bench servers. Images are reduced to 900 px wide and
256 colours to keep the folder small (about 8 MB); the full-size captures are in
`.superpowers/shots/final/` and the C# references in `.superpowers/ref-shots/`.

Two things to know when comparing:

- The C# reference captures have red and blue swapped for colours set in code, so Hull's teal
  accent shows as brown or tan there. Compare layout and hue family, not exact colour.
- These captures predate the ui-chrome work (2026-10-09), when the Rust shell still had an
  in-window menu bar row. The newer comparison, against C# 731b699, is
  `.superpowers/ui-chrome/compare.html`.

How to change what you see: `docs/ui-tweaking.md`.

## Main shell

| Screen | Rust | C# | Notes |
|---|---|---|---|
| `main-hull-light` | [![main-hull-light Rust](main-hull-light-rust.png)](main-hull-light-rust.png) | [![main-hull-light C#](main-hull-light-csharp.png)](main-hull-light-csharp.png) | Rust has an in-window menu bar row; no pin and collapse icons in panel headers; saved-world buttons are words, not icons |
| `main-hull-dark` | [![main-hull-dark Rust](main-hull-dark-rust.png)](main-hull-dark-rust.png) | [![main-hull-dark C#](main-hull-dark-csharp.png)](main-hull-dark-csharp.png) | Saved-world initials avatars are low contrast in Slate |
| `main-fleet-dark` | [![main-fleet-dark Rust](main-fleet-dark-rust.png)](main-fleet-dark-rust.png) | [![main-fleet-dark C#](main-fleet-dark-csharp.png)](main-fleet-dark-csharp.png) | Menu bar row under the toolbar makes the chrome about 36 px taller |
| `main-fleet-light` | [![main-fleet-light Rust](main-fleet-light-rust.png)](main-fleet-light-rust.png) | [![main-fleet-light C#](main-fleet-light-csharp.png)](main-fleet-light-csharp.png) |  |
| `main-armored-dark` | [![main-armored-dark Rust](main-armored-dark-rust.png)](main-armored-dark-rust.png) | [![main-armored-dark C#](main-armored-dark-csharp.png)](main-armored-dark-csharp.png) |  |
| `main-armored-light` | [![main-armored-light Rust](main-armored-light-rust.png)](main-armored-light-rust.png) | [![main-armored-light C#](main-armored-light-csharp.png)](main-armored-light-csharp.png) |  |
| `main-system-dark` | [![main-system-dark Rust](main-system-dark-rust.png)](main-system-dark-rust.png) | [![main-system-dark C#](main-system-dark-csharp.png)](main-system-dark-csharp.png) |  |
| `main-system-light` | [![main-system-light Rust](main-system-light-rust.png)](main-system-light-rust.png) | [![main-system-light C#](main-system-light-csharp.png)](main-system-light-csharp.png) |  |
| `session-play` | [![session-play Rust](session-play-rust.png)](session-play-rust.png) | [![session-play C#](session-play-csharp.png)](session-play-csharp.png) | Close match. Rust keeps the Find a MUD tab open; the Hull map is light (M4 ruling) where C# is dark |
| `skin-menu` | [![skin-menu Rust](skin-menu-rust.png)](skin-menu-rust.png) | [![skin-menu C#](skin-menu-csharp.png)](skin-menu-csharp.png) |  |
| `world-theme-session` | [![world-theme-session Rust](world-theme-session-rust.png)](world-theme-session-rust.png) | [![world-theme-session C#](world-theme-session-csharp.png)](world-theme-session-csharp.png) | Theme pictures (tiled chrome, bezels) are not drawn, only colours |

## Directory

| Screen | Rust | C# | Notes |
|---|---|---|---|
| `directory` | [![directory Rust](directory-rust.png)](directory-rust.png) | [![directory C#](directory-csharp.png)](directory-csharp.png) |  |
| `world-details` | [![world-details Rust](world-details-rust.png)](world-details-rust.png) | [![world-details C#](world-details-csharp.png)](world-details-csharp.png) | Starfall shows In my worlds; C# shows Add to my worlds |

## Menus and dialogs

| Screen | Rust | C# | Notes |
|---|---|---|---|
| `menu-file` | [![menu-file Rust](menu-file-rust.png)](menu-file-rust.png) | [![menu-file C#](menu-file-csharp.png)](menu-file-csharp.png) |  |
| `menu-view` | [![menu-view Rust](menu-view-rust.png)](menu-view-rust.png) | [![menu-view C#](menu-view-csharp.png)](menu-view-csharp.png) |  |
| `menu-help` | [![menu-help Rust](menu-help-rust.png)](menu-help-rust.png) | [![menu-help C#](menu-help-csharp.png)](menu-help-csharp.png) |  |
| `about` | [![about Rust](about-rust.png)](about-rust.png) | [![about C#](about-csharp.png)](about-csharp.png) |  |
| `link-confirm` | [![link-confirm Rust](link-confirm-rust.png)](link-confirm-rust.png) | [![link-confirm C#](link-confirm-csharp.png)](link-confirm-csharp.png) |  |
| `update-notice` | [![update-notice Rust](update-notice-rust.png)](update-notice-rust.png) | [![update-notice C#](update-notice-csharp.png)](update-notice-csharp.png) | Strip sits under the menu bar, not directly under the toolbar |
| `mudlet-import` | [![mudlet-import Rust](mudlet-import-rust.png)](mudlet-import-rust.png) | [![mudlet-import C#](mudlet-import-csharp.png)](mudlet-import-csharp.png) |  |
| `mudlet-import-summary` | [![mudlet-import-summary Rust](mudlet-import-summary-rust.png)](mudlet-import-summary-rust.png) | [![mudlet-import-summary C#](mudlet-import-summary-csharp.png)](mudlet-import-summary-csharp.png) |  |
| `demo-session` | [![demo-session Rust](demo-session-rust.png)](demo-session-rust.png) | [![demo-session C#](demo-session-csharp.png)](demo-session-csharp.png) |  |

## Settings

| Screen | Rust | C# | Notes |
|---|---|---|---|
| `settings-general` | [![settings-general Rust](settings-general-rust.png)](settings-general-rust.png) | [![settings-general C#](settings-general-csharp.png)](settings-general-csharp.png) | Rust has a sixth page, Connections |
| `settings-general-de` | [![settings-general-de Rust](settings-general-de-rust.png)](settings-general-de-rust.png) | [![settings-general-de C#](settings-general-de-csharp.png)](settings-general-de-csharp.png) |  |
| `settings-general-history` | [![settings-general-history Rust](settings-general-history-rust.png)](settings-general-history-rust.png) | [![settings-general-history C#](settings-general-history-csharp.png)](settings-general-history-csharp.png) |  |
| `settings-appearance` | [![settings-appearance Rust](settings-appearance-rust.png)](settings-appearance-rust.png) | [![settings-appearance C#](settings-appearance-csharp.png)](settings-appearance-csharp.png) | Preset swatches draw faded (disabled tint); the hex values match |
| `settings-mud-colors` | [![settings-mud-colors Rust](settings-mud-colors-rust.png)](settings-mud-colors-rust.png) | [![settings-mud-colors C#](settings-mud-colors-csharp.png)](settings-mud-colors-csharp.png) | Preset swatches draw faded |
| `settings-terminal` | [![settings-terminal Rust](settings-terminal-rust.png)](settings-terminal-rust.png) | [![settings-terminal C#](settings-terminal-csharp.png)](settings-terminal-csharp.png) |  |
| `settings-input` | [![settings-input Rust](settings-input-rust.png)](settings-input-rust.png) | [![settings-input C#](settings-input-csharp.png)](settings-input-csharp.png) |  |
| `settings-scripting-lua` | [![settings-scripting-lua Rust](settings-scripting-lua-rust.png)](settings-scripting-lua-rust.png) | [![settings-scripting-lua C#](settings-scripting-lua-csharp.png)](settings-scripting-lua-csharp.png) |  |

## World editor

| Screen | Rust | C# | Notes |
|---|---|---|---|
| `world-editor-connection` | [![world-editor-connection Rust](world-editor-connection-rust.png)](world-editor-connection-rust.png) | [![world-editor-connection C#](world-editor-connection-csharp.png)](world-editor-connection-csharp.png) | Extra Reconnect automatically checkbox; port is a text field, not a spin box |
| `world-editor-login` | [![world-editor-login Rust](world-editor-login-rust.png)](world-editor-login-rust.png) | [![world-editor-login C#](world-editor-login-csharp.png)](world-editor-login-csharp.png) |  |
| `world-editor-scripts` | [![world-editor-scripts Rust](world-editor-scripts-rust.png)](world-editor-scripts-rust.png) | [![world-editor-scripts C#](world-editor-scripts-csharp.png)](world-editor-scripts-csharp.png) |  |
| `world-editor-scripts-pack` | [![world-editor-scripts-pack Rust](world-editor-scripts-pack-rust.png)](world-editor-scripts-pack-rust.png) | none | Rust only: no C# reference |
| `world-editor-macros` | [![world-editor-macros Rust](world-editor-macros-rust.png)](world-editor-macros-rust.png) | [![world-editor-macros C#](world-editor-macros-csharp.png)](world-editor-macros-csharp.png) |  |
| `world-editor-channels` | [![world-editor-channels Rust](world-editor-channels-rust.png)](world-editor-channels-rust.png) | [![world-editor-channels C#](world-editor-channels-csharp.png)](world-editor-channels-csharp.png) |  |
| `agent-settings` | [![agent-settings Rust](agent-settings-rust.png)](agent-settings-rust.png) | [![agent-settings C#](agent-settings-csharp.png)](agent-settings-csharp.png) |  |
| `agent-goals` | [![agent-goals Rust](agent-goals-rust.png)](agent-goals-rust.png) | [![agent-goals C#](agent-goals-csharp.png)](agent-goals-csharp.png) |  |

## Session

| Screen | Rust | C# | Notes |
|---|---|---|---|
| `session-footer-macros` | [![session-footer-macros Rust](session-footer-macros-rust.png)](session-footer-macros-rust.png) | [![session-footer-macros C#](session-footer-macros-csharp.png)](session-footer-macros-csharp.png) |  |
| `session-private` | [![session-private Rust](session-private-rust.png)](session-private-rust.png) | [![session-private C#](session-private-csharp.png)](session-private-csharp.png) |  |
| `session-character-title` | [![session-character-title Rust](session-character-title-rust.png)](session-character-title-rust.png) | [![session-character-title C#](session-character-title-csharp.png)](session-character-title-csharp.png) |  |
| `session-completion` | [![session-completion Rust](session-completion-rust.png)](session-completion-rust.png) | [![session-completion C#](session-completion-csharp.png)](session-completion-csharp.png) |  |
| `session-tail-split` | [![session-tail-split Rust](session-tail-split-rust.png)](session-tail-split-rust.png) | [![session-tail-split C#](session-tail-split-csharp.png)](session-tail-split-csharp.png) |  |
| `session-vitals` | [![session-vitals Rust](session-vitals-rust.png)](session-vitals-rust.png) | [![session-vitals C#](session-vitals-csharp.png)](session-vitals-csharp.png) |  |
| `server-details` | [![server-details Rust](server-details-rust.png)](server-details-rust.png) | [![server-details C#](server-details-csharp.png)](server-details-csharp.png) |  |

## Scripts

| Screen | Rust | C# | Notes |
|---|---|---|---|
| `scripts-menu` | [![scripts-menu Rust](scripts-menu-rust.png)](scripts-menu-rust.png) | [![scripts-menu C#](scripts-menu-csharp.png)](scripts-menu-csharp.png) |  |
| `script-library` | [![script-library Rust](script-library-rust.png)](script-library-rust.png) | [![script-library C#](script-library-csharp.png)](script-library-csharp.png) |  |
| `script-library-lua` | [![script-library-lua Rust](script-library-lua-rust.png)](script-library-lua-rust.png) | [![script-library-lua C#](script-library-lua-csharp.png)](script-library-lua-csharp.png) |  |
| `script-library-mudlet-item` | [![script-library-mudlet-item Rust](script-library-mudlet-item-rust.png)](script-library-mudlet-item-rust.png) | [![script-library-mudlet-item C#](script-library-mudlet-item-csharp.png)](script-library-mudlet-item-csharp.png) | Run as Lua shows (C# rule; the C# capture had Lua off) |
| `script-editor` | [![script-editor Rust](script-editor-rust.png)](script-editor-rust.png) | [![script-editor C#](script-editor-csharp.png)](script-editor-csharp.png) | Token colours follow the C# paper palette |
| `script-editor-completion` | [![script-editor-completion Rust](script-editor-completion-rust.png)](script-editor-completion-rust.png) | [![script-editor-completion C#](script-editor-completion-csharp.png)](script-editor-completion-csharp.png) |  |
| `script-panel-rail` | [![script-panel-rail Rust](script-panel-rail-rust.png)](script-panel-rail-rust.png) | [![script-panel-rail C#](script-panel-rail-csharp.png)](script-panel-rail-csharp.png) |  |
| `script-bars` | [![script-bars Rust](script-bars-rust.png)](script-bars-rust.png) | [![script-bars C#](script-bars-csharp.png)](script-bars-csharp.png) |  |

## Mapper

| Screen | Rust | C# | Notes |
|---|---|---|---|
| `mapper-full` | [![mapper-full Rust](mapper-full-rust.png)](mapper-full-rust.png) | [![mapper-full C#](mapper-full-csharp.png)](mapper-full-csharp.png) |  |
| `mapper-search` | [![mapper-search Rust](mapper-search-rust.png)](mapper-search-rust.png) | [![mapper-search C#](mapper-search-csharp.png)](mapper-search-csharp.png) | One of five matches is cut off at the right edge |
| `mapper-grid` | [![mapper-grid Rust](mapper-grid-rust.png)](mapper-grid-rust.png) | [![mapper-grid C#](mapper-grid-csharp.png)](mapper-grid-csharp.png) |  |
| `mapper-route` | [![mapper-route Rust](mapper-route-rust.png)](mapper-route-rust.png) | [![mapper-route C#](mapper-route-csharp.png)](mapper-route-csharp.png) | Session header text overlaps Save world in the narrow column |
| `mapper-tools` | [![mapper-tools Rust](mapper-tools-rust.png)](mapper-tools-rust.png) | [![mapper-tools C#](mapper-tools-csharp.png)](mapper-tools-csharp.png) |  |
| `map-editor` | [![map-editor Rust](map-editor-rust.png)](map-editor-rust.png) | [![map-editor C#](map-editor-csharp.png)](map-editor-csharp.png) |  |
| `map-room-editor` | [![map-room-editor Rust](map-room-editor-rust.png)](map-room-editor-rust.png) | [![map-room-editor C#](map-room-editor-csharp.png)](map-room-editor-csharp.png) |  |
| `map-inferred-terrain` | [![map-inferred-terrain Rust](map-inferred-terrain-rust.png)](map-inferred-terrain-rust.png) | [![map-inferred-terrain C#](map-inferred-terrain-csharp.png)](map-inferred-terrain-csharp.png) | Keyword stand-in for the model, as the C# capture |

## Channels

| Screen | Rust | C# | Notes |
|---|---|---|---|
| `channels-panel` | [![channels-panel Rust](channels-panel-rust.png)](channels-panel-rust.png) | [![channels-panel C#](channels-panel-csharp.png)](channels-panel-csharp.png) |  |
| `mark-channel-dialog` | [![mark-channel-dialog Rust](mark-channel-dialog-rust.png)](mark-channel-dialog-rust.png) | [![mark-channel-dialog C#](mark-channel-dialog-csharp.png)](mark-channel-dialog-csharp.png) |  |

## History

| Screen | Rust | C# | Notes |
|---|---|---|---|
| `history-search` | [![history-search Rust](history-search-rust.png)](history-search-rust.png) | [![history-search C#](history-search-csharp.png)](history-search-csharp.png) | The History window is drawn inside the main window in captures |
| `history-sessions` | [![history-sessions Rust](history-sessions-rust.png)](history-sessions-rust.png) | [![history-sessions C#](history-sessions-csharp.png)](history-sessions-csharp.png) |  |
| `history-notice` | [![history-notice Rust](history-notice-rust.png)](history-notice-rust.png) | [![history-notice C#](history-notice-csharp.png)](history-notice-csharp.png) |  |

## Diagnostics

| Screen | Rust | C# | Notes |
|---|---|---|---|
| `diagnostics-messages` | [![diagnostics-messages Rust](diagnostics-messages-rust.png)](diagnostics-messages-rust.png) | [![diagnostics-messages C#](diagnostics-messages-csharp.png)](diagnostics-messages-csharp.png) | First row half clipped; more chips because the bench also sends Room.Info |
| `diagnostics-observed` | [![diagnostics-observed Rust](diagnostics-observed-rust.png)](diagnostics-observed-rust.png) | [![diagnostics-observed C#](diagnostics-observed-csharp.png)](diagnostics-observed-csharp.png) |  |
| `diagnostics-console` | [![diagnostics-console Rust](diagnostics-console-rust.png)](diagnostics-console-rust.png) | [![diagnostics-console C#](diagnostics-console-csharp.png)](diagnostics-console-csharp.png) |  |

## Agents

| Screen | Rust | C# | Notes |
|---|---|---|---|
| `agent-menu` | [![agent-menu Rust](agent-menu-rust.png)](agent-menu-rust.png) | [![agent-menu C#](agent-menu-csharp.png)](agent-menu-csharp.png) | Footer status shown in full (Paused) where C# cuts it |
