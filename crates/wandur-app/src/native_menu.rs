//! The macOS menu bar at the top of the screen, as the C# client's `NativeMenu` (`DesktopMenus`
//! and `App.axaml`): the same menus as the window menu ([`crate::menus`]), with their labels,
//! shortcuts, check marks and enabled states, plus the system's Hide, Hide Others and Show All in
//! the application menu. Windows and Linux keep the window menu under the title bar.
//!
//! [`spec`] turns the menu model into a plain description (tested on every platform);
//! [`NativeMenuBar`] (macOS only) builds it with `muda`, keeps checks and enabled states in step
//! every frame, rebuilds it when its labels change (a language switch), and hands the chosen
//! commands back to the app.

use std::hash::{Hash, Hasher};

use egui::Key;
use wandur_core::l10n::t;

use crate::menus::{Command, Entry, Menu, Platform, Shortcut, Title};

/// One entry of a native menu.
#[derive(Clone, Debug, PartialEq)]
pub enum NativeEntry {
    Item {
        /// The command's stable id (its debug name, `Skin(1)` for a skin).
        id: String,
        command: Command,
        label: String,
        /// The `muda` accelerator text, `super+shift+KeyZ`.
        accelerator: Option<String>,
        enabled: bool,
        checked: Option<bool>,
    },
    Separator,
    Submenu {
        label: String,
        entries: Vec<NativeEntry>,
    },
    /// The system's own items in the application menu.
    Hide,
    HideOthers,
    ShowAll,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeMenu {
    pub title: String,
    pub entries: Vec<NativeEntry>,
    /// The Window and Help menus are given to macOS as such (it adds its own items there).
    pub role: Role,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    App,
    Window,
    Help,
    Other,
}

/// The id a command has in the native menu.
pub fn id_of(command: Command) -> String {
    format!("{command:?}")
}

/// The accelerator text `muda` parses for a shortcut on macOS.
pub fn accelerator(shortcut: Shortcut) -> Option<String> {
    let key = match shortcut.key {
        Key::Comma => "Comma".to_string(),
        Key::Enter => "Enter".to_string(),
        Key::Tab => "Tab".to_string(),
        Key::F11 => "F11".to_string(),
        k => {
            let name = k.name();
            if name.len() == 1 && name.chars().all(|c| c.is_ascii_alphabetic()) {
                format!("Key{}", name.to_ascii_uppercase())
            } else {
                return None;
            }
        }
    };
    let mut parts = Vec::new();
    if shortcut.command {
        parts.push("super");
    }
    if shortcut.ctrl {
        parts.push("control");
    }
    if shortcut.shift {
        parts.push("shift");
    }
    parts.push(&key);
    Some(parts.join("+"))
}

fn entries(list: &[Entry]) -> Vec<NativeEntry> {
    list.iter()
        .map(|e| match e {
            Entry::Separator => NativeEntry::Separator,
            Entry::Submenu { label, entries: sub } => NativeEntry::Submenu {
                label: t(*label).to_string(),
                entries: entries(sub),
            },
            Entry::Item(item) => NativeEntry::Item {
                id: id_of(item.command),
                command: item.command,
                label: t(item.label).to_string(),
                accelerator: item.shortcut.and_then(accelerator),
                enabled: item.enabled,
                checked: item.checked,
            },
        })
        .collect()
}

/// The native menus for the macOS menu model: the application menu gains Hide, Hide Others and
/// Show All before Quit, as every Mac app has them.
pub fn spec(menus: &[Menu]) -> Vec<NativeMenu> {
    menus
        .iter()
        .map(|m| {
            let mut list = entries(&m.entries);
            let role = match m.title {
                Title::App => Role::App,
                Title::Key(wandur_core::l10n::S::Window) => Role::Window,
                Title::Key(wandur_core::l10n::S::Help) => Role::Help,
                Title::Key(_) => Role::Other,
            };
            if role == Role::App {
                let quit = list
                    .iter()
                    .position(|e| {
                        matches!(
                            e,
                            NativeEntry::Item {
                                command: Command::Quit,
                                ..
                            }
                        )
                    })
                    .unwrap_or(list.len());
                list.splice(
                    quit..quit,
                    [
                        NativeEntry::Hide,
                        NativeEntry::HideOthers,
                        NativeEntry::ShowAll,
                        NativeEntry::Separator,
                    ],
                );
            }
            NativeMenu {
                title: m.title.text().to_string(),
                entries: list,
                role,
            }
        })
        .collect()
}

/// What must not change for the built menu to be kept (labels, shortcuts, structure); checks
/// and enabled states are updated in place.
pub fn signature(menus: &[NativeMenu]) -> u64 {
    fn walk(entries: &[NativeEntry], h: &mut std::collections::hash_map::DefaultHasher) {
        for e in entries {
            match e {
                NativeEntry::Item {
                    id,
                    label,
                    accelerator,
                    checked,
                    ..
                } => {
                    (0u8, id, label, accelerator, checked.is_some()).hash(h);
                }
                NativeEntry::Separator => 1u8.hash(h),
                NativeEntry::Submenu { label, entries } => {
                    (2u8, label).hash(h);
                    walk(entries, h);
                    3u8.hash(h);
                }
                NativeEntry::Hide => 4u8.hash(h),
                NativeEntry::HideOthers => 5u8.hash(h),
                NativeEntry::ShowAll => 6u8.hash(h),
            }
        }
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for m in menus {
        m.title.hash(&mut h);
        walk(&m.entries, &mut h);
    }
    h.finish()
}

/// Whether this platform shows the menus at the top of the screen (the window then has no menu
/// row of its own).
pub fn screen_menu(platform: Platform) -> bool {
    platform == Platform::Mac
}

#[cfg(target_os = "macos")]
pub use mac::NativeMenuBar;

#[cfg(target_os = "macos")]
mod mac {
    use std::collections::HashMap;
    use std::sync::mpsc;

    use muda::accelerator::Accelerator;
    use muda::{CheckMenuItem, Menu as MudaMenu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};

    use super::{NativeEntry, NativeMenu, Role, signature};
    use crate::menus::Command;

    enum Handle {
        Normal(MenuItem),
        Check(CheckMenuItem),
    }

    /// The installed menu bar.
    pub struct NativeMenuBar {
        menu: Option<MudaMenu>,
        items: HashMap<String, (Handle, bool, Option<bool>)>,
        commands: HashMap<MenuId, Command>,
        signature: u64,
        events: mpsc::Receiver<MenuId>,
    }

    impl NativeMenuBar {
        /// Listen for menu choices (waking the app when one arrives). The menus are built on the
        /// first [`Self::sync`].
        pub fn new(ctx: &egui::Context) -> Self {
            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let _ = tx.send(event.id().clone());
                ctx.request_repaint();
            }));
            Self {
                menu: None,
                items: HashMap::new(),
                commands: HashMap::new(),
                signature: 0,
                events: rx,
            }
        }

