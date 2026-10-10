//! The menu bar: the C# client's menus (`DesktopMenus`) in the same order, with the same labels,
//! shortcuts, check marks and enabled rules, plus the macOS application menu (`App.axaml`).
//!
//! The menus are data ([`menus`]) built from what the app knows this frame ([`MenuState`]), so
//! their order and state are testable without drawing. [`MenuButton`] draws them as the title
//! bar's menu button and dropdown (Chrome's menu; on macOS the menus are also in the menu bar,
//! `native_menu`), and [`shortcuts`] turns key presses into the same [`Command`]s whether or not
//! a menu shows. Items whose feature arrives in a later task are present and disabled.

use egui::{Align2, Color32, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, Ui, vec2};
use wandur_core::l10n::{S, t};

use crate::theme::Theme;

/// What a menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    FindAMud,
    AddWorld,
    BrowseWorlds,
    OpenDemo,
    /// File > Import from Mudlet (the chooser, then the summary).
    ImportMudlet,
    /// File > Import from Wandur (C#) (the C# client's data folder, then the summary).
    ImportCsharp,
    /// File > Import map (a Wandur map file or Mudlet's JSON export, then the summary).
    ImportMap,
    SaveTranscript,
    CloseItem,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Preferences,
    ToggleWorkspace,
    ToggleSavedWorlds,
    ToggleMap,
    ToggleChannels,
    RestorePanels,
    /// View > Layout: a preset, by its index in `workspace::Preset::ALL`.
    Layout(u8),
    ToggleToolbar,
    FocusInput,
    SessionHistory,
    /// Window skins (t15): Fleet, Armored, System.
    Skin(u8),
    ConnectSelected,
    Disconnect,
    PrivateInput,
    ClearTranscript,
    /// The active session's Map page, or back to Play (Cmd+Shift+M, Ctrl+Shift+M).
    SessionMap,
    /// The active session's Play and Map side by side.
    SessionSplit,
    Scripts,
    NextItem,
    PreviousItem,
    Minimize,
    FullScreen,
    GettingStarted,
    OtherClients,
    CheckForUpdates,
    About,
}

/// Which platform's menu layout to use (the C# client moves a few items on macOS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Other,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Other
        }
    }
}

/// A key and its modifiers. `command` is Cmd on macOS and Ctrl elsewhere (the C# `Primary`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub key: Key,
    pub command: bool,
    pub ctrl: bool,
    pub shift: bool,
}

impl Shortcut {
    const fn primary(key: Key) -> Self {
        Self {
            key,
            command: true,
            ctrl: false,
            shift: false,
        }
    }

    const fn ctrl(key: Key) -> Self {
        Self {
            key,
            command: false,
            ctrl: true,
            shift: false,
        }
    }

    const fn bare(key: Key) -> Self {
        Self {
            key,
            command: false,
            ctrl: false,
            shift: false,
        }
    }

    const fn shift(mut self) -> Self {
        self.shift = true;
        self
    }

    /// The egui modifiers on `platform`.
    pub fn modifiers(self, platform: Platform) -> Modifiers {
        let mut m = Modifiers::NONE;
        if self.command {
            m |= if platform == Platform::Mac {
                Modifiers::MAC_CMD
            } else {
                Modifiers::CTRL
            };
        }
        if self.ctrl {
            m |= Modifiers::CTRL;
        }
        if self.shift {
            m |= Modifiers::SHIFT;
        }
        m
    }

    /// As the C# menus write it: `Cmd+T` on macOS, `Ctrl+T` elsewhere.
    pub fn text(self, platform: Platform) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.command {
            parts.push(if platform == Platform::Mac { "Cmd" } else { "Ctrl" });
        }
        if self.ctrl && !(self.command && platform != Platform::Mac) {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        let key = match self.key {
            Key::Comma => ",".to_string(),
            Key::Enter => "Enter".to_string(),
            other => other.name().to_string(),
        };
        let mut text = parts.join("+");
        if !text.is_empty() {
            text.push('+');
        }
        text + &key
    }
}

/// One menu item.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub label: S,
    pub command: Command,
    pub shortcut: Option<Shortcut>,
    /// Whether the shortcut is bound by the window (edit shortcuts belong to text fields).
    pub bind: bool,
    pub enabled: bool,
    /// `Some` for a toggle item, with its state.
    pub checked: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Item(Item),
    Separator,
    Submenu { label: S, entries: Vec<Entry> },
}

/// A menu's title: a string key, or the app's name (the macOS application menu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Title {
    Key(S),
    App,
}

