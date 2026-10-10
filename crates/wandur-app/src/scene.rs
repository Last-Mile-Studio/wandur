//! Named scenes: `wandur --data-dir D --scene NAME` sets up one screen of the app (theme, saved
//! worlds, open panels, a loopback session) so it can be looked at, or, with `WANDUR_SCREENSHOT`
//! set to a `.png` path, rendered headless (wgpu without a window) and saved. Scene names follow
//! the C# reference captures in `.superpowers/ref-shots/`.
//!
//! Scenes run in memory: nothing they set up is saved to the data directory. Loopback servers
//! come from `wandur-bench` (`scripts/capture-scenes.sh` starts them):
//! - `WANDUR_SCENE_MUD` (default `127.0.0.1:4400`): `wandur-bench mud-server --page lantern`.
//! - `WANDUR_SCENE_LOGIN_MUD` (default `127.0.0.1:4401`): `wandur-bench mud-server --page login`
//!   (Starfall Reach's name and password screen, for `session-private`).
//! - `WANDUR_SCENE_JEDI_MUD` (default `127.0.0.1:4402`): `wandur-bench mud-server --page jedi`
//!   (Legends of the Jedi's one room, for `world-theme-session`).
//! - `WANDUR_DIRECTORY_URL`: `wandur-bench directory-server --fixture`.
//!
//! `WANDUR_SCENE_CENTRE` (points) sets the document area's width for the directory scenes.
//! `WANDUR_SCREENSHOT_SCALE` (default 1) sets pixels per point; `WANDUR_SCENE_TIMEOUT` (seconds,
//! default 15) caps the wait for a scene to be ready (it is captured anyway, with a warning).

use std::path::Path;
use std::time::{Duration, Instant};

use egui::{RawInput, ViewportId};
use wandur_core::Endpoint;
use wandur_core::db::scripts::LibraryEntry;
use wandur_core::l10n::{Language, S};
use wandur_core::macros::{MacroDefinition, MacroKind};
use wandur_core::settings::SavedWorld;

use crate::app::{Options, SceneAfter, SceneInput, SceneProbe, WandurApp};
use crate::diagnostics_view::DiagTab;
use crate::world_form::Section;

/// What a scene shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    /// The idle shell with Find a MUD as the document.
    Directory,
    /// One world's page in the directory.
    World(&'static str),
    /// A session to the Lantern Road bench world.
    Session,
    /// The Lantern Road, then a second session to Starfall Reach's login screen: the name sent
    /// by hand, a password typed at the password prompt (private input).
    SessionPrivate,
    /// The offline demo world, with no saved worlds.
    Demo,
    /// The Lantern Road after `wave` (the lamplighter answers) with "lamp" typed: the ghost
    /// completes "lighter".
    SessionCompletion,
    /// The Lantern Road after `lamps`, scrolled back to marker 14: the live view runs below.
    SessionTailSplit,
    /// The Lantern Road after `note`, its web address Ctrl+clicked: the "Open this link?" bar.
    SessionLink,
    /// The Lantern Road after `wisp`: a fight, the vitals strip with the opponent card.
    SessionVitals,
    /// The Lantern Road's Diagnostics page on a tab.
    SessionDiagnostics(DiagTab),
    /// The Lantern Road after `clan` (two clan lines), Mark as channel on Bastian's line.
    SessionMarkChannel,
    /// The Lantern Road, then View > Session history: searching "leaning signpost" with world,
    /// character and dates (`true`), or the Sessions tab (`false`), the first item opened.
    History(bool),
    /// The Lantern Road with Lantern watch switched on: the footer's Scripts menu open.
    SessionScripts,
    /// The Lantern Road with Lantern watch running after `wave` and `gutter`: its panel in the
    /// session rail, its oil gauge in the vitals strip.
    SessionPanels,
    /// The Lantern Road with the footer's Agent menu open: two goals, a recent decision.
    SessionAgent,
    /// The Lantern Road after `chat`: long channel lines, word wrapped (or not, for `-off`
    /// scenes; with a hanging indent for `-indent` scenes).
    SessionChat,
    /// The Lantern Road with a script pack like Legends of the Jedi's: Skills (a table), Combat
    /// and Affects panels in the session rail.
    SessionPack,
    /// A session to Legends of the Jedi, whose saved world carries the fixture world theme
    /// (`world-theme.json`), with world themes allowed.
    WorldTheme,
    /// Several sessions in the session tabs (Panels on the left, the Workspace closed).
    Tabs(TabsScene),
}

/// What a session tabs scene shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabsScene {
    /// Two sessions; the background one has new output.
    Two,
    /// Six sessions: four connected, one disconnected, one reconnecting; the second shown.
    Six,
    /// Fourteen sessions: the tabs at their narrowest, scrolled, with the arrows and the list.
    Overflow,
    /// Four sessions, two background ones with new output.
    Activity,
    /// The undo toast after deleting three rooms of the shown session's map.
    ToastRooms,
    /// The undo toast after deleting a saved world.
    ToastWorld,
}

impl Show {
    fn is_session(self) -> bool {
        matches!(
            self,
            Show::Session
                | Show::SessionPrivate
                | Show::SessionCompletion
                | Show::SessionTailSplit
                | Show::SessionLink
                | Show::SessionVitals
                | Show::SessionDiagnostics(_)
                | Show::SessionMarkChannel
                | Show::History(_)
                | Show::SessionScripts
                | Show::SessionPanels
                | Show::SessionAgent
                | Show::SessionChat
                | Show::SessionPack
                | Show::WorldTheme
                | Show::Tabs(_)
        )
    }
}

/// Something a scene's pointer goes to (`WandurApp::scene_target`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A panel's header.
    Header(crate::workspace::Tab),
    /// A panel's whole leaf.
    Leaf(crate::workspace::Tab),
    /// The leaf holding the sessions.
    Session,
    /// A saved world's row in Saved worlds.
    SavedRow(usize),
    /// The whole dock.
    Dock,
    /// The toolbar's world picker.
    Picker,
    /// The address field in the open world picker.
    PickerField,
}

/// What a scene's pointer does once the screen is ready (the C# captures' hovers and drags).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    None,
    /// Rest over a panel's header, at this fraction of its width.
    Hover(crate::workspace::Tab, f32),
    /// Right-click a saved world's row.
    RowMenu(usize),
    /// Press on a panel's header and drag it, held, onto the session panel's guide (`None`:
    /// the centre).
    DragToGuide(crate::workspace::Tab, Option<egui_dock::Split>),
    /// The same onto a window-edge guide.
    DragToEdge(crate::workspace::Tab, crate::dock_drop::WindowEdge),
    /// Open the toolbar's world picker, then click its address field (the popup stays open).
    PickerField,
}

/// The C# `ui/panel-headers` captures (`.superpowers/ref-shots-ui/`): the pointer's part.
pub fn gesture(scene: &Scene) -> Gesture {
    use crate::workspace::Tab;
    match scene.name {
        "ui-header-hover" => Gesture::Hover(Tab::Map, 0.3),
        "ui-saved-worlds-menu" => Gesture::RowMenu(1),
        "ui-world-picker" => Gesture::PickerField,
        "ui-drop-preview" => Gesture::DragToGuide(Tab::Channels, Some(egui_dock::Split::Left)),
        "ui-drop-preview-edge" => Gesture::DragToEdge(Tab::Channels, crate::dock_drop::WindowEdge::Bottom),
        _ => Gesture::None,
    }
}

/// Something open over the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    /// A menu of the menu bar, by its title.
    Menu(S),
    /// The About dialog.
    About,
    /// The settings dialog on a section (`general`, `mud-colors`, `terminal`, `input`).
    Settings(&'static str),
    /// The world editor on the first saved world, at this section.
    WorldEditor(Section),
    /// File > Import from Mudlet: the chooser.
    MudletImport,
    /// The Import finished summary of the two Mudlet fixtures.
    MudletSummary,
    /// File > Import from Wandur (C#) over the synthetic C# fixture folder: what it holds.
    CsharpImport,
    /// The title bar's Skin menu.
    SkinMenu,
}

#[derive(Clone, Copy, Debug)]
pub struct Scene {
    pub name: &'static str,
    pub about: &'static str,
    pub theme: &'static str,
    pub show: Show,
    pub overlay: Overlay,
    /// The UI language (English when `None`).
    pub language: Option<Language>,
    /// Record session history (into a database in the scene's data directory).
    pub history: bool,
    /// The window skin (Fleet, the C# captures' default, unless a scene names another).
    pub skin: &'static str,
}

const fn scene(name: &'static str, about: &'static str, theme: &'static str, show: Show) -> Scene {
    Scene {
        name,
        about,
        theme,
        show,
        overlay: Overlay::None,
        language: None,
        history: false,
        skin: "Fleet",
    }
}

const fn skinned(mut scene: Scene, skin: &'static str) -> Scene {
    scene.skin = skin;
    scene
}

const fn tabs(
    name: &'static str,
    about: &'static str,
    theme: &'static str,
    skin: &'static str,
    kind: TabsScene,
) -> Scene {
    skinned(scene(name, about, theme, Show::Tabs(kind)), skin)
}

const fn recorded(mut scene: Scene) -> Scene {
    scene.history = true;
    scene
}

const fn with(mut scene: Scene, overlay: Overlay) -> Scene {
    scene.overlay = overlay;
    scene
}

const fn in_language(mut scene: Scene, language: Language) -> Scene {
    scene.language = Some(language);
    scene
}

