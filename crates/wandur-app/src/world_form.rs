//! The world editor, as the C# profile dialog: a modal window with a bar on top (show or hide the
//! sections, the world being edited, New), the sections on the left (Connection, Login, Scripts,
//! Macros, Agent settings, Channels; the ones later tasks bring are shown disabled), the section
//! on the right, and the draft note with Remove, Cancel and Save world at the bottom.
//!
//! Everything is a draft until Save world: the connection fields, the macros (added, edited,
//! enabled or deleted) and Remove, which only marks the world for removal. Save validates every
//! section before anything is written.

use egui::{Align, Layout, RichText, Ui};
use wandur_core::Charset;
use wandur_core::db::scripts::{self, ImportInfo, LibraryEntry};
use wandur_core::l10n::{S, t, tf};
use wandur_core::macros::{MacroDefinition, MacroKind, MacroMatch, MacroStatus, SHORTCUT_KEYS};
use wandur_core::scripting::{Compatibility, Language};
use wandur_core::settings::SavedWorld;

use crate::dialogs;
use crate::select::Select;
use crate::theme::Theme;
use crate::toast::{Toast, Undo};
use crate::widgets::{self, Icon};

/// The editor window's id (and its viewport's).
pub const WINDOW_ID: &str = "world-form";

/// The sections of the editor, in the C# order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Connection,
    Login,
    Scripts,
    Macros,
    Agent,
    Channels,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::Connection,
        Section::Login,
        Section::Scripts,
        Section::Macros,
        Section::Agent,
        Section::Channels,
    ];

    pub fn label(self) -> &'static str {
        t(match self {
            Section::Connection => S::ProfileConnectionSection,
            Section::Login => S::ProfileLoginSection,
            Section::Scripts => S::WorldScripts,
            Section::Macros => S::MacrosTab,
            Section::Agent => S::AgentSettings,
            Section::Channels => S::ProfileChannelsSection,
        })
    }

    /// Whether this build has the section (Agent settings needs the `agent` feature).
    pub fn available(self) -> bool {
        !matches!(self, Section::Agent) || cfg!(feature = "agent")
    }

    /// Script, macro and agent sections hide Remove (the C# `IsConnectionForm`).
    fn is_automation(self) -> bool {
        matches!(self, Section::Scripts | Section::Macros | Section::Agent)
    }
}

/// "Pack · Generated" (or Reviewed): a supplied script's marker (C# `PackLabel`).
pub fn pack_label(info: &scripts::PackInfo) -> String {
    let provenance = if info.provenance == scripts::PackInfo::REVIEWED {
        S::ScriptPackReviewed
    } else {
        S::ScriptPackGenerated
    };
    format!("{} · {}", t(S::ScriptPackMarker), t(provenance))
}

/// The world's library (scripts and macros) as a draft: the Scripts and Macros sections edit it
/// together and Save world writes it.
#[derive(Clone, Debug, Default)]
pub struct MacroEditor {
    /// The whole library being edited (scripts and macros, in order).
    pub entries: Vec<LibraryEntry>,
    baseline: Vec<LibraryEntry>,
    /// The selected macro's id.
    pub selected: Option<String>,
    /// The selected script's id (the Scripts section).
    pub selected_script: Option<String>,
    /// The Scripts section's output panel is shown.
    pub show_output: bool,
    /// The Scripts section's API help is open.
    pub show_help: bool,
    /// The code editor's caret and completion list.
    pub editor: crate::script_editor::EditorState,
    /// Someone typed in a pack script: the prompt offering an editable copy is shown, with the
    /// keystrokes to replay into the copy.
    pub pack_prompt: Option<Vec<egui::Event>>,
    /// What the code editor shows for the selected script (formatted for reading).
    pub shown: Option<crate::script_editor::Shown>,
}

impl MacroEditor {
    pub fn new(library: Vec<LibraryEntry>) -> Self {
        let selected = library.iter().find(|e| e.is_macro()).map(|e| e.id.clone());
        let selected_script = library.iter().find(|e| !e.is_macro()).map(|e| e.id.clone());
        Self {
            baseline: library.clone(),
            entries: library,
            selected,
            selected_script,
            ..Self::default()
        }
    }

    /// The hand-written scripts, in library order.
    pub fn scripts(&self) -> impl Iterator<Item = &LibraryEntry> {
        self.entries.iter().filter(|e| !e.is_macro())
    }