impl Title {
    pub fn text(self) -> &'static str {
        match self {
            Title::Key(key) => t(key),
            Title::App => "Wandur",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub title: Title,
    pub entries: Vec<Entry>,
}

/// What the menus need to know this frame.
#[derive(Clone, Debug, Default)]
pub struct MenuState {
    /// The active session's transcript has text (Save and Clear Transcript).
    pub has_text: bool,
    /// A saved world is selected (Connect to Selected World).
    pub world_selected: bool,
    /// The active session is connected or connecting (Disconnect).
    pub can_disconnect: bool,
    /// A text field had the keyboard focus (Undo, Redo, Cut, Paste).
    pub text_target: bool,
    /// Copy has something: a text field or a transcript selection.
    pub can_copy: bool,
    /// Select All has a target: a text field or a transcript.
    pub can_select_all: bool,
    pub workspace_visible: bool,
    pub saved_worlds_visible: bool,
    pub map_visible: bool,
    pub channels_visible: bool,
    pub toolbar_visible: bool,
    /// More than one workspace view to move between (Find a MUD and the sessions).
    pub several_items: bool,
    pub full_screen: bool,
    /// The active session is connected (Private Input).
    pub can_private: bool,
    /// The active session's Private Input is on.
    pub manual_private: bool,
    /// Session history can be opened (the database is in use).
    pub has_history: bool,
    /// The active session belongs to a saved world (Scripts...).
    pub has_scripts: bool,
    /// The window skin: 0 Fleet, 1 Armored, 2 System (View > Skin).
    pub skin: u8,
    /// A session is the shown document (Show Map, side by side).
    pub has_session: bool,
    /// The active session shows its Map page.
    pub session_map: bool,
    /// The active session shows Play and Map side by side.
    pub session_split: bool,
    /// The active session's map editor is shown with an edit to undo (and to redo).
    pub map_undo: bool,
    pub map_redo: bool,
}

fn item(label: S, command: Command) -> Item {
    Item {
        label,
        command,
        shortcut: None,
        bind: true,
        enabled: true,
        checked: None,
    }
}

impl Item {
    fn key(mut self, shortcut: Shortcut) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    fn checked(mut self, on: bool) -> Self {
        self.checked = Some(on);
        self
    }

    fn unbound(mut self) -> Self {
        self.bind = false;
        self
    }

    fn entry(self) -> Entry {
        Entry::Item(self)
    }
}

/// The menus in the C# order for `platform`.
pub fn menus(platform: Platform, s: &MenuState) -> Vec<Menu> {
    use Command as C;
    let mac = platform == Platform::Mac;
    let mut list = Vec::new();
    if mac {
        // App.axaml: About, Check for Updates, separator, Preferences; Quit as macOS adds it.
        list.push(Menu {
            title: Title::App,
            entries: vec![
                item(S::AboutWandur, C::About).entry(),
                item(S::CheckForUpdatesMenu, C::CheckForUpdates).entry(),
                Entry::Separator,
                item(S::Preferences, C::Preferences)
                    .key(Shortcut::primary(Key::Comma))
                    .entry(),
                Entry::Separator,
                item(S::QuitWandur, C::Quit).key(Shortcut::primary(Key::Q)).entry(),
            ],
        });
    }
    let mut file = vec![
        item(S::FindAMUD, C::FindAMud).key(Shortcut::primary(Key::T)).entry(),
        item(S::AddWorld2, C::AddWorld).key(Shortcut::primary(Key::N)).entry(),
        item(S::BrowseWorlds, C::BrowseWorlds).entry(),
        item(S::OpenOfflineDemo, C::OpenDemo).entry(),
        Entry::Separator,
        item(S::MudletImportMenu, C::ImportMudlet).entry(),
        item(S::CsImportMenu, C::ImportCsharp).entry(),
        item(S::MapImport, C::ImportMap).entry(),
        item(S::SaveTranscript, C::SaveTranscript)
            .key(Shortcut::primary(Key::S))
            .enabled(s.has_text)
            .entry(),
        Entry::Separator,
        item(S::CloseWorkspaceItem, C::CloseItem)
            .key(Shortcut::primary(Key::W))
            .entry(),
    ];
    if !mac {
        file.extend([Entry::Separator, item(S::Exit, C::Quit).entry()]);
    }
    list.push(Menu {
        title: Title::Key(S::File),
        entries: file,
    });

    let mut edit = vec![
        item(S::Undo, C::Undo)
            .key(Shortcut::primary(Key::Z))
            .enabled(s.text_target || s.map_undo)
            .unbound()
            .entry(),
        item(S::Redo, C::Redo)
            .key(Shortcut::primary(Key::Z).shift())
            .enabled(s.text_target || s.map_redo)
            .unbound()
            .entry(),
        Entry::Separator,
        item(S::Cut, C::Cut)
            .key(Shortcut::primary(Key::X))
            .enabled(s.text_target)
            .unbound()
            .entry(),
        item(S::Copy, C::Copy)
            .key(Shortcut::primary(Key::C))
            .enabled(s.can_copy)
            .unbound()
            .entry(),
        item(S::Paste, C::Paste)
            .key(Shortcut::primary(Key::V))
            .enabled(s.text_target)
            .unbound()
            .entry(),
        item(S::SelectAll, C::SelectAll)
            .key(Shortcut::primary(Key::A))
            .enabled(s.can_select_all)
            .unbound()
            .entry(),
    ];
    if !mac {
        edit.extend([
            Entry::Separator,
            item(S::Preferences, C::Preferences)
                .key(Shortcut::primary(Key::Comma))
                .entry(),
        ]);
    }
    list.push(Menu {
        title: Title::Key(S::Edit),
        entries: edit,
    });

    list.push(Menu {
        title: Title::Key(S::View),
        entries: vec![
            item(S::Workspace, C::ToggleWorkspace)
                .checked(s.workspace_visible)
                .entry(),
            item(S::SavedWorlds, C::ToggleSavedWorlds)
                .checked(s.saved_worlds_visible)
                .entry(),
            item(S::MapPanel, C::ToggleMap).checked(s.map_visible).entry(),
            item(S::ChannelsPanel, C::ToggleChannels)
                .checked(s.channels_visible)
                .entry(),
            Entry::Submenu {
                label: S::LayoutMenu,
                entries: crate::workspace::Preset::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(i, preset)| item(preset.label(), C::Layout(i as u8)).entry())
                    .collect(),
            },
            item(S::RestorePanels, C::RestorePanels).entry(),
            Entry::Separator,
            item(S::ShowToolbar, C::ToggleToolbar)
                .checked(s.toolbar_visible)
                .entry(),
            item(S::FocusCommandInput, C::FocusInput)
                .key(Shortcut::primary(Key::L))
                .entry(),
            item(S::SessionHistory, C::SessionHistory)
                .enabled(s.has_history)
                .entry(),
            Entry::Submenu {
                label: S::Skin,
                entries: [S::SkinFleet, S::SkinArmored, S::SkinSystem]
                    .into_iter()
                    .enumerate()
                    .map(|(i, label)| item(label, C::Skin(i as u8)).checked(s.skin == i as u8).entry())
                    .collect(),
            },
        ],
    });