        /// Bring the menu bar in step with the model: rebuilt when its labels or structure
        /// changed, otherwise only the changed checks and enabled states are set.
        pub fn sync(&mut self, menus: &[NativeMenu]) {
            let sig = signature(menus);
            if self.menu.is_none() || sig != self.signature {
                self.build(menus);
                self.signature = sig;
                return;
            }
            fn walk(entries: &[NativeEntry], items: &mut HashMap<String, (Handle, bool, Option<bool>)>) {
                for e in entries {
                    match e {
                        NativeEntry::Item {
                            id, enabled, checked, ..
                        } => {
                            if let Some((handle, was_enabled, was_checked)) = items.get_mut(id) {
                                if *was_enabled != *enabled {
                                    match handle {
                                        Handle::Normal(i) => i.set_enabled(*enabled),
                                        Handle::Check(i) => i.set_enabled(*enabled),
                                    }
                                    *was_enabled = *enabled;
                                }
                                if *was_checked != *checked {
                                    if let (Handle::Check(i), Some(on)) = (&*handle, checked) {
                                        i.set_checked(*on);
                                    }
                                    *was_checked = *checked;
                                }
                            }
                        }
                        NativeEntry::Submenu { entries, .. } => walk(entries, items),
                        _ => {}
                    }
                }
            }
            for m in menus {
                walk(&m.entries, &mut self.items);
            }
        }