    pub fn selected_script_entry(&self) -> Option<&LibraryEntry> {
        let id = self.selected_script.as_deref()?;
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn selected_script_mut(&mut self) -> Option<&mut LibraryEntry> {
        let id = self.selected_script.clone()?;
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// Select a script (the editor's caret and list start fresh).
    pub fn select_script(&mut self, id: &str) {
        self.selected_script = Some(id.to_string());
        self.pack_prompt = None;
        self.editor = crate::script_editor::EditorState::default();
    }

    /// Make copy (the pack prompt): duplicate the selected pack script, select the copy with
    /// the caret where it was, and give the copy the keystrokes that asked for it.
    pub fn make_editable_copy(&mut self) -> Result<(), String> {
        let caret = self.editor.caret;
        let replay = self.pack_prompt.take().unwrap_or_default();
        self.duplicate_script()?;
        self.editor.set_caret = Some(caret);
        self.editor.replay = replay;
        Ok(())
    }

    /// New: "New script" with the starter example, disabled, selected (C#). Refused when the
    /// library is full.
    pub fn add_script(&mut self) -> Result<(), String> {
        if self.entries.len() >= scripts::MAX_ENTRIES {
            return Err(t(S::ScriptLibraryTooLarge).into());
        }
        let entry = scripts::starter();
        let id = entry.id.clone();
        self.entries.push(entry);
        self.select_script(&id);
        Ok(())
    }

    /// Duplicate: an ordinary hand-written copy ("Name copy"), disabled, without pack details.
    /// The copy starts from the text the editor shows (a pack script formatted for reading).
    pub fn duplicate_script(&mut self) -> Result<(), String> {
        let Some(mut source) = self.selected_script_entry().cloned() else {
            return Ok(());
        };
        if let Some(shown) = self.shown.as_ref().filter(|s| s.shows(&source.id, &source.source)) {
            source.source = shown.text.clone();
        }
        if self.entries.len() >= scripts::MAX_ENTRIES {
            return Err(t(S::ScriptLibraryTooLarge).into());
        }
        let name: String = tf(S::ScriptDuplicateName, &[&source.name])
            .chars()
            .take(scripts::MAX_NAME)
            .collect();
        let copy = LibraryEntry {
            id: scripts::new_id(),
            name,
            source: source.source,
            enabled: false,
            language: source.language,
            compatibility: source.compatibility,
            ..LibraryEntry::default()
        };
        let id = copy.id.clone();
        self.entries.push(copy);
        self.select_script(&id);
        Ok(())
    }

    /// Switch the selected script on or off. Imported Lua that needs conversion would only
    /// fail, so it stays off until it is rewritten (C# `MudletNeedsConversionCannotRun`).
    pub fn set_script_enabled(&mut self, on: bool) -> Result<(), String> {
        let Some(entry) = self.selected_script_mut() else {
            return Ok(());
        };
        if on && entry.needs_conversion() {
            return Err(t(S::MudletNeedsConversionCannotRun).into());
        }
        entry.enabled = on;
        Ok(())
    }

    /// The selected script's language (JavaScript drops the Mudlet layer).
    pub fn set_script_language(&mut self, language: Language) {
        if let Some(entry) = self.selected_script_mut()
            && !entry.is_pack()
            && !entry.is_macro()
        {
            entry.language = language;
            if language == Language::JavaScript {
                entry.compatibility = Compatibility::None;
            }
        }
    }

    /// The selected Lua script's Mudlet names.
    pub fn set_script_mudlet(&mut self, on: bool) {
        if let Some(entry) = self.selected_script_mut()
            && entry.is_lua()
        {
            entry.compatibility = if on { Compatibility::Mudlet } else { Compatibility::None };
        }
    }

    /// Run as Lua (C# `ConvertImportToLua`): an imported script that needs conversion becomes a
    /// switched-off Lua script running its kept Mudlet items through the Mudlet layer. The
    /// original Lua stays on the import record.
    pub fn run_import_as_lua(&mut self) {
        let Some(entry) = self.selected_script_mut() else {
            return;
        };
        let Some(import) = entry
            .import
            .as_mut()
            .filter(|i| i.needs_conversion && i.origin == ImportInfo::MUDLET)
        else {
            return;
        };
        import.needs_conversion = false;
        entry.source = wandur_core::mudlet::lua_wrapper::build(import, ";;");
        entry.enabled = false;
        entry.language = Language::Lua;
        entry.compatibility = Compatibility::Mudlet;
        self.editor = crate::script_editor::EditorState::default();
    }

    /// The marker under an imported script's name.
    pub fn import_label(entry: &LibraryEntry) -> Option<&'static str> {
        entry.import.as_ref().map(|i| {
            t(if i.needs_conversion {
                S::MudletImportMarkerLua
            } else {
                S::MudletImportMarker
            })
        })
    }

    /// Delete the selected script; the next one is selected. Returns the undo toast (Undo puts
    /// the script back where it was, [`Self::restore`]).
    pub fn delete_script(&mut self) -> Option<Toast> {
        let id = self.selected_script.take()?;
        let index = self.entries.iter().position(|e| e.id == id)?;
        let entry = self.entries.remove(index);
        let next = self.scripts().next().map(|e| e.id.clone());
        if let Some(next) = next {
            self.select_script(&next);
        }
        Some(Toast::new(
            tf(S::ToastDeletedScript, &[&entry.name]),
            Undo::Script {
                entry: Box::new(entry),
                index,
            },
        ))
    }

    /// Put a deleted script or macro back exactly as it was, at its place, and select it.
    pub fn restore(&mut self, undo: Undo) {
        match undo {
            Undo::Script { entry, index } => {
                let id = entry.id.clone();
                if self.entries.iter().all(|e| e.id != id) {
                    self.entries.insert(index.min(self.entries.len()), *entry);
                }
                self.select_script(&id);
            }
            Undo::Macro { entry, index } => {
                let id = entry.id.clone();
                if self.entries.iter().all(|e| e.id != id) {
                    self.entries.insert(index.min(self.entries.len()), *entry);
                }
                self.selected = Some(id);
            }
            Undo::Map { .. } | Undo::World { .. } | Undo::DetachedMap { .. } => {}
        }
    }

    pub fn macros(&self) -> impl Iterator<Item = &LibraryEntry> {
        self.entries.iter().filter(|e| e.is_macro())
    }

    pub fn selected_entry(&self) -> Option<&LibraryEntry> {
        let id = self.selected.as_deref()?;
        self.entries.iter().find(|e| e.id == id)
    }

    fn selected_mut(&mut self) -> Option<&mut LibraryEntry> {
        let id = self.selected.clone()?;
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// The selected macro's definition, to edit.
    pub fn definition_mut(&mut self) -> Option<&mut MacroDefinition> {
        self.selected_mut().and_then(|e| e.macro_def.as_mut())
    }

    /// The Add button: a new, disabled trigger ("You are hungry", "eat bread"), selected.
    /// Refused (with the C# message) when the library is full.
    pub fn add(&mut self) -> Result<(), String> {
        if self.entries.len() >= scripts::MAX_ENTRIES {
            return Err(t(S::ScriptLibraryTooLarge).into());
        }
        // Numbered after the first (New macro, New macro 2, ...), so two new ones do not read
        // as one listed twice (C# UI review, item 13).
        let base = t(S::MacroNewName);
        let taken = |name: &str| self.entries.iter().any(|e| e.name == name);
        let name = if taken(base) {
            (2..)
                .map(|n| format!("{base} {n}"))
                .find(|n| !taken(n))
                .unwrap_or_else(|| base.to_string())
        } else {
            base.to_string()
        };
        let entry = LibraryEntry::new_macro(&name, MacroDefinition::starter());
        self.selected = Some(entry.id.clone());
        self.entries.push(entry);
        Ok(())
    }

    /// Delete the selected macro; the next one is selected. Returns the undo toast.
    pub fn delete_selected(&mut self) -> Option<Toast> {
        let id = self.selected.take()?;
        let index = self.entries.iter().position(|e| e.id == id)?;
        let entry = self.entries.remove(index);
        let next = self.macros().next().map(|e| e.id.clone());
        self.selected = next;
        Some(Toast::new(
            tf(S::ToastDeletedMacro, &[&entry.name]),
            Undo::Macro {
                entry: Box::new(entry),
                index,
            },
        ))
    }

    /// Change the selected macro's kind. Becoming a shortcut picks F1, as in C#.
    pub fn set_kind(&mut self, kind: MacroKind) {
        if let Some(def) = self.definition_mut() {
            if kind == MacroKind::Shortcut && def.kind != MacroKind::Shortcut {
                def.pattern = SHORTCUT_KEYS[0].into();
            }
            def.kind = kind;
        }
    }

    /// Whether anything differs from the saved library.
    pub fn has_changes(&self) -> bool {
        self.entries.len() != self.baseline.len()
            || self.entries.iter().zip(&self.baseline).any(|(a, b)| {
                a.id != b.id
                    || a.name != b.name
                    || a.enabled != b.enabled
                    || a.macro_def != b.macro_def
                    || (!a.is_macro() && a.source != b.source)
                    || a.runtime() != b.runtime()
                    || a.import != b.import
                    || a.pack_json != b.pack_json
            })
    }

    /// Whether one entry's name or definition differs from what is saved (enabling alone does
    /// not count, as the C# "Unsaved changes" note).
    pub fn is_unsaved(&self, id: &str) -> bool {
        let now = self.entries.iter().find(|e| e.id == id);
        let saved = self.baseline.iter().find(|e| e.id == id);
        match (now, saved) {
            (Some(a), Some(b)) => {
                a.name != b.name
                    || a.macro_def != b.macro_def
                    || (!a.is_macro() && a.source != b.source)
                    || a.runtime() != b.runtime()
                    || a.import != b.import
            }
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// The library to save: every macro's source regenerated, everything validated. Saving
    /// changed source is the person taking over an imported script that needed conversion: it
    /// is theirs to run from now on (C# `SaveAsync`).
    pub fn finish(&self) -> Result<Vec<LibraryEntry>, String> {
        let mut entries = self.entries.clone();
        for entry in &mut entries {
            let saved_source = self
                .baseline
                .iter()
                .find(|b| b.id == entry.id)
                .map(|b| b.source.as_str());
            if entry.needs_conversion()
                && saved_source.is_some_and(|saved| saved != entry.source)
                && let Some(import) = &mut entry.import
            {
                import.needs_conversion = false;
            }
            entry.compile().map_err(|e| e.to_string())?;
        }
        scripts::validate_library(&entries).map_err(|e| e.to_string())?;
        Ok(entries)
    }

    /// What the list shows under a macro's name.
    pub fn status(&self, entry: &LibraryEntry) -> &'static str {
        // The editor runs nothing: enabled macros wait for a connection, as in the C# editor.
        if entry.enabled {
            MacroStatus::Waiting.label()
        } else {
            MacroStatus::Disabled.label()
        }
    }
}

/// A library change for the app to write: the draft's base and its result.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryChange {
    pub before: Vec<LibraryEntry>,
    pub after: Vec<LibraryEntry>,
}

/// Returned once per frame and dropped at once, so the variants' sizes do not matter.
#[allow(clippy::large_enum_variant)]
pub enum FormResult {
    Open,
    Cancelled,
    /// Save world: the world (at `index`, or new), and the macros when they changed. The app
    /// saves the password (`""` keeps the saved one) through the vault before the world.
    Saved {
        index: Option<usize>,
        world: SavedWorld,
        library: Option<LibraryChange>,
        password: String,
        remember: bool,
    },
    /// Save world with Remove marked.
    Removed(usize),
    /// Edit another saved world (or a new one, `None`); the draft had no changes or was dropped.
    Switch(Option<usize>),
}

/// What the Login section's password field can say about the saved password.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordState {
    /// No saved password (or Save password is off): the field asks for one.
    None,
    /// A saved password a blank field keeps.
    Saved,
    /// The server or username changed: the saved one cannot be kept (a changed port or TLS
    /// keeps it).
    Changed,
    /// The world refers to a saved password the credential store does not have.
    Missing,
}

/// The credential store's answer to "is this world's saved password there?", filled in by a
/// worker thread (the store may block or ask the person); empty until it answers.
pub type SavedCheck = std::sync::Arc<std::sync::OnceLock<Result<bool, String>>>;

#[derive(Debug, Clone)]
pub struct WorldForm {
    /// The world being edited (an index into the saved worlds), or `None` for a new one.
    pub index: Option<usize>,
    pub world: SavedWorld,
    pub(crate) port_text: String,
    baseline: (SavedWorld, String),
    error: Option<String>,
    pub section: Section,
    sections_visible: bool,
    pending_removal: bool,
    /// The undo toast of a script or macro deleted from the draft (shown in this window).
    pub toast: Option<Toast>,
    pub macros: MacroEditor,
    /// A world chosen in the bar while the draft has changes: confirm dropping them first.
    switch_to: Option<Option<usize>>,
    /// The password typed in the Login section (blank keeps the saved one).
    pub password: String,
    /// "Save password in system credential store".
    pub remember: bool,
    /// The Custom login prompts expander is open.
    prompts_open: bool,
    /// The credential store's name, for the help text ("macOS Keychain").
    vault_name: String,
    /// Whether the store has the saved password the world refers to (none when not checked).
    saved_check: Option<SavedCheck>,
    /// Lua scripts are allowed (the preference): the Scripts section offers the language
    /// picker and Run as Lua.
    pub lua_enabled: bool,
    /// The Agent settings section's draft (none when the app has no agent).
    #[cfg(feature = "agent")]
    pub agent: Option<crate::agent_settings::AgentDraft>,
}

impl WorldForm {
    pub fn new_world() -> Self {
        Self::edit(None, SavedWorld::default())
    }

    pub fn edit(index: Option<usize>, world: SavedWorld) -> Self {
        let port_text = world.port.to_string();
        let remember = world.password_id.is_some();
        Self {
            index,
            baseline: (world.clone(), port_text.clone()),
            port_text,
            world,
            error: None,
            section: Section::Connection,
            sections_visible: true,
            pending_removal: false,
            toast: None,
            macros: MacroEditor::default(),
            switch_to: None,
            password: String::new(),
            remember,
            prompts_open: false,
            vault_name: String::new(),
            saved_check: None,
            lua_enabled: false,
            #[cfg(feature = "agent")]
            agent: None,
        }
    }

    /// The credential store's name, for the Login section's help text.
    pub fn with_vault_name(mut self, name: String) -> Self {
        self.vault_name = name;
        self
    }

    /// Ask `vault` on a worker thread whether the saved password is there, so the Login section
    /// can say when it is not (passwords not imported). `wake` repaints when it answers.
    pub fn check_saved_password(
        &mut self,
        vault: std::sync::Arc<dyn wandur_core::login::PasswordVault>,
        wake: impl Fn() + Send + 'static,
    ) {
        if self.world.password_id.is_none() {
            return;
        }
        let check = SavedCheck::default();
        self.saved_check = Some(std::sync::Arc::clone(&check));
        let world = self.baseline.0.clone();
        let spawned = std::thread::Builder::new()
            .name("wandur-saved-password".into())
            .spawn(move || {
                let _ = check.set(wandur_core::login::credentials::has_saved(vault.as_ref(), &world));
                wake();
            });
        if spawned.is_err() {
            self.saved_check = None;
        }
    }

    /// The error shown above the buttons, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The store's answer, once it came (`None` while asking or not asked).
    pub fn saved_password_found(&self) -> Option<&Result<bool, String>> {
        self.saved_check.as_ref().and_then(|c| c.get())
    }

    /// The login as Save world would store it: port parsed, host and username trimmed, but
    /// spelled as saved when only spaces around them differ (they are part of the vault key,
    /// and a world imported or saved by an older version can have them).
    fn login_as_saved(&self) -> SavedWorld {
        let saved = &self.baseline.0;
        let keep = |typed: &str, saved: &str| {
            let typed = typed.trim();
            if typed == saved.trim() {
                saved.to_string()
            } else {
                typed.to_string()
            }
        };
        SavedWorld {
            port: self.port_text.trim().parse().unwrap_or(0),
            host: keep(&self.world.host, &saved.host),
            username: keep(&self.world.username, &saved.username),
            ..self.world.clone()
        }
    }

    /// What the password field says: whether a blank one keeps a saved password, and if not why.
    pub fn password_state(&self) -> PasswordState {
        if !self.remember || self.world.password_id.is_none() {
            return PasswordState::None;
        }
        // The same rule as Save world: the host and username decide; a changed port or TLS keeps
        // the password (it moves to the new key on save).
        let login = self.login_as_saved();
        if !wandur_core::login::credentials::same_login(&self.baseline.0, &login) {
            PasswordState::Changed
        } else if matches!(self.saved_password_found(), Some(Ok(false))) {
            PasswordState::Missing
        } else {
            PasswordState::Saved
        }
    }

    /// The app could not save the login (a vault error): show it on the Login section.
    pub fn login_failed(&mut self, error: String) {
        self.error = Some(error);
        self.section = Section::Login;
    }

    /// The "Save password" box: unticking it clears the password and auto-login (as C#).
    pub fn set_remember(&mut self, remember: bool) {
        self.remember = remember;
        if !remember {
            self.world.auto_login = false;
            self.password.clear();
        }
    }

    /// The world's saved library, for the Macros section.
    pub fn with_library(mut self, library: Vec<LibraryEntry>) -> Self {
        self.macros = MacroEditor::new(library);
        self
    }