    list.push(Menu {
        title: Title::Key(S::Session),
        entries: vec![
            item(S::ConnectToSelectedWorld, C::ConnectSelected)
                .key(Shortcut::primary(Key::Enter))
                .enabled(s.world_selected)
                .entry(),
            item(S::Disconnect2, C::Disconnect)
                .key(Shortcut::primary(Key::D))
                .enabled(s.can_disconnect)
                .entry(),
            Entry::Separator,
            item(S::PrivateInput, C::PrivateInput)
                .checked(s.manual_private)
                .enabled(s.can_private)
                .entry(),
            item(S::ClearTranscript, C::ClearTranscript).enabled(s.has_text).entry(),
            Entry::Separator,
            item(S::MenuShowSessionMap, C::SessionMap)
                .key(Shortcut::primary(Key::M).shift())
                .checked(s.session_map)
                .enabled(s.has_session)
                .entry(),
            item(S::MenuSplitPlayMap, C::SessionSplit)
                .checked(s.session_split)
                .enabled(s.has_session)
                .entry(),
            Entry::Separator,
            item(S::ScriptsMenu, C::Scripts).enabled(s.has_scripts).entry(),
        ],
    });

    list.push(Menu {
        title: Title::Key(S::Window),
        entries: vec![
            item(S::NextWorkspaceItem, C::NextItem)
                .key(Shortcut::ctrl(Key::Tab))
                .enabled(s.several_items)
                .entry(),
            item(S::PreviousWorkspaceItem, C::PreviousItem)
                .key(Shortcut::ctrl(Key::Tab).shift())
                .enabled(s.several_items)
                .entry(),
            Entry::Separator,
            {
                let minimize = item(S::Minimize, C::Minimize);
                if mac {
                    minimize.key(Shortcut::primary(Key::M)).entry()
                } else {
                    minimize.entry()
                }
            },
            {
                let label = if s.full_screen {
                    S::ExitFullScreen
                } else {
                    S::FullScreen
                };
                let key = if mac {
                    Shortcut {
                        ctrl: true,
                        ..Shortcut::primary(Key::F)
                    }
                } else {
                    Shortcut::bare(Key::F11)
                };
                item(label, C::FullScreen).key(key).entry()
            },
        ],
    });

    let mut help = vec![
        item(S::GettingStarted, C::GettingStarted).entry(),
        item(S::OtherMudClients, C::OtherClients).entry(),
        Entry::Separator,
    ];
    if !mac {
        help.push(item(S::CheckForUpdatesMenu, C::CheckForUpdates).entry());
    }
    help.push(item(S::AboutWandur, C::About).entry());
    list.push(Menu {
        title: Title::Key(S::Help),
        entries: help,
    });
    list
}

/// Every item of a menu list, submenus included.
pub fn all_items(menus: &[Menu]) -> Vec<&Item> {
    fn walk<'a>(entries: &'a [Entry], out: &mut Vec<&'a Item>) {
        for entry in entries {
            match entry {
                Entry::Item(item) => out.push(item),
                Entry::Submenu { entries, .. } => walk(entries, out),
                Entry::Separator => {}
            }
        }
    }
    let mut out = Vec::new();
    for menu in menus {
        walk(&menu.entries, &mut out);
    }
    out
}

/// The commands whose window shortcuts were pressed this frame (enabled items only; edit
/// shortcuts stay with text fields, and Ctrl+Tab with the workspace view switch).
pub fn shortcuts(ctx: &egui::Context, platform: Platform, state: &MenuState) -> Vec<Command> {
    let list = menus(platform, state);
    let mut fired = Vec::new();
    ctx.input_mut(|input| {
        for item in all_items(&list) {
            let Some(shortcut) = item.shortcut else { continue };
            if !item.bind || !item.enabled || matches!(item.command, Command::NextItem | Command::PreviousItem) {
                continue;
            }
            let wanted = egui::KeyboardShortcut::new(shortcut.modifiers(platform), shortcut.key);
            if input.consume_shortcut(&wanted) {
                fired.push(item.command);
            }
        }
    });
    fired
}

/// The title bar's menu button and its dropdown (Chrome's menu): every menu of the model as a
/// submenu, then the application menu's items on macOS. On Windows and Linux it is the only
/// menu (there is never a menu bar); Alt alone, Alt+F or F10 open it with the keyboard on its
/// first item. On macOS it sits beside the menu bar at the top of the screen.
pub fn button_entries(menus: &[Menu]) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut app = Vec::new();
    for menu in menus {
        match menu.title {
            Title::Key(label) => out.push(Entry::Submenu {
                label,
                entries: menu.entries.clone(),
            }),
            Title::App => app = menu.entries.clone(),
        }
    }
    if !app.is_empty() {
        out.push(Entry::Separator);
        out.extend(app);
    }
    out
}