        fn build(&mut self, menus: &[NativeMenu]) {
            let bar = MudaMenu::new();
            let mut items = HashMap::new();
            let mut commands = HashMap::new();
            fn fill(
                sub: &Submenu,
                entries: &[NativeEntry],
                items: &mut HashMap<String, (Handle, bool, Option<bool>)>,
                commands: &mut HashMap<MenuId, Command>,
            ) {
                for e in entries {
                    match e {
                        NativeEntry::Item {
                            id,
                            command,
                            label,
                            accelerator,
                            enabled,
                            checked,
                        } => {
                            let accel: Option<Accelerator> = accelerator.as_deref().and_then(|a| a.parse().ok());
                            let handle = match checked {
                                Some(on) => {
                                    let item = CheckMenuItem::with_id(id.as_str(), label, *enabled, *on, accel);
                                    let _ = sub.append(&item);
                                    Handle::Check(item)
                                }
                                None => {
                                    let item = MenuItem::with_id(id.as_str(), label, *enabled, accel);
                                    let _ = sub.append(&item);
                                    Handle::Normal(item)
                                }
                            };
                            commands.insert(MenuId::new(id), *command);
                            items.insert(id.clone(), (handle, *enabled, *checked));
                        }
                        NativeEntry::Separator => {
                            let _ = sub.append(&PredefinedMenuItem::separator());
                        }
                        NativeEntry::Submenu { label, entries } => {
                            let inner = Submenu::new(label, true);
                            fill(&inner, entries, items, commands);
                            let _ = sub.append(&inner);
                        }
                        NativeEntry::Hide => {
                            let _ = sub.append(&PredefinedMenuItem::hide(None));
                        }
                        NativeEntry::HideOthers => {
                            let _ = sub.append(&PredefinedMenuItem::hide_others(None));
                        }
                        NativeEntry::ShowAll => {
                            let _ = sub.append(&PredefinedMenuItem::show_all(None));
                        }
                    }
                }
            }
            for m in menus {
                let sub = Submenu::new(&m.title, true);
                fill(&sub, &m.entries, &mut items, &mut commands);
                let _ = bar.append(&sub);
                match m.role {
                    Role::Window => sub.set_as_windows_menu_for_nsapp(),
                    Role::Help => sub.set_as_help_menu_for_nsapp(),
                    _ => {}
                }
            }
            bar.init_for_nsapp();
            self.menu = Some(bar);
            self.items = items;
            self.commands = commands;
        }

        /// The commands chosen in the menu bar since the last call.
        pub fn take_commands(&self) -> Vec<Command> {
            self.events
                .try_iter()
                .filter_map(|id| self.commands.get(&id).copied())
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menus::{MenuState, menus};
    use wandur_core::l10n::{Language, S, override_thread};

    fn labels(entries: &[NativeEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                NativeEntry::Item { label, .. } => label.clone(),
                NativeEntry::Separator => "-".into(),
                NativeEntry::Submenu { label, .. } => format!("{label} >"),
                NativeEntry::Hide => "[Hide]".into(),
                NativeEntry::HideOthers => "[Hide Others]".into(),
                NativeEntry::ShowAll => "[Show All]".into(),
            })
            .collect()
    }

    fn item(list: &[NativeMenu], command: Command) -> &NativeEntry {
        fn find(entries: &[NativeEntry], command: Command) -> Option<&NativeEntry> {
            entries.iter().find_map(|e| match e {
                NativeEntry::Item { command: c, .. } if *c == command => Some(e),
                NativeEntry::Submenu { entries, .. } => find(entries, command),
                _ => None,
            })
        }
        list.iter()
            .find_map(|m| find(&m.entries, command))
            .expect("in the menus")
    }