    pub fn has_unsaved_changes(&self) -> bool {
        #[cfg(feature = "agent")]
        if self.agent.as_ref().is_some_and(|a| a.has_unsaved_changes()) {
            return true;
        }
        (self.world.clone(), self.port_text.clone()) != self.baseline
            || self.macros.has_changes()
            || self.pending_removal
            || !self.password.is_empty()
            || self.remember != self.baseline.0.password_id.is_some()
    }

    /// Check and finish the connection and login fields; the error says what to fix and the
    /// section shows where.
    pub fn finish(&mut self) -> Result<SavedWorld, String> {
        let login = self.login_as_saved();
        self.world.port = login.port;
        self.world.name = self.world.name.trim().to_string();
        self.world.host = login.host;
        self.world.username = login.username;
        if !self.remember {
            self.world.auto_login = false;
        }
        // The connection fields alone, then with the login as it will be saved (a new password
        // gets its reference when the app saves it).
        let connection = SavedWorld {
            username: String::new(),
            password_id: None,
            auto_login: false,
            username_prompt: wandur_core::login::DEFAULT_USERNAME_PROMPT.into(),
            password_prompt: wandur_core::login::DEFAULT_PASSWORD_PROMPT.into(),
            channel_rules: Vec::new(),
            ..self.world.clone()
        };
        if let Err(e) = connection.validate() {
            self.section = Section::Connection;
            return Err(e);
        }
        // The Channels section: names and replies trimmed (an empty reply is none), then the
        // world's own validation with its message (C# `ChannelRulesInvalid`).
        for rule in &mut self.world.channel_rules {
            rule.channel = rule.channel.trim().to_string();
            rule.reply_command = rule
                .reply_command
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(str::to_string);
        }
        if let Err(e) = wandur_core::channels::rules::validate(&self.world.channel_rules) {
            self.section = Section::Channels;
            return Err(e);
        }
        // A blank password keeps the saved one only when it can (the app checks again).
        if self.remember && self.password.is_empty() {
            let why = match self.password_state() {
                PasswordState::Changed => Some(S::PasswordLoginChanged),
                PasswordState::Missing => Some(S::SavedPasswordMissing),
                PasswordState::None | PasswordState::Saved => None,
            };
            if let Some(why) = why {
                self.section = Section::Login;
                return Err(t(why).into());
            }
        }
        let mut login = self.world.clone();
        login.password_id = match (self.remember, &login.password_id) {
            (false, _) => None,
            (true, Some(id)) => Some(id.clone()),
            (true, None) => (!self.password.is_empty()).then(|| "new".to_string()),
        };
        if let Err(e) = login.validate() {
            self.section = Section::Login;
            return Err(e);
        }
        let mut world = self.world.clone();
        // Another address (host, port or TLS) is another world as far as the directory knows:
        // the codebase and theme it gave for the old one go (C# `SaveProfileAsync`), and so does
        // the listing link, which the codebase is also read through. The taught channel rules,
        // the world id and the saved password stay.
        if self.index.is_some() && !self.baseline.0.is_at(&world.endpoint()) {
            world.codebase.clear();
            world.theme = None;
            world.listing_id.clear();
        }
        Ok(world)
    }

    /// The agent settings could not be saved: show why on their section.
    pub fn agent_failed(&mut self, error: String) {
        self.error = Some(error);
        self.section = Section::Agent;
    }

    /// Save world: every section validated, then the result.
    pub fn save(&mut self) -> FormResult {
        if self.pending_removal
            && let Some(i) = self.index
        {
            return FormResult::Removed(i);
        }
        let world = match self.finish() {
            Ok(w) => w,
            Err(e) => {
                self.error = Some(e);
                return FormResult::Open;
            }
        };
        // The agent settings are checked before anything is written (C# `ValidateDraft`).
        #[cfg(feature = "agent")]
        if let Some(agent) = self.agent.as_ref().filter(|a| a.has_unsaved_changes())
            && let Err(e) = agent.validate()
        {
            self.error = Some(e);
            self.section = Section::Agent;
            return FormResult::Open;
        }
        let library = if self.macros.has_changes() {
            match self.macros.finish() {
                Ok(after) => Some(LibraryChange {
                    before: self.macros.baseline.clone(),
                    after,
                }),
                Err(e) => {
                    self.error = Some(e);
                    self.section = Section::Macros;
                    return FormResult::Open;
                }
            }
        } else {
            None
        };
        FormResult::Saved {
            index: self.index,
            world,
            library,
            password: self.password.clone(),
            remember: self.remember,
        }
    }

    /// The editor in its own window (the C# `ProfileDialog`: "World settings", 1100 by 780,
    /// at least 780 by 560, resizable). Its close button and Escape cancel, as Cancel does.
    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme, worlds: &[SavedWorld]) -> FormResult {
        let mut result = FormResult::Open;
        let spec = crate::dialog_window::Spec {
            id: WINDOW_ID,
            title: t(S::WorldSettings),
            default_size: egui::vec2(1100.0, 780.0),
            min_size: egui::vec2(780.0, 560.0),
        };
        let close = crate::dialog_window::show(ctx, spec, theme, |ui| self.content(ui, theme, worlds, &mut result));
        if matches!(result, FormResult::Open) && close.requested() {
            result = FormResult::Cancelled;
        }
        result
    }