/// A key the open dropdown answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
}

/// The menu button's state: open or not, the open submenus, the keyboard's row at each level.
#[derive(Clone, Debug, Default)]
pub struct MenuButton {
    pub open: bool,
    /// The submenu entry opened at each level.
    pub path: Vec<usize>,
    /// The highlighted row at each level (`path.len() + 1` long while open).
    pub cursor: Vec<Option<usize>>,
    /// Opened from the keyboard: the highlight shows.
    pub keyboard: bool,
    /// Where the button was drawn this frame, and whether the dropdown aligns to its right edge.
    pub button: Option<Rect>,
    pub align_right: bool,
}

fn entries_at<'a>(root: &'a [Entry], path: &[usize]) -> &'a [Entry] {
    let mut at = root;
    for &i in path {
        match at.get(i) {
            Some(Entry::Submenu { entries, .. }) => at = entries,
            _ => return &[],
        }
    }
    at
}

fn selectable(e: &Entry) -> bool {
    !matches!(e, Entry::Separator)
}

fn first_row(entries: &[Entry]) -> Option<usize> {
    entries.iter().position(selectable)
}

impl MenuButton {
    pub fn close(&mut self) {
        self.open = false;
        self.path.clear();
        self.cursor.clear();
        self.keyboard = false;
    }

    /// The button was clicked: open (no highlight) or close.
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open = true;
            self.keyboard = false;
            self.path.clear();
            self.cursor = vec![None];
        }
    }

    /// Alt, Alt+F or F10: open with the keyboard on the first item.
    pub fn open_with_keyboard(&mut self, root: &[Entry]) {
        self.open = true;
        self.keyboard = true;
        self.path.clear();
        self.cursor = vec![first_row(root)];
    }

    /// Open with a menu's submenu showing (scenes).
    pub fn open_menu(&mut self, root: &[Entry], label: S) {
        self.open = true;
        self.keyboard = false;
        let index = root
            .iter()
            .position(|e| matches!(e, Entry::Submenu { label: l, .. } if *l == label));
        self.path = index.into_iter().collect();
        self.cursor = vec![index];
        if index.is_some() {
            self.cursor.push(None);
        }
    }

    /// One key on the open dropdown. Returns the command chosen.
    pub fn key(&mut self, root: &[Entry], key: NavKey) -> Option<Command> {
        if !self.open {
            return None;
        }
        self.keyboard = true;
        self.cursor.resize(self.path.len() + 1, None);
        let level = self.path.len();
        let entries = entries_at(root, &self.path);
        let at = self.cursor[level];
        let step = |from: Option<usize>, down: bool| -> Option<usize> {
            let n = entries.len();
            if n == 0 {
                return None;
            }
            let mut i = match (from, down) {
                (Some(i), true) => (i + 1) % n,
                (Some(i), false) => (i + n - 1) % n,
                (None, true) => 0,
                (None, false) => n - 1,
            };
            for _ in 0..n {
                if selectable(&entries[i]) {
                    return Some(i);
                }
                i = if down { (i + 1) % n } else { (i + n - 1) % n };
            }
            None
        };
        match key {
            NavKey::Down => self.cursor[level] = step(at, true),
            NavKey::Up => self.cursor[level] = step(at, false),
            NavKey::Left => {
                if self.path.pop().is_some() {
                    self.cursor.truncate(self.path.len() + 1);
                }
            }
            NavKey::Escape => {
                if self.path.pop().is_some() {
                    self.cursor.truncate(self.path.len() + 1);
                } else {
                    self.close();
                }
            }
            NavKey::Right | NavKey::Enter => match at.and_then(|i| entries.get(i).map(|e| (i, e))) {
                Some((i, Entry::Submenu { entries: sub, .. })) => {
                    self.path.push(i);
                    self.cursor.push(first_row(sub));
                }
                Some((_, Entry::Item(item))) if key == NavKey::Enter && item.enabled => {
                    let command = item.command;
                    self.close();
                    return Some(command);
                }
                _ => {}
            },
        }
        None
    }

    /// Draw the open dropdown and its submenus under the button, answer the keyboard and the
    /// pointer. Returns the command chosen this frame.
    pub fn show(&mut self, ctx: &egui::Context, root: &[Entry], theme: &Theme, platform: Platform) -> Option<Command> {
        if !self.open {
            return None;
        }
        // Without a drawn button (the toolbar hidden, full screen) the dropdown hangs from the
        // window's top left corner.
        let button = self
            .button
            .unwrap_or_else(|| Rect::from_min_size(ctx.content_rect().min + vec2(8.0, 4.0), vec2(0.0, 0.0)));
        let mut chosen = None;
        for (key, nav) in [
            (Key::ArrowDown, NavKey::Down),
            (Key::ArrowUp, NavKey::Up),
            (Key::ArrowLeft, NavKey::Left),
            (Key::ArrowRight, NavKey::Right),
            (Key::Enter, NavKey::Enter),
            (Key::Escape, NavKey::Escape),
        ] {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key)) {
                chosen = chosen.or(self.key(root, nav));
            }
        }
        if !self.open {
            return chosen;
        }
        self.cursor.resize(self.path.len() + 1, None);
        let width = popup_width(ctx, root, platform);
        let mut anchor = if self.align_right {
            Pos2::new(button.right() - width, button.bottom() + 2.0)
        } else {
            button.left_bottom() + vec2(0.0, 2.0)
        };
        let mut areas = vec![button];
        let mut level = 0;
        loop {
            let entries = entries_at(root, &self.path[..level]);
            let (rect, pick, sub_anchor) = popup(
                ctx,
                ("menu-button", level),
                anchor,
                entries,
                theme,
                platform,
                self.path.get(level).copied(),
                if self.keyboard { self.cursor[level] } else { None },
            );
            areas.push(rect);
            match pick {
                Some(Pick::Command(c)) => {
                    chosen = Some(c);
                    self.close();
                    break;
                }
                Some(Pick::Submenu(i)) => {
                    self.path.truncate(level);
                    self.path.push(i);
                    self.cursor.truncate(level + 1);
                    self.cursor[level] = Some(i);
                    self.cursor.push(None);
                }
                Some(Pick::Hover(i)) => {
                    self.path.truncate(level);
                    self.cursor.truncate(level + 1);
                    self.cursor[level] = Some(i);
                }
                Some(Pick::Leave) | None => {}
            }
            match (self.path.get(level), sub_anchor) {
                (Some(_), Some(row)) => {
                    // Submenus open away from the window's edge: to the left of a dropdown hung
                    // from the right (macOS), to the right otherwise.
                    let sub = entries_at(root, &self.path[..=level]);
                    anchor = if self.align_right {
                        Pos2::new(row.left() - popup_width(ctx, sub, platform) - 2.0, row.top() - 4.0)
                    } else {
                        row.right_top() + vec2(2.0, -4.0)
                    };
                    level += 1;
                }
                _ => break,
            }
        }
        if self.open {
            let (pressed, pos) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos()));
            if pressed && pos.is_some_and(|p| !areas.iter().any(|r| r.contains(p))) {
                self.close();
            }
            crate::a11y::name_shown_popup(
                ctx,
                egui::Id::new(("menu-button", 0usize)),
                egui::accesskit::Role::Menu,
                t(S::MenuButton),
            );
        }
        chosen
    }
}