    #[test]
    fn the_mac_menu_bar_has_the_app_menu_then_the_csharp_menus() {
        override_thread(Some(Language::En));
        let list = spec(&menus(Platform::Mac, &MenuState::default()));
        let titles: Vec<&str> = list.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, ["Wandur", "File", "Edit", "View", "Session", "Window", "Help"]);
        assert_eq!(
            labels(&list[0].entries),
            [
                t(S::AboutWandur),
                t(S::CheckForUpdatesMenu),
                "-",
                t(S::Preferences),
                "-",
                "[Hide]",
                "[Hide Others]",
                "[Show All]",
                "-",
                t(S::QuitWandur),
            ]
        );
        assert_eq!(list[0].role, Role::App);
        assert_eq!(list[5].role, Role::Window);
        assert_eq!(list[6].role, Role::Help);
        // Shortcuts: Cmd is `super` on macOS.
        match item(&list, Command::Preferences) {
            NativeEntry::Item { accelerator, .. } => assert_eq!(accelerator.as_deref(), Some("super+Comma")),
            _ => unreachable!(),
        }
        match item(&list, Command::Redo) {
            NativeEntry::Item { accelerator, .. } => assert_eq!(accelerator.as_deref(), Some("super+shift+KeyZ")),
            _ => unreachable!(),
        }
        match item(&list, Command::FullScreen) {
            NativeEntry::Item { accelerator, .. } => assert_eq!(accelerator.as_deref(), Some("super+control+KeyF")),
            _ => unreachable!(),
        }
        match item(&list, Command::NextItem) {
            NativeEntry::Item { accelerator, .. } => assert_eq!(accelerator.as_deref(), Some("control+Tab")),
            _ => unreachable!(),
        }
        // No Quit in File and no Preferences in Edit on macOS (they are in the app menu).
        assert!(!labels(&list[1].entries).contains(&t(S::Exit).to_string()));
        assert!(!labels(&list[2].entries).contains(&t(S::Preferences).to_string()));
        override_thread(None);
    }

    #[test]
    fn every_native_accelerator_parses_and_checks_and_states_follow_the_model() {
        override_thread(Some(Language::En));
        let state = MenuState {
            workspace_visible: true,
            saved_worlds_visible: false,
            has_text: true,
            ..MenuState::default()
        };
        let list = spec(&menus(Platform::Mac, &state));
        fn walk(entries: &[NativeEntry], out: &mut Vec<String>) {
            for e in entries {
                match e {
                    NativeEntry::Item {
                        accelerator: Some(a), ..
                    } => out.push(a.clone()),
                    NativeEntry::Submenu { entries, .. } => walk(entries, out),
                    _ => {}
                }
            }
        }
        let mut all = Vec::new();
        for m in &list {
            walk(&m.entries, &mut all);
        }
        assert!(all.len() >= 15, "{all:?}");
        #[cfg(target_os = "macos")]
        for a in &all {
            assert!(a.parse::<muda::accelerator::Accelerator>().is_ok(), "{a}");
        }
        match item(&list, Command::ToggleWorkspace) {
            NativeEntry::Item { checked, .. } => assert_eq!(*checked, Some(true)),
            _ => unreachable!(),
        }
        match item(&list, Command::ToggleSavedWorlds) {
            NativeEntry::Item { checked, .. } => assert_eq!(*checked, Some(false)),
            _ => unreachable!(),
        }
        match item(&list, Command::SaveTranscript) {
            NativeEntry::Item { enabled, .. } => assert!(*enabled),
            _ => unreachable!(),
        }
        match item(&list, Command::Disconnect) {
            NativeEntry::Item { enabled, .. } => assert!(!*enabled),
            _ => unreachable!(),
        }
        // A check or an enabled state never rebuilds the menus; a language switch does.
        let other = spec(&menus(
            Platform::Mac,
            &MenuState {
                workspace_visible: false,
                can_disconnect: true,
                ..state.clone()
            },
        ));
        assert_eq!(signature(&list), signature(&other));
        override_thread(Some(Language::De));
        let german = spec(&menus(Platform::Mac, &state));
        assert_ne!(signature(&list), signature(&german));
        assert_eq!(german[1].title, t(S::File));
        override_thread(None);
    }

    #[test]
    fn only_macos_moves_the_menus_to_the_top_of_the_screen() {
        assert!(screen_menu(Platform::Mac));
        assert!(!screen_menu(Platform::Other));
        // Windows and Linux keep the window menu with Quit (Exit) in File and Preferences in Edit.
        override_thread(Some(Language::En));
        let list = menus(Platform::Other, &MenuState::default());
        assert_eq!(list.first().map(|m| m.title), Some(Title::Key(S::File)));
        let file = labels(&spec(&list)[0].entries);
        assert_eq!(file.last().map(String::as_str), Some(t(S::Exit)));
        override_thread(None);
    }
}