    /// The bar, the sections, the section and the footer, laid out over the whole of `ui`.
    fn content(&mut self, ui: &mut Ui, theme: &Theme, worlds: &[SavedWorld], result: &mut FormResult) {
        ui.spacing_mut().interact_size.y = 30.0;
        let full = ui.max_rect();
        let bar_height = 44.0;
        let footer = if self.error.is_some() { 86.0 } else { 54.0 };
        let nav_width = if self.sections_visible { 160.0 } else { 0.0 };
        let bar = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), bar_height));
        let foot = egui::Rect::from_min_max(egui::pos2(full.left(), full.bottom() - footer), full.max);
        let nav = egui::Rect::from_min_max(
            egui::pos2(full.left(), bar.bottom()),
            egui::pos2(full.left() + nav_width, foot.top()),
        );
        let body = egui::Rect::from_min_max(
            egui::pos2(nav.right(), bar.bottom()),
            egui::pos2(full.right(), foot.top()),
        );
        let p = ui.painter();
        p.rect_filled(bar, 0.0, theme.shell);
        p.hline(full.x_range(), bar.bottom(), egui::Stroke::new(1.0, theme.border));
        p.hline(full.x_range(), foot.top(), egui::Stroke::new(1.0, theme.border));
        if self.sections_visible {
            p.rect_filled(nav, 0.0, theme.shell);
            p.vline(nav.right(), nav.y_range(), egui::Stroke::new(1.0, theme.border));
        }
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(bar.shrink2(egui::vec2(8.0, 6.0))),
            |ui| self.top_bar(ui, theme, worlds, result),
        );
        if self.sections_visible {
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(nav.shrink2(egui::vec2(8.0, 16.0))),
                |ui| self.nav(ui, theme),
            );
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            ui.set_clip_rect(body.intersect(ui.clip_rect()));
            match self.section {
                Section::Macros => self.macros_section(ui, theme),
                Section::Scripts => self.scripts_section(ui, theme),
                Section::Login => self.login_section(ui, theme),
                Section::Channels => self.channels_section(ui, theme),
                #[cfg(feature = "agent")]
                Section::Agent => match &mut self.agent {
                    Some(agent) => crate::agent_settings::show(ui, agent, theme),
                    None => {
                        ui.add_space(24.0);
                        ui.label(RichText::new(t(S::AgentConfigurationRequired)).color(theme.muted));
                    }
                },
                _ => self.connection_section(ui, theme),
            }
        });
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(foot.shrink2(egui::vec2(16.0, 10.0))),
            |ui| self.footer(ui, theme, result),
        );
        if let Some(toast) = &mut self.toast {
            let id = egui::Id::new("world-editor-toast");
            let (out, expired) = crate::toast::show(
                ui.ctx(),
                id,
                body,
                crate::toast::BOTTOM_GAP,
                toast,
                theme,
                std::time::Instant::now(),
            );
            if expired {
                self.toast = None;
            } else if out == crate::toast::Output::Undo
                && let Some(toast) = self.toast.take()
            {
                self.macros.restore(toast.undo);
            }
        }
    }

    fn top_bar(&mut self, ui: &mut Ui, theme: &Theme, worlds: &[SavedWorld], result: &mut FormResult) {
        let rect = ui.max_rect();
        ui.horizontal_centered(|ui| {
            let toggle =
                widgets::tool_button(ui, Icon::Sidebar, None, theme, true).on_hover_text(t(S::ProfileToggleSections));
            if toggle.clicked() {
                self.sections_visible = !self.sections_visible;
            }
        });
        // The world picker, centred, at most 460 wide.
        let picker_width = (rect.width() - 160.0).clamp(160.0, 460.0);
        let picker = egui::Rect::from_center_size(rect.center(), egui::vec2(picker_width, 30.0));
        let mut chosen = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(picker), |ui| {
            let mut index = self.index.unwrap_or(usize::MAX);
            let response = Select::new("world-form-world", t(S::ChooseAWorld))
                .width(picker_width)
                .height(picker.height())
                .font_size(13.0)
                .max_list_height(360.0)
                .placeholder(t(S::NewWorld))
                .show_index(ui, &mut index, worlds.len(), |i| worlds[i].name.as_str());
            if response.changed() && self.index != Some(index) {
                chosen = Some(Some(index));
            }
        });
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                // "New world", not a bare "New" beside the world picker (C# UI review, item 13).
                if dialogs::secondary_button(ui, t(S::NewWorld)).clicked() {
                    chosen = Some(None);
                }
            },
        );
        if let Some(target) = chosen {
            if self.has_unsaved_changes() {
                self.switch_to = Some(target);
            } else {
                *result = FormResult::Switch(target);
            }
        }
    }

    fn nav(&mut self, ui: &mut Ui, theme: &Theme) {
        for section in Section::ALL {
            let selected = self.section == section;
            let enabled = section.available();
            let sense = if enabled {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            };
            let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 37.0), sense);
            if selected {
                ui.painter().rect_filled(rect, 3.0, theme.selection_fill());
            } else if enabled && response.hovered() {
                ui.painter().rect_filled(rect, 3.0, theme.hover_fill());
            }
            ui.painter().text(
                egui::pos2(rect.left() + 12.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                section.label(),
                egui::FontId::proportional(14.0),
                if enabled { theme.text } else { theme.disabled_text() },
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, enabled, selected, section.label())
            });
            if response.clicked() {
                self.section = section;
            }
            ui.add_space(4.0);
        }
    }

    fn connection_section(&mut self, ui: &mut Ui, theme: &Theme) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let outer = ui.available_width();
            let width = (outer - 56.0).min(700.0);
            ui.horizontal(|ui| {
                ui.add_space((outer - width) / 2.0);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.add_space(24.0);
                    ui.label(
                        RichText::new(t(S::ProfileConnectionSection))
                            .size(20.0)
                            .strong()
                            .color(theme.text),
                    );
                    ui.add_space(14.0);
                    field_label(ui, t(S::WorldName), theme);
                    crate::a11y::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.world.name)
                                .hint_text(t(S::AFamiliarWorld))
                                .char_limit(100)
                                .margin(egui::Margin::symmetric(10, 7))
                                .desired_width(width),
                        ),
                        t(S::WorldName),
                    );
                    ui.add_space(12.0);
                    let port_width = 140.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.vertical(|ui| {
                            ui.set_width(width - port_width - 10.0);
                            field_label(ui, t(S::HostnameAcceptsHostPort), theme);
                            crate::a11y::named(
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.world.host)
                                        .hint_text(t(S::MudExampleOrg4000))
                                        .margin(egui::Margin::symmetric(10, 7))
                                        .desired_width(width - port_width - 10.0),
                                ),
                                t(S::HostnameAcceptsHostPort),
                            );
                        });
                        ui.vertical(|ui| {
                            ui.set_width(port_width);
                            field_label(ui, t(S::Port), theme);
                            crate::a11y::named(
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.port_text)
                                        .char_limit(5)
                                        .margin(egui::Margin::symmetric(10, 7))
                                        .desired_width(port_width),
                                ),
                                t(S::Port),
                            );
                        });
                    });
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(t(S::PasteAnAddressToFindItsNameInThe))
                            .size(11.0)
                            .color(theme.muted),
                    );
                    ui.add_space(12.0);
                    field_label(ui, t(S::TextEncoding), theme);
                    ui.add_space(6.0);
                    Select::new("world-form-charset", t(S::TextEncoding))
                        .width(width)
                        .height(32.0)
                        .show_value(
                            ui,
                            &mut self.world.charset,
                            &[
                                (Charset::Utf8, charset_name(Charset::Utf8)),
                                (Charset::Latin1, charset_name(Charset::Latin1)),
                            ],
                        );
                    ui.add_space(12.0);
                    ui.checkbox(
                        &mut self.world.tls,
                        RichText::new(t(S::UseTLSServerMustSupportIt)).size(14.0),
                    );
                    ui.add_space(8.0);
                    // This client's own per-world choice (C# has the global setting only).
                    ui.checkbox(
                        &mut self.world.auto_reconnect,
                        RichText::new(t(S::ReconnectAutomaticallyHint)).size(14.0),
                    );
                    ui.add_space(16.0);
                });
            });
        });
    }

    /// The Login section, as the C# one: username, Save password (in the system store),
    /// password (masked; blank keeps a saved one), auto-login, the store note, and the custom
    /// prompt patterns in an expander.
    fn login_section(&mut self, ui: &mut Ui, theme: &Theme) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let outer = ui.available_width();
            let width = (outer - 56.0).min(700.0);
            ui.horizontal(|ui| {
                ui.add_space((outer - width) / 2.0);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.add_space(24.0);
                    ui.label(
                        RichText::new(t(S::ProfileLoginSection))
                            .size(20.0)
                            .strong()
                            .color(theme.text),
                    );
                    ui.add_space(12.0);
                    field_label(ui, t(S::Username), theme);
                    crate::a11y::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.world.username)
                                .id_salt("login-username")
                                .hint_text(t(S::CharacterOrAccountName))
                                .char_limit(256)
                                .margin(egui::Margin::symmetric(10, 7))
                                .desired_width(width),
                        ),
                        t(S::Username),
                    );
                    ui.add_space(10.0);
                    let mut remember = self.remember;
                    if ui
                        .checkbox(
                            &mut remember,
                            RichText::new(t(S::SavePasswordInSystemCredentialStore)).size(14.0),
                        )
                        .changed()
                    {
                        self.set_remember(remember);
                    }
                    ui.add_space(10.0);
                    field_label(ui, t(S::Password), theme);
                    let hint = match self.password_state() {
                        PasswordState::None => t(S::Password),
                        PasswordState::Saved => t(S::SavedLeaveBlankToKeep),
                        PasswordState::Changed => t(S::PasswordLoginChangedHint),
                        PasswordState::Missing => t(S::SavedPasswordMissing),
                    };
                    crate::a11y::named(
                        ui.add_enabled(
                            self.remember,
                            egui::TextEdit::singleline(&mut self.password)
                                .id_salt("login-password")
                                .password(true)
                                .hint_text(hint)
                                .char_limit(wandur_core::login::vault::MAX_PASSWORD)
                                .margin(egui::Margin::symmetric(10, 7))
                                .desired_width(width),
                        ),
                        t(S::Password),
                    );
                    ui.add_space(10.0);
                    ui.add_enabled(
                        self.remember,
                        egui::Checkbox::new(
                            &mut self.world.auto_login,
                            RichText::new(t(S::AutomaticallyLogInOnConnect)).size(14.0),
                        ),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(tf(S::PasswordsAreStoredInAutoLoginWaitsForEach, &[&self.vault_name]))
                            .size(11.0)
                            .color(theme.muted),
                    );
                    ui.add_space(8.0);
                    self.prompts_expander(ui, theme, width);
                    ui.add_space(16.0);
                });
            });
        });
    }

    /// The Channels section, as the C# `ChannelRulesView`: the rules taught from the
    /// transcript, one card each, with the channel, the reply command and the pattern editable,
    /// an Enabled box, the rule's kind (an exclusion, a private channel) and Delete. The
    /// teaching dialog is where a rule is built; this is where a broken one is fixed or dropped.
    fn channels_section(&mut self, ui: &mut Ui, theme: &Theme) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let outer = ui.available_width();
            let width = (outer - 56.0).min(700.0);
            let mut delete = None;
            ui.horizontal(|ui| {
                ui.add_space((outer - width) / 2.0);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.add_space(24.0);
                    ui.label(
                        RichText::new(t(S::ProfileChannelsSection))
                            .size(20.0)
                            .strong()
                            .color(theme.text),
                    );
                    ui.add_space(8.0);
                    ui.add(
                        egui::Label::new(RichText::new(t(S::ProfileChannelsHelp)).size(12.0).color(theme.muted)).wrap(),
                    );
                    ui.add_space(8.0);
                    if self.world.channel_rules.is_empty() {
                        ui.add(
                            egui::Label::new(RichText::new(t(S::ProfileChannelsEmpty)).size(12.0).color(theme.muted))
                                .wrap(),
                        );
                    }
                    for (i, rule) in self.world.channel_rules.iter_mut().enumerate() {
                        if channel_rule_card(ui, i, rule, width, theme) {
                            delete = Some(i);
                        }
                        ui.add_space(4.0);
                    }
                    ui.add_space(16.0);
                });
            });
            if let Some(i) = delete {
                self.world.channel_rules.remove(i);
            }
        });
    }

    /// "Custom login prompts": a header that opens the two pattern fields.
    fn prompts_expander(&mut self, ui: &mut Ui, theme: &Theme, width: f32) {
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(egui::Stroke::new(1.0, theme.border))
            .corner_radius(4)
            .show(ui, |ui| {
                ui.set_width(width - 2.0);
                let (rect, header) = ui.allocate_exact_size(egui::vec2(width - 2.0, 46.0), egui::Sense::click());
                ui.painter().text(
                    egui::pos2(rect.left() + 16.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    t(S::CustomLoginPrompts),
                    egui::FontId::proportional(14.0),
                    theme.text,
                );
                let chevron = egui::Rect::from_center_size(
                    egui::pos2(rect.right() - 24.0, rect.center().y),
                    egui::vec2(12.0, 12.0),
                );
                let icon = if self.prompts_open {
                    Icon::ChevronUp
                } else {
                    Icon::ChevronDown
                };
                widgets::paint_icon(ui, icon, chevron, theme.muted);
                header.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::CollapsingHeader,
                        true,
                        self.prompts_open,
                        t(S::CustomLoginPrompts),
                    )
                });
                if header.clicked() {
                    self.prompts_open = !self.prompts_open;
                }
                if !self.prompts_open {
                    return;
                }
                ui.painter()
                    .hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, theme.border));
                egui::Frame::new().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                    let inner = width - 34.0;
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.label(
                        RichText::new(t(S::CaseInsensitiveRegularExpressionsMatchTheWholePromptLine))
                            .size(11.0)
                            .color(theme.muted),
                    );
                    ui.add_space(6.0);
                    field_label(ui, t(S::UsernamePrompt), theme);
                    crate::a11y::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.world.username_prompt)
                                .id_salt("login-username-prompt")
                                .font(egui::TextStyle::Monospace)
                                .char_limit(wandur_core::login::sequence::MAX_PATTERN)
                                .margin(egui::Margin::symmetric(10, 7))
                                .desired_width(inner),
                        ),
                        t(S::UsernamePrompt),
                    );
                    ui.add_space(6.0);
                    field_label(ui, t(S::PasswordPrompt), theme);
                    crate::a11y::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut self.world.password_prompt)
                                .id_salt("login-password-prompt")
                                .font(egui::TextStyle::Monospace)
                                .char_limit(wandur_core::login::sequence::MAX_PATTERN)
                                .margin(egui::Margin::symmetric(10, 7))
                                .desired_width(inner),
                        ),
                        t(S::PasswordPrompt),
                    );
                });
            });
    }

    /// The Scripts section (the C# `ScriptLibraryView` inside the world editor): New, Duplicate,
    /// Delete, the name, Enabled, Output and Help on top; the script list and the code editor;
    /// the output panel; the status line.
    fn scripts_section(&mut self, ui: &mut Ui, theme: &Theme) {
        let full = ui.max_rect();
        let toolbar = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), 38.0));
        let status_h = 26.0;
        let output_h = if self.macros.show_output { 138.0 } else { 0.0 };
        let pack = self.macros.selected_script_entry().and_then(LibraryEntry::pack);
        let p = ui.painter();
        p.rect_filled(toolbar, 0.0, theme.shell);
        p.hline(
            toolbar.x_range(),
            toolbar.bottom(),
            egui::Stroke::new(1.0, theme.border),
        );
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(toolbar.shrink2(egui::vec2(6.0, 4.0))),
            |ui| self.script_toolbar(ui, theme),
        );
        // The notices take the height they need (they wrap in a narrow window).
        let mut body_top = toolbar.bottom();
        let notices = egui::Rect::from_min_max(
            egui::pos2(full.left() + 10.0, toolbar.bottom() + 6.0),
            egui::pos2(full.right() - 10.0, full.bottom() - status_h - output_h - 120.0),
        );
        if let Some(info) = &pack {
            let used = ui
                .scope_builder(egui::UiBuilder::new().max_rect(notices), |ui| {
                    self.pack_notice(ui, info, theme)
                })
                .response
                .rect;
            body_top = used.bottom() + 2.0;
        }
        let list_w = 150.0;
        let body_bottom = full.bottom() - status_h - output_h;
        let list = egui::Rect::from_min_max(
            egui::pos2(full.left() + 4.0, body_top + 4.0),
            egui::pos2(full.left() + 4.0 + list_w, body_bottom - 4.0),
        );
        let editor = egui::Rect::from_min_max(
            egui::pos2(list.right() + 5.0, body_top + 4.0),
            egui::pos2(full.right() - 4.0, body_bottom - 4.0),
        );
        ui.painter().rect_filled(list, 0.0, theme.hover_fill());
        ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| self.script_list(ui, theme));
        ui.scope_builder(egui::UiBuilder::new().max_rect(editor), |ui| {
            ui.set_clip_rect(editor);
            if self.macros.scripts().next().is_none() {
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.label(RichText::new(t(S::ScriptLibraryEmpty)).size(14.0).color(theme.muted));
                });
                return;
            }
            let id = egui::Id::new("world-script-source");
            let read_only = self
                .macros
                .selected_script_entry()
                .is_some_and(|e| e.pack_json.is_some());
            let MacroEditor {
                entries,
                selected_script,
                editor: state,
                shown,
                ..
            } = &mut self.macros;
            if let Some(entry) = entries.iter_mut().find(|e| Some(&e.id) == selected_script.as_ref()) {
                state.lua = entry.is_lua();
                // The script is shown formatted; the draft takes the text only once it is edited.
                let view = match shown {
                    Some(view) if view.shows(&entry.id, &entry.source) => view,
                    _ => shown.insert(crate::script_editor::Shown::new(
                        &entry.id,
                        &entry.source,
                        entry.is_lua(),
                    )),
                };
                crate::script_editor::code_editor(ui, id, &mut view.text, read_only, state, theme);
                if !read_only && view.source() != entry.source {
                    entry.source = view.source().to_string();
                }
            }
            // Typing, pasting or deleting in a pack script asks whether to make an editable copy.
            if let Some(tried) = self.macros.editor.blocked_edit.take() {
                self.macros.pack_prompt.get_or_insert_with(Vec::new).extend(tried);
            }
        });
        if self.macros.show_output {
            let output = egui::Rect::from_min_max(
                egui::pos2(full.left() + 4.0, body_bottom),
                egui::pos2(full.right() - 4.0, body_bottom + output_h - 4.0),
            );
            ui.painter().rect(
                output,
                3.0,
                theme.hover_fill(),
                egui::Stroke::new(1.0, theme.border),
                egui::StrokeKind::Inside,
            );
            // The editor runs nothing, so a script's output belongs to its session (C#).
            let log = String::new();
            ui.scope_builder(egui::UiBuilder::new().max_rect(output.shrink(8.0)), |ui| {
                egui::ScrollArea::vertical().id_salt("script-output").show(ui, |ui| {
                    if !log.is_empty() {
                        ui.label(RichText::new(log).monospace().size(12.0).color(theme.text));
                    }
                });
            });
        }
        if let Some(entry) = self.macros.selected_script_entry() {
            let mut text = self.macros.status(entry).to_string();
            if self.macros.is_unsaved(&entry.id) {
                text = format!("{text}    {}", t(S::ScriptUnsaved));
            }
            ui.painter().text(
                egui::pos2(full.left() + 10.0, full.bottom() - status_h / 2.0),
                egui::Align2::LEFT_CENTER,
                text,
                egui::FontId::proportional(11.0),
                theme.muted,
            );
        }
    }

    /// The pack script notice (the C# `PackScriptNotice`, made plainer): a lock, where the
    /// script came from and Duplicate; that it is read only; what it does; and, once someone
    /// tried to type in it, the offer of an editable copy.
    fn pack_notice(&mut self, ui: &mut Ui, info: &scripts::PackInfo, theme: &Theme) {
        ui.spacing_mut().item_spacing.y = 4.0;
        let mut duplicate = false;
        ui.horizontal(|ui| {
            let (lock, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
            widgets::paint_icon(ui, Icon::Lock, lock, theme.muted);
            ui.label(RichText::new(pack_label(info)).size(11.0).color(theme.muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                duplicate = ui
                    .add(egui::Button::new(RichText::new(t(S::ScriptDuplicate)).size(12.0)))
                    .clicked();
            });
        });
        ui.add(
            egui::Label::new(
                RichText::new(t(S::ScriptPackReadOnlyNotice))
                    .size(12.0)
                    .color(theme.text),
            )
            .wrap(),
        );
        if !info.description.trim().is_empty() {
            ui.add(egui::Label::new(RichText::new(&info.description).size(11.0).color(theme.muted)).wrap());
        }
        let mut copy = false;
        if self.macros.pack_prompt.is_some() {
            egui::Frame::new()
                .fill(theme.selection_fill())
                .corner_radius(3)
                .inner_margin(egui::Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(t(S::ScriptPackEditPrompt)).size(12.0).color(theme.text));
                        ui.add_space(8.0);
                        copy = dialogs::primary_button_small(ui, t(S::ScriptPackMakeCopy), theme).clicked();
                        if ui.button(RichText::new(t(S::Cancel)).size(12.0)).clicked() {
                            self.macros.pack_prompt = None;
                        }
                    });
                });
        }
        if copy && let Err(e) = self.macros.make_editable_copy() {
            self.error = Some(e);
        } else if duplicate && let Err(e) = self.macros.duplicate_script() {
            self.error = Some(e);
        }
    }

    fn script_toolbar(&mut self, ui: &mut Ui, theme: &Theme) {
        let has_selected = self.macros.selected_script_entry().is_some();
        let read_only = self
            .macros
            .selected_script_entry()
            .is_some_and(|e| e.pack_json.is_some());
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if widgets::tool_button(ui, Icon::Plus, None, theme, true)
                .on_hover_text(t(S::ScriptNew))
                .clicked()
                && let Err(e) = self.macros.add_script()
            {
                self.error = Some(e);
            }
            if widgets::tool_button(ui, Icon::Copy, None, theme, has_selected)
                .on_hover_text(t(S::ScriptDuplicate))
                .clicked()
                && let Err(e) = self.macros.duplicate_script()
            {
                self.error = Some(e);
            }
            if widgets::tool_button(ui, Icon::Trash, None, theme, has_selected)
                .on_hover_text(t(S::ScriptDelete))
                .clicked()
            {
                self.toast = self.macros.delete_script().or(self.toast.take());
            }
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let help = widgets::tool_button(ui, Icon::Help, None, theme, true).on_hover_text(t(S::ScriptHelpTitle));
                if help.clicked() {
                    self.macros.show_help = !self.macros.show_help;
                }
                if self.macros.show_help {
                    let mut open = true;
                    egui::Popup::from_response(&help)
                        .id(egui::Id::new("script-help"))
                        .open_bool(&mut open)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .align(egui::RectAlign::BOTTOM_END)
                        .show(|ui| {
                            ui.set_max_width(560.0);
                            ui.label(RichText::new(t(S::ScriptCompletionHint)).size(12.0).color(theme.muted));
                            ui.add_space(6.0);
                            egui::ScrollArea::both().max_height(320.0).show(ui, |ui| {
                                ui.label(RichText::new(t(S::ScriptApiExamples)).monospace().size(12.0));
                            });
                        });
                    self.macros.show_help = open;
                }
                let output =
                    widgets::tool_button(ui, Icon::Console, None, theme, true).on_hover_text(t(S::ScriptOutputLabel));
                if self.macros.show_output {
                    ui.painter()
                        .rect_filled(output.rect.shrink(1.0), 4.0, theme.selection_fill());
                    widgets::paint_icon(ui, Icon::Console, output.rect, theme.text);
                }
                if output.clicked() {
                    self.macros.show_output = !self.macros.show_output;
                }
                ui.add_space(6.0);
                let mut enabled = self.macros.selected_script_entry().is_some_and(|e| e.enabled);
                let is_pack = self.macros.selected_script_entry().is_some_and(LibraryEntry::is_pack);
                let check = ui
                    .add_enabled(
                        has_selected,
                        egui::Checkbox::new(&mut enabled, RichText::new(t(S::ScriptEnabled)).size(11.0)),
                    )
                    .on_hover_text(t(S::ScriptEnableHint));
                if check.changed()
                    && let Err(e) = self.macros.set_script_enabled(enabled)
                {
                    self.error = Some(e);
                }
                if is_pack {
                    // The person's choice to lift a supplied script's send policy (saved with
                    // the world; open sessions take it at once).
                    ui.add_space(8.0);
                    let mut allow = self
                        .macros
                        .selected_script_entry()
                        .is_some_and(LibraryEntry::allow_send);
                    let check = ui.add(egui::Checkbox::new(
                        &mut allow,
                        RichText::new(t(S::ScriptPackAllowSend)).size(11.0),
                    ));
                    if check.changed()
                        && let Some(entry) = self.macros.selected_script_mut()
                    {
                        entry.set_allow_send(allow);
                    }
                }
                self.script_language(ui, theme);
                ui.add_space(8.0);
                let mut empty = String::new();
                let name = match self.macros.selected_script_mut() {
                    Some(entry) => &mut entry.name,
                    None => &mut empty,
                };
                crate::a11y::named(
                    ui.add_enabled(
                        has_selected && !read_only,
                        egui::TextEdit::singleline(name)
                            .hint_text(t(S::ScriptName))
                            .char_limit(scripts::MAX_NAME)
                            .margin(egui::Margin::symmetric(6, 5))
                            .desired_width(ui.available_width()),
                    ),
                    t(S::ScriptName),
                );
            });
        });
    }

    /// The C# language picker (JavaScript or Lua, once Lua is allowed or for a script already in
    /// Lua), Mudlet names for a Lua script, and Run as Lua for imported Mudlet code; drawn right
    /// to left before the name box.
    fn script_language(&mut self, ui: &mut Ui, theme: &Theme) {
        let Some(entry) = self.macros.selected_script_entry() else {
            return;
        };
        let show_language =
            !entry.is_pack() && !entry.is_macro() && !entry.needs_conversion() && (self.lua_enabled || entry.is_lua());
        let can_run_as_lua = self.lua_enabled && entry.needs_conversion();
        let lua = entry.is_lua();
        let mudlet = entry.compatibility == Compatibility::Mudlet;
        if show_language && lua {
            ui.add_space(8.0);
            let mut on = mudlet;
            let check = ui
                .add(egui::Checkbox::new(
                    &mut on,
                    RichText::new(t(S::ScriptMudletCompatibility)).size(11.0),
                ))
                .on_hover_text(t(S::ScriptMudletCompatibilityHint));
            if check.changed() {
                self.macros.set_script_mudlet(on);
            }
        }
        if show_language {
            ui.add_space(8.0);
            let mut chosen = if lua { Language::Lua } else { Language::JavaScript };
            let label = |l: Language| {
                t(match l {
                    Language::JavaScript => S::ScriptLanguageJavaScript,
                    Language::Lua => S::ScriptLanguageLua,
                })
            };
            Select::new("world-script-language", t(S::ScriptLanguageLabel))
                .width(112.0)
                .height(24.0)
                .font_size(11.0)
                .show_value(
                    ui,
                    &mut chosen,
                    &[
                        (Language::JavaScript, label(Language::JavaScript)),
                        (Language::Lua, label(Language::Lua)),
                    ],
                )
                .on_hover_text(t(S::ScriptLanguageLabel));
            if chosen != (if lua { Language::Lua } else { Language::JavaScript }) {
                self.macros.set_script_language(chosen);
            }
        }
        if can_run_as_lua {
            ui.add_space(8.0);
            if ui
                .add(egui::Button::new(
                    RichText::new(t(S::MudletRunAsLua)).size(11.0).color(theme.text),
                ))
                .on_hover_text(t(S::MudletRunAsLuaHint))
                .clicked()
            {
                self.macros.run_import_as_lua();
            }
        }
    }

    fn script_list(&mut self, ui: &mut Ui, theme: &Theme) {
        egui::ScrollArea::vertical()
            .id_salt("script-list")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut chosen = None;
                for entry in self.macros.scripts() {
                    let selected = self.macros.selected_script.as_deref() == Some(entry.id.as_str());
                    let pack = entry.pack();
                    let imported = MacroEditor::import_label(entry);
                    let height = if pack.is_some() || imported.is_some() {
                        61.0
                    } else {
                        47.0
                    };
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click());
                    if selected {
                        ui.painter().rect_filled(rect, 0.0, theme.selection_fill());
                    } else if response.hovered() {
                        ui.painter().rect_filled(rect, 0.0, theme.hover_fill());
                    }
                    let name = widgets::clipped(
                        ui,
                        &entry.name,
                        egui::FontId::proportional(12.0),
                        theme.text,
                        rect.width() - 20.0,
                        1,
                    );
                    ui.painter().galley(
                        egui::pos2(rect.left() + 11.0, rect.top() + 15.0 - name.size().y / 2.0),
                        name,
                        theme.text,
                    );
                    ui.painter().text(
                        egui::pos2(rect.left() + 11.0, rect.top() + 32.0),
                        egui::Align2::LEFT_CENTER,
                        self.macros.status(entry),
                        egui::FontId::proportional(10.0),
                        theme.muted,
                    );
                    let marker = pack.as_ref().map(pack_label).or(imported.map(str::to_string));
                    if let Some(marker) = marker {
                        ui.painter().text(
                            egui::pos2(rect.left() + 11.0, rect.top() + 46.0),
                            egui::Align2::LEFT_CENTER,
                            marker,
                            egui::FontId::proportional(10.0),
                            theme.muted,
                        );
                    }
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &entry.name)
                    });
                    if response.clicked() {
                        chosen = Some(entry.id.clone());
                    }
                }
                if let Some(id) = chosen {
                    self.macros.select_script(&id);
                }
            });
    }

    fn macros_section(&mut self, ui: &mut Ui, theme: &Theme) {
        let full = ui.max_rect();
        let toolbar = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), 38.0));
        let status_h = 24.0;
        let list_w = 170.0;
        let body_top = toolbar.bottom();
        let list = egui::Rect::from_min_max(
            egui::pos2(full.left(), body_top),
            egui::pos2(full.left() + list_w, full.bottom() - status_h),
        );
        let form = egui::Rect::from_min_max(egui::pos2(list.right() + 5.0, body_top), full.max);
        let p = ui.painter();
        p.rect_filled(toolbar, 0.0, theme.shell);
        p.hline(
            toolbar.x_range(),
            toolbar.bottom(),
            egui::Stroke::new(1.0, theme.border),
        );
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(list.right(), body_top),
                egui::pos2(list.right() + 5.0, full.bottom()),
            ),
            0.0,
            theme.hover_fill(),
        );
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(toolbar.shrink2(egui::vec2(6.0, 4.0))),
            |ui| self.macro_toolbar(ui, theme),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| self.macro_list(ui, theme));
        let status = egui::Rect::from_min_max(
            egui::pos2(full.left() + 10.0, list.bottom()),
            egui::pos2(list.right(), full.bottom()),
        );
        if let Some(entry) = self.macros.selected_entry() {
            let mut text = self.macros.status(entry).to_string();
            if self.macros.is_unsaved(&entry.id) {
                text = format!("{text}    {}", t(S::ScriptUnsaved));
            }
            ui.painter().text(
                egui::pos2(status.left(), status.center().y),
                egui::Align2::LEFT_CENTER,
                text,
                egui::FontId::proportional(11.0),
                theme.muted,
            );
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(form), |ui| self.macro_form(ui, theme));
    }

    fn macro_toolbar(&mut self, ui: &mut Ui, theme: &Theme) {
        let has_selected = self.macros.selected_entry().is_some();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if widgets::tool_button(ui, Icon::Plus, None, theme, true).clicked()
                && let Err(e) = self.macros.add()
            {
                self.error = Some(e);
            }
            if widgets::tool_button(ui, Icon::Trash, None, theme, has_selected).clicked() {
                self.toast = self.macros.delete_selected().or(self.toast.take());
            }
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let mut enabled = self.macros.selected_entry().is_some_and(|e| e.enabled);
                let check = ui.add_enabled(
                    has_selected,
                    egui::Checkbox::new(&mut enabled, RichText::new(t(S::ScriptEnabled)).size(12.0)),
                );
                if check.changed()
                    && let Some(entry) = self.macros.selected_mut()
                {
                    entry.enabled = enabled;
                }
                let mut empty = String::new();
                let name = match self.macros.selected_mut() {
                    Some(entry) => &mut entry.name,
                    None => &mut empty,
                };
                crate::a11y::named(
                    ui.add_enabled(
                        has_selected,
                        egui::TextEdit::singleline(name)
                            .hint_text(t(S::ScriptName))
                            .char_limit(scripts::MAX_NAME)
                            .margin(egui::Margin::symmetric(6, 5))
                            .desired_width(ui.available_width()),
                    ),
                    t(S::ScriptName),
                );
            });
        });
    }

    fn macro_list(&mut self, ui: &mut Ui, theme: &Theme) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut chosen = None;
            for entry in self.macros.macros() {
                let selected = self.macros.selected.as_deref() == Some(entry.id.as_str());
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), 49.0), egui::Sense::click());
                if selected {
                    ui.painter().rect_filled(rect, 0.0, theme.hover_fill());
                }
                let name = ui.painter().layout(
                    entry.name.clone(),
                    egui::FontId::proportional(12.0),
                    theme.text,
                    rect.width() - 22.0,
                );
                ui.painter()
                    .galley(egui::pos2(rect.left() + 11.0, rect.top() + 9.0), name, theme.text);
                ui.painter().text(
                    egui::pos2(rect.left() + 11.0, rect.top() + 33.0),
                    egui::Align2::LEFT_CENTER,
                    self.macros.status(entry),
                    egui::FontId::proportional(10.0),
                    theme.muted,
                );
                response.widget_info(|| {
                    egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &entry.name)
                });
                if response.clicked() {
                    chosen = Some(entry.id.clone());
                }
            }
            if let Some(id) = chosen {
                self.macros.selected = Some(id);
            }
        });
    }

    fn macro_form(&mut self, ui: &mut Ui, theme: &Theme) {
        if self.macros.macros().next().is_none() {
            ui.add_space(28.0);
            ui.horizontal(|ui| {
                ui.add_space(28.0);
                ui.label(RichText::new(t(S::MacroEmpty)).size(14.0).color(theme.muted));
            });
            return;
        }
        let Some(def) = self.macros.selected_entry().and_then(|e| e.macro_def.clone()) else {
            return;
        };
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let width = (ui.available_width() - 44.0).min(660.0);
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    self.macro_fields(ui, theme, &def, width);
                    ui.add_space(16.0);
                });
            });
        });
    }

    fn macro_fields(&mut self, ui: &mut Ui, theme: &Theme, def: &MacroDefinition, width: f32) {
        let small = |ui: &mut Ui, key: S| {
            ui.label(RichText::new(t(key)).size(12.0).color(theme.muted));
        };
        let gap = |ui: &mut Ui| ui.add_space(8.0);
        small(ui, S::MacroType);
        let mut kind = def.kind;
        let kinds: Vec<(MacroKind, &str)> = MacroKind::ALL.iter().map(|&k| (k, k.label())).collect();
        Select::new("macro-kind", t(S::MacroType))
            .width(width)
            .height(28.0)
            .font_size(12.0)
            .show_value(ui, &mut kind, &kinds);
        if kind != def.kind {
            self.macros.set_kind(kind);
        }
        let Some(def) = self.macros.definition_mut() else {
            return;
        };
        let text_kind = matches!(def.kind, MacroKind::Trigger | MacroKind::Alias);
        if text_kind {
            gap(ui);
            small(ui, S::MacroPattern);
            crate::a11y::named(
                ui.add(
                    egui::TextEdit::singleline(&mut def.pattern)
                        .char_limit(wandur_core::macros::definition::MAX_LINE)
                        .margin(egui::Margin::symmetric(10, 7))
                        .desired_width(width),
                ),
                t(S::MacroPattern),
            );
        }
        if def.kind == MacroKind::Trigger {
            gap(ui);
            small(ui, S::MacroMatchLabel);
            let matches: Vec<(MacroMatch, &str)> = MacroMatch::ALL.iter().map(|&m| (m, m.label())).collect();
            Select::new("macro-match", t(S::MacroMatchLabel))
                .width(width)
                .height(28.0)
                .font_size(12.0)
                .show_value(ui, &mut def.match_kind, &matches);
        }
        if text_kind {
            gap(ui);
            ui.add_space(6.0);
            ui.checkbox(&mut def.ignore_case, RichText::new(t(S::MacroIgnoreCase)).size(14.0));
        }
        if def.kind == MacroKind::Alias {
            small(ui, S::MacroAliasHelp);
        }
        if def.kind == MacroKind::Timer {
            gap(ui);
            small(ui, S::MacroInterval);
            crate::a11y::named(
                ui.add(
                    egui::DragValue::new(&mut def.interval_seconds)
                        .range(
                            wandur_core::macros::definition::MIN_INTERVAL
                                ..=wandur_core::macros::definition::MAX_INTERVAL,
                        )
                        .speed(1.0),
                ),
                t(S::MacroInterval),
            );
        }
        if def.kind == MacroKind::Shortcut {
            gap(ui);
            small(ui, S::MacroKey);
            let mut index = SHORTCUT_KEYS
                .iter()
                .position(|k| def.pattern == *k)
                .unwrap_or(usize::MAX);
            if Select::new("macro-key", t(S::MacroKey))
                .width(width)
                .height(28.0)
                .font_size(12.0)
                .placeholder(def.pattern.as_str())
                .show_index(ui, &mut index, SHORTCUT_KEYS.len(), |i| SHORTCUT_KEYS[i])
                .changed()
            {
                def.pattern = SHORTCUT_KEYS[index].into();
            }
            small(ui, S::MacroKeyHelp);
        }
        gap(ui);
        small(ui, S::MacroCommands);
        crate::a11y::named(
            ui.add_sized(
                egui::vec2(width, 140.0),
                egui::TextEdit::multiline(&mut def.commands)
                    .font(egui::TextStyle::Monospace)
                    .char_limit(wandur_core::macros::definition::MAX_COMMAND_TEXT)
                    .margin(egui::Margin::symmetric(10, 8))
                    .desired_rows(6),
            ),
            t(S::MacroCommands),
        );
        gap(ui);
        small(ui, S::MacroCommandsHelp);
        ui.add_space(6.0);
        small(ui, S::MacroHelp);
    }

    fn footer(&mut self, ui: &mut Ui, theme: &Theme, result: &mut FormResult) {
        if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(theme.error));
            ui.add_space(4.0);
        }
        if let Some(target) = self.switch_to {
            ui.horizontal(|ui| {
                ui.label(RichText::new(t(S::ProfileDiscardMessage)).size(12.0).color(theme.text));
                if ui.button(t(S::ProfileDiscard)).clicked() {
                    *result = FormResult::Switch(target);
                }
                if ui.button(t(S::ProfileKeepEditing)).clicked() {
                    self.switch_to = None;
                }
            });
            return;
        }
        ui.horizontal_centered(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if dialogs::primary_button(ui, t(S::SaveWorld), theme).clicked() {
                    *result = self.save();
                }
                if dialogs::secondary_button(ui, t(S::Cancel)).clicked() {
                    *result = FormResult::Cancelled;
                }
                if self.index.is_some()
                    && !self.section.is_automation()
                    && dialogs::secondary_button(ui, t(S::Remove)).clicked()
                {
                    self.pending_removal = !self.pending_removal;
                }
                ui.add_space(20.0);
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(t(S::ProfileDraftHelp)).size(11.0).color(theme.muted));
                    if self.pending_removal {
                        ui.label(RichText::new(t(S::ProfileRemovalPending)).size(12.0).color(theme.text));
                    }
                });
            });
        });
    }
}