const ROW: f32 = 28.0;
const POPUP_MIN_WIDTH: f32 = 220.0;

enum Pick {
    Command(Command),
    Submenu(usize),
    /// The pointer is on an item row (not a submenu).
    Hover(usize),
    Leave,
}

fn label_x(entries: &[Entry]) -> f32 {
    let has_checks = entries
        .iter()
        .any(|e| matches!(e, Entry::Item(Item { checked: Some(_), .. })));
    if has_checks { 40.0 } else { 12.0 }
}

/// The width a popup of `entries` takes: the widest label plus its shortcut.
fn popup_width(ctx: &egui::Context, entries: &[Entry], platform: Platform) -> f32 {
    let font = FontId::proportional(14.0);
    let label_x = label_x(entries);
    ctx.fonts_mut(|f| {
        entries
            .iter()
            .map(|e| match e {
                Entry::Item(item) => {
                    let label = f
                        .layout_no_wrap(t(item.label).to_string(), font.clone(), Color32::WHITE)
                        .size()
                        .x;
                    let key = item
                        .shortcut
                        .map(|s| {
                            f.layout_no_wrap(s.text(platform), font.clone(), Color32::WHITE)
                                .size()
                                .x
                                + 32.0
                        })
                        .unwrap_or(0.0);
                    label_x + label + key + 16.0
                }
                Entry::Submenu { label, .. } => {
                    label_x
                        + f.layout_no_wrap(t(*label).to_string(), font.clone(), Color32::WHITE)
                            .size()
                            .x
                        + 48.0
                }
                Entry::Separator => 0.0,
            })
            .fold(POPUP_MIN_WIDTH, f32::max)
    })
}