/// Every scene, in `--scene list` order.
pub const SCENES: &[Scene] = &[
    tabs(
        "tabs-two-system",
        "Two sessions in the session tabs, the background one with new output (System, Linen)",
        "Linen",
        "System",
        TabsScene::Two,
    ),
    tabs(
        "tabs-six-system",
        "Six sessions: connected, disconnected and reconnecting dots (System, Linen)",
        "Linen",
        "System",
        TabsScene::Six,
    ),
    tabs(
        "tabs-overflow-system",
        "Fourteen sessions: the narrowest tabs scroll, with arrows and the list (System, Linen)",
        "Linen",
        "System",
        TabsScene::Overflow,
    ),
    tabs(
        "tabs-activity-system",
        "Background sessions with new output: the accent dot (System, Linen)",
        "Linen",
        "System",
        TabsScene::Activity,
    ),
    tabs(
        "toast-world-system",
        "The undo toast after deleting a saved world (System, Linen)",
        "Linen",
        "System",
        TabsScene::ToastWorld,
    ),
    tabs(
        "tabs-two-fleet",
        "Two sessions in the session tabs, the background one with new output (Fleet, Hull)",
        "Hull",
        "Fleet",
        TabsScene::Two,
    ),
    tabs(
        "tabs-six-fleet",
        "Six sessions: connected, disconnected and reconnecting dots (Fleet, Hull)",
        "Hull",
        "Fleet",
        TabsScene::Six,
    ),
    tabs(
        "tabs-overflow-fleet",
        "Fourteen sessions: the narrowest tabs scroll, with arrows and the list (Fleet, Hull)",
        "Hull",
        "Fleet",
        TabsScene::Overflow,
    ),
    tabs(
        "tabs-activity-fleet",
        "Background sessions with new output: the accent dot (Fleet, Hull)",
        "Hull",
        "Fleet",
        TabsScene::Activity,
    ),
    tabs(
        "toast-rooms-fleet",
        "The undo toast after deleting three map rooms (Fleet, Hull)",
        "Hull",
        "Fleet",
        TabsScene::ToastRooms,
    ),
    tabs(
        "tabs-two-armored",
        "Two sessions in the session tabs, the background one with new output (Armored, Slate)",
        "Slate",
        "Armored",
        TabsScene::Two,
    ),
    tabs(
        "tabs-six-armored",
        "Six sessions: connected, disconnected and reconnecting dots (Armored, Slate)",
        "Slate",
        "Armored",
        TabsScene::Six,
    ),
    tabs(
        "tabs-overflow-armored",
        "Fourteen sessions: the narrowest tabs scroll, with arrows and the list (Armored, Slate)",
        "Slate",
        "Armored",
        TabsScene::Overflow,
    ),
    tabs(
        "tabs-activity-armored",
        "Background sessions with new output: the accent dot (Armored, Slate)",
        "Slate",
        "Armored",
        TabsScene::Activity,
    ),
    tabs(
        "toast-rooms-armored",
        "The undo toast after deleting three map rooms (Armored, Slate)",
        "Slate",
        "Armored",
        TabsScene::ToastRooms,
    ),
    skinned(
        scene(
            "reading-layout-focus-limit",
            "Focus with Limit text width (100 columns) centring the transcript (System, Linen)",
            "Linen",
            Show::SessionPack,
        ),
        "System",
    ),
    skinned(
        scene(
            "reading-layout-left",
            "Panels on the left: Map, Saved worlds and Channels in one column (System, Linen)",
            "Linen",
            Show::SessionPack,
        ),
        "System",
    ),
    skinned(
        scene(
            "reading-layout-right",
            "Panels on the right (System, Linen)",
            "Linen",
            Show::SessionPack,
        ),
        "System",
    ),
    skinned(
        scene(
            "reading-layout-both",
            "Both sides (the C# layout) (System, Linen)",
            "Linen",
            Show::SessionPack,
        ),
        "System",
    ),
    skinned(
        scene(
            "reading-layout-focus",
            "Focus: every tool panel unpinned, the transcript and its script rail (System, Linen)",
            "Linen",
            Show::SessionPack,
        ),
        "System",
    ),
    scene(
        "reading-layout-left-slate",
        "Panels on the left (Fleet, Slate)",
        "Slate",
        Show::SessionPack,
    ),
    skinned(
        scene(
            "reading-picker-linen",
            "The toolbar's world picker while a session is active (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    scene(
        "reading-picker-slate",
        "The toolbar's world picker while a session is active (Fleet, Slate)",
        "Slate",
        Show::Session,
    ),
    skinned(
        scene(
            "reading-skills-rail",
            "A Legends of the Jedi style Skills table in the session rail (System, Linen)",
            "Linen",
            Show::SessionPack,
        ),
        "System",
    ),
    scene(
        "reading-skills-rail-slate",
        "A Legends of the Jedi style Skills table in the session rail (Fleet, Slate)",
        "Slate",
        Show::SessionPack,
    ),
    skinned(
        scene(
            "reading-channels-220",
            "Channels in a 220 point column after the chat lines (System, Linen)",
            "Linen",
            Show::SessionChat,
        ),
        "System",
    ),
    skinned(
        scene(
            "reading-channels-260",
            "Channels in a 260 point column after the chat lines (System, Linen)",
            "Linen",
            Show::SessionChat,
        ),
        "System",
    ),
    skinned(
        scene(
            "reading-channels-400",
            "Channels in a 400 point column after the chat lines (System, Linen)",
            "Linen",
            Show::SessionChat,
        ),
        "System",
    ),
    scene(
        "reading-channels-260-slate",
        "Channels in a 260 point column after the chat lines (Fleet, Slate)",
        "Slate",
        Show::SessionChat,
    ),
    skinned(
        scene(
            "reading-wrap-linen",
            "Long chat lines wrapped at word boundaries (System, Linen)",
            "Linen",
            Show::SessionChat,
        ),
        "System",
    ),
    skinned(
        scene(
            "reading-wrap-linen-off",
            "Long chat lines wrapped at the last column, word wrapping off (System, Linen)",
            "Linen",
            Show::SessionChat,
        ),
        "System",
    ),
    skinned(
        scene(
            "reading-wrap-linen-indent",
            "Long chat lines word wrapped with a hanging indent (System, Linen)",
            "Linen",
            Show::SessionChat,
        ),
        "System",
    ),
    scene(
        "reading-wrap-slate",
        "Long chat lines wrapped at word boundaries (Fleet, Slate)",
        "Slate",
        Show::SessionChat,
    ),
    scene(
        "reading-wrap-slate-off",
        "Long chat lines wrapped at the last column, word wrapping off (Fleet, Slate)",
        "Slate",
        Show::SessionChat,
    ),
    skinned(
        scene(
            "ui-system-linen",
            "System skin, Linen: the owner's setup, flat headers, Saved worlds panel",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    scene(
        "ui-menu-button-windows",
        "Windows and Linux: the menu button at the top left, closed (Fleet, Hull)",
        "Hull",
        Show::Session,
    ),
    scene(
        "ui-menu-button-open-windows",
        "Windows and Linux: the menu button's dropdown, opened by a click",
        "Hull",
        Show::Session,
    ),
    scene(
        "ui-menu-button-alt-windows",
        "Windows and Linux: Alt opened the dropdown, its first item lit",
        "Hull",
        Show::Session,
    ),
    skinned(
        scene(
            "ui-menu-button-mac",
            "macOS: the menu button as the last title control (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "ui-menu-button-open-mac",
            "macOS: the menu button's dropdown (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "ui-empty-system-linen",
            "No session: Find a MUD, empty Map and Channels (System, Linen)",
            "Linen",
            Show::Directory,
        ),
        "System",
    ),
    skinned(
        scene("ui-system-midnight", "System skin, Midnight", "Midnight", Show::Session),
        "System",
    ),
    scene(
        "ui-fleet-hull",
        "Fleet skin, Hull: flat 30 point headers",
        "Hull",
        Show::Session,
    ),
    skinned(
        scene(
            "ui-armored-hull",
            "Armored skin, Hull: dock buttons always shown",
            "Hull",
            Show::Session,
        ),
        "Armored",
    ),
    skinned(
        scene(
            "ui-saved-worlds-menu",
            "Saved worlds: a row's menu open (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "ui-world-picker",
            "Toolbar world picker open, its address field clicked (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "ui-saved-worlds-find",
            "Saved worlds: Find open with a word typed (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "ui-map-header-wide",
            "Map's actions in its header (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "ui-map-header-narrow",
            "A narrow right column: Map's actions drop to a row (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "ui-header-hover",
            "The pointer over Map's header: options, pin and close show (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "ui-drop-preview",
            "Channels dragged over the session's left half: the drop preview (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "ui-drop-preview-edge",
            "Channels dragged to the window's bottom edge guide (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    scene(
        "main-hull-light",
        "Idle shell, no session, Hull theme: Find a MUD open, three saved worlds, empty map and channels",
        "Hull",
        Show::Directory,
    ),
    scene(
        "main-hull-dark",
        "The same idle shell in Slate",
        "Slate",
        Show::Directory,
    ),
    scene(
        "directory",
        "Find a MUD with the fixture catalog: search, filters, sort, cards with art",
        "Hull",
        Show::Directory,
    ),
    scene(
        "directory-dark",
        "Find a MUD in a dark theme (Fleet, Slate); WANDUR_SCENE_CENTRE=520 sets the panel width",
        "Slate",
        Show::Directory,
    ),
    scene(
        "directory-dark-filters",
        "The same with Filters open: the advanced search grid (Fleet, Slate)",
        "Slate",
        Show::Directory,
    ),
    scene(
        "directory-dark-sort-open",
        "Find a MUD with the Sort select's list open (Fleet, Slate)",
        "Slate",
        Show::Directory,
    ),
    skinned(
        scene(
            "directory-light-filters-open",
            "The advanced filters with the Genre select's list open (System, Linen)",
            "Linen",
            Show::Directory,
        ),
        "System",
    ),
    skinned(
        scene(
            "directory-light",
            "Find a MUD in a light theme (System, Linen); WANDUR_SCENE_CENTRE sets the panel width",
            "Linen",
            Show::Directory,
        ),
        "System",
    ),
    skinned(
        scene(
            "directory-light-filters",
            "The same with Filters open (System, Linen)",
            "Linen",
            Show::Directory,
        ),
        "System",
    ),
    scene(
        "world-details",
        "Starfall's page in the directory",
        "Hull",
        Show::World("starfall"),
    ),
    with(
        scene(
            "settings-general",
            "Settings, General section, over a Lantern Road session",
            "Hull",
            Show::Session,
        ),
        Overlay::Settings("general"),
    ),
    in_language(
        with(
            scene(
                "settings-general-de",
                "The same with German chosen: every label in German",
                "Hull",
                Show::Session,
            ),
            Overlay::Settings("general"),
        ),
        Language::De,
    ),
    scene(
        "session-play",
        "A live Lantern Road session: transcript, map, channels, workspace list",
        "Hull",
        Show::Session,
    ),
    with(
        scene("menu-file", "The File menu open over a session", "Hull", Show::Session),
        Overlay::Menu(S::File),
    ),
    with(
        scene("menu-view", "The View menu with its check marks", "Hull", Show::Session),
        Overlay::Menu(S::View),
    ),
    with(
        scene("menu-help", "The Help menu", "Hull", Show::Session),
        Overlay::Menu(S::Help),
    ),
    with(
        scene("about", "The About dialog over a session", "Hull", Show::Session),
        Overlay::About,
    ),
    scene(
        "demo-session",
        "The offline five-room demo world, no saved worlds",
        "Hull",
        Show::Demo,
    ),
    scene(
        "update-notice",
        "The update strip offering 0.1.6 to a 0.1.5 build (asked of the loopback directory's /client/latest), the offline demo behind it",
        "Hull",
        Show::Demo,
    ),
    with(
        scene(
            "world-editor-connection",
            "The world editor on The Lantern Road, Connection section",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Connection),
    ),
    with(
        scene(
            "world-editor-macros",
            "The world editor's Macros section: three macros, the trigger selected",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Macros),
    ),
    scene(
        "session-footer-macros",
        "The session footer with the Macros switch on (three macros saved)",
        "Hull",
        Show::Session,
    ),
    with(
        scene(
            "world-editor-login",
            "The world editor's Login section: username, saved password, auto-login",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Login),
    ),
    scene(
        "session-private",
        "A second session (Starfall Reach) at its password prompt: private input, masked command line",
        "Hull",
        Show::SessionPrivate,
    ),
    scene(
        "session-character-title",
        "Tab, Workspace row, status bar and window title named for the character (Odo)",
        "Hull",
        Show::Session,
    ),
    scene(
        "session-completion",
        "The composer with \"lamp\" typed and the ghost completion \"lighter\"",
        "Hull",
        Show::SessionCompletion,
    ),
    scene(
        "session-tail-split",
        "Scrolled back through eighty lines: transcript above, live view below the divider",
        "Hull",
        Show::SessionTailSplit,
    ),
    scene(
        "link-confirm",
        "A web address in the transcript Ctrl+clicked: the Open this link? bar",
        "Hull",
        Show::SessionLink,
    ),
    with(
        scene(
            "settings-mud-colors",
            "Settings, MUD colors: the scheme, Create a copy, the sixteen terminal colours",
            "Hull",
            Show::Session,
        ),
        Overlay::Settings("mud-colors"),
    ),
    with(
        scene(
            "settings-terminal",
            "Settings, Terminal: blinking, text size with preview, live view share, colours",
            "Hull",
            Show::Session,
        ),
        Overlay::Settings("terminal"),
    ),
    with(
        scene(
            "settings-input",
            "Settings, Input: local echo and composer suggestions",
            "Hull",
            Show::Session,
        ),
        Overlay::Settings("input"),
    ),
    scene(
        "session-vitals",
        "A fight with the marsh wisp: Health, Mana and Movement, and the opponent card under them",
        "Hull",
        Show::SessionVitals,
    ),
    scene(
        "diagnostics-messages",
        "Diagnostics, Messages: kind chips with counts, the list, a Char.Vitals message selected",
        "Hull",
        Show::SessionDiagnostics(DiagTab::Messages),
    ),
    scene(
        "diagnostics-observed",
        "Diagnostics, Observed fields: the values-free inventory and its fingerprint",
        "Hull",
        Show::SessionDiagnostics(DiagTab::Observed),
    ),
    scene(
        "diagnostics-console",
        "Diagnostics, Console: the raw stream with visible control characters and sent commands",
        "Hull",
        Show::SessionDiagnostics(DiagTab::Console),
    ),
    scene(
        "server-details",
        "Diagnostics, Server details: the world's MSSP table",
        "Hull",
        Show::SessionDiagnostics(DiagTab::Server),
    ),
    recorded(scene(
        "history-search",
        "Session history: \"leaning signpost\" with world, character and dates, the hit opened in its transcript",
        "Hull",
        Show::History(true),
    )),
    recorded(scene(
        "history-sessions",
        "Session history, Sessions tab: the recorded session opened in the transcript pane",
        "Hull",
        Show::History(false),
    )),
    recorded(scene(
        "history-notice",
        "The \"Session history is saved on this device\" strip with Don't show again, over the demo",
        "Hull",
        Show::Demo,
    )),
    scene(
        "channels-panel",
        "The Channels panel on All: tell, guild, ooc and chat with unread counts",
        "Hull",
        Show::Session,
    ),
    scene(
        "mark-channel-dialog",
        "Mark as channel on \"[Clan] Bastian: ...\": pieces, rule, channel, reply and the preview",
        "Hull",
        Show::SessionMarkChannel,
    ),
    with(
        scene(
            "world-editor-channels",
            "The world editor's Channels section with two taught rules (trade, shout)",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Channels),
    ),
    scene(
        "scripts-menu",
        "The footer's Scripts menu: Lantern watch and Guild greeter running, Marsh warning off",
        "Hull",
        Show::SessionScripts,
    ),
    scene(
        "script-panel-rail",
        "Lantern watch's panel in the session rail: two gauges, the coloured road label, Refill lantern",
        "Hull",
        Show::SessionPanels,
    ),
    scene(
        "script-bars",
        "The same frame; the vitals strip carries the script's Lantern oil gauge under the mapped vitals",
        "Hull",
        Show::SessionPanels,
    ),
    with(
        scene(
            "world-editor-scripts",
            "The world editor's Scripts section as it opens: Lantern watch selected, Disabled",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Scripts),
    ),
    with(
        scene(
            "world-editor-scripts-pack",
            "The Scripts section on a pack script: Pack and Generated, read only, the send switch",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Scripts),
    ),
    with(
        scene(
            "world-editor-scripts-pack-dense",
            "A pack script stored on one line, shown formatted (read only, the stored source unchanged)",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Scripts),
    ),
    with(
        scene(
            "world-editor-scripts-formatted",
            "An editable script stored on one line, shown formatted; nothing is unsaved",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Scripts),
    ),
    with(
        scene(
            "world-editor-scripts-unparsed",
            "A script that does not parse, shown as stored with no message",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Scripts),
    ),
    with(
        scene(
            "script-library",
            "The world editor's Scripts section: Guild greeter selected, the output panel shown",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Scripts),
    ),
    with(
        scene(
            "script-editor",
            "The code editor with the Lantern watch script, coloured, with line numbers",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Scripts),
    ),
    with(
        scene(
            "script-editor-completion",
            "The code editor with the completion list after \"mud.\"",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Scripts),
    ),
    with(
        scene(
            "mudlet-import",
            "File > Import from Mudlet: the chooser, a package going into The Lantern Road",
            "Hull",
            Show::Session,
        ),
        Overlay::MudletImport,
    ),
    with(
        scene(
            "csharp-import",
            "File > Import from Wandur (C#): the synthetic C# data folder and what it holds",
            "Hull",
            Show::Demo,
        ),
        Overlay::CsharpImport,
    ),
    with(
        scene(
            "mudlet-import-summary",
            "Import finished: the Lantern Road profile and the Gate helper package",
            "Hull",
            Show::Session,
        ),
        Overlay::MudletSummary,
    ),
    with(
        scene(
            "script-library-lua",
            "The Scripts section with Lua allowed: a Lua script, the language picker, imported Mudlet scripts",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Scripts),
    ),
    with(
        scene(
            "script-library-mudlet-item",
            "An imported Mudlet script that needs conversion, its original Lua shown, Run as Lua",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Scripts),
    ),
    with(
        scene(
            "settings-scripting-lua",
            "Settings, Input with Allow Lua scripts (prototype) switched on",
            "Hull",
            Show::Session,
        ),
        Overlay::Settings("input"),
    ),
    scene(
        "mapper-full",
        "The map widened, the whole Lantern Road map fitted, Watcher's Hill selected",
        "Hull",
        Show::Session,
    ),
    scene(
        "mapper-search",
        "The map's search box with \"lantern\" and its match count, matches ringed",
        "Hull",
        Show::Session,
    ),
    scene(
        "mapper-grid",
        "The Lantern Road in contiguous grid mode",
        "Hull",
        Show::Session,
    ),
    scene(
        "mapper-route",
        "Map tools on Route planning: a route to Hidden Grotto, its steps, Walk route",
        "Hull",
        Show::Session,
    ),
    scene(
        "mapper-tools",
        "The full map's Map tools: the session map's status, area, contiguous grid, floor",
        "Hull",
        Show::Session,
    ),
    scene(
        "map-editor",
        "The full map with Edit on: the canvas fitted, the inspector on the right",
        "Hull",
        Show::Session,
    ),
    scene(
        "map-room-editor",
        "Editing the full map with Below the Falls selected: its room details, a description and notes",
        "Hull",
        Show::Session,
    ),
    scene(
        "map-inferred-terrain",
        "Rooms with no world terrain coloured by the classifier (a keyword stand-in for the model)",
        "Hull",
        Show::Session,
    ),
    // The full map and the mini map (ui/full-map), in System with Linen and Fleet with Hull.
    skinned(
        scene(
            "full-map-play-system",
            "Play with the mini map docked on the right (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "full-map-map-system",
            "The session's Map page: the full map fitted, the newest output under it (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "full-map-edit-system",
            "The Map page with Edit on: the inspector beside the canvas (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "full-map-split-system",
            "Play and Map side by side (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "full-map-mini-system",
            "The mini map's menu: Edit map, Fit floor, Open full map, the remaining tools (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    scene(
        "full-map-play-fleet",
        "Play with the mini map docked on the right (Fleet, Hull)",
        "Hull",
        Show::Session,
    ),
    scene(
        "full-map-map-fleet",
        "The session's Map page: the full map fitted, the newest output under it (Fleet, Hull)",
        "Hull",
        Show::Session,
    ),
    scene(
        "full-map-edit-fleet",
        "The Map page with Edit on: the inspector beside the canvas (Fleet, Hull)",
        "Hull",
        Show::Session,
    ),
    scene(
        "full-map-split-fleet",
        "Play and Map side by side (Fleet, Hull)",
        "Hull",
        Show::Session,
    ),
    scene(
        "full-map-mini-fleet",
        "The mini map's menu: Edit map, Fit floor, Open full map, the remaining tools (Fleet, Hull)",
        "Hull",
        Show::Session,
    ),
    // Map import and labels (map/mudlet-json-import): the Lantern Road's Mudlet map fixture.
    scene(
        "map-import-summary",
        "File > Import map with the Lantern Road's Mudlet map read: counts, what was left out, Import",
        "Hull",
        Show::Session,
    ),
    scene(
        "map-labels-full",
        "The imported Lantern Road area on the Map page: a text label over the rooms, a picture label",
        "Hull",
        Show::Session,
    ),
    skinned(
        scene(
            "map-labels-mini-system",
            "Play with the mini map in the imported area and its labels (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    scene(
        "map-labels-mini-fleet",
        "Play with the mini map in the imported area and its labels (Fleet, Hull)",
        "Hull",
        Show::Session,
    ),
    scene(
        "map-labels-edit",
        "Editing the imported area: the text label selected, its fields in the inspector",
        "Hull",
        Show::Session,
    ),
    // The map editor (ui/map-editor): the toolbar, the inspector and the canvas tools.
    skinned(
        scene(
            "map-edit-room-system",
            "Editing the Map page: Willow Ford selected, its properties in the inspector (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    scene(
        "map-edit-room-fleet",
        "Editing the Map page: Willow Ford selected, its properties in the inspector (Fleet, Hull)",
        "Hull",
        Show::Session,
    ),
    skinned(
        scene(
            "map-edit-exit-system",
            "Editing the Map page: Willow Ford's east exit selected, its own fields (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    scene(
        "map-edit-exit-fleet",
        "Editing the Map page: Willow Ford's east exit selected, its own fields (Fleet, Hull)",
        "Hull",
        Show::Session,
    ),
    skinned(
        scene(
            "map-edit-multi-system",
            "Editing the Map page: three rooms selected, shared fields and Mixed values (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    scene(
        "map-edit-multi-fleet",
        "Editing the Map page: three rooms selected, shared fields and Mixed values (Fleet, Hull)",
        "Hull",
        Show::Session,
    ),
    skinned(
        scene(
            "map-edit-connect-system",
            "Editing the Map page: a Connect drag from Hollis Farmstead to Lantern Marsh (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    scene(
        "map-edit-connect-fleet",
        "Editing the Map page: a Connect drag from Hollis Farmstead to Lantern Marsh (Fleet, Hull)",
        "Hull",
        Show::Session,
    ),
    skinned(
        scene(
            "map-edit-menu-system",
            "Editing the Map page: the context menu on Mossy Clearing (System, Linen)",
            "Linen",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene(
            "map-edit-room-armored",
            "Editing the Map page: Willow Ford selected, its properties in the inspector (Armored, Midnight)",
            "Midnight",
            Show::Session,
        ),
        "Armored",
    ),
    scene(
        "map-edit-menu-fleet",
        "Editing the Map page: the context menu on Mossy Clearing (Fleet, Hull)",
        "Hull",
        Show::Session,
    ),
    with(
        scene(
            "settings-general-history",
            "Settings, General: the Keep history list open (30 days, 90 days, 1 year, Forever)",
            "Hull",
            Show::Session,
        ),
        Overlay::Settings("general:history"),
    ),
    with(
        scene(
            "agent-settings",
            "The world editor's Agent settings: server, provider, model, API key, Instructions tab",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Agent),
    ),
    with(
        scene(
            "agent-goals",
            "Agent settings, Goals tab: Scout the road and Keep watch at the Rest, the Markdown editor",
            "Hull",
            Show::Session,
        ),
        Overlay::WorldEditor(Section::Agent),
    ),
    scene(
        "agent-menu",
        "The footer's Agent menu: goals, Play and Stop, a recent decision, More controls",
        "Hull",
        Show::SessionAgent,
    ),
    scene(
        "main-fleet-dark",
        "A live Lantern Road session in the Fleet skin, Slate",
        "Slate",
        Show::Session,
    ),
    scene(
        "main-fleet-light",
        "A live Lantern Road session in the Fleet skin, Hull",
        "Hull",
        Show::Session,
    ),
    skinned(
        scene(
            "main-armored-dark",
            "The Armored skin (plates, rails, foot), Slate",
            "Slate",
            Show::Session,
        ),
        "Armored",
    ),
    skinned(
        scene("main-armored-light", "The Armored skin, Hull", "Hull", Show::Session),
        "Armored",
    ),
    skinned(
        scene(
            "main-system-dark",
            "The System skin (a plain toolbar in the caption area), Slate",
            "Slate",
            Show::Session,
        ),
        "System",
    ),
    skinned(
        scene("main-system-light", "The System skin, Hull", "Hull", Show::Session),
        "System",
    ),
    with(
        scene(
            "skin-menu",
            "The title bar's Skin menu open: Fleet (checked), Armored, System",
            "Hull",
            Show::Session,
        ),
        Overlay::SkinMenu,
    ),
    with(
        scene(
            "settings-appearance",
            "Settings, Appearance: skin, colour scheme, copy and delete, world themes, the colour rows",
            "Hull",
            Show::Session,
        ),
        Overlay::Settings("appearance"),
    ),
    scene(
        "world-theme-session",
        "A session on Legends of the Jedi, whose directory theme (world-theme.json) the shell takes",
        "Hull",
        Show::WorldTheme,
    ),
];

/// The world theme of the C# `world-theme-session` capture.
pub const WORLD_THEME_FIXTURE: &str = include_str!("../../wandur-core/tests/fixtures/world-theme/world-theme.json");

/// The loopback Legends of the Jedi world (`--page jedi`) `world-theme-session` connects to.
pub fn jedi_endpoint() -> Endpoint {
    std::env::var("WANDUR_SCENE_JEDI_MUD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| Endpoint::new("127.0.0.1", 4402))
}

/// The Lantern Road's protocol mapping in the C# captures (`LanternRoadSession` with the
/// reference capture's extra bindings): Char.Vitals for the three bars, Char.Status for the
/// character's name, Char.Combat for the opponent card.
pub fn lantern_mapping(endpoint: &Endpoint) -> wandur_core::protocol::mapping::WorldMapping {
    use wandur_core::protocol::mapping::{FieldBinding, FieldReference, MappingEndpoint, MappingTarget, WorldMapping};
    let bind = |package: &str,
                path: &str,
                entity: &str,
                category: &str,
                key: &str,
                member: &str,
                label: &str,
                conversion: &str| FieldBinding {
        source: FieldReference {
            protocol: "GMCP".into(),
            package: package.into(),
            path: path.into(),
        },
        target: MappingTarget {
            entity: entity.into(),
            category: category.into(),
            key: key.into(),
            member: member.into(),
        },
        label: label.into(),
        conversion: conversion.into(),
        scale: 1.0,
    };
    let vital = |path: &str, key: &str, member: &str, label: &str| {
        bind(
            "Char.Vitals",
            path,
            "character",
            "resource",
            key,
            member,
            label,
            "number",
        )
    };
    WorldMapping {
        schema_version: 1,
        world_id: "lantern-road-demo".into(),
        endpoint: MappingEndpoint {
            host: endpoint.host.clone(),
            port: i64::from(endpoint.port),
            use_tls: endpoint.tls,
        },
        schema_fingerprint: "b".repeat(64),
        revision: 1,
        generated_at: "2026-10-08T12:00:00Z".into(),
        provenance: "deterministic".into(),
        provisional: true,
        bindings: vec![
            vital("/hp", "health", "current", "Health"),
            vital("/maxhp", "health", "maximum", "Health"),
            vital("/mana", "mana", "current", "Mana"),
            vital("/maxmana", "mana", "maximum", "Mana"),
            vital("/moves", "movement", "current", "Movement"),
            vital("/maxmoves", "movement", "maximum", "Movement"),
            bind(
                "Char.Status",
                "/name",
                "character",
                "identity",
                "name",
                "value",
                "Name",
                "text",
            ),
            bind(
                "Char.Combat",
                "/enemy",
                "opponent",
                "identity",
                "name",
                "value",
                "Name",
                "text",
            ),
            bind(
                "Char.Combat",
                "/enemyhp",
                "opponent",
                "resource",
                "health",
                "current",
                "Health",
                "number",
            ),
            bind(
                "Char.Combat",
                "/enemymaxhp",
                "opponent",
                "resource",
                "health",
                "maximum",
                "Health",
                "number",
            ),
        ],
    }
}

/// The Lantern Road's world id in scenes (so a scene can store its password in the memory vault).
const LANTERN_ID: &str = "1a2b3c4d5e6f708192a3b4c5d6e7f801";
/// The password typed in the world editor scene and kept in the scene's memory vault (fixture).
const EDITOR_PASSWORD: &str = "lantern-demo-pass!";

/// The three macros the C# reference captures save for The Lantern Road.
pub fn lantern_macros() -> Vec<LibraryEntry> {
    let mut entries = vec![
        LibraryEntry::new_macro(
            "Refill when low",
            MacroDefinition::new(MacroKind::Trigger, "Your lantern gutters", "fill lantern"),
        ),
        LibraryEntry::new_macro(
            "Go to the ford",
            MacroDefinition::new(MacroKind::Alias, "ford", "east\neast\neast"),
        ),
        LibraryEntry::new_macro("Look", MacroDefinition::new(MacroKind::Shortcut, "F2", "look")),
    ];
    for entry in &mut entries {
        entry.enabled = true;
    }
    entries
}

/// The agent profile of the C# agent captures: a local LM Studio, Llama 3.1 8B, and two goals
/// for The Lantern Road, the first selected by default.
#[cfg(feature = "agent")]
pub fn lantern_agent_profile() -> wandur_core::agent::AgentProfile {
    use wandur_core::agent::AgentGoal;
    wandur_core::agent::AgentProfile {
        model: "llama-3.1-8b-instruct".into(),
        goals: vec![
            AgentGoal {
                name: "Scout the road".into(),
                rules: "- Use only look and the four directions.\n- Stop at Willow Ford.".into(),
                ..AgentGoal::new(
                    "Walk the Lantern Road east to Willow Ford, looking at each room on the way.",
                    true,
                )
            },
            AgentGoal {
                name: "Keep watch at the Rest".into(),
                rules: "- Do not leave the Rest.\n- Look around, then wait.".into(),
                ..AgentGoal::new("Stay at the Wayfarer's Rest and watch who comes and goes.", false)
            },
        ],
        ..Default::default()
    }
}

/// The help scripts of the C# `HelpScreenshotTests`, as the reference captures save them.
pub const LANTERN_WATCH: &str = r#"// Lantern watch: oil and the road ahead, beside the transcript.
const watch = mud.panel("lantern-watch", { title: "Lantern watch", dock: "right" });
const strip = mud.panel("lantern-oil", { dock: "bars" });
let oil = 8;

function draw() {
    watch.gauge("oil", { label: "Lantern oil", value: oil, max: 10, warn: 0.3 });
    watch.gauge("wick", { label: "Wick", value: 64, max: 100 });
    watch.label("road", { text: "&YRoad clear to the ford&D" });
    strip.gauge("oil", { label: "Lantern oil", value: oil, max: 10 });
}

draw();
watch.button("refill", { label: "Refill lantern", onClick: () => mud.send("fill lantern") });
mud.trigger(/^Your lantern gutters/, () => { oil = Math.max(0, oil - 1); draw(); });"#;

pub const GUILD_GREETER: &str = r#"// Wave back when a guildmate arrives at the Rest.
mud.trigger(/^(\w+) arrives from the east\.$/, match => mud.send("wave " + match[1]));"#;

pub const MARSH_WARNING: &str = r#"// Warn before walking into the marsh after dark.
mud.alias(/^marsh$/, () => mud.echo("Take a lantern: the marsh is bad tonight."));"#;

/// The three help scripts: Lantern watch (off), Guild greeter (on), Marsh warning (off).
pub fn lantern_scripts() -> Vec<LibraryEntry> {
    let script = |name: &str, source: &str, enabled: bool| LibraryEntry {
        id: wandur_core::db::scripts::new_id(),
        name: name.into(),
        source: source.into(),
        enabled,
        ..LibraryEntry::default()
    };
    vec![
        script("Lantern watch", LANTERN_WATCH, false),
        script("Guild greeter", GUILD_GREETER, true),
        script("Marsh warning", MARSH_WARNING, false),
    ]
}

/// The Lua script of the C# reference capture (its patterns are Lua's, kept as captured).
pub const LANTERN_GREETER_LUA: &str = r#"-- Greet a guildmate who reaches the Rest.
mud.trigger("^(%w+) arrives from the east%.$", function(m)
  mud.send("wave " .. m[2])
end)"#;

/// The two Mudlet fixtures (fictional, hand-written for the C# tests).
const MUDLET_PROFILE: &[u8] = include_bytes!("../../wandur-core/tests/fixtures/mudlet/lantern-road-profile.xml");
const MUDLET_PACKAGE: &[u8] = include_bytes!("../../wandur-core/tests/fixtures/mudlet/gate-helper-package.xml");

/// The Lantern Road's library after the C# capture's import: the macros, a Lua script, and the
/// profile and the package imported together; with the summary that import shows.
pub fn lantern_mudlet_library() -> (Vec<LibraryEntry>, String) {
    use wandur_core::mudlet::{importer, model::Source, parser};
    let mut library = lantern_macros();
    library.push(LibraryEntry {
        id: wandur_core::db::scripts::new_id(),
        name: "Lantern greeter (Lua)".into(),
        source: LANTERN_GREETER_LUA.into(),
        enabled: false,
        language: wandur_core::scripting::Language::Lua,
        ..LibraryEntry::default()
    });
    let packages = [MUDLET_PROFILE, MUDLET_PACKAGE]
        .into_iter()
        .filter_map(|xml| parser::parse(xml).ok())
        .collect();
    let source = Source {
        name: "The Lantern Road".into(),
        host: mud_endpoint().host,
        port: Some(mud_endpoint().port),
        packages,
        ..Source::default()
    };
    let (after, summary) = importer::apply(&source, LANTERN_ID, "The Lantern Road", true, &library);
    (after, summary.to_text(5))
}

/// A pack script written on one line, as generated pack scripts often are (fixture): the ferry
/// times, kept on a panel, asked for at the ford.
pub const FERRY_WATCH_DENSE: &str = r#"// Ferry watch at the ford, from the world's directory pack.
const ferry=mud.panel("ferry-watch",{title:"Ferry watch",dock:"right"});let times=[],next=null;function draw(){ferry.label("next",{text:next?"Next crossing at "+next:"No crossing known"});ferry.gauge("wait",{label:"Crossings seen",value:times.length,max:10,warn:0.3})}mud.trigger(/^The ferryman calls: next crossing at (\w+)\.$/,m=>{next=m[1];times.push(m[1]);if(times.length>10){times.shift()}draw()});mud.on(Events.Gmcp,e=>{if(e.package==="Room.Info"&&e.data&&e.data.name==="The Ford"){mud.send("ask ferryman crossing")}});ferry.button("ask",{label:"Ask the ferryman",onClick:()=>mud.send("ask ferryman crossing")});draw();"#;

/// A pack in the style of Legends of the Jedi's (fixture, no real pack's text): the skills
/// table, a combat panel and the affects list, in the session rail.
pub const SKILLS_PACK: &str = r#"// Skills, combat and affects panels, like a Legends of the Jedi pack.
const skills = mud.panel("skills", { title: "Skills" });
skills.table("skills", {
  columns: ["Skill", "Level", "Adept"],
  rows: [
    ["lightsaber combat", "87%", "95%"],
    ["advanced pickpocketing", "41%", "80%"],
    ["space combat 2", "64%", "90%"],
    ["mount weapon", "12%", "75%"],
    ["first aid", "100%", "100%"],
    ["droid construction and repair", "33%", "85%"],
    ["hide", "70%", "70%"],
  ],
});
const combat = mud.panel("combat", { title: "Combat" });
combat.gauge("target", { label: "Target: a dewback", value: 42, max: 100 });
combat.label("stance", { text: "Stance: &Gaggressive&w, wielding a vibro-blade" });
const affects = mud.panel("affects", { title: "Affects" });
affects.list("affects", { items: ["sanctuary (24 hours)", "bodyguard: Ilsa (3 rounds)", "force sense (until you rest)"] });
"#;

/// A script that does not parse (a parenthesis left open): shown as stored.
pub const HALF_WRITTEN: &str = r#"// Half written: the if is missing its closing parenthesis.
mud.on(Events.Line, e => {
    if (e.text.startsWith("Exits:") {
        mud.echo(e.text);
    }
});"#;

/// [`FERRY_WATCH_DENSE`] as the directory's pack would supply it.
pub fn lantern_dense_pack_script() -> LibraryEntry {
    let mut entry = LibraryEntry {
        id: wandur_core::db::scripts::pack_entry_id("lantern-road", "ferry-watch"),
        name: "Ferry watch".into(),
        source: FERRY_WATCH_DENSE.into(),
        enabled: true,
        ..LibraryEntry::default()
    };
    entry.set_pack(
        &wandur_core::db::scripts::PackInfo {
            pack_id: "ferry-watch".into(),
            provenance: wandur_core::db::scripts::PackInfo::GENERATED.into(),
            version: 1,
            description: "Keeps the ferry's crossing times on a panel.".into(),
        },
        false,
    );
    entry
}

/// A pack script as the directory would supply it for The Lantern Road (fixture): the ferry
/// times on a panel, a button that asks the ferryman.
pub fn lantern_pack_script() -> LibraryEntry {
    let mut entry = LibraryEntry {
        id: wandur_core::db::scripts::pack_entry_id("lantern-road", "ferry-times"),
        name: "Ferry times".into(),
        source: r#"// Ferry times at the ford, from the world's directory pack.
const ferry = mud.panel("ferry", { title: "Ferry", dock: "right" });
ferry.label("next", { text: "Next crossing at dusk" });
ferry.button("ask", { label: "Ask the ferryman", onClick: () => mud.send("ask ferryman crossing") });"#
            .into(),
        enabled: true,
        ..LibraryEntry::default()
    };
    entry.set_pack(
        &wandur_core::db::scripts::PackInfo {
            pack_id: "ferry-times".into(),
            provenance: wandur_core::db::scripts::PackInfo::GENERATED.into(),
            version: 1,
            description: "Shows when the ferry crosses at the ford.".into(),
        },
        false,
    );
    entry
}

/// The C# reference capture's stand-in for the room classifier: a terrain picked from words
/// in the room's text.
pub struct KeywordClassifier;

impl wandur_core::classify::RoomClassifier for KeywordClassifier {
    fn model_version(&self) -> &str {
        "fixture-1"
    }

    fn default_threshold(&self) -> f64 {
        0.5
    }

    fn classify(
        &self,
        name: &str,
        description: &str,
        _threshold: f64,
    ) -> Result<Option<wandur_core::classify::RoomEnvironmentPrediction>, String> {
        let text = format!("{name} {description}").to_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
        let pick = if has(&["cave", "grotto"]) {
            Some(("cave", 0.91))
        } else if has(&["forest", "oak", "wood"]) {
            Some(("forest", 0.87))
        } else if has(&["river", "ford"]) {
            Some(("water", 0.82))
        } else if has(&["market", "town"]) {
            Some(("city", 0.78))
        } else if has(&["road"]) {
            Some(("road", 0.74))
        } else {
            None
        };
        Ok(pick.map(
            |(environment, confidence)| wandur_core::classify::RoomEnvironmentPrediction {
                environment: environment.into(),
                confidence,
                model_version: "fixture-1".into(),
            },
        ))
    }
}

/// The C# `map-inferred-terrain` rooms: six rooms seen through GMCP with no terrain, the player
/// at Edge of Hollowwood.
pub fn inferred_terrain_rooms() -> Vec<wandur_core::map::RoomObservation> {
    use wandur_core::map::{RoomObservation, RoomSource};
    type Room<'a> = (&'a str, &'a str, &'a str, f64, f64, &'a [(&'a str, &'a str)]);
    let rooms: [Room; 6] = [
        (
            "r1",
            "Lantern Crossroads",
            "Four roads meet beneath a leaning signpost.",
            0.0,
            0.0,
            &[("east", "r2"), ("north", "r4"), ("south", "r5")],
        ),
        (
            "r2",
            "Edge of Hollowwood",
            "Tall oaks crowd the road and moss covers every stone.",
            1.0,
            0.0,
            &[("west", "r1"), ("east", "r3")],
        ),
        (
            "r3",
            "Hollowwood Deeps",
            "The forest closes in, dark pines and old trees.",
            2.0,
            0.0,
            &[("west", "r2")],
        ),
        (
            "r4",
            "Market Square",
            "Stalls and shops line the town square.",
            0.0,
            -1.0,
            &[("south", "r1")],
        ),
        (
            "r5",
            "Willow Ford",
            "Shallow water runs over stones at the river ford.",
            0.0,
            1.0,
            &[("north", "r1"), ("east", "r6")],
        ),
        (
            "r6",
            "Hidden Grotto",
            "A damp cave behind the falls, dripping with water.",
            1.0,
            1.0,
            &[("west", "r5")],
        ),
    ];
    let observe = |(id, name, description, x, y, exits): &Room| {
        let exits: Vec<(&str, Option<&str>)> = exits.iter().map(|(d, to)| (*d, Some(*to))).collect();
        let mut o = RoomObservation::new(Some(id), name, description, &exits)
            .with_source(RoomSource::Gmcp)
            .with_area("The Lantern Road");
        o.exits_provided = true;
        o.x = Some(*x);
        o.y = Some(-*y);
        o.z = Some(0.0);
        o
    };
    // The player ends at Edge of Hollowwood (observed again last).
    let mut list: Vec<RoomObservation> = rooms.iter().map(observe).collect();
    list.push(observe(&rooms[1]));
    list
}

pub fn find(name: &str) -> Option<&'static Scene> {
    SCENES.iter().find(|s| s.name == name)
}

/// The lines `--scene list` prints.
pub fn list() -> String {
    let width = SCENES.iter().map(|s| s.name.len()).max().unwrap_or(0);
    SCENES
        .iter()
        .map(|s| format!("{:width$}  {}\n", s.name, s.about))
        .collect()
}

/// The loopback MUD a session scene connects to.
pub fn mud_endpoint() -> Endpoint {
    std::env::var("WANDUR_SCENE_MUD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| Endpoint::new("127.0.0.1", 4400))
}

/// The loopback login world (`--page login`) `session-private` connects to.
pub fn login_endpoint() -> Endpoint {
    std::env::var("WANDUR_SCENE_LOGIN_MUD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| Endpoint::new("127.0.0.1", 4401))
}

fn world(name: &str, host: &str, port: u16, listing: &str) -> SavedWorld {
    SavedWorld {
        name: name.into(),
        host: host.into(),
        port,
        listing_id: listing.into(),
        ..SavedWorld::default()
    }
}

/// The folder in a scene's data directory that holds its session history while it runs.
const SCENE_HISTORY: &str = "scene-history";

/// Fill `options` for `scene`: theme, the saved worlds of the C# captures, what to open.
pub fn configure(scene: &Scene, options: &mut Options) {
    options.ephemeral = true;
    if scene.history
        && options.history_store.is_none()
        && let Some(dir) = &options.data_dir
    {
        // A scene saves nothing in its data directory itself; history needs a real database
        // (full-text search), so it gets a fresh one of its own there, removed after a capture.
        let history = dir.join(SCENE_HISTORY);
        let _ = std::fs::remove_dir_all(&history);
        if let Ok((db, _)) = wandur_core::db::Database::open(&history) {
            options.history_store = Some(std::sync::Arc::new(wandur_core::history::SqliteHistoryStore::new(db)));
        }
    }
    options.theme = Some(scene.theme.into());
    options.skin = Some(scene.skin.into());
    // reading-channels-<points>: the right column (Map over Channels) that wide.
    if let Some(width) = scene
        .name
        .strip_prefix("reading-channels-")
        .and_then(|rest| rest.split('-').next())
        .and_then(|w| w.parse::<f32>().ok())
    {
        let right = (width / ((1300.0 - 16.0) * 0.82)).clamp(0.05, 0.9);
        options.dock_shares = Some((1.0 - right, right));
    }
    if let Some(rest) = scene.name.strip_prefix("reading-layout-") {
        use crate::workspace::Preset;
        options.layout_preset = Some(match rest.split('-').next() {
            Some("left") => Preset::Left,
            Some("right") => Preset::Right,
            Some("focus") => Preset::Focus,
            _ => Preset::Both,
        });
    }
    if scene.name.ends_with("-limit") {
        options.adjust_settings = Some(|s| s.limit_text_width = true);
    }
    if scene.name.starts_with("reading-wrap") {
        options.adjust_settings = Some(if scene.name.ends_with("-off") {
            |s| s.wrap_words = false
        } else if scene.name.ends_with("-indent") {
            |s| s.wrap_indent = true
        } else {
            |s| s.wrap_words = true
        });
    }
    // The C# captures run with world themes off, except the one that shows a world's theme.
    options.use_world_themes = Some(scene.show == Show::WorldTheme);
    let mut worlds = vec![
        world("The Lantern Road", "lanternroad.example.org", 4000, ""),
        world("Starfall Reach", "starfall.example.org", 4000, "starfall"),
        world("Emberwake", "emberwake.example.org", 4000, ""),
    ];
    options.language = Some(scene.language.unwrap_or(Language::En));
    match scene.overlay {
        Overlay::None => {}
        Overlay::Menu(title) => options.open_menu = Some(title),
        Overlay::About => options.dialog = Some("about".into()),
        Overlay::Settings(section) => options.show = Some(format!("settings:{section}")),
        Overlay::WorldEditor(section) => options.world_editor = Some((0, section)),
        Overlay::MudletImport => options.dialog = Some("mudlet-import".into()),
        Overlay::MudletSummary => options.dialog = Some("mudlet-import-summary".into()),
        Overlay::CsharpImport => {
            options.dialog = Some("csharp-import".into());
            // The synthetic fixture of the importer's tests, never a real C# folder.
            options.csharp_folder = Some(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../wandur-core/tests/fixtures/csharp-data/main"),
            );
        }
        Overlay::SkinMenu => options.title_menu = Some(crate::title_bar::TitleMenu::Skin),
    }
    // The C# captures of the Mudlet and Lua screens run with Lua scripts allowed.
    if matches!(
        scene.name,
        "mudlet-import"
            | "mudlet-import-summary"
            | "script-library-lua"
            | "script-library-mudlet-item"
            | "settings-scripting-lua"
    ) {
        options.lua_scripts = Some(true);
    }
    match scene.show {
        Show::Directory => {
            options.show = Some("directory".into());
            options.directory_filters_open = scene.name.contains("-filters");
            options.directory_open_select = if scene.name.ends_with("-sort-open") {
                Some("sort")
            } else if scene.name.ends_with("-filters-open") {
                Some("genre")
            } else {
                None
            };
        }
        Show::World(id) => options.show = Some(format!("world:{id}")),
        Show::WorldTheme => {
            let jedi = options.connect.first().cloned().unwrap_or_else(jedi_endpoint);
            let theme = serde_json::from_str::<serde_json::Value>(WORLD_THEME_FIXTURE)
                .ok()
                .and_then(|v| wandur_core::directory::WorldTheme::parse(&v));
            let mut world = world("Legends of the Jedi", &jedi.host, jedi.port, "");
            world.theme = theme;
            options.connect = vec![jedi];
            options.worlds = Some(vec![world]);
            options.select_world = Some(0);
            options.font_size = Some(15.0);
            return;
        }
        Show::Demo => {
            options.demo = true;
            options.worlds = Some(Vec::new());
            options.select_world = None;
            options.font_size = Some(15.0);
            if scene.name == "update-notice" {
                // The C# capture: a 0.1.5 build told of 0.1.6. The check runs at once, against
                // the directory the scene is given (`wandur-bench directory-server --latest
                // 0.1.6`), or a fixed answer when a test passes its own source.
                options.update_version = Some("0.1.5".into());
                options.update_delay = Some(Duration::ZERO);
            }
            return;
        }
        show if show.is_session() => {
            // A test passes its own servers in `connect` (the Lantern Road, then the login world).
            let mud = options.connect.first().cloned().unwrap_or_else(mud_endpoint);
            let login = options_login(options);
            worlds[0].host = mud.host.clone();
            worlds[0].port = mud.port;
            worlds[0].world_id = LANTERN_ID.into();
            worlds[0].protocol_mapping = Some(lantern_mapping(&mud));
            // The C# captures play The Lantern Road as Odo.
            worlds[0].username = "Odo".into();
            worlds.push(world("The Verdant Roads", "verdant.example.org", 7777, ""));
            options.connect = vec![mud];
            if scene.show == Show::SessionPrivate {
                worlds[1].host = login.host.clone();
                worlds[1].port = login.port;
                options.connect.push(login);
                options.scene_input = vec![
                    SceneInput {
                        when: "known?".into(),
                        text: "lantern-demo".into(),
                        submit: true,
                        after: None,
                    },
                    SceneInput {
                        when: "Password:".into(),
                        text: "fixture-pass".into(),
                        submit: false,
                        after: None,
                    },
                ];
            }
            if scene.overlay == Overlay::WorldEditor(Section::Channels) {
                // The two rules of the C# capture.
                worlds[0].channel_rules = vec![
                    wandur_core::channels::ChannelRule::new(
                        "trade",
                        r"^\[Trade\] (?<speaker>[A-Za-z]+): (?<text>.*)$",
                        Some("trade"),
                    ),
                    wandur_core::channels::ChannelRule::new(
                        "shout",
                        r"^(?<speaker>[A-Za-z]+) shouts, '(?<text>.*)'$",
                        Some("shout"),
                    ),
                ];
            }
            if scene.overlay == Overlay::WorldEditor(Section::Login) {
                // A saved password and auto-login, as in the C# capture; the password is in the
                // scene's memory vault, so the session reads it without a notice.
                let lantern = &mut worlds[0];
                lantern.username = "lantern-demo".into();
                lantern.password_id = Some("3f2504e0-4f89-11d3-9a0c-0305e82c3301".into());
                lantern.auto_login = true;
                let vault = wandur_core::login::MemoryVault::named(wandur_core::login::vault::system().name());
                if let Ok(key) = wandur_core::login::vault::key(lantern) {
                    let _ = wandur_core::login::PasswordVault::write(&vault, &key, EDITOR_PASSWORD);
                }
                options.vault = Some(std::sync::Arc::new(vault));
                options.form_password = Some(EDITOR_PASSWORD.into());
            }
            let step = |when: &str, text: &str, submit: bool, after: Option<(&str, SceneAfter)>| SceneInput {
                when: when.into(),
                text: text.into(),
                submit,
                after: after.map(|(w, a)| (w.into(), a)),
            };
            match scene.show {
                Show::SessionCompletion => {
                    options.scene_input = vec![
                        step("Lantern Crossroads >", "wave", true, None),
                        step("nods to you", "lamp", false, None),
                    ];
                }
                Show::SessionTailSplit => {
                    options.scene_input = vec![step(
                        "Lantern Crossroads >",
                        "lamps",
                        true,
                        Some(("marker 80 of 80", SceneAfter::ScrollTo("marker 14 of 80".into()))),
                    )];
                }
                Show::SessionLink => {
                    options.scene_input = vec![step(
                        "Lantern Crossroads >",
                        "note",
                        true,
                        Some(("lanternroad.example.org/map", SceneAfter::ClickLink)),
                    )];
                }
                Show::SessionVitals => {
                    options.scene_input = vec![step("Lantern Crossroads >", "wisp", true, None)];
                }
                Show::SessionChat => {
                    options.scene_input = vec![step("Lantern Crossroads >", "chat", true, None)];
                }
                Show::SessionMarkChannel => {
                    options.scene_input = vec![step(
                        "Lantern Crossroads >",
                        "clan",
                        true,
                        Some(("bring the spyglass", SceneAfter::MarkChannel("[Clan] Bastian".into()))),
                    )];
                }
                Show::SessionDiagnostics(DiagTab::Messages) => {
                    // As the C# capture: after the fight, follow off, the last Char.Vitals selected.
                    options.scene_input = vec![step(
                        "Lantern Crossroads >",
                        "wisp",
                        true,
                        Some((
                            "[wisp: wounded]",
                            SceneAfter::Diagnostics(DiagTab::Messages, Some("Char.Vitals".into())),
                        )),
                    )];
                }
                Show::SessionDiagnostics(DiagTab::Console) => {
                    options.scene_input = vec![
                        step("Lantern Crossroads >", "wave", true, None),
                        step(
                            "nods to you",
                            "look",
                            true,
                            Some(("nods to you", SceneAfter::Diagnostics(DiagTab::Console, None))),
                        ),
                    ];
                }
                Show::History(search) => {
                    let day = |days: i64| {
                        let now = std::time::SystemTime::now();
                        let shift = Duration::from_secs(days.unsigned_abs() * 86_400);
                        crate::history_view::local_day(if days < 0 { now - shift } else { now + shift })
                    };
                    options.history_window = Some(if search {
                        crate::app::HistoryScene {
                            query: "\"leaning signpost\"".into(),
                            world: "Lantern Road".into(),
                            character: "Odo".into(),
                            from: Some(day(-7)),
                            until: Some(day(1)),
                            open_result: true,
                        }
                    } else {
                        crate::app::HistoryScene::default()
                    });
                    options.scene_input = vec![step(
                        "Lantern Crossroads >",
                        "",
                        false,
                        Some(("Lantern Crossroads >", SceneAfter::History)),
                    )];
                }
                Show::SessionScripts => {
                    options.scene_input = vec![step(
                        "Lantern Crossroads >",
                        "",
                        false,
                        Some(("Lantern Crossroads >", SceneAfter::ScriptsMenu("Lantern watch".into()))),
                    )];
                }
                Show::SessionPanels => {
                    options.scene_input = vec![
                        step("Lantern Crossroads >", "wave", true, None),
                        step("nods to you", "gutter", true, None),
                    ];
                }
                Show::SessionAgent => {
                    options.scene_input = vec![step(
                        "Lantern Crossroads >",
                        "",
                        false,
                        Some((
                            "Lantern Crossroads >",
                            SceneAfter::AgentMenu("look: Check the crossroads before choosing a road.".into()),
                        )),
                    )];
                }
                Show::SessionDiagnostics(tab) => {
                    options.scene_input = vec![step(
                        "Lantern Crossroads >",
                        "",
                        false,
                        Some(("Lantern Crossroads >", SceneAfter::Diagnostics(tab, None))),
                    )];
                }
                Show::Tabs(kind) => tabs_scene(kind, options, &mut worlds),
                _ => {}
            }
            // Saved before the C# captures of every session screen; the script screens add the
            // help scripts.
            let mut library = lantern_macros();
            if scene.show == Show::SessionScripts || scene.overlay == Overlay::WorldEditor(Section::Scripts) {
                library.splice(0..0, lantern_scripts());
            }
            if scene.show == Show::SessionPack {
                library.insert(
                    0,
                    LibraryEntry {
                        id: wandur_core::db::scripts::new_id(),
                        name: "Skills and affects".into(),
                        source: SKILLS_PACK.into(),
                        enabled: true,
                        ..LibraryEntry::default()
                    },
                );
            }
            if scene.show == Show::SessionPanels {
                let mut scripts = lantern_scripts();
                scripts[0].enabled = true;
                library.splice(0..0, scripts);
            }
            if scene.name == "world-editor-scripts-pack" {
                library.insert(0, lantern_pack_script());
            }
            if scene.name == "world-editor-scripts-pack-dense" {
                library.insert(0, lantern_dense_pack_script());
            }
            if scene.name == "world-editor-scripts-formatted" {
                library.insert(
                    0,
                    LibraryEntry {
                        name: "Ferry watch (mine)".into(),
                        source: FERRY_WATCH_DENSE.into(),
                        ..wandur_core::db::scripts::starter()
                    },
                );
            }
            if scene.name == "world-editor-scripts-unparsed" {
                library.insert(
                    0,
                    LibraryEntry {
                        name: "Half written".into(),
                        source: HALF_WRITTEN.into(),
                        ..wandur_core::db::scripts::starter()
                    },
                );
            }
            if matches!(
                scene.name,
                "mudlet-import-summary" | "script-library-lua" | "script-library-mudlet-item"
            ) {
                let (imported, summary) = lantern_mudlet_library();
                library = imported;
                options.mudlet_summary = Some(summary);
            }
            options.macros = Some(library);
            options.script_editor = match scene.name {
                "script-library" => Some(crate::app::ScriptEditorScene {
                    select: "Guild greeter".into(),
                    output: true,
                    completion_at: None,
                }),
                "world-editor-scripts" => Some(crate::app::ScriptEditorScene {
                    select: "Lantern watch".into(),
                    ..Default::default()
                }),
                "world-editor-scripts-pack" => Some(crate::app::ScriptEditorScene {
                    select: "Ferry times".into(),
                    ..Default::default()
                }),
                "world-editor-scripts-pack-dense" => Some(crate::app::ScriptEditorScene {
                    select: "Ferry watch".into(),
                    ..Default::default()
                }),
                "world-editor-scripts-formatted" => Some(crate::app::ScriptEditorScene {
                    select: "Ferry watch (mine)".into(),
                    ..Default::default()
                }),
                "world-editor-scripts-unparsed" => Some(crate::app::ScriptEditorScene {
                    select: "Half written".into(),
                    ..Default::default()
                }),
                "script-editor" => Some(crate::app::ScriptEditorScene {
                    select: "Lantern watch".into(),
                    ..Default::default()
                }),
                "script-editor-completion" => Some(crate::app::ScriptEditorScene {
                    select: "Lantern watch".into(),
                    output: false,
                    completion_at: Some("mud.trigger".into()),
                }),
                "script-library-lua" => Some(crate::app::ScriptEditorScene {
                    select: "Lantern greeter (Lua)".into(),
                    ..Default::default()
                }),
                "script-library-mudlet-item" => Some(crate::app::ScriptEditorScene {
                    select: "Combat (Mudlet, needs conversion)".into(),
                    ..Default::default()
                }),
                _ => None,
            };
            // The C# showcase's transcript size.
            options.font_size = Some(15.0);
            #[cfg(feature = "agent")]
            if scene.name.starts_with("agent-") {
                // The C# captures' agent profile, in memory; models are not looked up (the
                // capture shows the settings as they open).
                let world = wandur_core::agent::AgentWorld::Id(LANTERN_ID.into());
                options.agent_store = Some(std::sync::Arc::new(wandur_core::agent::MemoryAgentProfileStore::with(
                    &world,
                    lantern_agent_profile(),
                )));
                options.agent_no_discovery = true;
                options.agent_scene = match scene.name {
                    "agent-settings" => Some(crate::app::AgentScene::Settings(
                        crate::agent_settings::EditorTab::Instructions,
                    )),
                    "agent-goals" => Some(crate::app::AgentScene::Settings(
                        crate::agent_settings::EditorTab::Goals,
                    )),
                    _ => None,
                };
            }
            // The map screens (the C# `MapScenes`), on the session's full map (the C# captures
            // widened the docked panel instead; the full map has the centre's width).
            use crate::map_view::MapScene;
            options.map_scene = match scene.name {
                "mapper-search" => Some(MapScene::Search("lantern".into())),
                "mapper-grid" => Some(MapScene::Grid),
                "mapper-tools" => Some(MapScene::Tools),
                "mapper-route" => Some(MapScene::Route("Hidden Grotto".into())),
                "mapper-full" => Some(MapScene::Full("Watcher's Hill".into())),
                "map-inferred-terrain" => Some(MapScene::Full("Edge of Hollowwood".into())),
                _ => None,
            };
            options.session_view = match scene.name {
                "full-map-map-system" | "full-map-map-fleet" => Some(crate::app::SessionViewScene::Map),
                "full-map-split-system" | "full-map-split-fleet" => Some(crate::app::SessionViewScene::Split),
                "full-map-mini-system" | "full-map-mini-fleet" => Some(crate::app::SessionViewScene::MiniMenu),
                _ => None,
            };
            options.map_editor_scene = match scene.name {
                "map-editor" | "full-map-edit-system" | "full-map-edit-fleet" => Some(MapScene::Editor),
                "map-room-editor" => Some(MapScene::EditorRoom {
                    name: "Below the Falls".into(),
                    description:
                        "Spray hangs over a deep green pool. Behind the curtain of water, a dark gap leads west.".into(),
                    notes: "Cave entrance behind the water, west side.".into(),
                }),
                n if n.starts_with("map-edit-room-") => Some(MapScene::EditorRoom {
                    name: "Willow Ford".into(),
                    description: String::new(),
                    notes: String::new(),
                }),
                n if n.starts_with("map-edit-exit-") => Some(MapScene::EditorExit {
                    from: "Willow Ford".into(),
                    direction: "east".into(),
                }),
                n if n.starts_with("map-edit-multi-") => Some(MapScene::EditorRooms(vec![
                    "Barley Meadow".into(),
                    "Lantern Marsh".into(),
                    "Reedbank".into(),
                ])),
                n if n.starts_with("map-edit-connect-") => Some(MapScene::EditorConnect {
                    from: "Hollis Farmstead".into(),
                    toward: "Lantern Marsh".into(),
                }),
                n if n.starts_with("map-edit-menu-") => Some(MapScene::EditorMenu("Mossy Clearing".into())),
                _ => None,
            };
            options.map_import_scene = match scene.name {
                "map-import-summary" => Some(crate::app::MapImportScene::Summary),
                "map-labels-full" => Some(crate::app::MapImportScene::Full),
                "map-labels-mini-system" | "map-labels-mini-fleet" => Some(crate::app::MapImportScene::Mini),
                "map-labels-edit" => Some(crate::app::MapImportScene::EditLabel),
                _ => None,
            };
            if scene.name == "map-inferred-terrain" {
                options.room_classifier = Some(std::sync::Arc::new(KeywordClassifier));
                options.map_seed = inferred_terrain_rooms();
            }
        }
        _ => {}
    }
    if scene.name.starts_with("ui-") {
        // The saved worlds of the C# `ui/panel-headers` captures.
        worlds.truncate(1);
        worlds.extend([
            world("Starfall Reach", "starfall.example.org", 4000, "starfall"),
            world("Emberwild", "emberwild.example.org", 4000, "emberwild"),
            world("The Last Harbor", "harbor.example.org", 5000, "harbor"),
        ]);
        options.fetch_directory = true;
        if scene.name.ends_with("-windows") {
            options.platform = Some(crate::menus::Platform::Other);
        } else if scene.name.ends_with("-mac") {
            options.platform = Some(crate::menus::Platform::Mac);
        }
        options.open_menu_button = scene.name.contains("-open-") || scene.name.contains("-alt-");
        options.open_menu_with_keyboard = scene.name.contains("-alt-");
        match scene.name {
            "ui-map-header-narrow" => options.dock_shares = Some((0.705, 0.115)),
            "ui-saved-worlds-find" => options.saved_filter = Some("star".into()),
            _ => {}
        }
    }
    options.worlds = Some(worlds);
    // reading-picker-*: Emberwake was chosen in Saved worlds before the session opened.
    options.select_world = Some(if scene.name.starts_with("reading-picker") { 2 } else { 0 });
}

/// The names the session tabs scenes give their sessions (world · character).
const TAB_NAMES: [&str; 14] = [
    "The Lantern Road · Odo",
    "Starfall Reach · Vela",
    "Emberwake · Tamsin",
    "The Verdant Roads · Bram",
    "Duskmere · Pell",
    "Harrowgate · Sable",
    "Thornwick · Ada",
    "Saltmarsh · Quill",
    "Highcairn · Rook",
    "Gloamreach · Wren",
    "Copperdeep · Hale",
    "Lanternfall · Mira",
    "Riverbend · Tobin",
    "Cinderhold · Maren",
];

/// A session tabs scene: its sessions (to The Lantern Road, which `options.connect` already
/// holds, and to closed loopback ports for the disconnected and reconnecting tabs), names and
/// toast; the new-install layout, so the Workspace panel is closed.
fn tabs_scene(kind: TabsScene, options: &mut Options, worlds: &mut Vec<SavedWorld>) {
    use crate::app::{SceneTabs, SceneToast};
    let mud = options.connect.first().cloned().unwrap_or_else(mud_endpoint);
    let (lantern, broken, active, activity, toast) = match kind {
        TabsScene::Two => (2, false, 0, vec![1], None),
        TabsScene::Six => (4, true, 1, vec![], None),
        TabsScene::Overflow => (14, false, 9, vec![3, 12], None),
        TabsScene::Activity => (4, false, 0, vec![1, 3], None),
        TabsScene::ToastRooms => (2, false, 0, vec![], Some(SceneToast::Rooms(3))),
        TabsScene::ToastWorld => (2, false, 0, vec![], Some(SceneToast::World(2))),
    };
    options.connect = vec![mud; lantern];
    let mut names: Vec<String> = TAB_NAMES[..lantern].iter().map(|n| n.to_string()).collect();
    if broken {
        // Nothing listens on these loopback ports: one session stays disconnected, the other
        // keeps reconnecting.
        worlds.push(world("Moonfall Keep", "127.0.0.1", 9, ""));
        let mut ashgrove = world("Ashgrove", "127.0.0.1", 7, "");
        ashgrove.auto_reconnect = true;
        worlds.push(ashgrove);
        options.connect.push(Endpoint::new("127.0.0.1", 9));
        options.connect.push(Endpoint::new("127.0.0.1", 7));
        names.extend(["Moonfall Keep · Ilse".to_string(), "Ashgrove · Corvin".to_string()]);
    }
    options.layout_preset = Some(crate::workspace::Preset::Left);
    options.scene_tabs = Some(SceneTabs {
        names,
        active,
        activity,
        toast,
        lantern,
    });
}

/// The login world's address: the second `connect` entry a test passed, else the environment's.
fn options_login(options: &Options) -> Endpoint {
    options.connect.get(1).cloned().unwrap_or_else(login_endpoint)
}

/// Whether the scene has what it should show.
fn ready(scene: &Scene, probe: &SceneProbe) -> bool {
    let lantern = probe.session_text.contains("Lantern Crossroads >") && probe.map_rooms >= 21;
    match scene.show {
        Show::Directory | Show::World(_) => {
            (probe.catalog_worlds > 0 || probe.directory_failed) && probe.art_pending == 0
        }
        Show::Session if scene.overlay == Overlay::WorldEditor(Section::Scripts) => {
            lantern && probe.script_selected && (scene.name != "script-editor-completion" || probe.script_completion)
        }
        Show::Session if scene.name == "map-inferred-terrain" => {
            probe.session_text.contains("Lantern Crossroads >")
                && probe.map_rooms == 6
                && !probe.map_scene_pending
                && probe.inference_pending == 0
        }
        Show::Session if scene.name.starts_with("ui-") => {
            lantern
                && !probe.map_scene_pending
                && (probe.catalog_worlds > 0 || probe.directory_failed)
                && probe.art_pending == 0
        }
        Show::Session => lantern && !probe.map_scene_pending,
        Show::SessionPack => lantern && probe.rail_widgets >= 1,
        Show::SessionChat => lantern && probe.steps_done && probe.session_text.contains("draining my lantern oil"),
        Show::SessionScripts => lantern && probe.steps_done && probe.scripts_menu && probe.scripts_running >= 2,
        Show::SessionAgent => lantern && probe.steps_done && probe.agent_menu,
        Show::SessionPanels => {
            lantern
                && probe.steps_done
                && probe.rail_widgets == 4
                && probe.vitals_text.iter().any(|v| v == "Lantern oil 7 / 10")
        }
        Show::SessionPrivate => {
            lantern && probe.last_text.contains("Password:") && probe.last_private && probe.last_input > 0
        }
        Show::Demo => {
            probe.session_text.contains("Exits: north")
                && (scene.overlay != Overlay::CsharpImport || probe.csharp_import_looked)
                && (!scene.history || probe.strip_shown)
                && (scene.name != "update-notice" || probe.update_shown)
        }
        Show::SessionCompletion => lantern && probe.last_ghost.as_deref() == Some("lighter"),
        Show::SessionTailSplit => lantern && probe.steps_done && probe.last_tail_rows > 0,
        Show::SessionLink => lantern && probe.steps_done && probe.last_pending_link.is_some(),
        Show::SessionVitals => lantern && probe.steps_done && probe.last_vitals >= 4,
        Show::SessionDiagnostics(DiagTab::Server) => lantern && probe.steps_done && probe.last_server_details,
        Show::SessionDiagnostics(_) => lantern && probe.steps_done && probe.last_messages > 0,
        Show::History(_) => lantern && probe.steps_done && probe.history_ready,
        Show::SessionMarkChannel => lantern && probe.steps_done && probe.mark_open,
        Show::WorldTheme => probe.session_text.contains("Exits: north"),
        // The rooms toast scene deletes three of the map's rooms: its prompt, not the room count.
        Show::Tabs(_) => {
            probe.session_text.contains("Lantern Crossroads >") && probe.tabs_ready && !probe.map_scene_pending
        }
    }
}

/// Run frames until the scene is ready (for a few frames in a row, so textures are uploaded and
/// layout settled) or `timeout` passes. `each` sees every frame's output. Returns the last
/// output and a warning if the scene timed out.
fn settle(
    scene: &Scene,
    app: &mut WandurApp,
    ctx: &egui::Context,
    input: &RawInput,
    timeout: Duration,
    mut each: impl FnMut(&mut egui::FullOutput),
) -> (egui::FullOutput, Option<String>) {
    let started = Instant::now();
    let mut ready_frames = 0;
    let mut gestured = false;
    loop {
        let frame_start = Instant::now();
        let mut output = app.run_frame(ctx, input.clone());
        each(&mut output);
        if ready(scene, &app.scene_probe()) {
            ready_frames += 1;
        } else {
            ready_frames = 0;
        }
        if ready_frames >= 12 {
            if !gestured {
                gestured = true;
                play(gesture(scene), app, ctx, input, &mut each);
                ready_frames = 0;
                continue;
            }
            return (output, None);
        }
        if started.elapsed() > timeout {
            let warning = format!(
                "scene {} was not ready after {timeout:?}; captured as it was",
                scene.name
            );
            return (output, Some(warning));
        }
        std::thread::sleep(Duration::from_millis(16).saturating_sub(frame_start.elapsed()));
    }
}

/// Play a scene's pointer gesture, one frame per step (the pointer and a held button stay as
/// they are afterwards).
fn play(
    gesture: Gesture,
    app: &mut WandurApp,
    ctx: &egui::Context,
    input: &RawInput,
    each: &mut impl FnMut(&mut egui::FullOutput),
) {
    use egui::{Event, PointerButton, pos2};
    let mut step = |events: Vec<Event>, app: &mut WandurApp| {
        let mut frame = input.clone();
        frame.events = events;
        let mut output = app.run_frame(ctx, frame);
        each(&mut output);
        std::thread::sleep(Duration::from_millis(16));
    };
    let button = |pos, pressed, button| Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    match gesture {
        Gesture::None => {}
        Gesture::Hover(tab, x) => {
            if let Some(r) = app.scene_target(Target::Header(tab)) {
                step(
                    vec![Event::PointerMoved(pos2(r.left() + r.width() * x, r.center().y))],
                    app,
                );
            }
        }
        Gesture::RowMenu(i) => {
            if let Some(r) = app.scene_target(Target::SavedRow(i)) {
                let at = pos2(r.left() + r.width() * 0.45, r.center().y + 6.0);
                step(vec![Event::PointerMoved(at)], app);
                step(vec![button(at, true, PointerButton::Secondary)], app);
                step(vec![button(at, false, PointerButton::Secondary)], app);
            }
        }
        Gesture::PickerField => {
            let mut click = |target, app: &mut WandurApp| {
                if let Some(r) = app.scene_target(target) {
                    let at = r.center();
                    step(vec![Event::PointerMoved(at)], app);
                    step(vec![button(at, true, PointerButton::Primary)], app);
                    step(vec![button(at, false, PointerButton::Primary)], app);
                    step(vec![], app);
                }
            };
            click(Target::Picker, app);
            click(Target::PickerField, app);
        }
        Gesture::DragToGuide(..) | Gesture::DragToEdge(..) => {
            let (tab, end) = match gesture {
                Gesture::DragToGuide(tab, split) => {
                    let Some(leaf) = app.scene_target(Target::Session) else {
                        return;
                    };
                    let tiles = crate::dock_drop::compass(leaf);
                    let Some((_, tile)) = tiles.into_iter().find(|(s, _)| *s == split) else {
                        return;
                    };
                    (tab, tile.center())
                }
                Gesture::DragToEdge(tab, edge) => {
                    let Some(area) = app.scene_target(Target::Dock) else {
                        return;
                    };
                    let Some((_, tile)) = crate::dock_drop::edge_tiles(area).into_iter().find(|(e, _)| *e == edge)
                    else {
                        return;
                    };
                    (tab, tile.center())
                }
                _ => return,
            };
            let Some(from) = app.scene_target(Target::Header(tab)) else {
                return;
            };
            let start = pos2(from.left() + 60.0, from.center().y);
            step(vec![Event::PointerMoved(start)], app);
            step(vec![button(start, true, PointerButton::Primary)], app);
            for n in 1..=24 {
                let t = n as f32 / 24.0;
                step(vec![Event::PointerMoved(start + (end - start) * t)], app);
            }
        }
    }
}

/// Dock shares that make the document area about `centre` points wide in a window `window`
/// points wide (`WANDUR_SCENE_CENTRE`, for Find a MUD at a chosen width). The left column keeps
/// its 0.18; the frame and the dock's gaps take about 16 points.
pub fn centre_shares(centre: f32, window: f32) -> (f32, f32) {
    let rest = ((window - 16.0) * 0.82).max(1.0);
    let documents = (centre / rest).clamp(0.1, 0.95);
    (documents, 1.0 - documents)
}

fn scene_timeout() -> Duration {
    Duration::from_secs_f64(
        std::env::var("WANDUR_SCENE_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(15.0),
    )
}

fn scene_input(size: [f32; 2], scale: f32) -> RawInput {
    let mut input = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size.into())),
        ..Default::default()
    };
    if let Some(viewport) = input.viewports.get_mut(&ViewportId::ROOT) {
        viewport.native_pixels_per_point = Some(scale);
    }
    input
}

/// Run `scene` headless and save it to `path` as PNG. Returns a warning if it timed out. With
/// `WANDUR_A11Y_AUDIT` set to a file, the scene's AccessKit tree is checked too and every
/// control without an accessible name is appended to that file (`scene<TAB>control`).
pub fn capture(scene: &Scene, mut options: Options, size: [f32; 2], path: &Path) -> Result<Option<String>, String> {
    configure(scene, &mut options);
    if scene.show == Show::Directory
        && let Some(centre) = std::env::var("WANDUR_SCENE_CENTRE").ok().and_then(|v| v.parse().ok())
    {
        options.dock_shares = Some(centre_shares(centre, size[0]));
    }
    let history = options.data_dir.as_ref().map(|d| d.join(SCENE_HISTORY));
    let scale: f32 = std::env::var("WANDUR_SCREENSHOT_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v: &f32| (0.5..=4.0).contains(v))
        .unwrap_or(1.0);
    let audit_file = std::env::var_os("WANDUR_A11Y_AUDIT").map(std::path::PathBuf::from);
    let ctx = egui::Context::default();
    if audit_file.is_some() {
        ctx.enable_accesskit();
    }
    let mut app = WandurApp::new(&ctx, options);
    let mut renderer = egui_kittest::wgpu::WgpuTestRenderer::new();
    let input = scene_input(size, scale);
    let (output, warning) = settle(scene, &mut app, &ctx, &input, scene_timeout(), |output| {
        egui_kittest::TestRenderer::handle_delta(&mut renderer, &mut output.textures_delta);
    });
    if let (Some(file), Some(update)) = (&audit_file, &output.platform_output.accesskit_update) {
        use std::io::Write as _;
        let found = crate::a11y::unnamed(update);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
            let _ = writeln!(
                f,
                "{}\t{} nodes\t{} unnamed",
                scene.name,
                update.nodes.len(),
                found.len()
            );
            for u in found {
                let _ = writeln!(f, "{}\t{u}", scene.name);
            }
        }
    }
    let image = egui_kittest::TestRenderer::render(&mut renderer, &ctx, &output)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    image
        .save(path)
        .map_err(|e| format!("could not save {}: {e}", path.display()))?;
    app.shutdown();
    drop(app);
    if let Some(history) = history.filter(|_| scene.history) {
        let _ = std::fs::remove_dir_all(history);
    }
    Ok(warning)
}

/// What a screen reader finds in `scene` once it is ready: the number of nodes in its
/// AccessKit tree and every control without an accessible name. Nothing is rendered.
pub fn audit(
    scene: &Scene,
    mut options: Options,
    size: [f32; 2],
    timeout: Duration,
) -> (usize, Vec<crate::a11y::Unnamed>, Option<String>) {
    configure(scene, &mut options);
    let history = options.data_dir.as_ref().map(|d| d.join(SCENE_HISTORY));
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut app = WandurApp::new(&ctx, options);
    let input = scene_input(size, 1.0);
    let (output, warning) = settle(scene, &mut app, &ctx, &input, timeout, |output| {
        output.textures_delta.clear();
    });
    app.shutdown();
    drop(app);
    if let Some(history) = history.filter(|_| scene.history) {
        let _ = std::fs::remove_dir_all(history);
    }
    match &output.platform_output.accesskit_update {
        Some(update) => (update.nodes.len(), crate::a11y::unnamed(update), warning),
        None => (
            0,
            Vec::new(),
            Some(format!("scene {} gave no AccessKit tree", scene.name)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_has_a_unique_name_and_is_listed() {
        let listed = list();
        for (i, s) in SCENES.iter().enumerate() {
            assert!(listed.contains(s.name));
            assert!(SCENES[i + 1..].iter().all(|o| o.name != s.name), "{} twice", s.name);
            assert_eq!(find(s.name).map(|f| f.name), Some(s.name));
        }
        for required in [
            "main-hull-dark",
            "main-hull-light",
            "directory",
            "world-details",
            "settings-general",
            "settings-general-de",
            "session-play",
            "menu-file",
            "menu-view",
            "menu-help",
            "about",
            "demo-session",
            "update-notice",
            "world-editor-connection",
            "world-editor-macros",
            "session-footer-macros",
            "world-editor-login",
            "session-private",
            "session-character-title",
            "session-completion",
            "session-tail-split",
            "link-confirm",
            "settings-mud-colors",
            "settings-terminal",
            "settings-input",
            "session-vitals",
            "diagnostics-messages",
            "diagnostics-observed",
            "diagnostics-console",
            "server-details",
            "history-search",
            "history-sessions",
            "history-notice",
            "settings-general-history",
            "channels-panel",
            "mark-channel-dialog",
            "world-editor-channels",
            "scripts-menu",
            "script-library",
            "script-editor",
            "script-editor-completion",
            "script-panel-rail",
            "script-bars",
            "world-editor-scripts",
            "mudlet-import",
            "mudlet-import-summary",
            "script-library-lua",
            "settings-scripting-lua",
            "agent-settings",
            "agent-goals",
            "agent-menu",
            "main-fleet-dark",
            "main-fleet-light",
            "main-armored-dark",
            "main-armored-light",
            "main-system-dark",
            "main-system-light",
            "skin-menu",
            "settings-appearance",
            "world-theme-session",
        ] {
            assert!(find(required).is_some(), "{required}");
        }
        for (name, skin) in [
            ("main-armored-dark", "Armored"),
            ("main-system-light", "System"),
            ("main-fleet-dark", "Fleet"),
            ("session-play", "Fleet"),
        ] {
            let mut options = Options::default();
            configure(find(name).unwrap(), &mut options);
            assert_eq!(options.skin.as_deref(), Some(skin), "{name}");
        }
    }

    #[test]
    fn scenes_save_nothing() {
        for scene in SCENES {
            let mut options = Options::default();
            configure(scene, &mut options);
            assert!(options.ephemeral, "{}", scene.name);
            let worlds = options.worlds.as_ref().map_or(0, Vec::len);
            if scene.show == Show::Demo {
                assert_eq!(worlds, 0);
                assert!(options.demo);
            } else if scene.show == Show::WorldTheme {
                // The C# capture's one saved world, with its directory theme, world themes on.
                let world = &options.worlds.as_ref().unwrap()[0];
                assert_eq!(worlds, 1);
                assert_eq!(world.theme.as_ref().unwrap().id, "lotj-navy-cyan-gold");
                assert_eq!(options.use_world_themes, Some(true));
            } else {
                assert!(worlds >= 3, "{}", scene.name);
            }
        }
        let mut options = Options::default();
        configure(find("session-play").unwrap(), &mut options);
        assert_eq!(options.connect.len(), 1);
        assert!(options.worlds.unwrap()[0].is_at(&options.connect[0]));
    }
}