/// One rule's card in the Channels section. Returns whether Delete was clicked.
fn channel_rule_card(
    ui: &mut Ui,
    index: usize,
    rule: &mut wandur_core::channels::ChannelRule,
    width: f32,
    theme: &Theme,
) -> bool {
    use wandur_core::channels::rules;
    let mut delete = false;
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(egui::Stroke::new(1.0, theme.border))
        .corner_radius(4)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            let inner = width - 26.0;
            ui.set_width(inner);
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let mut enabled = !rule.disabled;
                if ui
                    .checkbox(&mut enabled, RichText::new(t(S::ProfileChannelEnabled)).size(12.0))
                    .changed()
                {
                    rule.disabled = !enabled;
                }
                let kind = if rule.exclude {
                    t(S::ProfileChannelExclude)
                } else if rule.private {
                    t(S::TeachChannelPrivate)
                } else {
                    ""
                };
                if !kind.is_empty() {
                    ui.label(RichText::new(kind).size(11.0).color(theme.muted));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let button = egui::Button::new(RichText::new(t(S::ProfileChannelDelete)).size(11.0))
                        .min_size(egui::vec2(54.0, 28.0));
                    if ui.add(button).clicked() {
                        delete = true;
                    }
                });
            });
            let gap = 10.0;
            let half = (inner - gap) / 2.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.vertical(|ui| {
                    ui.set_width(half);
                    ui.label(RichText::new(t(S::ChannelRuleChannel)).size(12.0).color(theme.muted));
                    crate::a11y::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut rule.channel)
                                .id_salt(("channel-rule-channel", index))
                                .char_limit(rules::MAX_CHANNEL)
                                .margin(egui::Margin::symmetric(10, 7))
                                .desired_width(half),
                        ),
                        t(S::ChannelRuleChannel),
                    );
                });
                ui.vertical(|ui| {
                    ui.set_width(half);
                    ui.label(RichText::new(t(S::ChannelRuleReply)).size(12.0).color(theme.muted));
                    let mut reply = rule.reply_command.clone().unwrap_or_default();
                    let edited = crate::a11y::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut reply)
                                .id_salt(("channel-rule-reply", index))
                                .char_limit(rules::MAX_REPLY)
                                .margin(egui::Margin::symmetric(10, 7))
                                .desired_width(half),
                        ),
                        t(S::ChannelRuleReply),
                    )
                    .changed();
                    if edited {
                        rule.reply_command = (!reply.is_empty()).then_some(reply);
                    }
                });
            });
            ui.label(RichText::new(t(S::ChannelRulePattern)).size(12.0).color(theme.muted));
            crate::a11y::named(
                ui.add(
                    egui::TextEdit::singleline(&mut rule.pattern)
                        .id_salt(("channel-rule-pattern", index))
                        .font(egui::TextStyle::Monospace)
                        .char_limit(rules::MAX_PATTERN)
                        .margin(egui::Margin::symmetric(10, 7))
                        .desired_width(inner),
                ),
                t(S::ChannelRulePattern),
            );
        });
    delete
}