/// Draw one level of the dropdown at `anchor`. Returns its rectangle, what the pointer did, and
/// the row an open submenu hangs from.
#[allow(clippy::too_many_arguments)]
fn popup(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    anchor: Pos2,
    entries: &[Entry],
    theme: &Theme,
    platform: Platform,
    open_sub: Option<usize>,
    highlight: Option<usize>,
) -> (Rect, Option<Pick>, Option<Rect>) {
    let font = FontId::proportional(14.0);
    let label_x = label_x(entries);
    let width = popup_width(ctx, entries, platform);
    let mut pick = None;
    let mut sub_anchor = None;
    let response = egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme.menu_fill())
                .stroke(Stroke::new(1.0, theme.border))
                .corner_radius(4)
                .shadow(ui.visuals().popup_shadow)
                .inner_margin(egui::Margin::symmetric(0, 4))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (index, entry) in entries.iter().enumerate() {
                        let lit = highlight == Some(index);
                        match entry {
                            Entry::Separator => {
                                let (rect, _) = ui.allocate_exact_size(vec2(width, 9.0), Sense::hover());
                                ui.painter().hline(
                                    rect.left() + 12.0..=rect.right() - 12.0,
                                    rect.center().y,
                                    Stroke::new(1.0, theme.border),
                                );
                            }
                            Entry::Item(item) => {
                                let sense = if item.enabled { Sense::click() } else { Sense::hover() };
                                let (rect, response) = ui.allocate_exact_size(vec2(width, ROW), sense);
                                if lit || (item.enabled && response.hovered()) {
                                    ui.painter()
                                        .rect_filled(rect.shrink2(vec2(4.0, 1.0)), 3.0, theme.hover_fill());
                                }
                                if response.hovered() {
                                    pick = Some(Pick::Hover(index));
                                }
                                let color = if item.enabled {
                                    theme.text
                                } else {
                                    theme.disabled_text()
                                };
                                if item.checked == Some(true) {
                                    check_mark(ui, Pos2::new(rect.left() + 20.0, rect.center().y), color);
                                }
                                ui.painter().text(
                                    Pos2::new(rect.left() + label_x, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    t(item.label),
                                    font.clone(),
                                    color,
                                );
                                if let Some(shortcut) = item.shortcut {
                                    ui.painter().text(
                                        Pos2::new(rect.right() - 12.0, rect.center().y),
                                        Align2::RIGHT_CENTER,
                                        shortcut.text(platform),
                                        font.clone(),
                                        if item.enabled {
                                            theme.muted
                                        } else {
                                            theme.disabled_text()
                                        },
                                    );
                                }
                                match item.checked {
                                    Some(on) => crate::a11y::toggle(
                                        &response,
                                        egui::accesskit::Role::MenuItemCheckBox,
                                        t(item.label),
                                        on,
                                    ),
                                    None => {
                                        crate::a11y::control(&response, egui::accesskit::Role::MenuItem, t(item.label))
                                    }
                                }
                                if response.clicked() {
                                    pick = Some(Pick::Command(item.command));
                                }
                            }
                            Entry::Submenu { label, .. } => {
                                let (rect, response) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
                                crate::a11y::control(&response, egui::accesskit::Role::MenuItem, t(*label));
                                let open = open_sub == Some(index);
                                if open || lit || response.hovered() {
                                    ui.painter()
                                        .rect_filled(rect.shrink2(vec2(4.0, 1.0)), 3.0, theme.hover_fill());
                                }
                                ui.painter().text(
                                    Pos2::new(rect.left() + label_x, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    t(*label),
                                    font.clone(),
                                    theme.text,
                                );
                                chevron(ui, Pos2::new(rect.right() - 16.0, rect.center().y), theme.text);
                                if (response.hovered() && !open) || response.clicked() {
                                    pick = Some(Pick::Submenu(index));
                                }
                                if open || response.hovered() || response.clicked() {
                                    sub_anchor = Some(rect);
                                }
                            }
                        }
                    }
                });
        });
    if pick.is_none()
        && response
            .response
            .rect
            .contains(ctx.input(|i| i.pointer.hover_pos()).unwrap_or(Pos2::ZERO))
    {
        pick = Some(Pick::Leave);
    }
    (response.response.rect, pick, sub_anchor)
}

/// A check mark drawn with two strokes (the UI font has no check glyph).
fn check_mark(ui: &Ui, center: Pos2, color: Color32) {
    let stroke = Stroke::new(1.8, color);
    let a = center + vec2(-6.0, 0.0);
    let b = center + vec2(-2.0, 4.5);
    let c = center + vec2(6.0, -5.0);
    ui.painter().line_segment([a, b], stroke);
    ui.painter().line_segment([b, c], stroke);
}

fn chevron(ui: &Ui, center: Pos2, color: Color32) {
    let stroke = Stroke::new(1.4, color);
    ui.painter()
        .line_segment([center + vec2(-3.0, -6.0), center + vec2(3.0, 0.0)], stroke);
    ui.painter()
        .line_segment([center + vec2(3.0, 0.0), center + vec2(-3.0, 6.0)], stroke);
}

#[cfg(test)]
mod tests {
    use super::*;
    use wandur_core::l10n::{Language, override_thread, text_in};

    fn labels(entries: &[Entry]) -> Vec<Option<S>> {
        entries
            .iter()
            .map(|e| match e {
                Entry::Item(i) => Some(i.label),
                Entry::Submenu { label, .. } => Some(*label),
                Entry::Separator => None,
            })
            .collect()
    }

    fn find(list: &[Menu], title: S) -> &Menu {
        list.iter().find(|m| m.title == Title::Key(title)).unwrap()
    }

    /// The C# `DesktopMenus` order on Windows and Linux.
    #[test]
    fn menus_list_the_csharp_items_in_csharp_order() {
        let list = menus(Platform::Other, &MenuState::default());
        let titles: Vec<Title> = list.iter().map(|m| m.title).collect();
        assert_eq!(
            titles,
            [S::File, S::Edit, S::View, S::Session, S::Window, S::Help].map(Title::Key)
        );
        use S::*;
        assert_eq!(
            labels(&find(&list, File).entries),
            [
                Some(FindAMUD),
                Some(AddWorld2),
                Some(BrowseWorlds),
                Some(OpenOfflineDemo),
                None,
                Some(MudletImportMenu),
                Some(CsImportMenu),
                Some(MapImport),
                Some(SaveTranscript),
                None,
                Some(CloseWorkspaceItem),
                None,
                Some(Exit)
            ]
        );
        assert_eq!(
            labels(&find(&list, Edit).entries),
            [
                Some(Undo),
                Some(Redo),
                None,
                Some(Cut),
                Some(Copy),
                Some(Paste),
                Some(SelectAll),
                None,
                Some(Preferences)
            ]
        );
        assert_eq!(
            labels(&find(&list, View).entries),
            [
                Some(Workspace),
                Some(SavedWorlds),
                Some(MapPanel),
                Some(ChannelsPanel),
                // Rust only: the layout presets (ui/reading).
                Some(LayoutMenu),
                Some(RestorePanels),
                None,
                Some(ShowToolbar),
                Some(FocusCommandInput),
                Some(SessionHistory),
                Some(Skin)
            ]
        );
        assert_eq!(
            labels(&find(&list, Session).entries),
            [
                Some(ConnectToSelectedWorld),
                Some(Disconnect2),
                None,
                Some(PrivateInput),
                Some(ClearTranscript),
                None,
                // Rust only: the session's full map (C# has a map editor document instead).
                Some(MenuShowSessionMap),
                Some(MenuSplitPlayMap),
                None,
                Some(ScriptsMenu)
            ]
        );
        assert_eq!(
            labels(&find(&list, Window).entries),
            [
                Some(NextWorkspaceItem),
                Some(PreviousWorkspaceItem),
                None,
                Some(Minimize),
                Some(FullScreen)
            ]
        );
        assert_eq!(
            labels(&find(&list, Help).entries),
            [
                Some(GettingStarted),
                Some(OtherMudClients),
                None,
                Some(CheckForUpdatesMenu),
                Some(AboutWandur)
            ]
        );
    }

    /// On macOS About, Check for Updates, Preferences and Quit live in the application menu.
    #[test]
    fn macos_moves_items_to_the_application_menu() {
        let list = menus(Platform::Mac, &MenuState::default());
        assert_eq!(list[0].title, Title::App);
        assert_eq!(
            labels(&list[0].entries),
            [
                Some(S::AboutWandur),
                Some(S::CheckForUpdatesMenu),
                None,
                Some(S::Preferences),
                None,
                Some(S::QuitWandur)
            ]
        );
        assert!(!labels(&find(&list, S::File).entries).contains(&Some(S::Exit)));
        assert!(!labels(&find(&list, S::Edit).entries).contains(&Some(S::Preferences)));
        assert_eq!(
            labels(&find(&list, S::Help).entries),
            [
                Some(S::GettingStarted),
                Some(S::OtherMudClients),
                None,
                Some(S::AboutWandur)
            ]
        );
        let shortcut = |label: S| {
            all_items(&list)
                .into_iter()
                .find(|i| i.label == label)
                .and_then(|i| i.shortcut)
                .map(|s| s.text(Platform::Mac))
        };
        assert_eq!(shortcut(S::FindAMUD).as_deref(), Some("Cmd+T"));
        assert_eq!(shortcut(S::FocusCommandInput).as_deref(), Some("Cmd+L"));
        assert_eq!(shortcut(S::NextWorkspaceItem).as_deref(), Some("Ctrl+Tab"));
        assert_eq!(shortcut(S::FullScreen).as_deref(), Some("Cmd+Ctrl+F"));
        assert_eq!(shortcut(S::Redo).as_deref(), Some("Cmd+Shift+Z"));
        // Cmd+M stays Minimize; the session's map is Cmd+Shift+M.
        assert_eq!(shortcut(S::Minimize).as_deref(), Some("Cmd+M"));
        assert_eq!(shortcut(S::MenuShowSessionMap).as_deref(), Some("Cmd+Shift+M"));
        let other = menus(Platform::Other, &MenuState::default());
        let map = all_items(&other)
            .into_iter()
            .find(|i| i.label == S::MenuShowSessionMap)
            .and_then(|i| i.shortcut)
            .map(|s| s.text(Platform::Other));
        assert_eq!(map.as_deref(), Some("Ctrl+Shift+M"));
    }

    /// Every item is enabled when its conditions hold (no item waits for a later task now).
    #[test]
    fn every_item_is_enabled_when_its_conditions_hold() {
        let everything = MenuState {
            has_text: true,
            world_selected: true,
            can_disconnect: true,
            text_target: true,
            can_copy: true,
            can_select_all: true,
            several_items: true,
            can_private: true,
            has_history: true,
            has_scripts: true,
            has_session: true,
            ..MenuState::default()
        };
        for platform in [Platform::Mac, Platform::Other] {
            let list = menus(platform, &everything);
            for item in all_items(&list) {
                assert!(item.enabled, "{:?}", item.label);
            }
        }
        // View > Skin checks the skin in use.
        let armored = MenuState {
            skin: 1,
            ..MenuState::default()
        };
        let list = menus(Platform::Other, &armored);
        let skins: Vec<_> = all_items(&list)
            .into_iter()
            .filter(|i| matches!(i.command, Command::Skin(_)))
            .map(|i| (i.command, i.checked))
            .collect();
        assert_eq!(
            skins,
            vec![
                (Command::Skin(0), Some(false)),
                (Command::Skin(1), Some(true)),
                (Command::Skin(2), Some(false))
            ]
        );
        let none = menus(Platform::Other, &MenuState::default());
        for label in [
            S::SaveTranscript,
            S::ClearTranscript,
            S::Disconnect2,
            S::ConnectToSelectedWorld,
            S::Copy,
            S::PrivateInput,
        ] {
            let item = all_items(&none).into_iter().find(|i| i.label == label).unwrap();
            assert!(!item.enabled, "{label:?} needs something to act on");
        }
    }