/// The encoding names the C# editor lists.
fn charset_name(charset: Charset) -> &'static str {
    match charset {
        Charset::Utf8 => "utf-8",
        Charset::Latin1 => "latin1",
    }
}

fn field_label(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.label(RichText::new(text).size(14.0).color(theme.muted));
}

#[cfg(test)]
mod tests {
    use super::*;
    use wandur_core::l10n::{Language, override_thread};

    #[test]
    fn the_form_validates_and_trims() {
        let mut f = WorldForm::new_world();
        f.world.name = "  My MUD ".into();
        f.world.host = " mud.example.org ".into();
        f.port_text = "70000".into();
        assert!(f.finish().is_err(), "port out of range");
        f.port_text = "4000".into();
        let w = f.finish().unwrap();
        assert_eq!(
            (w.name.as_str(), w.host.as_str(), w.port),
            ("My MUD", "mud.example.org", 4000)
        );
    }

    fn world() -> SavedWorld {
        SavedWorld {
            name: "The Lantern Road".into(),
            host: "lanternroad.example.org".into(),
            ..SavedWorld::default()
        }
    }

    /// The C# macro form cases: new macros start disabled with the starter rule; becoming a
    /// shortcut picks F1; values survive kind changes; an invalid macro blocks Save world with
    /// the C# message and keeps the draft; a valid one comes back compiled.
    #[test]
    fn macro_drafts_validate_and_save_with_the_world() {
        override_thread(Some(Language::En));
        let mut form = WorldForm::edit(Some(0), world()).with_library(Vec::new());
        assert!(!form.has_unsaved_changes());
        form.macros.add().unwrap();
        let entry = form.macros.selected_entry().unwrap();
        assert_eq!(entry.name, "New macro");
        assert!(!entry.enabled);
        // The next new ones are numbered.
        {
            let mut other = WorldForm::edit(Some(0), world()).with_library(Vec::new());
            for _ in 0..3 {
                other.macros.add().unwrap();
            }
            let names: Vec<String> = other.macros.entries.iter().map(|e| e.name.clone()).collect();
            assert_eq!(names, ["New macro", "New macro 2", "New macro 3"]);
        }
        assert_eq!(entry.macro_def.as_ref().unwrap().pattern, "You are hungry");
        assert!(form.has_unsaved_changes());
        form.macros.set_kind(MacroKind::Timer);
        form.macros.definition_mut().unwrap().interval_seconds = 45;
        form.macros.set_kind(MacroKind::Shortcut);
        assert_eq!(form.macros.definition_mut().unwrap().pattern, "F1");
        form.macros.definition_mut().unwrap().pattern = "F4".into();
        form.macros.set_kind(MacroKind::Shortcut);
        assert_eq!(
            form.macros.definition_mut().unwrap().pattern,
            "F4",
            "already a shortcut"
        );
        assert_eq!(form.macros.definition_mut().unwrap().interval_seconds, 45);
        form.macros.definition_mut().unwrap().commands = String::new();
        assert!(matches!(form.save(), FormResult::Open));
        assert_eq!(form.error.as_deref(), Some(t(S::MacroInvalidCommands)));
        assert!(t(S::MacroInvalidCommands).starts_with("Enter 1–20 nonempty command lines"));
        assert_eq!(form.section, Section::Macros);
        form.macros.definition_mut().unwrap().commands = "look".into();
        match form.save() {
            FormResult::Saved {
                index: Some(0),
                library: Some(change),
                ..
            } => {
                assert!(change.before.is_empty());
                assert_eq!(change.after.len(), 1);
                let saved = &change.after[0];
                assert_eq!(
                    saved.source,
                    saved.macro_def.as_ref().unwrap().compile_javascript().unwrap()
                );
            }
            _ => panic!("expected a save with the library"),
        }
        override_thread(None);
    }