    #[test]
    fn check_marks_follow_the_panels() {
        let state = MenuState {
            workspace_visible: true,
            channels_visible: true,
            toolbar_visible: true,
            ..MenuState::default()
        };
        let list = menus(Platform::Other, &state);
        let checked: Vec<(S, Option<bool>)> = find(&list, S::View)
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(i) => Some((i.label, i.checked)),
                _ => None,
            })
            .collect();
        assert_eq!(checked[0], (S::Workspace, Some(true)));
        assert_eq!(checked[1], (S::SavedWorlds, Some(false)));
        assert_eq!(checked[2], (S::MapPanel, Some(false)));
        assert_eq!(checked[3], (S::ChannelsPanel, Some(true)));
        assert_eq!(checked[5], (S::ShowToolbar, Some(true)));
    }

    #[test]
    fn titles_follow_the_language() {
        override_thread(Some(Language::De));
        let list = menus(Platform::Other, &MenuState::default());
        assert_eq!(list[0].title.text(), text_in(Language::De, S::File));
        assert_eq!(Title::App.text(), "Wandur");
        override_thread(None);
    }

    #[test]
    fn window_shortcuts_fire_enabled_commands_only() {
        let ctx = egui::Context::default();
        let press = |key: Key, modifiers: Modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let command = Shortcut::primary(Key::T).modifiers(Platform::Other);
        let mut fired = Vec::new();
        let input = egui::RawInput {
            events: vec![press(Key::T, command), press(Key::S, command), press(Key::C, command)],
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            fired = shortcuts(ui.ctx(), Platform::Other, &MenuState::default());
        });
        out.textures_delta.clear();
        // Save Transcript is disabled without text; Copy belongs to text fields.
        assert_eq!(fired, [Command::FindAMud]);
    }

    #[test]
    fn the_menu_button_holds_the_whole_model_on_every_platform() {
        let other = button_entries(&menus(Platform::Other, &MenuState::default()));
        let titles: Vec<S> = other
            .iter()
            .filter_map(|e| match e {
                Entry::Submenu { label, .. } => Some(*label),
                _ => None,
            })
            .collect();
        assert_eq!(titles, [S::File, S::Edit, S::View, S::Session, S::Window, S::Help]);
        assert_eq!(other.len(), 6, "Windows and Linux: the six menus, nothing else");
        // Every item of the model is reachable from the button.
        let all = |entries: &[Entry]| {
            fn walk(entries: &[Entry], out: &mut Vec<Command>) {
                for e in entries {
                    match e {
                        Entry::Item(i) => out.push(i.command),
                        Entry::Submenu { entries, .. } => walk(entries, out),
                        Entry::Separator => {}
                    }
                }
            }
            let mut out = Vec::new();
            walk(entries, &mut out);
            out
        };
        let model: Vec<Command> = all_items(&menus(Platform::Other, &MenuState::default()))
            .iter()
            .map(|i| i.command)
            .collect();
        assert_eq!(all(&other), model);
        // macOS: the same menus, then the application menu's items (About, Settings, Quit).
        let mac = button_entries(&menus(Platform::Mac, &MenuState::default()));
        assert!(matches!(mac[6], Entry::Separator));
        let tail: Vec<Command> = all(&mac[7..]);
        assert_eq!(
            tail,
            [
                Command::About,
                Command::CheckForUpdates,
                Command::Preferences,
                Command::Quit
            ]
        );
    }

    #[test]
    fn the_keyboard_walks_the_dropdown() {
        let root = button_entries(&menus(
            Platform::Other,
            &MenuState {
                has_text: true,
                ..MenuState::default()
            },
        ));
        let mut button = MenuButton::default();
        button.open_with_keyboard(&root);
        assert!(button.open && button.keyboard);
        assert_eq!(button.cursor, [Some(0)], "the first item has the keyboard");
        assert_eq!(button.key(&root, NavKey::Down), None);
        assert_eq!(button.cursor, [Some(1)]);
        assert_eq!(button.key(&root, NavKey::Up), None);
        assert_eq!(button.key(&root, NavKey::Up), None);
        assert_eq!(button.cursor, [Some(5)], "Up from the first wraps to Help");
        assert_eq!(button.key(&root, NavKey::Down), None);
        // Right (or Enter) opens File on its first item; Down skips its separators.
        assert_eq!(button.key(&root, NavKey::Right), None);
        assert_eq!(button.path, [0]);
        assert_eq!(button.cursor, [Some(0), Some(0)]);
        for _ in 0..4 {
            button.key(&root, NavKey::Down);
        }
        assert_eq!(button.cursor[1], Some(5), "past Open Offline Demo and the separator");
        // Left closes the submenu; Escape closes the dropdown.
        button.key(&root, NavKey::Left);
        assert!(button.path.is_empty() && button.open);
        // Enter on an item runs it and closes: File > Find a MUD.
        button.key(&root, NavKey::Enter);
        assert_eq!(button.key(&root, NavKey::Enter), Some(Command::FindAMud));
        assert!(!button.open);
        // A disabled item does nothing on Enter: Session > Disconnect.
        button.open_with_keyboard(&root);
        for _ in 0..3 {
            button.key(&root, NavKey::Down);
        }
        button.key(&root, NavKey::Enter);
        button.key(&root, NavKey::Down);
        assert_eq!(button.key(&root, NavKey::Enter), None, "Disconnect needs a session");
        assert!(button.open);
        button.key(&root, NavKey::Escape);
        assert!(button.open && button.path.is_empty());
        button.key(&root, NavKey::Escape);
        assert!(!button.open);
        // Nested: View > Skin > Armored.
        button.open_menu(&root, S::View);
        assert_eq!(button.path, [2]);
        let view = entries_at(&root, &[2]);
        let skin = view
            .iter()
            .position(|e| matches!(e, Entry::Submenu { label: S::Skin, .. }))
            .unwrap();
        button.cursor[1] = Some(skin);
        button.key(&root, NavKey::Right);
        button.key(&root, NavKey::Down);
        assert_eq!(button.key(&root, NavKey::Enter), Some(Command::Skin(1)));
    }
}