    #[test]
    fn enabling_is_a_change_but_not_unsaved_text_and_delete_selects_the_next() {
        let a = LibraryEntry::new_macro("A", MacroDefinition::new(MacroKind::Alias, "a", "look"));
        let b = LibraryEntry::new_macro("B", MacroDefinition::new(MacroKind::Alias, "b", "look"));
        let mut form = WorldForm::edit(Some(0), world()).with_library(vec![a.clone(), b.clone()]);
        assert_eq!(form.macros.selected.as_deref(), Some(a.id.as_str()));
        form.macros.selected_mut().unwrap().enabled = true;
        assert!(form.macros.has_changes());
        assert!(!form.macros.is_unsaved(&a.id));
        form.macros.selected_mut().unwrap().name = "A2".into();
        assert!(form.macros.is_unsaved(&a.id));
        let library = form.macros.entries.clone();
        let toast = form.macros.delete_selected().expect("the undo toast");
        assert_eq!(toast.text, "Deleted macro A2");
        assert_eq!(form.macros.selected.as_deref(), Some(b.id.as_str()));
        assert_eq!(form.macros.macros().count(), 1);
        // Undo puts it back exactly, at its place, selected.
        form.macros.restore(toast.undo);
        assert_eq!(form.macros.entries, library);
        assert_eq!(form.macros.selected.as_deref(), Some(a.id.as_str()));
        let toast = form.macros.delete_selected().unwrap();
        assert!(toast.undo.in_editor());
        form.macros.delete_selected();
        assert_eq!(form.macros.macros().count(), 0);
    }

    /// C# `SaveProfileAsync`: what came from the directory for the old address (the codebase,
    /// the theme, and here the listing link the codebase is also read through) goes when the
    /// host, port or TLS changes; the taught rules and the saved password stay. A name change
    /// or a host differing only in case or a trailing dot keeps them.
    #[test]
    fn a_new_address_drops_what_the_directory_said_about_the_old_one() {
        use wandur_core::channels::ChannelRule;
        override_thread(Some(Language::En));
        let mut listed = world();
        listed.world_id = "0123456789abcdef0123456789abcdef".into();
        listed.codebase = "SMAUG 1.4a".into();
        listed.listing_id = "lotj".into();
        listed.theme = serde_json::from_str(crate::scene::WORLD_THEME_FIXTURE)
            .ok()
            .and_then(|v| wandur_core::directory::WorldTheme::parse(&v));
        listed.channel_rules = vec![ChannelRule::new("clan", r"^\[CLAN\] (?<text>.*)$", Some("clan"))];
        let saved = |edit: &dyn Fn(&mut WorldForm)| {
            let mut form = WorldForm::edit(Some(0), listed.clone());
            edit(&mut form);
            match form.save() {
                FormResult::Saved { world, .. } => world,
                _ => panic!("expected a save: {:?}", form.error),
            }
        };
        let kept = |w: &SavedWorld| {
            w.codebase == listed.codebase && w.listing_id == listed.listing_id && w.theme == listed.theme
        };
        assert!(listed.theme.is_some());
        assert!(kept(&saved(&|f| f.world.name = "Renamed".into())));
        assert!(kept(&saved(&|f| f.world.host = "LANTERNROAD.example.org.".into())));
        let edits: [&dyn Fn(&mut WorldForm); 3] = [
            &|f| f.world.host = "elsewhere.example.org".into(),
            &|f| f.port_text = "4001".into(),
            &|f| f.world.tls = true,
        ];
        for edit in edits {
            let world = saved(edit);
            assert!(world.codebase.is_empty() && world.listing_id.is_empty() && world.theme.is_none());
            assert_eq!(world.channel_rules, listed.channel_rules);
            assert_eq!(world.world_id, listed.world_id);
        }
        override_thread(None);
    }

    /// The C# world editor case: the rules are listed, a rule turned off and an exclusion
    /// deleted are changes Save world writes; an unreadable pattern is refused with the world's
    /// own message on the Channels section; the last rule gone, the list is empty.
    #[test]
    fn the_channels_section_toggles_edits_and_deletes_rules() {
        use wandur_core::channels::ChannelRule;
        override_thread(Some(Language::En));
        let mut taught = world();
        taught.codebase = "SMAUG 1.4a".into();
        taught.channel_rules = vec![
            ChannelRule::new("clan", r"^\[CLAN\] (?<speaker>[A-Za-z]+): (?<text>.*)$", Some("clan")),
            ChannelRule::exclusion("ooc", r"^\[OOC\] Aldric: "),
        ];
        let mut form = WorldForm::edit(Some(0), taught);
        assert!(Section::Channels.available());
        assert!(!form.has_unsaved_changes());
        form.world.channel_rules[0].disabled = true;
        assert!(form.has_unsaved_changes());
        form.world.channel_rules.remove(1);
        let saved = match form.save() {
            FormResult::Saved { world, .. } => world,
            _ => panic!("expected a save"),
        };
        assert_eq!(saved.channel_rules.len(), 1);
        assert!(saved.channel_rules[0].disabled);
        assert!(!saved.channel_rules[0].exclude);
        assert_eq!(saved.codebase, "SMAUG 1.4a");

        let mut form = WorldForm::edit(Some(0), saved);
        form.section = Section::Connection;
        form.world.channel_rules[0].pattern = "(?<speaker>[A-Za-z".into();
        assert!(matches!(form.save(), FormResult::Open));
        assert_eq!(form.error.as_deref(), Some(t(S::ChannelRulesInvalid)));
        assert_eq!(form.section, Section::Channels);
        // A blank reply is no reply; names are trimmed.
        form.world.channel_rules[0].pattern = r"^\[CLAN\] (?<text>.*)$".into();
        form.world.channel_rules[0].channel = "  clan ".into();
        form.world.channel_rules[0].reply_command = Some("  ".into());
        match form.save() {
            FormResult::Saved { world, .. } => {
                assert_eq!(world.channel_rules[0].channel, "clan");
                assert_eq!(world.channel_rules[0].reply_command, None);
            }
            _ => panic!("expected a save"),
        }
        form.world.channel_rules.clear();
        assert!(matches!(form.save(), FormResult::Saved { .. }));
        override_thread(None);
    }

    /// The Scripts section's draft (C# `ScriptLibraryViewModel`): New gives the starter, disabled
    /// and selected; Duplicate an editable copy; Delete selects the next; a source edit is an
    /// unsaved change; Save world carries the scripts and keeps the macros.
    #[test]
    fn script_drafts_add_duplicate_delete_and_save_with_the_world() {
        let macro_entry = LibraryEntry::new_macro("Look", MacroDefinition::new(MacroKind::Shortcut, "F2", "look"));
        let mut form = WorldForm::edit(Some(0), world()).with_library(vec![macro_entry.clone()]);
        assert!(form.macros.selected_script.is_none());
        form.macros.add_script().unwrap();
        let first = form.macros.selected_script.clone().unwrap();
        assert_eq!(
            form.macros.selected_script_entry().unwrap().source,
            t(S::ScriptStarterExample)
        );
        assert!(form.macros.is_unsaved(&first));
        form.macros.selected_script_mut().unwrap().name = "Greeter".into();
        form.macros.duplicate_script().unwrap();
        let copy = form.macros.selected_script_entry().unwrap().clone();
        assert_eq!(copy.name, tf(S::ScriptDuplicateName, &[&"Greeter"]));
        assert!(!copy.enabled && copy.pack_json.is_none());
        assert_eq!(form.macros.scripts().count(), 2);
        let library = form.macros.entries.clone();
        let toast = form.macros.delete_script().expect("the undo toast");
        assert_eq!(toast.text, format!("Deleted script {}", copy.name));
        assert_eq!(form.macros.selected_script.as_deref(), Some(first.as_str()));
        assert_eq!(form.macros.scripts().count(), 1);
        form.macros.restore(toast.undo);
        assert_eq!(form.macros.entries, library, "the script back exactly, at its place");
        assert_eq!(form.macros.selected_script.as_deref(), Some(copy.id.as_str()));
        form.macros.delete_script();
        assert_eq!(form.macros.scripts().count(), 1);
        let FormResult::Saved {
            library: Some(change), ..
        } = form.save()
        else {
            panic!("expected a save with the library");
        };
        assert_eq!(change.after.len(), 2);
        assert_eq!(change.after[0], macro_entry);
        assert_eq!(change.after[1].name, "Greeter");

        // Editing only the source of a saved script is a change too.
        let mut form = WorldForm::edit(Some(0), world()).with_library(change.after.clone());
        assert!(!form.has_unsaved_changes());
        form.macros.selected_script_mut().unwrap().source.push_str("\n// more");
        assert!(form.has_unsaved_changes());
        assert!(form.macros.is_unsaved(&first));
    }

    #[test]
    fn remove_is_marked_and_applied_on_save() {
        let mut form = WorldForm::edit(Some(2), world());
        form.pending_removal = true;
        assert!(form.has_unsaved_changes());
        assert!(matches!(form.save(), FormResult::Removed(2)));
    }

    #[test]
    fn only_built_sections_can_be_chosen() {
        let available: Vec<Section> = Section::ALL.into_iter().filter(|s| s.available()).collect();
        let mut expected = vec![Section::Connection, Section::Login, Section::Scripts, Section::Macros];
        if cfg!(feature = "agent") {
            expected.push(Section::Agent);
        }
        expected.push(Section::Channels);
        assert_eq!(available, expected);
        for language in Language::ALL {
            override_thread(Some(language));
            for s in Section::ALL {
                assert!(!s.label().is_empty());
            }
        }
        override_thread(None);
    }

    /// The C# `LuaScriptLibraryTests` and `MudletImportLibraryTests` rules in the editor draft:
    /// a script's language and Mudlet names, a duplicate keeps them, JavaScript drops the layer;
    /// imported code that needs conversion cannot be switched on, Run as Lua turns it into a
    /// switched-off Mudlet Lua script, and saving changed source takes the script over.
    #[test]
    fn lua_language_mudlet_names_and_imported_scripts_in_the_draft() {
        use wandur_core::db::scripts::ImportedItem;
        let imported = LibraryEntry {
            id: scripts::new_id(),
            name: "Combat (Mudlet, needs conversion)".into(),
            source: "-- kept Lua\nsend('quaff tonic')\n".into(),
            enabled: false,
            import: Some(ImportInfo {
                origin: ImportInfo::MUDLET.into(),
                source: "The Lantern Road".into(),
                group: "Combat".into(),
                needs_conversion: true,
                items: vec![ImportedItem {
                    kind: "trigger".into(),
                    path: "Combat".into(),
                    name: "Low health".into(),
                    reason: "lua".into(),
                    code: "send(\"quaff tonic\")".into(),
                    patterns: vec![r"regex:^HP: (\d+)".into()],
                    command: String::new(),
                    active: true,
                }],
                source_hash: ImportInfo::hash("-- kept Lua\nsend('quaff tonic')\n"),
            }),
            ..LibraryEntry::default()
        };
        let mut form = WorldForm::edit(Some(0), world()).with_library(vec![imported.clone()]);
        assert_eq!(MacroEditor::import_label(&imported), Some(t(S::MudletImportMarkerLua)));
        assert_eq!(
            form.macros.set_script_enabled(true),
            Err(t(S::MudletNeedsConversionCannotRun).to_string())
        );
        assert!(!form.macros.selected_script_entry().unwrap().enabled);
        form.macros.run_import_as_lua();
        let converted = form.macros.selected_script_entry().unwrap().clone();
        assert_eq!(converted.runtime(), wandur_core::scripting::Runtime::MUDLET);
        assert!(!converted.enabled && !converted.needs_conversion());
        assert!(converted.source.contains("__mudlet.trigger(\"Combat / Low health\""));
        assert_eq!(MacroEditor::import_label(&converted), Some(t(S::MudletImportMarker)));
        form.macros.set_script_enabled(true).unwrap();

        // A hand-written script: Lua with Mudlet names, kept by a duplicate, dropped by JavaScript.
        form.macros.add_script().unwrap();
        form.macros.set_script_language(wandur_core::scripting::Language::Lua);
        form.macros.set_script_mudlet(true);
        form.macros.duplicate_script().unwrap();
        assert_eq!(
            form.macros.selected_script_entry().unwrap().runtime(),
            wandur_core::scripting::Runtime::MUDLET
        );
        form.macros
            .set_script_language(wandur_core::scripting::Language::JavaScript);
        assert_eq!(
            form.macros.selected_script_entry().unwrap().runtime(),
            wandur_core::scripting::Runtime::JAVASCRIPT
        );
        form.macros.set_script_mudlet(true);
        assert_eq!(
            form.macros.selected_script_entry().unwrap().compatibility,
            Compatibility::None,
            "only Lua takes a layer"
        );

        // Saving edited imported source takes it over: it no longer needs conversion.
        let mut form = WorldForm::edit(Some(0), world()).with_library(vec![imported.clone()]);
        form.macros.selected_script_mut().unwrap().source = "mud.echo('rewritten');".into();
        let FormResult::Saved {
            library: Some(change), ..
        } = form.save()
        else {
            panic!("expected a save with the library");
        };
        assert!(!change.after[0].needs_conversion());
        // Saving it unchanged keeps it as it was.
        let form = WorldForm::edit(Some(0), world()).with_library(vec![imported.clone()]);
        assert!(!form.has_unsaved_changes());
        assert!(form.macros.finish().unwrap()[0].needs_conversion());
    }
}
